mod protocol;
mod redis_backend;

use std::{collections::BTreeMap, fmt, sync::Arc, time::Duration};

use async_trait::async_trait;
use buglogin_domain::{
    errors::Problem,
    ids::{ResourceId, WorkspaceId},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use redis_backend::{RedisCoordination, RedisCoordinationConfig};

#[derive(Clone, Eq, PartialEq)]
pub struct CoordinationAuth {
    actor_id: String,
    bearer_token: String,
}

impl CoordinationAuth {
    pub fn new(
        actor_id: impl Into<String>,
        bearer_token: impl Into<String>,
    ) -> Result<Self, Problem> {
        let actor_id = actor_id.into();
        let bearer_token = bearer_token.into();
        protocol::validate_identifier("actorId", &actor_id)?;
        if bearer_token.is_empty() || bearer_token.len() > 4_096 {
            return Err(protocol::invalid("invalid_coordination_credential"));
        }
        Ok(Self {
            actor_id,
            bearer_token,
        })
    }
    pub fn actor_id(&self) -> &str {
        &self.actor_id
    }
    pub fn bearer_token(&self) -> &str {
        &self.bearer_token
    }
}

impl fmt::Debug for CoordinationAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CoordinationAuth")
            .field("actor_id", &self.actor_id)
            .field("bearer_token", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoordinationOperation {
    LeaseAcquire,
    LeaseRenew,
    LeaseRelease,
    PresenceWrite,
    PresenceRead,
    EventPublish,
    EventRead,
    ManifestRead,
    ManifestWrite,
    PayloadWrite,
}

#[derive(Clone, Debug)]
pub struct AuthorizationRequest<'a> {
    pub operation: CoordinationOperation,
    pub workspace_id: &'a WorkspaceId,
    pub resource_id: Option<&'a ResourceId>,
}

#[async_trait]
pub trait CoordinationAuthorizer: Send + Sync {
    async fn authorize(
        &self,
        auth: &CoordinationAuth,
        request: AuthorizationRequest<'_>,
    ) -> Result<(), Problem>;
}

#[derive(Debug, Default)]
pub struct DenyAllAuthorizer;

#[async_trait]
impl CoordinationAuthorizer for DenyAllAuthorizer {
    async fn authorize(
        &self,
        _auth: &CoordinationAuth,
        _request: AuthorizationRequest<'_>,
    ) -> Result<(), Problem> {
        Err(Problem::new(
            "coordination_forbidden",
            "Coordination access denied",
            403,
            "sync-coordination",
        ))
    }
}

#[derive(Clone, Debug)]
pub struct CoordinationScope<'a> {
    pub workspace_id: &'a WorkspaceId,
    pub auth: &'a CoordinationAuth,
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaseGrant {
    pub resource_id: ResourceId,
    pub workspace_id: WorkspaceId,
    pub owner_id: String,
    pub owner_token: String,
    pub fencing_token: u64,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
}

impl fmt::Debug for LeaseGrant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LeaseGrant")
            .field("resource_id", &self.resource_id)
            .field("workspace_id", &self.workspace_id)
            .field("owner_id", &self.owner_id)
            .field("owner_token", &"[REDACTED]")
            .field("fencing_token", &self.fencing_token)
            .field("issued_at_ms", &self.issued_at_ms)
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Presence {
    pub session_id: String,
    pub actor_id: String,
    pub connected_at_ms: u64,
    pub last_seen_at_ms: u64,
    pub expires_at_ms: u64,
    pub metadata: BTreeMap<String, String>,
}

impl Presence {
    pub fn encode(&self) -> Result<String, Problem> {
        protocol::encode(self)
    }
    pub fn decode(encoded: &str) -> Result<Self, Problem> {
        protocol::decode(encoded)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventResource {
    pub resource_type: String,
    pub id: String,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoordinationEvent {
    pub event_id: String,
    pub event_version: String,
    pub name: String,
    pub occurred_at_ms: u64,
    pub correlation_id: String,
    pub causation_id: String,
    pub actor_id: String,
    pub resource: EventResource,
    pub data: Value,
}

impl CoordinationEvent {
    pub fn encode(&self) -> Result<String, Problem> {
        protocol::encode(self)
    }
    pub fn decode(encoded: &str) -> Result<Self, Problem> {
        protocol::decode(encoded)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedEvent {
    pub event_id: String,
    pub stream_id: String,
    pub duplicate: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StreamEvent {
    pub stream_id: String,
    pub event: CoordinationEvent,
}

#[async_trait]
pub trait CoordinationBackend: Send + Sync {
    async fn acquire_lease(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        ttl: Duration,
    ) -> Result<LeaseGrant, Problem>;
    async fn acquire_lease_idempotent(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        ttl: Duration,
        _idempotency_key: &str,
    ) -> Result<LeaseGrant, Problem> {
        self.acquire_lease(scope, resource_id, ttl).await
    }
    async fn renew_lease(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        ttl: Duration,
    ) -> Result<LeaseGrant, Problem>;
    async fn release_lease(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
    ) -> Result<(), Problem>;
    async fn validate_write_authority(
        &self,
        _scope: CoordinationScope<'_>,
        _resource_id: &ResourceId,
        _owner_token: &str,
        _fencing_token: u64,
    ) -> Result<(), Problem> {
        Err(Problem::new(
            "write_authority_validation_unsupported",
            "Write authority validation is unsupported",
            501,
            "sync-coordination",
        ))
    }
    async fn heartbeat_presence(
        &self,
        scope: CoordinationScope<'_>,
        session_id: &str,
        metadata: BTreeMap<String, String>,
        ttl: Duration,
    ) -> Result<Presence, Problem>;
    async fn list_presence(&self, scope: CoordinationScope<'_>) -> Result<Vec<Presence>, Problem>;
    async fn publish_event(
        &self,
        scope: CoordinationScope<'_>,
        idempotency_key: &str,
        event: CoordinationEvent,
    ) -> Result<PublishedEvent, Problem>;
    async fn read_events(
        &self,
        scope: CoordinationScope<'_>,
        after_stream_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StreamEvent>, Problem>;

    async fn current_manifest(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
    ) -> Result<Option<crate::manifest::SyncManifest>, Problem>;

    async fn commit_manifest(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        manifest: crate::manifest::SyncManifest,
    ) -> Result<(), Problem>;
}

pub(crate) type SharedAuthorizer = Arc<dyn CoordinationAuthorizer>;
