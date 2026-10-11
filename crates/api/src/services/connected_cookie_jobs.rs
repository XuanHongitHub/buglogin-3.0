use std::time::{SystemTime, UNIX_EPOCH};

use buglogin_domain::errors::Problem;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::control_persistence::ControlPersistence;
use buglogin_storage::control::{
    ControlAuditEntry, ControlPut, ExecuteControlCommand, HttpResponse,
};

const RECORD_TYPE: &str = "connected_cookie_job";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedCookieJob {
    pub job_id: String,
    pub operation_id: String,
    pub command_id: String,
    pub workspace_id: String,
    pub device_id: String,
    pub profile_id: String,
    pub expected_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged_import_ref: Option<String>,
    pub requested_by: String,
    pub idempotency_key: String,
    pub created_at: u64,
    pub expires_at: u64,
    pub state: ConnectedCookieJobState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<ConnectedCookieReceipt>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectedCookieJobState {
    Accepted,
    Dispatched,
    Running,
    Completed,
    Failed,
    Cancelled,
    Expired,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedCookieReceipt {
    pub operation_id: String,
    pub job_id: String,
    pub device_id: String,
    pub profile_id: String,
    pub previous_generation: u64,
    pub new_generation: u64,
    pub created_count: u64,
    pub replaced_count: u64,
    pub deleted_count: u64,
    pub metadata_checksum: String,
    pub completed_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnqueueConnectedCookieJob {
    pub command_id: String,
    pub workspace_id: String,
    pub device_id: String,
    pub profile_id: String,
    pub expected_generation: u64,
    #[serde(default)]
    pub source_workspace_id: Option<String>,
    #[serde(default)]
    pub source_profile_id: Option<String>,
    #[serde(default)]
    pub source_device_id: Option<String>,
    #[serde(default)]
    pub staged_import_ref: Option<String>,
    pub requested_by: String,
    pub idempotency_key: String,
    pub now: u64,
    pub expires_at: u64,
}

#[derive(Clone)]
pub struct ConnectedCookieJobService {
    persistence: ControlPersistence,
}

impl ConnectedCookieJobService {
    pub fn new(persistence: ControlPersistence) -> Self {
        Self { persistence }
    }

    pub fn enqueue(&self, input: EnqueueConnectedCookieJob) -> Result<ConnectedCookieJob, Problem> {
        validate_enqueue(&input)?;
        let job = ConnectedCookieJob {
            job_id: stable_id("job", &input.idempotency_key),
            operation_id: stable_id("operation", &input.idempotency_key),
            command_id: input.command_id,
            workspace_id: input.workspace_id,
            device_id: input.device_id,
            profile_id: input.profile_id,
            expected_generation: input.expected_generation,
            source_workspace_id: input.source_workspace_id,
            source_profile_id: input.source_profile_id,
            source_device_id: input.source_device_id,
            staged_import_ref: input.staged_import_ref,
            requested_by: input.requested_by,
            idempotency_key: input.idempotency_key,
            created_at: input.now,
            expires_at: input.expires_at,
            state: ConnectedCookieJobState::Accepted,
            receipt: None,
        };
        let value = serde_json::to_value(&job).map_err(|_| problem("job_encode_failed"))?;
        let result = self.persistence.execute(ExecuteControlCommand {
            tenant_id: job.workspace_id.clone(),
            command_id: "cookie.job.enqueue".into(),
            idempotency_key: job.idempotency_key.clone(),
            expected_revisions: Default::default(),
            puts: vec![ControlPut {
                tenant_id: job.workspace_id.clone(),
                record_type: RECORD_TYPE.into(),
                record_id: job.job_id.clone(),
                expected_revision: Some(0),
                value,
            }],
            deletes: vec![],
            audit: audit(&job, "cookie.job.enqueue"),
            response: HttpResponse {
                status: 202,
                headers: Default::default(),
                body: json!({"job": job}),
            },
        })?;
        serde_json::from_value(result.response.body["job"].clone())
            .map_err(|_| problem("job_record_corrupt"))
    }

    pub fn get(
        &self,
        tenant_id: &str,
        job_id: &str,
        actor_id: &str,
    ) -> Result<Option<ConnectedCookieJob>, Problem> {
        let job: Option<ConnectedCookieJob> =
            self.persistence.load(tenant_id, RECORD_TYPE, job_id)?;
        if job.as_ref().is_some_and(|job| job.requested_by != actor_id) {
            return Err(problem("cookie_job_forbidden"));
        }
        Ok(job)
    }

    pub fn claim(
        &self,
        tenant_id: &str,
        job_id: &str,
        actor_id: &str,
        device_id: &str,
        now: u64,
    ) -> Result<ConnectedCookieJob, Problem> {
        self.transition(
            tenant_id,
            job_id,
            actor_id,
            device_id,
            now,
            ConnectedCookieJobState::Dispatched,
        )
    }

    pub fn acknowledge(
        &self,
        tenant_id: &str,
        job_id: &str,
        actor_id: &str,
        device_id: &str,
        now: u64,
    ) -> Result<ConnectedCookieJob, Problem> {
        self.transition(
            tenant_id,
            job_id,
            actor_id,
            device_id,
            now,
            ConnectedCookieJobState::Running,
        )
    }

    pub fn complete(
        &self,
        tenant_id: &str,
        job_id: &str,
        actor_id: &str,
        now: u64,
        receipt: ConnectedCookieReceipt,
    ) -> Result<ConnectedCookieJob, Problem> {
        let (mut job, revision) = self.load_required(tenant_id, job_id)?;
        if job.requested_by != actor_id {
            return Err(problem("cookie_job_forbidden"));
        }
        if job.state == ConnectedCookieJobState::Completed {
            if job.receipt.as_ref() == Some(&receipt) {
                return Ok(job);
            }
            return Err(problem("cookie_job_invalid_state"));
        }
        if job.state != ConnectedCookieJobState::Running {
            return Err(problem("cookie_job_invalid_state"));
        }
        if now >= job.expires_at {
            job.state = ConnectedCookieJobState::Expired;
            return self.save(tenant_id, &job, revision, "cookie.job.expire");
        }
        if receipt.job_id != job.job_id
            || receipt.operation_id != job.operation_id
            || receipt.device_id != job.device_id
            || receipt.profile_id != job.profile_id
            || receipt.previous_generation != job.expected_generation
            || receipt.new_generation != receipt.previous_generation.saturating_add(1)
            || receipt.metadata_checksum.len() != 64
            || !receipt
                .metadata_checksum
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        {
            return Err(problem("cookie_receipt_binding_invalid"));
        }
        job.state = ConnectedCookieJobState::Completed;
        job.receipt = Some(receipt);
        self.save(tenant_id, &job, revision, "cookie.job.complete")
    }

    fn transition(
        &self,
        tenant_id: &str,
        job_id: &str,
        actor_id: &str,
        device_id: &str,
        now: u64,
        state: ConnectedCookieJobState,
    ) -> Result<ConnectedCookieJob, Problem> {
        let (mut job, revision) = self.load_required(tenant_id, job_id)?;
        if job.requested_by != actor_id {
            return Err(problem("cookie_job_forbidden"));
        }
        if job.device_id != device_id {
            return Err(problem("cookie_job_device_mismatch"));
        }
        if job.state == state {
            return Ok(job);
        }
        if now >= job.expires_at {
            job.state = ConnectedCookieJobState::Expired;
            return self.save(tenant_id, &job, revision, "cookie.job.expire");
        }
        let valid = matches!(
            (job.state, state),
            (
                ConnectedCookieJobState::Accepted,
                ConnectedCookieJobState::Dispatched
            ) | (
                ConnectedCookieJobState::Dispatched,
                ConnectedCookieJobState::Running
            )
        );
        if !valid {
            return Err(problem("cookie_job_invalid_state"));
        }
        job.state = state;
        self.save(tenant_id, &job, revision, "cookie.job.transition")
    }

    fn load_required(
        &self,
        tenant_id: &str,
        job_id: &str,
    ) -> Result<(ConnectedCookieJob, u64), Problem> {
        let record: Option<(ConnectedCookieJob, u64)> =
            self.persistence
                .load_versioned(tenant_id, RECORD_TYPE, job_id)?;
        record
            .filter(|(job, _)| job.workspace_id == tenant_id)
            .ok_or_else(|| problem("cookie_job_not_found"))
    }

    fn save(
        &self,
        tenant_id: &str,
        job: &ConnectedCookieJob,
        revision: u64,
        command_id: &str,
    ) -> Result<ConnectedCookieJob, Problem> {
        let value = serde_json::to_value(job).map_err(|_| problem("job_encode_failed"))?;
        let result = self.persistence.execute(ExecuteControlCommand {
            tenant_id: tenant_id.into(),
            command_id: command_id.into(),
            idempotency_key: format!("{}:{}:{}", command_id, job.job_id, revision),
            expected_revisions: Default::default(),
            puts: vec![ControlPut {
                tenant_id: tenant_id.into(),
                record_type: RECORD_TYPE.into(),
                record_id: job.job_id.clone(),
                expected_revision: Some(revision),
                value,
            }],
            deletes: vec![],
            audit: audit(job, command_id),
            response: HttpResponse {
                status: 200,
                headers: Default::default(),
                body: json!({"job": job}),
            },
        })?;
        serde_json::from_value(result.response.body["job"].clone())
            .map_err(|_| problem("job_record_corrupt"))
    }
}

fn validate_enqueue(input: &EnqueueConnectedCookieJob) -> Result<(), Problem> {
    if !matches!(input.command_id.as_str(), "cookie.import" | "cookie.copy") {
        return Err(problem("cookie_job_command_invalid"));
    }
    if input.workspace_id.trim().is_empty()
        || input.device_id.trim().is_empty()
        || input.profile_id.trim().is_empty()
        || input.requested_by.trim().is_empty()
        || input.idempotency_key.trim().is_empty()
        || input.expires_at <= input.now
    {
        return Err(problem("cookie_job_input_invalid"));
    }
    if input.command_id == "cookie.import" {
        if input.staged_import_ref.as_deref().is_none_or(str::is_empty) {
            return Err(problem("cookie_stage_reference_required"));
        }
        if input.source_workspace_id.is_some()
            || input.source_profile_id.is_some()
            || input.source_device_id.is_some()
        {
            return Err(problem("cookie_copy_source_forbidden"));
        }
    } else if input.staged_import_ref.is_some() {
        return Err(problem("cookie_stage_reference_forbidden"));
    }
    if input.command_id == "cookie.copy" {
        let source = [
            input.source_workspace_id.as_deref(),
            input.source_profile_id.as_deref(),
            input.source_device_id.as_deref(),
        ];
        if source.iter().any(|value| value.is_none_or(str::is_empty)) {
            return Err(problem("cookie_copy_source_required"));
        }
    }
    Ok(())
}

fn stable_id(prefix: &str, key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let suffix = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{prefix}_{suffix}")
}

fn audit(job: &ConnectedCookieJob, command_id: &str) -> ControlAuditEntry {
    ControlAuditEntry {
        tenant_id: job.workspace_id.clone(),
        command_id: command_id.into(),
        actor_id: job.requested_by.clone(),
        allowed: true,
        detail: json!({"jobId": job.job_id, "deviceId": job.device_id, "profileId": job.profile_id}),
    }
}

fn problem(code: &'static str) -> Problem {
    Problem::new(
        code,
        "Connected cookie job request is invalid",
        409,
        "cookie-jobs",
    )
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use buglogin_storage::control::MemoryControlRecordStore;

    fn service() -> ConnectedCookieJobService {
        ConnectedCookieJobService::new(ControlPersistence::new(MemoryControlRecordStore::default()))
    }

    fn input(command_id: &str) -> EnqueueConnectedCookieJob {
        EnqueueConnectedCookieJob {
            command_id: command_id.into(),
            workspace_id: "ws_1".into(),
            device_id: "device_1".into(),
            profile_id: "profile_1".into(),
            expected_generation: 7,
            source_workspace_id: (command_id == "cookie.copy").then(|| "ws_source".into()),
            source_profile_id: (command_id == "cookie.copy").then(|| "profile_source".into()),
            source_device_id: (command_id == "cookie.copy").then(|| "device_source".into()),
            staged_import_ref: (command_id == "cookie.import").then(|| "stage_1".into()),
            requested_by: "account_1".into(),
            idempotency_key: "cookie-job-idempotency-0001".into(),
            now: 100,
            expires_at: 200,
        }
    }

    #[test]
    fn enqueue_is_durable_and_replayable() {
        let service = service();
        let first = service.enqueue(input("cookie.import")).unwrap();
        let second = service
            .enqueue(EnqueueConnectedCookieJob {
                ..input("cookie.import")
            })
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(
            service.get("ws_1", &first.job_id, "account_1").unwrap(),
            Some(first)
        );
    }

    #[test]
    fn device_state_machine_and_receipt_binding_are_fail_closed() {
        let service = service();
        let job = service.enqueue(input("cookie.import")).unwrap();
        assert_eq!(
            service
                .claim("ws_1", &job.job_id, "account_1", "device_1", 101)
                .unwrap()
                .state,
            ConnectedCookieJobState::Dispatched
        );
        assert_eq!(
            service
                .acknowledge("ws_1", &job.job_id, "account_1", "device_1", 102)
                .unwrap()
                .state,
            ConnectedCookieJobState::Running
        );
        let bad = ConnectedCookieReceipt {
            operation_id: "wrong".into(),
            job_id: job.job_id.clone(),
            device_id: "device_1".into(),
            profile_id: "profile_1".into(),
            previous_generation: 7,
            new_generation: 8,
            created_count: 1,
            replaced_count: 0,
            deleted_count: 0,
            metadata_checksum: "a".repeat(64),
            completed_at: 103,
        };
        assert_eq!(
            service
                .complete("ws_1", &job.job_id, "account_1", 103, bad)
                .unwrap_err()
                .code,
            "cookie_receipt_binding_invalid"
        );
    }

    #[test]
    fn expiry_and_wrong_device_do_not_mutate_as_running() {
        let service = service();
        let job = service.enqueue(input("cookie.import")).unwrap();
        assert_eq!(
            service
                .claim("ws_1", &job.job_id, "account_1", "other", 101)
                .unwrap_err()
                .code,
            "cookie_job_device_mismatch"
        );
        assert_eq!(
            service
                .claim("ws_1", &job.job_id, "account_1", "device_1", 200)
                .unwrap()
                .state,
            ConnectedCookieJobState::Expired
        );
    }
}
