use std::{
    collections::BTreeMap,
    future::Future,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use buglogin_domain::errors::Problem;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex as AsyncMutex;

use crate::{
    desktop_state::DesktopState,
    remote_api::{FeedbackReceipt, FeedbackSubmission},
};

const DIAGNOSTIC_SCHEMA: &str = "buglogin.desktop-operational-error/1";
const OUTBOX_SCHEMA_VERSION: u8 = 1;
const MAX_OUTBOX_ITEMS: usize = 128;
const MAX_OUTBOX_BYTES: usize = 512 * 1024;
const MAX_OUTBOX_ITEM_BYTES: usize = 8 * 1024;
const MAX_FLUSH_PER_TICK: usize = 8;
const REPORT_TIMEOUT: Duration = Duration::from_secs(5);
const REPORT_RETRY_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct QueuedDiagnostic {
    schema_version: u8,
    idempotency_key: String,
    correlation_id: String,
    submission: FeedbackSubmission,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutboxError {
    Io,
    Database,
    Serialization,
    ItemTooLarge,
}

impl From<std::io::Error> for OutboxError {
    fn from(_: std::io::Error) -> Self {
        Self::Io
    }
}

impl From<rusqlite::Error> for OutboxError {
    fn from(_: rusqlite::Error) -> Self {
        Self::Database
    }
}

impl From<serde_json::Error> for OutboxError {
    fn from(_: serde_json::Error) -> Self {
        Self::Serialization
    }
}

impl OutboxError {
    const fn code(self) -> &'static str {
        match self {
            Self::Io => "outbox_io_error",
            Self::Database => "outbox_database_error",
            Self::Serialization => "outbox_serialization_error",
            Self::ItemTooLarge => "outbox_item_too_large",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FlushOutcome {
    delivered: usize,
    pending: usize,
    deferred_reason: Option<&'static str>,
}

fn report_flush_gate() -> &'static AsyncMutex<()> {
    static GATE: OnceLock<AsyncMutex<()>> = OnceLock::new();
    GATE.get_or_init(|| AsyncMutex::new(()))
}

fn outbox_path() -> PathBuf {
    let base = if let Ok(directory) = std::env::var("BUGLOGIN_DATA_DIR") {
        PathBuf::from(directory)
    } else if let Some(directory) = dirs::data_dir() {
        directory.join("buglogin")
    } else {
        PathBuf::from(".buglogin")
    };
    let app_data = crate::app_instance::AppInstance::app_data_dir(base);
    app_data.join("db").join("operational-diagnostics.sqlite")
}

fn open_outbox(path: &Path) -> Result<Connection, OutboxError> {
    let parent = path.parent().ok_or(OutboxError::Io)?;
    std::fs::create_dir_all(parent)?;
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(2))?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS operational_diagnostics (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             idempotency_key TEXT NOT NULL UNIQUE,
             payload BLOB NOT NULL,
             enqueued_at_ms INTEGER NOT NULL,
             attempts INTEGER NOT NULL DEFAULT 0,
             last_failure_code TEXT
         );",
    )?;
    Ok(connection)
}

fn enqueue(path: &Path, diagnostic: &QueuedDiagnostic) -> Result<bool, OutboxError> {
    let payload = serde_json::to_vec(diagnostic)?;
    if payload.len() > MAX_OUTBOX_ITEM_BYTES {
        return Err(OutboxError::ItemTooLarge);
    }

    let mut connection = open_outbox(path)?;
    let transaction = connection.transaction()?;
    let changed = transaction.execute(
        "INSERT OR IGNORE INTO operational_diagnostics
             (idempotency_key, payload, enqueued_at_ms)
         VALUES (?1, ?2, ?3)",
        params![diagnostic.idempotency_key, payload, unix_time_millis()],
    )?;
    if changed == 0 {
        transaction.commit()?;
        return Ok(false);
    }

    transaction.execute(
        "DELETE FROM operational_diagnostics
         WHERE id NOT IN (
             SELECT id FROM operational_diagnostics ORDER BY id DESC LIMIT ?1
         )",
        [MAX_OUTBOX_ITEMS as i64],
    )?;
    loop {
        let bytes: i64 = transaction.query_row(
            "SELECT COALESCE(SUM(length(payload)), 0) FROM operational_diagnostics",
            [],
            |row| row.get(0),
        )?;
        if bytes <= MAX_OUTBOX_BYTES as i64 {
            break;
        }
        let removed = transaction.execute(
            "DELETE FROM operational_diagnostics WHERE id = (
                 SELECT id FROM operational_diagnostics ORDER BY id ASC LIMIT 1
             )",
            [],
        )?;
        if removed == 0 {
            break;
        }
    }
    transaction.commit()?;
    Ok(true)
}

