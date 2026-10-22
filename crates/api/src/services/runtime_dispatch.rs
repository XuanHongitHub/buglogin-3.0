use std::collections::BTreeMap;

use buglogin_domain::errors::Problem;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::control_persistence::ControlPersistence;
use buglogin_storage::control::{
    ControlAuditEntry, ControlPut, ControlRecordKey, ExecuteControlCommand, HttpResponse,
};

const RECORD_TYPE: &str = "runtime_dispatch_job";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeDispatchState {
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
pub struct RuntimeDispatchJob {
    pub job_id: String,
    pub operation_id: String,
    pub command_id: String,
    pub workspace_id: String,
    pub device_id: String,
    pub profile_id: String,
    pub requested_by: String,
    pub idempotency_key: String,
    pub created_at: u64,
    pub expires_at: u64,
    pub state: RuntimeDispatchState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnqueueRuntimeDispatch {
    pub command_id: String,
    pub workspace_id: String,
    pub device_id: String,
    pub profile_id: String,
    pub requested_by: String,
    pub idempotency_key: String,
    pub now: u64,
    pub expires_at: u64,
    #[serde(default)]
    pub input: Value,
}

#[derive(Clone)]
pub struct RuntimeDispatchService {
    persistence: ControlPersistence,
}

impl RuntimeDispatchService {
    pub fn new(persistence: ControlPersistence) -> Self {
        Self { persistence }
    }

    pub fn enqueue(
        &self,
        input: EnqueueRuntimeDispatch,
        control_revision: Option<u64>,
    ) -> Result<RuntimeDispatchJob, Problem> {
        validate(&input)?;
        let job = RuntimeDispatchJob {
            job_id: stable_id("runtime", &input.idempotency_key),
            operation_id: stable_id("operation", &input.idempotency_key),
            command_id: input.command_id,
            workspace_id: input.workspace_id,
            device_id: input.device_id,
            profile_id: input.profile_id,
            requested_by: input.requested_by,
            idempotency_key: input.idempotency_key,
            created_at: input.now,
            expires_at: input.expires_at,
            state: RuntimeDispatchState::Accepted,
            executor_account_id: None,
            input: Some(input.input),
            result: None,
            failure_code: None,
        };
        let mut expected_revisions = BTreeMap::new();
        if let Some(revision) = control_revision {
            expected_revisions.insert(
                ControlRecordKey {
                    tenant_id: "control".into(),
                    record_type: "control_state".into(),
                    record_id: "primary".into(),
                },
                revision,
            );
        }
        let result = self.persistence.execute(ExecuteControlCommand {
            tenant_id: job.workspace_id.clone(),
            command_id: "runtime.dispatch.enqueue".into(),
            idempotency_key: job.idempotency_key.clone(),
            expected_revisions,
            puts: vec![ControlPut {
                tenant_id: job.workspace_id.clone(),
                record_type: RECORD_TYPE.into(),
                record_id: job.job_id.clone(),
                expected_revision: Some(0),
                value: serde_json::to_value(&job)
                    .map_err(|_| problem("runtime_job_encode_failed"))?,
            }],
            deletes: vec![],
            audit: audit(&job, "runtime.dispatch.enqueue", &job.requested_by),
            response: HttpResponse {
                status: 202,
                headers: Default::default(),
                body: json!({"job": job}),
            },
        })?;
        serde_json::from_value(result.response.body["job"].clone())
            .map_err(|_| problem("runtime_job_record_corrupt"))
    }

    pub fn inbox(
        &self,
        tenant_ids: &[String],
        device_id: &str,
        executor_account_id: &str,
        now: u64,
    ) -> Result<Vec<RuntimeDispatchJob>, Problem> {
        if tenant_ids.len() > 4096 {
            return Err(problem("runtime_job_inbox_capacity_exceeded"));
        }
        let mut jobs = self
            .persistence
            .list_versioned_for_tenants::<RuntimeDispatchJob>(tenant_ids, RECORD_TYPE)?
            .into_iter()
            .filter_map(|(tenant, _, job, _)| {
                (job.workspace_id == tenant
                    && job.device_id == device_id
                    && now < job.expires_at
                    && (matches!(
                        job.state,
                        RuntimeDispatchState::Accepted | RuntimeDispatchState::Dispatched
                    ) || (job.state == RuntimeDispatchState::Running
                        && job.executor_account_id.as_deref() == Some(executor_account_id))))
                .then_some(job)
            })
            .collect::<Vec<_>>();
        if jobs.len() > 4096 {
            return Err(problem("runtime_job_inbox_capacity_exceeded"));
        }
        jobs.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.job_id.cmp(&right.job_id))
        });
        Ok(jobs)
    }

    pub fn get_for_device(
        &self,
        workspace_id: &str,
        job_id: &str,
        device_id: &str,
    ) -> Result<Option<RuntimeDispatchJob>, Problem> {
        Ok(self
            .persistence
            .load_versioned::<RuntimeDispatchJob>(workspace_id, RECORD_TYPE, job_id)?
            .map(|(job, _)| job)
            .filter(|job| job.workspace_id == workspace_id && job.device_id == device_id))
    }

    pub fn claim(
        &self,
        workspace_id: &str,
        job_id: &str,
        device_id: &str,
        executor_account_id: &str,
        now: u64,
    ) -> Result<RuntimeDispatchJob, Problem> {
        self.update(
            workspace_id,
            job_id,
            "runtime.dispatch.claim",
            executor_account_id,
            |job| {
                if job.device_id != device_id {
                    return Err(problem("runtime_job_device_mismatch"));
                }
                match job.state {
                    RuntimeDispatchState::Accepted | RuntimeDispatchState::Dispatched
                        if now < job.expires_at =>
                    {
                        job.state = RuntimeDispatchState::Running;
                        job.executor_account_id = Some(executor_account_id.to_owned());
                        Ok(())
                    }
                    RuntimeDispatchState::Running
                        if job.executor_account_id.as_deref() == Some(executor_account_id) =>
                    {
                        Ok(())
                    }
                    _ => Err(problem("runtime_job_invalid_state")),
                }
            },
        )
    }

    pub fn complete(
        &self,
        workspace_id: &str,
        job_id: &str,
        device_id: &str,
        executor_account_id: &str,
        result: Value,
    ) -> Result<RuntimeDispatchJob, Problem> {
        self.update(
            workspace_id,
            job_id,
            "runtime.dispatch.complete",
            executor_account_id,
            |job| {
                verify_executor(job, device_id, executor_account_id)?;
                if job.state == RuntimeDispatchState::Completed
                    && job.result.as_ref() == Some(&result)
                {
                    return Ok(());
                }
                if job.state != RuntimeDispatchState::Running {
                    return Err(problem("runtime_job_invalid_state"));
                }
                if result.to_string().len() > 1024 {
                    return Err(problem("runtime_job_result_invalid"));
                }
                job.state = RuntimeDispatchState::Completed;
                job.result = Some(result);
                job.input = None;
                Ok(())
            },
        )
    }

    pub fn fail(
        &self,
        workspace_id: &str,
        job_id: &str,
        device_id: &str,
        executor_account_id: &str,
        failure_code: &str,
    ) -> Result<RuntimeDispatchJob, Problem> {
        if failure_code.is_empty()
            || failure_code.len() > 96
            || !failure_code
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err(problem("runtime_job_failure_code_invalid"));
        }
        self.update(
            workspace_id,
            job_id,
            "runtime.dispatch.fail",
            executor_account_id,
            |job| {
                verify_executor(job, device_id, executor_account_id)?;
                if job.state != RuntimeDispatchState::Running {
                    return Err(problem("runtime_job_invalid_state"));
                }
                job.state = RuntimeDispatchState::Failed;
                job.failure_code = Some(failure_code.to_owned());
                job.input = None;
                Ok(())
            },
        )
    }

    fn update(
        &self,
        workspace_id: &str,
        job_id: &str,
        command: &str,
        actor_id: &str,
        mutate: impl FnOnce(&mut RuntimeDispatchJob) -> Result<(), Problem>,
    ) -> Result<RuntimeDispatchJob, Problem> {
        let (mut job, revision) = self
            .persistence
            .load_versioned::<RuntimeDispatchJob>(workspace_id, RECORD_TYPE, job_id)?
            .filter(|(job, _)| job.workspace_id == workspace_id)
            .ok_or_else(|| problem("runtime_job_not_found"))?;
        mutate(&mut job)?;
        let value = serde_json::to_value(&job).map_err(|_| problem("runtime_job_encode_failed"))?;
        self.persistence.execute(ExecuteControlCommand {
            tenant_id: workspace_id.to_owned(),
            command_id: command.into(),
            idempotency_key: format!("{command}-{job_id}-{revision}"),
            expected_revisions: BTreeMap::new(),
            puts: vec![ControlPut {
                tenant_id: workspace_id.to_owned(),
                record_type: RECORD_TYPE.into(),
                record_id: job_id.to_owned(),
                expected_revision: Some(revision),
                value,
            }],
            deletes: vec![],
            audit: audit(&job, command, actor_id),
            response: HttpResponse {
                status: 200,
                headers: Default::default(),
                body: json!({"job": job}),
            },
        })?;
        Ok(job)
    }
}

