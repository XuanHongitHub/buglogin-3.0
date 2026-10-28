use std::sync::{Arc, OnceLock};

use buglogin_domain::errors::Problem;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const TRANSFER_UNIT_BYTES: u64 = 1024 * 1024;
const MAX_PROFILE_TRANSFER_MEMORY_BYTES: u64 = 128 * 1024 * 1024;
const MAX_PROFILE_TRANSFER_MEMORY_UNITS: u32 =
    (MAX_PROFILE_TRANSFER_MEMORY_BYTES / TRANSFER_UNIT_BYTES) as u32;
static PROFILE_TRANSFER_MEMORY: OnceLock<Arc<Semaphore>> = OnceLock::new();

pub(crate) fn try_acquire(
    size_bytes: u64,
    multiplier: u64,
    correlation_id: &str,
) -> Result<OwnedSemaphorePermit, Problem> {
    let units = requested_units(size_bytes, multiplier, correlation_id)?;
    let slots = Arc::clone(
        PROFILE_TRANSFER_MEMORY
            .get_or_init(|| Arc::new(Semaphore::new(MAX_PROFILE_TRANSFER_MEMORY_UNITS as usize))),
    );
    try_acquire_from(&slots, units, correlation_id)
}

pub(crate) async fn acquire(
    size_bytes: u64,
    multiplier: u64,
    correlation_id: &str,
) -> Result<OwnedSemaphorePermit, Problem> {
    let units = requested_units(size_bytes, multiplier, correlation_id)?;
    let slots = Arc::clone(
        PROFILE_TRANSFER_MEMORY
            .get_or_init(|| Arc::new(Semaphore::new(MAX_PROFILE_TRANSFER_MEMORY_UNITS as usize))),
    );
    slots
        .acquire_many_owned(units)
        .await
        .map_err(|_| capacity_problem(correlation_id))
}

fn requested_units(size_bytes: u64, multiplier: u64, correlation_id: &str) -> Result<u32, Problem> {
    let charged_bytes = size_bytes.saturating_mul(multiplier).max(1);
    let units = charged_bytes
        .saturating_add(TRANSFER_UNIT_BYTES - 1)
        .checked_div(TRANSFER_UNIT_BYTES)
        .unwrap_or(u64::MAX);
    let units = u32::try_from(units).map_err(|_| capacity_problem(correlation_id))?;
    if units > MAX_PROFILE_TRANSFER_MEMORY_UNITS {
        return Err(capacity_problem(correlation_id));
    }
    Ok(units)
}

fn try_acquire_from(
    slots: &Arc<Semaphore>,
    units: u32,
    correlation_id: &str,
) -> Result<OwnedSemaphorePermit, Problem> {
    slots
        .clone()
        .try_acquire_many_owned(units)
        .map_err(|_| capacity_problem(correlation_id))
}

fn capacity_problem(correlation_id: &str) -> Problem {
    Problem::new(
        "profile_transfer_capacity_exhausted",
        "Profile transfer memory is busy; retry the profile operation",
        503,
        correlation_id,
    )
    .retryable(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_transfer_budget_accounts_for_bytes_and_releases_on_drop() {
        let slots = Arc::new(Semaphore::new(MAX_PROFILE_TRANSFER_MEMORY_UNITS as usize));
        let first_units = (64 * 1024 * 1024 / TRANSFER_UNIT_BYTES) as u32;
        let first = try_acquire_from(&slots, first_units, "transfer-budget-test").unwrap();
        let second = try_acquire_from(&slots, first_units, "transfer-budget-test").unwrap();
        let exhausted = try_acquire_from(&slots, 1, "transfer-budget-test").unwrap_err();
        assert_eq!(exhausted.code, "profile_transfer_capacity_exhausted");
        assert!(exhausted.retryable);

        drop(first);
        assert!(try_acquire_from(&slots, first_units, "transfer-budget-test").is_ok());
        drop(second);
    }

    #[test]
    fn download_charge_accounts_for_ciphertext_and_plaintext_overlap() {
        let slots = Arc::new(Semaphore::new(MAX_PROFILE_TRANSFER_MEMORY_UNITS as usize));
        let largest_download = try_acquire_from(
            &slots,
            MAX_PROFILE_TRANSFER_MEMORY_UNITS,
            "transfer-budget-test",
        )
        .unwrap();
        assert!(try_acquire_from(&slots, 1, "transfer-budget-test").is_err());
        drop(largest_download);
        assert!(
            try_acquire_from(
                &slots,
                MAX_PROFILE_TRANSFER_MEMORY_UNITS,
                "transfer-budget-test"
            )
            .is_ok()
        );
    }
}
