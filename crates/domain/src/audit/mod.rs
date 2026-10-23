use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{errors::Problem, ids::AccountId, permissions::ResourceScope};

pub const REDACTED: &str = "<redacted>";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditResource {
    pub resource_type: String,
    pub resource_id: String,
    pub scope: ResourceScope,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditOutcome {
    Succeeded,
    Failed,
    Denied,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEvent {
    pub event_id: String,
    pub occurred_at: u64,
    pub actor_id: AccountId,
    pub resource: AuditResource,
    pub command_id: String,
    pub correlation_id: String,
    pub outcome: AuditOutcome,
    pub metadata: BTreeMap<String, String>,
}

pub type AuditEntry = AuditEvent;

impl AuditEvent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        event_id: impl Into<String>,
        occurred_at: u64,
        actor_id: AccountId,
        resource: AuditResource,
        command_id: impl Into<String>,
        correlation_id: impl Into<String>,
        outcome: AuditOutcome,
        metadata: BTreeMap<String, String>,
    ) -> Result<Self, Problem> {
        let event_id = required(event_id.into(), "event_id")?;
        let command_id = required(command_id.into(), "command_id")?;
        let correlation_id = required(correlation_id.into(), "correlation_id")?;
        required(resource.resource_type.clone(), "resource_type")?;
        required(resource.resource_id.clone(), "resource_id")?;
        Ok(Self {
            event_id,
            occurred_at,
            actor_id,
            resource,
            command_id,
            correlation_id,
            outcome,
            metadata: redact_metadata(metadata),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportAccess {
    pub access_id: String,
    pub support_actor_id: AccountId,
    pub issued_by: AccountId,
    pub scope: ResourceScope,
    pub reason: String,
    pub issued_at: u64,
    pub expires_at: u64,
    pub revoked_at: Option<u64>,
}

impl SupportAccess {
    pub fn new(
        access_id: impl Into<String>,
        support_actor_id: AccountId,
        issued_by: AccountId,
        scope: ResourceScope,
        reason: impl Into<String>,
        issued_at: u64,
        expires_at: u64,
    ) -> Result<Self, Problem> {
        let access_id = required(access_id.into(), "access_id")?;
        let reason = reason.into();
        if reason.trim().is_empty() || reason.chars().count() > 512 {
            return Err(problem(
                "invalid_support_reason",
                "Support reason is invalid",
                400,
            ));
        }
        if expires_at <= issued_at {
            return Err(problem(
                "invalid_support_expiry",
                "Support access expiry is invalid",
                400,
            ));
        }
        Ok(Self {
            access_id,
            support_actor_id,
            issued_by,
            scope,
            reason,
            issued_at,
            expires_at,
            revoked_at: None,
        })
    }

    pub fn allows(&self, actor_id: &AccountId, scope: &ResourceScope, now: u64) -> bool {
        &self.support_actor_id == actor_id
            && self.revoked_at.is_none()
            && now >= self.issued_at
            && now < self.expires_at
            && self.scope.contains(scope)
    }

    pub fn revoke(&mut self, now: u64) {
        self.revoked_at.get_or_insert(now);
    }
}

fn required(value: String, field: &str) -> Result<String, Problem> {
    if value.trim().is_empty() || value.chars().count() > 256 {
        Err(
            problem("invalid_audit_event", "Audit event is invalid", 400)
                .with_field("field", field),
        )
    } else {
        Ok(value)
    }
}

fn redact_metadata(metadata: BTreeMap<String, String>) -> BTreeMap<String, String> {
    metadata
        .into_iter()
        .map(|(key, value)| {
            let value = if is_secret_key(&key) || looks_secret(&value) {
                REDACTED.to_owned()
            } else {
                value
            };
            (key, value)
        })
        .collect()
}

fn is_secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "authorization",
        "cookie",
        "credential",
        "password",
        "proxy",
        "secret",
        "token",
    ]
    .iter()
    .any(|sensitive| key.contains(sensitive))
}

fn looks_secret(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    value.starts_with("bearer ")
        || value.starts_with("basic ")
        || (value.contains("://") && value.contains('@'))
}

fn problem(code: &str, title: &str, status: u16) -> Problem {
    Problem::new(code, title, status, "audit")
}