fn next_queued(path: &Path) -> Result<Option<QueuedDiagnostic>, OutboxError> {
    let connection = open_outbox(path)?;
    let payload = connection
        .query_row(
            "SELECT payload FROM operational_diagnostics ORDER BY id ASC LIMIT 1",
            [],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?;
    let Some(payload) = payload else {
        return Ok(None);
    };
    let diagnostic: QueuedDiagnostic = serde_json::from_slice(&payload)?;
    if diagnostic.schema_version != OUTBOX_SCHEMA_VERSION {
        return Err(OutboxError::Serialization);
    }
    Ok(Some(diagnostic))
}

fn acknowledge(path: &Path, idempotency_key: &str) -> Result<(), OutboxError> {
    let connection = open_outbox(path)?;
    connection.execute(
        "DELETE FROM operational_diagnostics WHERE idempotency_key = ?1",
        [idempotency_key],
    )?;
    Ok(())
}

fn record_delivery_failure(
    path: &Path,
    idempotency_key: &str,
    failure_code: &'static str,
) -> Result<(), OutboxError> {
    let connection = open_outbox(path)?;
    connection.execute(
        "UPDATE operational_diagnostics
         SET attempts = attempts + 1, last_failure_code = ?2
         WHERE idempotency_key = ?1",
        params![idempotency_key, failure_code],
    )?;
    Ok(())
}

fn unix_time_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}

fn safe_correlation_id(idempotency_key: &str) -> String {
    let digest = Sha256::digest(idempotency_key.as_bytes());
    let suffix = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("diagnostic-{suffix}")
}

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
    operational_diagnostic_with_context(
        operation,
        error_code,
        status,
        is_machine_code,
        serde_json::Map::new(),
    )
}

fn operational_diagnostic_with_context(
    operation: &str,
    error_code: &str,
    status: u16,
    is_machine_code: bool,
    context: serde_json::Map<String, Value>,
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
        "{DIAGNOSTIC_SCHEMA}|{}|{}|{}|{}|{}|{}|{}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        operation,
        error_code,
        status,
        serde_json::to_string(&context).unwrap_or_default(),
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
        "failureContext": context,
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
    let context = safe_problem_failure_context(&error.fields);
    report_failure_with_context(
        state,
        operation,
        &error.code,
        error.status,
        &error.correlation_id,
        true,
        context,
    );
}

fn safe_problem_failure_context(
    fields: &BTreeMap<String, Value>,
) -> serde_json::Map<String, Value> {
    let mut context = serde_json::Map::new();
    if let Some(Value::String(reason)) = fields.get("cdpFailureReason")
        && matches!(
            reason.as_str(),
            "generation_state_unavailable" | "custom_fingerprint_plan_required"
        )
    {
        context.insert("cdpFailureReason".into(), Value::String(reason.clone()));
    }
    if let Some(Value::String(class)) = fields.get("cdpErrorClass")
        && matches!(
            class.as_str(),
            "timeout" | "transport" | "invalid_response" | "protocol_error"
        )
    {
        context.insert("cdpErrorClass".into(), Value::String(class.clone()));
    }
    if let Some(Value::Number(code)) = fields.get("cdpProtocolCode")
        && let Some(code) = code.as_i64()
        && (-32_768..=32_767).contains(&code)
    {
        context.insert("cdpProtocolCode".into(), Value::from(code));
    }
    context
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
    _correlation_id: &str,
    is_machine_code: bool,
) {
    report_failure_with_context(
        state,
        operation,
        error_code,
        status,
        _correlation_id,
        is_machine_code,
        serde_json::Map::new(),
    );
}

