use std::collections::{BTreeMap, VecDeque};

use buglogin_domain::{
    errors::Problem,
    permissions::{Action, Capability, Grant, Principal, ResourceScope, is_allowed},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const CAPABILITY: &str = "operation.read";
const DEFAULT_CAPACITY: usize = 1_024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperationStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl OperationStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub operation_id: String,
    pub command_id: String,
    pub scope: ResourceScope,
    pub status: OperationStatus,
    pub progress: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Problem>,
}

impl Operation {
    pub fn queued(
        operation_id: impl Into<String>,
        command_id: impl Into<String>,
        scope: ResourceScope,
    ) -> Result<Self, Problem> {
        let operation_id = operation_id.into();
        let command_id = command_id.into();
        validate_identifier(&operation_id, "operation_id")?;
        validate_identifier(&command_id, "command_id")?;
        Ok(Self {
            operation_id,
            command_id,
            scope,
            status: OperationStatus::Queued,
            progress: Some(0.0),
            result: None,
            error: None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct OperationService {
    capacity: usize,
    operations: BTreeMap<String, Operation>,
    insertion_order: VecDeque<String>,
}

impl Default for OperationService {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY).expect("default operation capacity is non-zero")
    }
}

impl OperationService {
    pub fn with_capacity(capacity: usize) -> Result<Self, Problem> {
        if capacity == 0 {
            return Err(problem(
                "invalid_operation_capacity",
                "Operation capacity must be greater than zero",
                400,
            ));
        }
        Ok(Self {
            capacity,
            operations: BTreeMap::new(),
            insertion_order: VecDeque::new(),
        })
    }

    pub fn insert(&mut self, operation: Operation) -> Result<(), Problem> {
        validate_operation(&operation)?;
        if self.operations.contains_key(&operation.operation_id) {
            return Err(problem("operation_exists", "Operation already exists", 409));
        }
        self.make_room()?;
        self.insertion_order
            .push_back(operation.operation_id.clone());
        self.operations
            .insert(operation.operation_id.clone(), operation);
        Ok(())
    }

    pub(crate) fn find(&self, operation_id: &str) -> Option<&Operation> {
        self.operations.get(operation_id)
    }

    pub fn get<'a>(
        &'a self,
        principal: &Principal,
        grants: &[Grant],
        operation_id: &str,
    ) -> Result<&'a Operation, Problem> {
        let operation = self
            .operations
            .get(operation_id)
            .ok_or_else(|| problem("operation_not_found", "Operation not found", 404))?;
        authorize(principal, grants, &operation.scope)?;
        Ok(operation)
    }

    pub fn list<'a>(
        &'a self,
        principal: &Principal,
        grants: &[Grant],
        scope: &ResourceScope,
    ) -> Result<Vec<&'a Operation>, Problem> {
        authorize(principal, grants, scope)?;
        Ok(self
            .insertion_order
            .iter()
            .filter_map(|id| self.operations.get(id))
            .filter(|operation| scope.contains(&operation.scope))
            .collect())
    }

    pub fn transition(
        &mut self,
        operation_id: &str,
        status: OperationStatus,
        progress: Option<f64>,
        result: Option<Value>,
        error: Option<Problem>,
    ) -> Result<&Operation, Problem> {
        let operation = self
            .operations
            .get_mut(operation_id)
            .ok_or_else(|| problem("operation_not_found", "Operation not found", 404))?;
        validate_transition(operation.status, status)?;
        validate_progress(status, progress)?;
        if let Some(value) = result.as_ref() {
            reject_secrets(value)?;
        }
        if let Some(value) = error.as_ref() {
            reject_problem_secrets(value)?;
        }
        validate_outcome(status, result.as_ref(), error.as_ref())?;
        operation.status = status;
        operation.progress = progress;
        operation.result = result;
        operation.error = error;
        Ok(operation)
    }

    pub fn len(&self) -> usize {
        self.operations.len()
    }
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    fn make_room(&mut self) -> Result<(), Problem> {
        if self.operations.len() < self.capacity {
            return Ok(());
        }
        let Some(index) = self.insertion_order.iter().position(|id| {
            self.operations
                .get(id)
                .is_some_and(|op| op.status.is_terminal())
        }) else {
            return Err(
                problem("operation_store_full", "Operation store is full", 503).retryable(true),
            );
        };
        let id = self
            .insertion_order
            .remove(index)
            .expect("terminal operation index exists");
        self.operations.remove(&id);
        Ok(())
    }
}

