use std::time::{SystemTime, UNIX_EPOCH};

use buglogin_domain::errors::Problem;

pub(crate) fn unix_time_seconds(correlation: &str) -> Result<u64, Problem> {
    unix_time_seconds_at(SystemTime::now(), correlation)
}

pub(crate) fn unix_time_seconds_at(now: SystemTime, correlation: &str) -> Result<u64, Problem> {
    now.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| clock_unavailable(correlation))
}

pub(crate) fn checked_add_seconds(
    now: u64,
    seconds: u64,
    correlation: &str,
) -> Result<u64, Problem> {
    now.checked_add(seconds)
        .ok_or_else(|| clock_unavailable(correlation))
}

pub(crate) fn unix_time_millis(correlation: &str) -> Result<u64, Problem> {
    unix_time_millis_at(SystemTime::now(), correlation)
}

pub(crate) fn checked_add_millis(now: u64, millis: u64, correlation: &str) -> Result<u64, Problem> {
    now.checked_add(millis)
        .ok_or_else(|| clock_unavailable(correlation))
}

pub(crate) fn unix_time_nanos(correlation: &str) -> Result<u128, Problem> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .map_err(|_| clock_unavailable(correlation))
}

fn unix_time_millis_at(now: SystemTime, correlation: &str) -> Result<u64, Problem> {
    let millis = now
        .duration_since(UNIX_EPOCH)
        .map_err(|_| clock_unavailable(correlation))?
        .as_millis();
    u64::try_from(millis).map_err(|_| clock_unavailable(correlation))
}

fn clock_unavailable(correlation: &str) -> Problem {
    Problem::new(
        "clock_unavailable",
        "System clock cannot provide a valid Unix timestamp",
        503,
        correlation,
    )
    .retryable(true)
}

#[cfg(test)]
mod tests {
    use super::{unix_time_millis_at, unix_time_seconds_at};
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn timestamps_before_epoch_fail_closed() {
        let before_epoch = UNIX_EPOCH - Duration::from_secs(1);
        for error in [
            unix_time_seconds_at(before_epoch, "cor").unwrap_err(),
            unix_time_millis_at(before_epoch, "cor").unwrap_err(),
        ] {
            assert_eq!(error.code, "clock_unavailable");
            assert_eq!(error.status, 503);
            assert_eq!(error.correlation_id, "cor");
            assert!(error.retryable);
        }
    }

    #[test]
    fn timestamps_after_epoch_convert_to_expected_units() {
        let time = UNIX_EPOCH + Duration::from_millis(1_234);
        assert_eq!(unix_time_seconds_at(time, "cor").unwrap(), 1);
        assert_eq!(unix_time_millis_at(time, "cor").unwrap(), 1_234);
    }
}