fn report_failure_with_context(
    state: DesktopState,
    operation: &str,
    error_code: &str,
    status: u16,
    _correlation_id: &str,
    is_machine_code: bool,
    context: serde_json::Map<String, Value>,
) {
    let (submission, idempotency_key) = operational_diagnostic_with_context(
        operation,
        error_code,
        status,
        is_machine_code,
        context,
    );
    if matches!(&state, DesktopState::Local(_)) {
        return;
    }
    let diagnostic = QueuedDiagnostic {
        schema_version: OUTBOX_SCHEMA_VERSION,
        correlation_id: safe_correlation_id(&idempotency_key),
        idempotency_key,
        submission,
    };
    let path = outbox_path();
    if let Err(error) = enqueue(&path, &diagnostic) {
        report_delivery_problem(error.code());
        return;
    }
    tauri::async_runtime::spawn(async move {
        flush_pending(state).await;
    });
}

/// Retry durable diagnostics after startup and whenever a new report is queued.
/// A failed auth/network request is retained for the next interval; it never
/// delays or changes the command result that originally produced the report.
pub fn start_background_flush(state: DesktopState) {
    if matches!(&state, DesktopState::Local(_)) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(REPORT_RETRY_INTERVAL);
        loop {
            interval.tick().await;
            flush_pending(state.clone()).await;
        }
    });
}

async fn flush_pending(state: DesktopState) {
    if matches!(state, DesktopState::Local(_)) {
        return;
    }
    let _guard = report_flush_gate().lock().await;
    let path = outbox_path();
    let outcome = flush_outbox_with(&path, |submission, idempotency_key, correlation_id| {
        let state = state.clone();
        async move {
            state
                .submit_feedback(submission, &idempotency_key, &correlation_id)
                .await
        }
    })
    .await;
    match outcome {
        Ok(outcome) => {
            if let Some(reason) = outcome.deferred_reason {
                report_delivery_problem(reason);
            }
        }
        Err(error) => report_delivery_problem(error.code()),
    }
}

async fn flush_outbox_with<F, Fut>(path: &Path, mut submit: F) -> Result<FlushOutcome, OutboxError>
where
    F: FnMut(FeedbackSubmission, String, String) -> Fut,
    Fut: Future<Output = Result<FeedbackReceipt, Problem>>,
{
    let mut delivered = 0;
    for _ in 0..MAX_FLUSH_PER_TICK {
        let Some(diagnostic) = next_queued(path)? else {
            return Ok(FlushOutcome {
                delivered,
                pending: 0,
                deferred_reason: None,
            });
        };
        let idempotency_key = diagnostic.idempotency_key.clone();
        let result = tokio::time::timeout(
            REPORT_TIMEOUT,
            submit(
                diagnostic.submission,
                diagnostic.idempotency_key,
                diagnostic.correlation_id,
            ),
        )
        .await;
        let failure_code = match result {
            Ok(Ok(_receipt)) => {
                match acknowledge(path, &idempotency_key) {
                    Ok(()) => delivered += 1,
                    Err(error) => {
                        return Ok(FlushOutcome {
                            delivered,
                            pending: pending_count(path).unwrap_or_default(),
                            deferred_reason: Some(error.code()),
                        });
                    }
                }
                continue;
            }
            Ok(Err(problem)) => delivery_failure_code(&problem),
            Err(_) => "delivery_timeout",
        };
        record_delivery_failure(path, &idempotency_key, failure_code)?;
        return Ok(FlushOutcome {
            delivered,
            pending: pending_count(path)?,
            deferred_reason: Some(failure_code),
        });
    }

    Ok(FlushOutcome {
        delivered,
        pending: pending_count(path)?,
        deferred_reason: None,
    })
}

