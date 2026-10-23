use std::collections::BTreeMap;

use buglogin_domain::{errors::Problem, ids::DeviceId};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::control_persistence::ControlPersistence;
use buglogin_storage::control::{
    ControlAuditEntry, ControlPut, ControlRecordKey, ExecuteControlCommand, HttpResponse,
};

const RECORD_TYPE: &str = "connected_cookie_job";

fn control_state_key() -> ControlRecordKey {
    ControlRecordKey {
        tenant_id: "control".into(),
        record_type: "control_state".into(),
        record_id: "primary".into(),
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedCookieJob {
    pub job_id: String,
    pub operation_id: String,
    pub command_id: String,
    pub workspace_id: String,
    pub device_id: String,
    pub profile_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_account_id: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<ConnectedCookieFailureCode>,
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

/// Stable, non-sensitive failure classes reported by the device executor.
/// Free-form error strings are deliberately excluded from the hosted record.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectedCookieFailureCode {
    RuntimeUnavailable,
    ProfileRunning,
    GenerationConflict,
    StageExpired,
    MaterializationFailed,
    VaultUnavailable,
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
        self.enqueue_with_control_revision(input, None)
    }

    pub fn enqueue_with_control_revision(
        &self,
        input: EnqueueConnectedCookieJob,
        control_revision: Option<u64>,
    ) -> Result<ConnectedCookieJob, Problem> {
        validate_enqueue(&input)?;
        let job = ConnectedCookieJob {
            job_id: stable_id("job", &input.idempotency_key),
            operation_id: stable_id("operation", &input.idempotency_key),
            command_id: input.command_id,
            workspace_id: input.workspace_id,
            device_id: input.device_id,
            profile_id: input.profile_id,
            executor_account_id: None,
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
            failure_code: None,
        };
        let value = serde_json::to_value(&job).map_err(|_| problem("job_encode_failed"))?;
        let mut expected_revisions: BTreeMap<ControlRecordKey, u64> = BTreeMap::new();
        if let Some(revision) = control_revision {
            expected_revisions.insert(control_state_key(), revision);
        }
        let result = self.persistence.execute(ExecuteControlCommand {
            tenant_id: job.workspace_id.clone(),
            command_id: "cookie.job.enqueue".into(),
            idempotency_key: job.idempotency_key.clone(),
            expected_revisions,
            puts: vec![ControlPut {
                tenant_id: job.workspace_id.clone(),
                record_type: RECORD_TYPE.into(),
                record_id: job.job_id.clone(),
                expected_revision: Some(0),
                value,
            }],
            deletes: vec![],
            audit: audit(&job, "cookie.job.enqueue", &job.requested_by),
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

    pub fn get_for_device(
        &self,
        tenant_id: &str,
        job_id: &str,
        device_id: &str,
    ) -> Result<Option<ConnectedCookieJob>, Problem> {
        let job: Option<ConnectedCookieJob> =
            self.persistence.load(tenant_id, RECORD_TYPE, job_id)?;
        Ok(job.filter(|job| job.device_id == device_id))
    }

    pub fn inbox(
        &self,
        tenant_id: &str,
        device_id: &str,
        executor_account_id: &str,
        now: u64,
    ) -> Result<Vec<ConnectedCookieJob>, Problem> {
        self.inbox_for_tenants(&[tenant_id.to_owned()], device_id, executor_account_id, now)
    }

    pub fn inbox_for_tenants(
        &self,
        tenant_ids: &[String],
        device_id: &str,
        executor_account_id: &str,
        now: u64,
    ) -> Result<Vec<ConnectedCookieJob>, Problem> {
        const MAX_TENANTS: usize = 4096;
        const MAX_JOBS: usize = 4096;
        if tenant_ids.len() > MAX_TENANTS {
            return Err(Problem::new(
                "cookie_job_inbox_capacity_exceeded",
                "Device inbox exceeds the supported workspace limit",
                503,
                "cookie-jobs",
            ));
        }
        let mut jobs = self
            .persistence
            .list_versioned_for_tenants::<ConnectedCookieJob>(tenant_ids, RECORD_TYPE)?
            .into_iter()
            .filter_map(|(tenant_id, _record_id, job, _revision)| {
                if job.workspace_id != tenant_id || job.device_id != device_id {
                    return None;
                }
                let active = match job.state {
                    ConnectedCookieJobState::Running => {
                        job.executor_account_id.as_deref() == Some(executor_account_id)
                    }
                    ConnectedCookieJobState::Accepted | ConnectedCookieJobState::Dispatched => {
                        now < job.expires_at
                    }
                    _ => false,
                };
                active.then_some(job)
            })
            .collect::<Vec<_>>();
        if jobs.len() > MAX_JOBS {
            return Err(Problem::new(
                "cookie_job_inbox_capacity_exceeded",
                "Device inbox has too many pending jobs",
                503,
                "cookie-jobs",
            ));
        }
        jobs.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.job_id.cmp(&right.job_id))
        });
        Ok(jobs)
    }

    pub fn claim(
        &self,
        tenant_id: &str,
        job_id: &str,
        actor_id: &str,
        device_id: &str,
        now: u64,
    ) -> Result<ConnectedCookieJob, Problem> {
        self.claim_with_control_revision(tenant_id, job_id, actor_id, device_id, now, None)
    }

    pub fn claim_with_control_revision(
        &self,
        tenant_id: &str,
        job_id: &str,
        actor_id: &str,
        device_id: &str,
        now: u64,
        control_revision: Option<u64>,
    ) -> Result<ConnectedCookieJob, Problem> {
        self.transition(
            tenant_id,
            job_id,
            actor_id,
            device_id,
            now,
            ConnectedCookieJobState::Dispatched,
            control_revision,
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
        self.acknowledge_with_control_revision(tenant_id, job_id, actor_id, device_id, now, None)
    }

    pub fn acknowledge_with_control_revision(
        &self,
        tenant_id: &str,
        job_id: &str,
        actor_id: &str,
        device_id: &str,
        now: u64,
        control_revision: Option<u64>,
    ) -> Result<ConnectedCookieJob, Problem> {
        let (mut job, revision) = self.load_required(tenant_id, job_id)?;
        if job.device_id != device_id {
            return Err(problem("cookie_job_device_mismatch"));
        }
        if job.state == ConnectedCookieJobState::Running {
            return if job.executor_account_id.as_deref() == Some(actor_id) {
                Ok(job)
            } else {
                Err(problem("cookie_job_invalid_state"))
            };
        }
        if job.state != ConnectedCookieJobState::Dispatched {
            return Err(problem("cookie_job_invalid_state"));
        }
        if now >= job.expires_at {
            job.state = ConnectedCookieJobState::Expired;
            return self.save(
                tenant_id,
                &job,
                revision,
                "cookie.job.expire",
                actor_id,
                control_revision,
            );
        }
        job.state = ConnectedCookieJobState::Running;
        job.executor_account_id = Some(actor_id.to_owned());
        self.save(
            tenant_id,
            &job,
            revision,
            "cookie.job.transition",
            actor_id,
            control_revision,
        )
    }

    pub fn complete(
        &self,
        tenant_id: &str,
        job_id: &str,
        actor_id: &str,
        device_id: &str,
        now: u64,
        receipt: ConnectedCookieReceipt,
    ) -> Result<ConnectedCookieJob, Problem> {
        self.complete_with_control_revision(
            tenant_id, job_id, actor_id, device_id, now, receipt, None,
        )
    }

    pub fn complete_with_control_revision(
        &self,
        tenant_id: &str,
        job_id: &str,
        actor_id: &str,
        device_id: &str,
        _now: u64,
        receipt: ConnectedCookieReceipt,
        control_revision: Option<u64>,
    ) -> Result<ConnectedCookieJob, Problem> {
        let (mut job, revision) = self.load_required(tenant_id, job_id)?;
        if job.device_id != device_id || job.executor_account_id.as_deref() != Some(actor_id) {
            return Err(if job.device_id != device_id {
                problem("cookie_job_device_mismatch")
            } else {
                problem("cookie_job_executor_mismatch")
            });
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
        let Some(next_generation) = receipt.previous_generation.checked_add(1) else {
            return Err(problem("cookie_receipt_binding_invalid"));
        };
        if receipt.job_id != job.job_id
            || receipt.operation_id != job.operation_id
            || receipt.device_id != job.device_id
            || receipt.profile_id != job.profile_id
            || receipt.previous_generation != job.expected_generation
            || receipt.new_generation != next_generation
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
        self.save(
            tenant_id,
            &job,
            revision,
            "cookie.job.complete",
            actor_id,
            control_revision,
        )
    }

    pub fn fail(
        &self,
        tenant_id: &str,
        job_id: &str,
        actor_id: &str,
        device_id: &str,
        _now: u64,
        failure_code: ConnectedCookieFailureCode,
    ) -> Result<ConnectedCookieJob, Problem> {
        self.fail_with_control_revision(
            tenant_id,
            job_id,
            actor_id,
            device_id,
            _now,
            failure_code,
            None,
        )
    }

    pub fn fail_with_control_revision(
        &self,
        tenant_id: &str,
        job_id: &str,
        actor_id: &str,
        device_id: &str,
        _now: u64,
        failure_code: ConnectedCookieFailureCode,
        control_revision: Option<u64>,
    ) -> Result<ConnectedCookieJob, Problem> {
        let (mut job, revision) = self.load_required(tenant_id, job_id)?;
        if job.device_id != device_id || job.executor_account_id.as_deref() != Some(actor_id) {
            return Err(if job.device_id != device_id {
                problem("cookie_job_device_mismatch")
            } else {
                problem("cookie_job_executor_mismatch")
            });
        }
        if job.state == ConnectedCookieJobState::Failed {
            return if job.failure_code == Some(failure_code) {
                Ok(job)
            } else {
                Err(problem("cookie_job_invalid_state"))
            };
        }
        if job.state != ConnectedCookieJobState::Running {
            return Err(problem("cookie_job_invalid_state"));
        }
        job.state = ConnectedCookieJobState::Failed;
        job.failure_code = Some(failure_code);
        self.save(
            tenant_id,
            &job,
            revision,
            "cookie.job.fail",
            actor_id,
            control_revision,
        )
    }

    pub fn cancel(
        &self,
        tenant_id: &str,
        job_id: &str,
        actor_id: &str,
        now: u64,
    ) -> Result<ConnectedCookieJob, Problem> {
        self.cancel_with_control_revision(tenant_id, job_id, actor_id, now, None)
    }

    pub fn cancel_with_control_revision(
        &self,
        tenant_id: &str,
        job_id: &str,
        actor_id: &str,
        now: u64,
        control_revision: Option<u64>,
    ) -> Result<ConnectedCookieJob, Problem> {
        let (mut job, revision) = self.load_required(tenant_id, job_id)?;
        if job.requested_by != actor_id {
            return Err(problem("cookie_job_forbidden"));
        }
        if job.state == ConnectedCookieJobState::Cancelled {
            return Ok(job);
        }
        if !matches!(
            job.state,
            ConnectedCookieJobState::Accepted | ConnectedCookieJobState::Dispatched
        ) {
            return Err(problem("cookie_job_invalid_state"));
        }
        if now >= job.expires_at {
            job.state = ConnectedCookieJobState::Expired;
            return self.save(
                tenant_id,
                &job,
                revision,
                "cookie.job.expire",
                actor_id,
                control_revision,
            );
        }
        job.state = ConnectedCookieJobState::Cancelled;
        self.save(
            tenant_id,
            &job,
            revision,
            "cookie.job.cancel",
            actor_id,
            control_revision,
        )
    }

    fn transition(
        &self,
        tenant_id: &str,
        job_id: &str,
        actor_id: &str,
        device_id: &str,
        now: u64,
        state: ConnectedCookieJobState,
        control_revision: Option<u64>,
    ) -> Result<ConnectedCookieJob, Problem> {
        let (mut job, revision) = self.load_required(tenant_id, job_id)?;
        if job.device_id != device_id {
            return Err(problem("cookie_job_device_mismatch"));
        }
        if now >= job.expires_at
            && matches!(
                job.state,
                ConnectedCookieJobState::Accepted | ConnectedCookieJobState::Dispatched
            )
        {
            job.state = ConnectedCookieJobState::Expired;
            return self.save(
                tenant_id,
                &job,
                revision,
                "cookie.job.expire",
                actor_id,
                control_revision,
            );
        }
        if job.state == state {
            return Ok(job);
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
        self.save(
            tenant_id,
            &job,
            revision,
            "cookie.job.transition",
            actor_id,
            control_revision,
        )
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
        actor_id: &str,
        control_revision: Option<u64>,
    ) -> Result<ConnectedCookieJob, Problem> {
        let value = serde_json::to_value(job).map_err(|_| problem("job_encode_failed"))?;
        let mut expected_revisions: BTreeMap<ControlRecordKey, u64> = BTreeMap::new();
        if let Some(revision) = control_revision {
            expected_revisions.insert(control_state_key(), revision);
        }
        let result = self.persistence.execute(ExecuteControlCommand {
            tenant_id: tenant_id.into(),
            command_id: command_id.into(),
            idempotency_key: format!("{}:{}:{}", command_id, job.job_id, revision),
            expected_revisions,
            puts: vec![ControlPut {
                tenant_id: tenant_id.into(),
                record_type: RECORD_TYPE.into(),
                record_id: job.job_id.clone(),
                expected_revision: Some(revision),
                value,
            }],
            deletes: vec![],
            audit: audit(job, command_id, actor_id),
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
        || DeviceId::new(input.device_id.clone()).is_err()
        || input.profile_id.trim().is_empty()
        || input.requested_by.trim().is_empty()
        || input.idempotency_key.trim().is_empty()
        || input.expires_at <= input.now
    {
        return Err(problem("cookie_job_input_invalid"));
    }
    if input.expected_generation.checked_add(1).is_none() {
        return Err(problem("cookie_generation_exhausted"));
    }
    if input.command_id == "cookie.import" {
        let Some(stage_reference) = input.staged_import_ref.as_deref() else {
            return Err(problem("cookie_stage_reference_required"));
        };
        if !valid_stage_reference(stage_reference) {
            return Err(problem("cookie_stage_reference_invalid"));
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
        if input
            .source_device_id
            .as_deref()
            .is_none_or(|device_id| DeviceId::new(device_id).is_err())
        {
            return Err(problem("cookie_copy_source_required"));
        }
        if input.source_device_id.as_deref() != Some(input.device_id.as_str()) {
            return Err(problem("cookie_copy_cross_device_dispatch_unavailable"));
        }
    }
    Ok(())
}

fn valid_stage_reference(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn stable_id(prefix: &str, key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let suffix = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{prefix}_{suffix}")
}

fn audit(job: &ConnectedCookieJob, command_id: &str, actor_id: &str) -> ControlAuditEntry {
    ControlAuditEntry {
        tenant_id: job.workspace_id.clone(),
        command_id: command_id.into(),
        actor_id: actor_id.into(),
        allowed: true,
        detail: json!({"jobId": job.job_id, "deviceId": job.device_id, "profileId": job.profile_id}),
    }
}

fn problem(code: &'static str) -> Problem {
    let (title, status) = match code {
        "cookie_job_forbidden" => ("Connected cookie job access denied", 403),
        "cookie_job_executor_mismatch" => ("Connected cookie job executor mismatch", 403),
        "cookie_job_device_mismatch" => ("Connected cookie job device mismatch", 403),
        "cookie_job_not_found" => ("Connected cookie job not found", 404),
        "cookie_job_command_invalid"
        | "cookie_job_input_invalid"
        | "cookie_stage_reference_required"
        | "cookie_stage_reference_invalid"
        | "cookie_stage_reference_forbidden"
        | "cookie_copy_source_required"
        | "cookie_copy_source_forbidden" => ("Connected cookie job input is invalid", 400),
        _ => ("Connected cookie job request is invalid", 409),
    };
    Problem::new(code, title, status, "cookie-jobs")
}

pub fn unix_now() -> Result<u64, Problem> {
    crate::clock::unix_time_seconds("cookie-jobs")
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
            staged_import_ref: (command_id == "cookie.import").then(|| "a".repeat(64)),
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
    fn enqueue_rejects_noncanonical_device_identity() {
        let error = service()
            .enqueue(EnqueueConnectedCookieJob {
                device_id: "device id".into(),
                ..input("cookie.import")
            })
            .unwrap_err();
        assert_eq!(error.code, "cookie_job_input_invalid");
    }

    #[test]
    fn enqueue_accepts_initial_cookie_generation_zero() {
        let service = service();
        let job = service
            .enqueue(EnqueueConnectedCookieJob {
                expected_generation: 0,
                ..input("cookie.import")
            })
            .expect("generation zero is the initial cookie state");

        assert_eq!(job.expected_generation, 0);
    }

    #[test]
    fn enqueue_rejects_non_opaque_stage_references() {
        let error = service()
            .enqueue(EnqueueConnectedCookieJob {
                staged_import_ref: Some("stage_1".into()),
                ..input("cookie.import")
            })
            .unwrap_err();

        assert_eq!(error.code, "cookie_stage_reference_invalid");
        assert_eq!(error.status, 400);
    }

    #[test]
    fn enqueue_rejects_exhausted_cookie_generation() {
        let service = service();
        let error = service
            .enqueue(EnqueueConnectedCookieJob {
                expected_generation: u64::MAX,
                ..input("cookie.import")
            })
            .unwrap_err();

        assert_eq!(error.code, "cookie_generation_exhausted");
    }

    #[test]
    fn inbox_is_scoped_to_the_target_device_and_active_jobs() {
        let service = service();
        let job = service.enqueue(input("cookie.import")).unwrap();
        assert_eq!(
            service
                .inbox("ws_1", "device_other", "executor_account", 150)
                .unwrap(),
            Vec::new()
        );
        assert_eq!(
            service
                .inbox("ws_1", "device_1", "executor_account", 150)
                .unwrap(),
            vec![job.clone()]
        );
        let second_job = service
            .enqueue(EnqueueConnectedCookieJob {
                workspace_id: "ws_2".into(),
                idempotency_key: "cookie-job-idempotency-0002".into(),
                ..input("cookie.import")
            })
            .unwrap();
        let batched = service
            .inbox_for_tenants(
                &["ws_1".into(), "ws_2".into()],
                "device_1",
                "executor_account",
                150,
            )
            .unwrap();
        assert_eq!(batched.len(), 2);
        assert!(batched.contains(&job));
        assert!(batched.contains(&second_job));
        assert_eq!(
            service
                .get_for_device("ws_1", &job.job_id, "device_other")
                .unwrap(),
            None
        );
        service
            .claim("ws_1", &job.job_id, "executor_account", "device_1", 101)
            .unwrap();
        assert_eq!(
            service
                .inbox("ws_1", "device_1", "executor_account", 150)
                .unwrap()[0]
                .state,
            ConnectedCookieJobState::Dispatched
        );
        assert!(
            service
                .inbox("ws_1", "device_1", "executor_account", 200)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn cross_device_copy_is_rejected_until_local_source_transport_exists() {
        let error = service().enqueue(input("cookie.copy")).unwrap_err();
        assert_eq!(error.code, "cookie_copy_cross_device_dispatch_unavailable");
    }

    #[test]
    fn device_state_machine_and_receipt_binding_are_fail_closed() {
        let service = service();
        let job = service.enqueue(input("cookie.import")).unwrap();
        assert_eq!(
            service
                .claim("ws_1", &job.job_id, "executor_account", "device_1", 101)
                .unwrap()
                .state,
            ConnectedCookieJobState::Dispatched
        );
        assert_eq!(
            service
                .acknowledge("ws_1", &job.job_id, "executor_account", "device_1", 102)
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
                .complete(
                    "ws_1",
                    &job.job_id,
                    "executor_account",
                    "device_1",
                    103,
                    bad
                )
                .unwrap_err()
                .code,
            "cookie_receipt_binding_invalid"
        );
    }

    #[test]
    fn device_failure_is_redacted_terminal_and_idempotent() {
        let service = service();
        let job = service.enqueue(input("cookie.import")).unwrap();
        service
            .claim("ws_1", &job.job_id, "executor_account", "device_1", 101)
            .unwrap();
        service
            .acknowledge("ws_1", &job.job_id, "executor_account", "device_1", 102)
            .unwrap();
        let failed = service
            .fail(
                "ws_1",
                &job.job_id,
                "executor_account",
                "device_1",
                103,
                ConnectedCookieFailureCode::MaterializationFailed,
            )
            .unwrap();
        assert_eq!(failed.state, ConnectedCookieJobState::Failed);
        assert_eq!(
            failed.failure_code,
            Some(ConnectedCookieFailureCode::MaterializationFailed)
        );
        assert_eq!(failed.receipt, None);
        assert_eq!(
            service
                .fail(
                    "ws_1",
                    &job.job_id,
                    "executor_account",
                    "device_1",
                    104,
                    ConnectedCookieFailureCode::MaterializationFailed,
                )
                .unwrap(),
            failed
        );
        assert_eq!(
            service
                .fail(
                    "ws_1",
                    &job.job_id,
                    "executor_account",
                    "device_1",
                    104,
                    ConnectedCookieFailureCode::RuntimeUnavailable,
                )
                .unwrap_err()
                .code,
            "cookie_job_invalid_state"
        );
    }

    #[test]
    fn requester_can_cancel_before_execution_but_not_after_running() {
        let service = service();
        let queued = service.enqueue(input("cookie.import")).unwrap();
        let cancelled = service
            .cancel("ws_1", &queued.job_id, "account_1", 101)
            .unwrap();
        assert_eq!(cancelled.state, ConnectedCookieJobState::Cancelled);
        assert_eq!(
            service
                .cancel("ws_1", &queued.job_id, "account_1", 102)
                .unwrap(),
            cancelled
        );
        assert_eq!(
            service
                .cancel("ws_1", &queued.job_id, "another_account", 102)
                .unwrap_err()
                .code,
            "cookie_job_forbidden"
        );

        let running = service
            .enqueue(EnqueueConnectedCookieJob {
                idempotency_key: "cookie-job-idempotency-running-0001".into(),
                ..input("cookie.import")
            })
            .unwrap();
        service
            .claim("ws_1", &running.job_id, "executor_account", "device_1", 103)
            .unwrap();
        service
            .acknowledge("ws_1", &running.job_id, "executor_account", "device_1", 104)
            .unwrap();
        assert_eq!(
            service
                .cancel("ws_1", &running.job_id, "account_1", 105)
                .unwrap_err()
                .code,
            "cookie_job_invalid_state"
        );
    }

    #[test]
    fn expiry_and_wrong_device_do_not_mutate_as_running() {
        let service = service();
        let job = service.enqueue(input("cookie.import")).unwrap();
        assert_eq!(
            service
                .claim("ws_1", &job.job_id, "executor_account", "other", 101)
                .unwrap_err()
                .code,
            "cookie_job_device_mismatch"
        );
        assert_eq!(
            service
                .claim("ws_1", &job.job_id, "executor_account", "device_1", 200)
                .unwrap()
                .state,
            ConnectedCookieJobState::Expired
        );
    }

    #[test]
    fn running_jobs_remain_visible_and_reconcilable_after_enqueue_deadline() {
        let service = service();
        let job = service.enqueue(input("cookie.import")).unwrap();
        service
            .claim("ws_1", &job.job_id, "executor_account", "device_1", 198)
            .unwrap();
        service
            .acknowledge("ws_1", &job.job_id, "executor_account", "device_1", 199)
            .unwrap();
        assert_eq!(
            service
                .acknowledge("ws_1", &job.job_id, "executor_account", "device_1", 201)
                .unwrap()
                .state,
            ConnectedCookieJobState::Running
        );
        assert_eq!(
            service
                .inbox("ws_1", "device_1", "executor_account", 201)
                .unwrap()[0]
                .state,
            ConnectedCookieJobState::Running
        );
        let receipt = ConnectedCookieReceipt {
            operation_id: job.operation_id.clone(),
            job_id: job.job_id.clone(),
            device_id: job.device_id.clone(),
            profile_id: job.profile_id.clone(),
            previous_generation: job.expected_generation,
            new_generation: job.expected_generation + 1,
            created_count: 1,
            replaced_count: 0,
            deleted_count: 0,
            metadata_checksum: "c".repeat(64),
            completed_at: 201,
        };
        assert_eq!(
            service
                .complete(
                    "ws_1",
                    &job.job_id,
                    "different_account",
                    "device_1",
                    202,
                    receipt.clone(),
                )
                .unwrap_err()
                .code,
            "cookie_job_executor_mismatch"
        );
        assert_eq!(
            service
                .complete(
                    "ws_1",
                    &job.job_id,
                    "executor_account",
                    "device_1",
                    202,
                    receipt
                )
                .unwrap()
                .state,
            ConnectedCookieJobState::Completed
        );

        let second_job = service
            .enqueue(EnqueueConnectedCookieJob {
                idempotency_key: "cookie-job-idempotency-late-failure-01".into(),
                ..input("cookie.import")
            })
            .unwrap();
        service
            .claim(
                "ws_1",
                &second_job.job_id,
                "executor_account",
                "device_1",
                198,
            )
            .unwrap();
        service
            .acknowledge(
                "ws_1",
                &second_job.job_id,
                "executor_account",
                "device_1",
                199,
            )
            .unwrap();
        assert_eq!(
            service
                .fail(
                    "ws_1",
                    &second_job.job_id,
                    "executor_account",
                    "device_1",
                    202,
                    ConnectedCookieFailureCode::MaterializationFailed,
                )
                .unwrap()
                .state,
            ConnectedCookieJobState::Failed
        );
    }
}
