use std::time::{SystemTime, UNIX_EPOCH};

use buglogin_domain::errors::Problem;

pub(crate) fn unix_time_millis() -> Result<u64, Problem> {
    unix_time_millis_at(SystemTime::now())
}

fn unix_time_millis_at(time: SystemTime) -> Result<u64, Problem> {
    let duration = time
        .duration_since(UNIX_EPOCH)
        .map_err(|_| clock_unavailable())?;
    u64::try_from(duration.as_millis()).map_err(|_| clock_unavailable())
}

fn clock_unavailable() -> Problem {
    Problem::new(
        "clock_unavailable",
        "System clock is unavailable",
        503,
        "sync",
    )
    .retryable(true)
}