fn delivery_failure_code(problem: &Problem) -> &'static str {
    match problem.status {
        401 => "authentication_required",
        403 => "permission_denied",
        408 | 429 => "retryable_remote_response",
        500..=599 => "remote_service_error",
        _ if is_safe_machine_error_code(&problem.code) => {
            // Machine codes are validated and contain no free-form server text.
            // Preserve a few actionable transport codes, otherwise collapse
            // the response into a bounded category.
            match problem.code.as_str() {
                "authentication_required" => "authentication_required",
                "api_unavailable" | "api_timeout" => "api_unavailable",
                _ => "remote_rejected",
            }
        }
        _ => "remote_rejected",
    }
}

fn pending_count(path: &Path) -> Result<usize, OutboxError> {
    let connection = open_outbox(path)?;
    let count: i64 =
        connection.query_row("SELECT COUNT(*) FROM operational_diagnostics", [], |row| {
            row.get(0)
        })?;
    Ok(count.max(0) as usize)
}

fn report_delivery_problem(reason: &str) {
    let safe_reason = if is_safe_machine_error_code(reason) {
        reason
    } else {
        "diagnostic_delivery_deferred"
    };
    // Only bounded machine categories are written; never include the original
    // OS/network error, request body, profile identifier, or auth material.
    eprintln!("BugLogin diagnostic report retained (reason={safe_reason})");
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use buglogin_domain::errors::Problem;
    use rusqlite::Connection;

    use super::{
        MAX_OUTBOX_BYTES, MAX_OUTBOX_ITEM_BYTES, MAX_OUTBOX_ITEMS, OutboxError, QueuedDiagnostic,
        acknowledge, classify_known_string_error, enqueue, flush_outbox_with, next_queued,
        operational_diagnostic, pending_count, safe_problem_failure_context,
    };
    use crate::remote_api::FeedbackReceipt;
    use serde_json::json;

    fn test_outbox_path(label: &str) -> PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "buglogin-diagnostics-{label}-{}-{}.sqlite",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn remove_test_outbox(path: &PathBuf) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    }

    #[test]
    fn cdp_failure_context_keeps_protocol_diagnostics_and_drops_untrusted_fields() {
        let fields = BTreeMap::from([
            ("cdpErrorClass".to_owned(), json!("protocol_error")),
            ("cdpProtocolCode".to_owned(), json!(-32601)),
            ("path".to_owned(), json!(r"C:\Users\person\secret")),
        ]);

        assert_eq!(
            safe_problem_failure_context(&fields),
            json!({
                "cdpErrorClass": "protocol_error",
                "cdpProtocolCode": -32601
            })
            .as_object()
            .unwrap()
            .clone()
        );
    }

    fn queued_diagnostic(index: usize) -> QueuedDiagnostic {
        let (submission, idempotency_key) = operational_diagnostic(
            "profile.launch",
            &format!("profile_error_{index}"),
            409,
            true,
        );
        QueuedDiagnostic {
            schema_version: super::OUTBOX_SCHEMA_VERSION,
            correlation_id: super::safe_correlation_id(&idempotency_key),
            idempotency_key,
            submission,
        }
    }

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

    #[test]
    fn operational_outbox_persists_across_reopen() {
        let path = test_outbox_path("reopen");
        let diagnostic = queued_diagnostic(1);
        assert!(enqueue(&path, &diagnostic).unwrap());
        assert_eq!(pending_count(&path).unwrap(), 1);
        let reopened = next_queued(&path)
            .unwrap()
            .expect("queued report after reopen");
        assert_eq!(reopened.idempotency_key, diagnostic.idempotency_key);
        assert_eq!(reopened.correlation_id, diagnostic.correlation_id);
        remove_test_outbox(&path);
    }

    #[test]
    fn operational_outbox_deduplicates_by_idempotency_key() {
        let path = test_outbox_path("dedupe");
        let diagnostic = queued_diagnostic(2);
        assert!(enqueue(&path, &diagnostic).unwrap());
        assert!(!enqueue(&path, &diagnostic).unwrap());
        assert_eq!(pending_count(&path).unwrap(), 1);
        remove_test_outbox(&path);
    }

    #[test]
    fn operational_outbox_enforces_item_count_and_byte_bounds() {
        let count_path = test_outbox_path("count-bound");
        for index in 0..(MAX_OUTBOX_ITEMS + 2) {
            enqueue(&count_path, &queued_diagnostic(index)).unwrap();
        }
        assert_eq!(pending_count(&count_path).unwrap(), MAX_OUTBOX_ITEMS);
        assert_eq!(
            next_queued(&count_path).unwrap().unwrap().idempotency_key,
            queued_diagnostic(2).idempotency_key
        );
        remove_test_outbox(&count_path);

        let bytes_path = test_outbox_path("byte-bound");
        for index in 0..120 {
            let mut diagnostic = queued_diagnostic(index + 1_000);
            diagnostic.submission.logs = Some("x".repeat(6_000));
            enqueue(&bytes_path, &diagnostic).unwrap();
        }
        let connection = Connection::open(&bytes_path).unwrap();
        let bytes: i64 = connection
            .query_row(
                "SELECT COALESCE(SUM(length(payload)), 0) FROM operational_diagnostics",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM operational_diagnostics", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(bytes <= MAX_OUTBOX_BYTES as i64);
        assert!(count < 120, "byte cap should discard the oldest reports");
        drop(connection);

        let mut oversized = queued_diagnostic(9_999);
        oversized.submission.logs = Some("x".repeat(MAX_OUTBOX_ITEM_BYTES));
        assert_eq!(
            enqueue(&bytes_path, &oversized),
            Err(OutboxError::ItemTooLarge)
        );
        remove_test_outbox(&bytes_path);
    }

    #[tokio::test]
    async fn failed_delivery_is_retained_and_successful_receipt_acknowledges_it() {
        let path = test_outbox_path("retry-ack");
        let diagnostic = queued_diagnostic(3);
        enqueue(&path, &diagnostic).unwrap();

        let failed = flush_outbox_with(&path, |_, _, _| async {
            Err(Problem::new(
                "authentication_required",
                "Sign in to send feedback",
                401,
                "diagnostic-correlation",
            ))
        })
        .await
        .unwrap();
        assert_eq!(failed.delivered, 0);
        assert_eq!(failed.pending, 1);
        assert_eq!(failed.deferred_reason, Some("authentication_required"));
        assert_eq!(
            next_queued(&path).unwrap().unwrap().idempotency_key,
            diagnostic.idempotency_key
        );
        let connection = Connection::open(&path).unwrap();
        let (attempts, reason): (i64, String) = connection
            .query_row(
                "SELECT attempts, last_failure_code FROM operational_diagnostics",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(attempts, 1);
        assert_eq!(reason, "authentication_required");
        drop(connection);

        let delivered = flush_outbox_with(&path, |_, _, _| async {
            Ok(FeedbackReceipt {
                id: "fbk_test_ack".to_owned(),
            })
        })
        .await
        .unwrap();
        assert_eq!(delivered.delivered, 1);
        assert_eq!(delivered.pending, 0);
        assert_eq!(delivered.deferred_reason, None);
        assert!(next_queued(&path).unwrap().is_none());
        acknowledge(&path, &diagnostic.idempotency_key).unwrap();
        remove_test_outbox(&path);
    }
}