fn validate_operation(operation: &Operation) -> Result<(), Problem> {
    validate_identifier(&operation.operation_id, "operation_id")?;
    validate_identifier(&operation.command_id, "command_id")?;
    validate_progress(operation.status, operation.progress)?;
    if let Some(value) = operation.result.as_ref() {
        reject_secrets(value)?;
    }
    if let Some(value) = operation.error.as_ref() {
        reject_problem_secrets(value)?;
    }
    validate_outcome(
        operation.status,
        operation.result.as_ref(),
        operation.error.as_ref(),
    )
}

fn validate_transition(from: OperationStatus, to: OperationStatus) -> Result<(), Problem> {
    if matches!(
        (from, to),
        (
            OperationStatus::Queued,
            OperationStatus::Running | OperationStatus::Failed | OperationStatus::Cancelled
        ) | (
            OperationStatus::Running,
            OperationStatus::Completed | OperationStatus::Failed | OperationStatus::Cancelled
        )
    ) {
        Ok(())
    } else {
        Err(Problem::invalid_transition(
            "operation",
            format!("{from:?}").to_lowercase(),
            format!("{to:?}").to_lowercase(),
        ))
    }
}

fn validate_progress(status: OperationStatus, progress: Option<f64>) -> Result<(), Problem> {
    if progress.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
        return Err(problem(
            "invalid_operation_progress",
            "Operation progress must be between zero and one",
            400,
        ));
    }
    if status == OperationStatus::Completed && progress != Some(1.0) {
        return Err(problem(
            "invalid_operation_progress",
            "Completed operation progress must equal one",
            400,
        ));
    }
    Ok(())
}

fn validate_outcome(
    status: OperationStatus,
    result: Option<&Value>,
    error: Option<&Problem>,
) -> Result<(), Problem> {
    let valid = match status {
        OperationStatus::Queued | OperationStatus::Running | OperationStatus::Cancelled => {
            result.is_none() && error.is_none()
        }
        OperationStatus::Completed => result.is_some() && error.is_none(),
        OperationStatus::Failed => result.is_none() && error.is_some(),
    };
    if valid {
        Ok(())
    } else {
        Err(problem(
            "invalid_operation_outcome",
            "Operation outcome does not match status",
            400,
        ))
    }
}

fn validate_identifier(value: &str, field: &str) -> Result<(), Problem> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(problem(
            "invalid_operation_identifier",
            "Operation identifier is invalid",
            400,
        )
        .with_field("field", field));
    }
    Ok(())
}

fn reject_problem_secrets(value: &Problem) -> Result<(), Problem> {
    let json = serde_json::to_value(value).map_err(|_| {
        problem(
            "operation_serialization_failed",
            "Operation serialization failed",
            500,
        )
    })?;
    reject_secrets(&json)
}

pub(crate) fn reject_secrets(value: &Value) -> Result<(), Problem> {
    match value {
        Value::Object(values) => {
            for (key, value) in values {
                let key = key.to_ascii_lowercase();
                let metadata_key = matches!(
                    key.as_str(),
                    "secretscan"
                        | "secretfieldspresent"
                        | "fileswithsecretfields"
                        | "containssecrets"
                        | "containscookievalues"
                        | "redactionpolicy"
                );
                let material_key = key == "secret"
                    || key.contains("token")
                    || key.contains("password")
                    || key.contains("credential")
                    || key == "cookie"
                    || key.contains("authorization")
                    || key.contains("api_key")
                    || key.contains("apikey")
                    || key.contains("private_key")
                    || key.contains("secret_value")
                    || key.contains("cookie_value");
                if !metadata_key && material_key {
                    return Err(problem(
                        "secret_material_rejected",
                        "Secret material is not allowed",
                        400,
                    ));
                }
                reject_secrets(value)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                reject_secrets(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn authorize(
    principal: &Principal,
    grants: &[Grant],
    scope: &ResourceScope,
) -> Result<(), Problem> {
    let capability = Capability::new(CAPABILITY)
        .map_err(|_| problem("invalid_capability", "Invalid capability", 500))?;
    if is_allowed(principal, &capability, Action::Read, scope, grants) {
        Ok(())
    } else {
        Err(problem(
            "operation_forbidden",
            "Operation access denied",
            403,
        ))
    }
}

fn problem(code: &str, title: &str, status: u16) -> Problem {
    Problem::new(code, title, status, "operation")
}