fn validate(input: &EnqueueRuntimeDispatch) -> Result<(), Problem> {
    if !matches!(input.command_id.as_str(), "profile.launch" | "profile.stop")
        || input.workspace_id.is_empty()
        || input.device_id.is_empty()
        || input.profile_id.is_empty()
        || input.requested_by.is_empty()
        || input.idempotency_key.is_empty()
        || input.idempotency_key.len() > 256
        || input.expires_at <= input.now
        || input.expires_at - input.now > 300
        || input.input.to_string().len() > 32 * 1024
        || !valid_command_input(&input.command_id, &input.input)
    {
        return Err(problem("runtime_job_input_invalid"));
    }
    Ok(())
}

fn valid_command_input(command_id: &str, input: &Value) -> bool {
    match command_id {
        "profile.launch" => {
            let Some(object) = input.as_object() else {
                return false;
            };
            object.len() <= 1
                && object.keys().all(|key| key == "headless")
                && object.get("headless").is_none_or(Value::is_boolean)
        }
        "profile.stop" => input.as_object().is_some_and(serde_json::Map::is_empty),
        _ => false,
    }
}

fn verify_executor(
    job: &RuntimeDispatchJob,
    device_id: &str,
    account_id: &str,
) -> Result<(), Problem> {
    if job.device_id != device_id {
        return Err(problem("runtime_job_device_mismatch"));
    }
    if job.executor_account_id.as_deref() != Some(account_id) {
        return Err(problem("runtime_job_executor_mismatch"));
    }
    Ok(())
}

