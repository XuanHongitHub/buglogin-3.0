use std::time::Duration;

use buglogin_domain::errors::Problem;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{desktop_state::DesktopState, remote_api::FeedbackSubmission};

const PROFILE_LAUNCH_DIAGNOSTIC_SCHEMA: &str = "buglogin.profile-launch-error/1";

fn profile_launch_diagnostic(error: &Problem) -> (FeedbackSubmission, String) {
    let identity = format!(
        "{PROFILE_LAUNCH_DIAGNOSTIC_SCHEMA}|{}|{}|{}|{}|{}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        error.code,
        error.status,
    );
    let digest = Sha256::digest(identity.as_bytes());
    let idempotency_key = format!(
        "auto-profile-launch-{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let logs = json!({
        "schema": PROFILE_LAUNCH_DIAGNOSTIC_SCHEMA,
        "appVersion": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "architecture": std::env::consts::ARCH,
        "errorCode": error.code,
        "errorTitle": error.title,
        "status": error.status,
        "correlationId": error.correlation_id,
    })
    .to_string();

    (
        FeedbackSubmission {
            kind: "bug".to_owned(),
            message: format!("Automatic profile launch error: {}", error.code),
            include_logs: true,
            logs: Some(logs),
            email: None,
            locale: "en".to_owned(),
        },
        idempotency_key,
    )
}

/// Upload a redacted profile-launch failure without delaying or replacing the original error.
pub fn report_profile_launch_failure(state: DesktopState, error: Problem) {
    let correlation_id = error.correlation_id.clone();
    let (submission, idempotency_key) = profile_launch_diagnostic(&error);
    tauri::async_runtime::spawn(async move {
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            state.submit_feedback(submission, &idempotency_key, &correlation_id),
        )
        .await;
    });
}

#[cfg(test)]
mod tests {
    use super::profile_launch_diagnostic;
    use buglogin_domain::errors::Problem;

    #[test]
    fn profile_launch_reports_are_idempotent_and_exclude_sensitive_error_detail() {
        let mut first = Problem::new(
            "stale_manifest_resurrection",
            "Live object conflicts with tombstone",
            409,
            "correlation-one",
        )
        .with_detail("private profile path and cookie value");
        let (first_submission, first_key) = profile_launch_diagnostic(&first);
        first.correlation_id = "correlation-two".to_owned();
        first.detail = Some("different private detail".to_owned());
        let (retry_submission, retry_key) = profile_launch_diagnostic(&first);

        assert_eq!(first_key, retry_key);
        assert_eq!(first_submission.message, retry_submission.message);
        assert!(first_key.starts_with("auto-profile-launch-"));
        assert!(first_submission.include_logs);
        let report = first_submission.logs.unwrap();
        assert!(report.contains("stale_manifest_resurrection"));
        assert!(report.contains("correlation-one"));
        assert!(!report.contains("private profile path"));
        assert!(!report.contains("cookie value"));
    }
}
