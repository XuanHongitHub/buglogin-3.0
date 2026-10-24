use std::time::Duration;

use buglogin_domain::errors::Problem;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{desktop_state::DesktopState, remote_api::FeedbackSubmission};

const DIAGNOSTIC_SCHEMA: &str = "buglogin.desktop-operational-error/1";

fn safe_identifier(value: &str, max_len: usize) -> String {
    if value.is_empty()
        || value.len() > max_len
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
    {
        "unknown".to_owned()
    } else {
        value.to_owned()
    }
}

fn operational_diagnostic(
    operation: &str,
    error_code: &str,
    status: u16,
    is_machine_code: bool,
) -> (FeedbackSubmission, String) {
    let operation = safe_identifier(operation, 64);
    // Problem.code is a machine identifier (for example,
    // `stale_manifest_resurrection`). Keep safe identifiers actionable while
    // hashing arbitrary string errors, which may contain paths, URLs, or tokens.
    let error_code = if is_machine_code && is_safe_machine_error_code(error_code) {
        error_code.to_owned()
    } else {
        let error_digest = Sha256::digest(error_code.as_bytes());
        format!(
            "error-{}",
            error_digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        )
    };
    let identity = format!(
        "{DIAGNOSTIC_SCHEMA}|{}|{}|{}|{}|{}|{}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        operation,
        error_code,
        status,
    );
    let digest = Sha256::digest(identity.as_bytes());
    let idempotency_key = format!(
        "auto-desktop-error-{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let logs = json!({
        "schema": DIAGNOSTIC_SCHEMA,
        "appVersion": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "architecture": std::env::consts::ARCH,
        "operation": operation,
        "errorCode": error_code,
        "status": status,
    })
    .to_string();

    (
        FeedbackSubmission {
            kind: "bug".to_owned(),
            message: format!("Automatic desktop error ({operation}): {error_code}"),
            include_logs: true,
            logs: Some(logs),
            email: None,
            locale: "en".to_owned(),
        },
        idempotency_key,
    )
}

fn is_safe_machine_error_code(value: &str) -> bool {
    (2..=64).contains(&value.len())
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// Queue a redacted operational failure report without delaying or replacing the original error.
/// The stable idempotency key lets the API collapse repeated reports of the same failure class.
pub fn report_problem_failure(state: DesktopState, operation: &str, error: Problem) {
    if operation == "feedback.submit" {
        return;
    }
    report_failure(
        state,
        operation,
        &error.code,
        error.status,
        &error.correlation_id,
        true,
    );
}

/// Report a string-based command failure without transmitting the original message.
/// Arbitrary OS/network text remains a one-way hash and never enters diagnostics.
pub fn report_string_failure(
    state: DesktopState,
    operation: &str,
    error: &str,
    correlation_id: &str,
) {
    if let Some(error_code) = classify_known_string_error(error) {
        report_failure(state, operation, error_code, 0, correlation_id, true);
    } else {
        report_failure(state, operation, error, 0, correlation_id, false);
    }
}

fn classify_known_string_error(error: &str) -> Option<&'static str> {
    match error {
        "Live object conflicts with retained tombstone" => Some("stale_manifest_resurrection"),
        "A current profile read share is required to retrieve this envelope" => {
            Some("profile_read_share_required")
        }
        "Resource not found" => Some("resource_not_found"),
        _ => None,
    }
}

fn report_failure(
    state: DesktopState,
    operation: &str,
    error_code: &str,
    status: u16,
    correlation_id: &str,
    is_machine_code: bool,
) {
    let correlation_id = correlation_id.to_owned();
    let (submission, idempotency_key) =
        operational_diagnostic(operation, error_code, status, is_machine_code);
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
    use super::{classify_known_string_error, operational_diagnostic};

    #[test]
    fn operational_reports_dedupe_by_operation_code_and_status() {
        let (first, first_key) =
            operational_diagnostic("profile.stop", "lease_not_owner", 409, true);
        let (_, retry_key) = operational_diagnostic("profile.stop", "lease_not_owner", 409, true);
        let (_, different_operation_key) =
            operational_diagnostic("profile.sync", "lease_not_owner", 409, true);
        let (_, different_code_key) =
            operational_diagnostic("profile.stop", "resource_not_found", 409, true);
        let (_, different_status_key) =
            operational_diagnostic("profile.stop", "lease_not_owner", 503, true);

        assert_eq!(first_key, retry_key);
        assert_ne!(first_key, different_operation_key);
        assert_ne!(first_key, different_code_key);
        assert_ne!(first_key, different_status_key);
        assert!(first.message.contains("profile.stop"));
        assert!(first.message.contains("lease_not_owner"));
    }

    #[test]
    fn operational_reports_redact_details_paths_tokens_and_unsafe_identifiers() {
        let (submission, key) = operational_diagnostic(
            r"profile.stop C:\Users\alice\profile",
            "lease_not_owner?token=private-value",
            409,
            false,
        );
        let logs = submission.logs.unwrap();

        assert!(key.starts_with("auto-desktop-error-"));
        assert!(logs.contains("\"operation\":\"unknown\""));
        assert!(logs.contains("\"errorCode\":\"error-"));
        assert!(!logs.contains("C:\\Users\\alice"));
        assert!(!logs.contains("private-value"));
        assert!(!logs.contains("correlation"));
        assert!(!logs.contains("cookie"));
        assert!(!logs.contains("Authorization"));
    }

    #[test]
    fn operational_reports_preserve_only_valid_machine_error_codes() {
        let (known_problem, _) =
            operational_diagnostic("profile.launch", "profile_forbidden", 403, true);
        let known_logs = known_problem.logs.unwrap();
        assert!(known_logs.contains("\"errorCode\":\"profile_forbidden\""));

        let (untrusted, _) = operational_diagnostic(
            "profile.launch",
            "C:\\Users\\alice\\profile?token=secret",
            500,
            true,
        );
        let untrusted_logs = untrusted.logs.unwrap();
        assert!(untrusted_logs.contains("\"errorCode\":\"error-"));
        assert!(!untrusted_logs.contains("alice"));
        assert!(!untrusted_logs.contains("secret"));

        let (string_error, _) =
            operational_diagnostic("app.update_install", "download failed", 0, false);
        let string_logs = string_error.logs.unwrap();
        assert!(string_logs.contains("\"errorCode\":\"error-"));
        assert!(!string_logs.contains("download failed"));
    }

    #[test]
    fn known_runtime_string_errors_are_classified_without_exposing_messages() {
        for (message, code) in [
            (
                "Live object conflicts with retained tombstone",
                "stale_manifest_resurrection",
            ),
            (
                "A current profile read share is required to retrieve this envelope",
                "profile_read_share_required",
            ),
            ("Resource not found", "resource_not_found"),
        ] {
            assert_eq!(classify_known_string_error(message), Some(code));
            let (submission, _) = operational_diagnostic("profile.launch", code, 0, true);
            let logs = submission.logs.unwrap();
            assert!(
                logs.contains(&format!("\\\"errorCode\\\":\\\"{code}\\\""))
                    || logs.contains(&format!("\"errorCode\":\"{code}\""))
            );
            assert!(!logs.contains(message));
        }
        assert_eq!(
            classify_known_string_error("arbitrary error at C:\\secret"),
            None
        );
    }
}
