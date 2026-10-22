use std::time::{SystemTime, UNIX_EPOCH};

use buglogin_domain::errors::Problem;
use time::OffsetDateTime;

pub(crate) fn unix_time_seconds(correlation: &str) -> Result<u64, Problem> {
    unix_time_seconds_at(SystemTime::now(), correlation)
}

pub(crate) fn unix_time_millis(correlation: &str) -> Result<u64, Problem> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| clock_unavailable(correlation))?;
    u64::try_from(duration.as_millis()).map_err(|_| clock_unavailable(correlation))
}

pub(crate) fn checked_add_seconds(
    now: u64,
    seconds: u64,
    correlation: &str,
) -> Result<u64, Problem> {
    now.checked_add(seconds)
        .ok_or_else(|| clock_unavailable(correlation))
}

pub(crate) fn checked_add_millis(now: u64, millis: u64, correlation: &str) -> Result<u64, Problem> {
    now.checked_add(millis)
        .ok_or_else(|| clock_unavailable(correlation))
}

fn unix_time_seconds_at(now: SystemTime, correlation: &str) -> Result<u64, Problem> {
    now.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| clock_unavailable(correlation))
}

pub(crate) fn unix_datetime(correlation: &str) -> Result<OffsetDateTime, Problem> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| clock_unavailable(correlation))?;
    let unix_nanos =
        i128::try_from(duration.as_nanos()).map_err(|_| clock_unavailable(correlation))?;
    OffsetDateTime::from_unix_timestamp_nanos(unix_nanos)
        .map_err(|_| clock_unavailable(correlation))
}

pub(crate) fn rfc3339(correlation: &str) -> Result<String, Problem> {
    unix_datetime(correlation)?
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| clock_unavailable(correlation))
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
    use super::*;

    #[test]
    fn rfc3339_returns_a_parseable_utc_timestamp() {
        let timestamp = rfc3339("clock-test").expect("system clock should be available");
        let parsed =
            OffsetDateTime::parse(&timestamp, &time::format_description::well_known::Rfc3339)
                .expect("timestamp should match the canonical format");

        assert_eq!(parsed.offset(), time::UtcOffset::UTC);
    }
}