fn stable_id(prefix: &str, input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    format!(
        "{prefix}_{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

fn audit(job: &RuntimeDispatchJob, command: &str, actor: &str) -> ControlAuditEntry {
    ControlAuditEntry {
        tenant_id: job.workspace_id.clone(),
        command_id: command.into(),
        actor_id: actor.into(),
        allowed: true,
        detail: json!({"jobId": job.job_id, "deviceId": job.device_id, "profileId": job.profile_id, "commandId": job.command_id}),
    }
}

fn problem(code: &'static str) -> Problem {
    let (title, status) = match code {
        "runtime_job_not_found" => ("Runtime dispatch job not found", 404),
        "runtime_job_device_mismatch" | "runtime_job_executor_mismatch" => {
            ("Runtime dispatch executor mismatch", 403)
        }
        "runtime_job_inbox_capacity_exceeded" => ("Runtime dispatch inbox is full", 503),
        "runtime_job_input_invalid"
        | "runtime_job_failure_code_invalid"
        | "runtime_job_result_invalid" => ("Runtime dispatch input is invalid", 400),
        _ => ("Runtime dispatch job state is invalid", 409),
    };
    Problem::new(code, title, status, "runtime-dispatch")
}

#[cfg(test)]
mod tests {
    use super::*;
    use buglogin_storage::control::MemoryControlRecordStore;

    fn service() -> RuntimeDispatchService {
        RuntimeDispatchService::new(ControlPersistence::new(MemoryControlRecordStore::default()))
    }

    fn enqueue_input(command_id: &str) -> EnqueueRuntimeDispatch {
        EnqueueRuntimeDispatch {
            command_id: command_id.into(),
            workspace_id: "ws_runtime".into(),
            device_id: "device_runtime".into(),
            profile_id: "prf_runtime".into(),
            requested_by: "acc_runtime".into(),
            idempotency_key: "runtime-launch-1".into(),
            now: 100,
            expires_at: 200,
            input: if command_id == "profile.launch" {
                json!({"headless": false})
            } else {
                json!({})
            },
        }
    }

    #[test]
    fn dispatch_is_device_bound_and_records_terminal_receipt() {
        let service = service();
        let job = service
            .enqueue(enqueue_input("profile.launch"), None)
            .unwrap();
        assert_eq!(job.state, RuntimeDispatchState::Accepted);
        assert_eq!(
            service
                .inbox(&["ws_runtime".into()], "device_other", "acc_runtime", 101)
                .unwrap()
                .len(),
            0
        );

        let claimed = service
            .claim(
                "ws_runtime",
                &job.job_id,
                "device_runtime",
                "acc_runtime",
                101,
            )
            .unwrap();
        assert_eq!(claimed.state, RuntimeDispatchState::Running);
        let completed = service
            .complete(
                "ws_runtime",
                &job.job_id,
                "device_runtime",
                "acc_runtime",
                json!({"profileId":"prf_runtime","state":"running"}),
            )
            .unwrap();
        assert_eq!(completed.state, RuntimeDispatchState::Completed);
        assert_eq!(completed.input, None);
        assert_eq!(completed.result.unwrap()["state"], "running");
    }

    #[test]
    fn dispatch_rejects_caller_selected_browser_paths_and_wrong_executor() {
        let service = service();
        let error = service
            .enqueue(
                EnqueueRuntimeDispatch {
                    input: json!({"profilesRoot":"C:/untrusted"}),
                    ..enqueue_input("profile.launch")
                },
                None,
            )
            .unwrap_err();
        assert_eq!(error.code, "runtime_job_input_invalid");

        let job = service
            .enqueue(enqueue_input("profile.stop"), None)
            .unwrap();
        let error = service
            .claim(
                "ws_runtime",
                &job.job_id,
                "device_other",
                "acc_runtime",
                101,
            )
            .unwrap_err();
        assert_eq!(error.code, "runtime_job_device_mismatch");
    }
}
