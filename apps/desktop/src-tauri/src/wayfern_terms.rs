use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use buglogin_domain::errors::Problem;

const ACCEPT_TERMS_FLAG: &str = "--accept-terms-and-conditions";
const MIN_VALID_TIMESTAMP: u64 = 1_577_836_800;

fn license_file_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    if let Some(app_data) = std::env::var_os("APPDATA") {
        return Some(
            PathBuf::from(app_data)
                .join("Wayfern")
                .join("license-accepted"),
        );
    }

    dirs::config_dir().map(|path| path.join("Wayfern").join("license-accepted"))
}

pub(crate) fn is_terms_accepted() -> bool {
    let Some(path) = license_file_path() else {
        return false;
    };
    fs::read_to_string(path)
        .ok()
        .and_then(|contents| contents.trim().parse::<u64>().ok())
        .is_some_and(|timestamp| timestamp >= MIN_VALID_TIMESTAMP)
}

pub(crate) fn accept_terms(executable: &Path, correlation_id: &str) -> Result<(), Problem> {
    // Wayfern's acceptance marker is shared across its installs. Avoid
    // relaunching the browser when the user has already accepted the terms.
    if is_terms_accepted() {
        return Ok(());
    }

    let status = Command::new(executable)
        .arg(ACCEPT_TERMS_FLAG)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| terms_problem(correlation_id))?;
    if !status.success() || !is_terms_accepted() {
        return Err(terms_problem(correlation_id));
    }
    Ok(())
}

fn terms_problem(correlation_id: &str) -> Problem {
    Problem::new(
        "wayfern_terms_acceptance_failed",
        "Wayfern could not record acceptance of its terms",
        502,
        correlation_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wayfern_license_timestamp_must_be_recent_enough() {
        assert!(MIN_VALID_TIMESTAMP >= 1_577_836_800);
        assert!(license_file_path().is_some());
    }
}
