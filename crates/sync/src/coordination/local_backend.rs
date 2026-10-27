//! Process-scoped coordination for the standalone local API.
//!
//! This backend keeps leases, presence, events, and manifests in memory. It is
//! shared by clones in one process only; it is intentionally unsuitable for
//! multi-process deployments, production, or coordination across devices.
//! Callers must authenticate and authorize requests before invoking it.

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use buglogin_domain::{
    errors::Problem,
    ids::{ResourceId, WorkspaceId},
    lease::{MAX_TTL_SECONDS, MIN_TTL_SECONDS},
};
use sha2::{Digest, Sha256};

use super::{
    CoordinationBackend, CoordinationEvent, CoordinationScope, EventResource, LeaseGrant,
    LeaseStatus, Presence, PublishedEvent, StreamEvent, protocol,
};
use crate::manifest::SyncManifest;

const IDEMPOTENCY_TTL: Duration = Duration::from_secs(86_400);
const MAX_EVENTS: usize = 10_000;

type ScopeKey = (String, String);

#[derive(Clone, Default)]
pub struct LocalCoordination {
    inner: Arc<Mutex<LocalState>>,
}

#[derive(Default)]
struct LocalState {
    leases: HashMap<ScopeKey, LeaseGrant>,
    fencing_counters: HashMap<ScopeKey, u64>,
    acquire_replays: HashMap<(String, String), TimedReplay<LeaseGrant>>,
    renew_replays: HashMap<(String, String), TimedReplay<LeaseGrant>>,
    release_replays: HashMap<(String, String), TimedReplay<()>>,
    presence: HashMap<(String, String), Presence>,
    event_replays: HashMap<(String, String), TimedReplay<PublishedEvent>>,
    events: HashMap<String, VecDeque<StreamEvent>>,
    event_sequences: HashMap<String, u64>,
    manifests: HashMap<ScopeKey, SyncManifest>,
}

struct TimedReplay<T> {
    fingerprint: String,
    value: T,
    expires_at_ms: u64,
}

impl LocalCoordination {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, LocalState>, Problem> {
        self.inner.lock().map_err(|_| unavailable())
    }
}

#[async_trait]
impl CoordinationBackend for LocalCoordination {
    async fn readiness(&self) -> Result<(), Problem> {
        self.lock().map(|_| ())
    }

    async fn acquire_lease_idempotent(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        ttl: Duration,
        idempotency_key: &str,
    ) -> Result<LeaseGrant, Problem> {
        validate_idempotency_key(idempotency_key)?;
        let ttl_ms = validate_ttl(ttl)?;
        let now = now_ms()?;
        let expires_at_ms = now.checked_add(ttl_ms).ok_or_else(invalid_ttl)?;
        let key = scope_key(scope.workspace_id, resource_id);
        let replay_key = (
            scope.workspace_id.as_str().to_owned(),
            idempotency_key.to_owned(),
        );
        let fingerprint = fingerprint(&[
            scope.auth.actor_id(),
            resource_id.as_str(),
            &ttl_ms.to_string(),
        ]);
        let mut state = self.lock()?;
        prune_replays(&mut state, now);

        if let Some(replay) = state.acquire_replays.get(&replay_key) {
            if replay.fingerprint != fingerprint {
                return Err(idempotency_conflict());
            }
            if state.leases.get(&key).is_some_and(|current| {
                current.owner_id == replay.value.owner_id
                    && current.owner_token == replay.value.owner_token
                    && current.fencing_token == replay.value.fencing_token
                    && current.expires_at_ms > now
            }) {
                let mut grant = replay.value.clone();
                grant.expires_at_ms = state.leases[&key].expires_at_ms;
                return Ok(grant);
            }
            state.acquire_replays.remove(&replay_key);
        }

        if let Some(current) = state.leases.get(&key) {
            if current.expires_at_ms > now {
                return Err(Problem::new(
                    "lease_held",
                    "Resource lease is already held",
                    409,
                    "sync-coordination",
                )
                .with_field("ownerId", current.owner_id.clone())
                .with_field("expiresAtMs", current.expires_at_ms));
            }
            state.leases.remove(&key);
        }

        let fence = state.fencing_counters.entry(key.clone()).or_default();
        *fence = fence.checked_add(1).ok_or_else(invalid_fence)?;
        let grant = LeaseGrant {
            resource_id: resource_id.clone(),
            workspace_id: scope.workspace_id.clone(),
            owner_id: scope.auth.actor_id().to_owned(),
            owner_token: secure_token()?,
            fencing_token: *fence,
            issued_at_ms: now,
            expires_at_ms,
        };
        state.leases.insert(key, grant.clone());
        state.acquire_replays.insert(
            replay_key,
            TimedReplay {
                fingerprint,
                value: grant.clone(),
                expires_at_ms: now.saturating_add(IDEMPOTENCY_TTL.as_millis() as u64),
            },
        );
        Ok(grant)
    }

    async fn renew_lease_idempotent(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        ttl: Duration,
        idempotency_key: &str,
    ) -> Result<LeaseGrant, Problem> {
        validate_owner_token(owner_token)?;
        validate_idempotency_key(idempotency_key)?;
        let ttl_ms = validate_ttl(ttl)?;
        let now = now_ms()?;
        let key = scope_key(scope.workspace_id, resource_id);
        let replay_key = (
            scope.workspace_id.as_str().to_owned(),
            idempotency_key.to_owned(),
        );
        let fingerprint = fingerprint(&[
            scope.auth.actor_id(),
            resource_id.as_str(),
            owner_token,
            &ttl_ms.to_string(),
        ]);
        let mut state = self.lock()?;
        prune_replays(&mut state, now);
        if let Some(replay) = state.renew_replays.get(&replay_key) {
            return if replay.fingerprint == fingerprint {
                Ok(replay.value.clone())
            } else {
                Err(idempotency_conflict())
            };
        }
        let current = state.leases.get_mut(&key).ok_or_else(lease_missing)?;
        if current.expires_at_ms <= now {
            state.leases.remove(&key);
            return Err(Problem::new(
                "lease_expired",
                "Resource lease is expired",
                409,
                "sync-coordination",
            ));
        }
        if current.owner_id != scope.auth.actor_id() || current.owner_token != owner_token {
            return Err(lease_not_owner());
        }
        current.expires_at_ms = now.checked_add(ttl_ms).ok_or_else(invalid_ttl)?;
        let grant = current.clone();
        state.renew_replays.insert(
            replay_key,
            TimedReplay {
                fingerprint,
                value: grant.clone(),
                expires_at_ms: now.saturating_add(IDEMPOTENCY_TTL.as_millis() as u64),
            },
        );
        Ok(grant)
    }

    async fn release_lease_idempotent(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        idempotency_key: &str,
    ) -> Result<(), Problem> {
        validate_owner_token(owner_token)?;
        validate_idempotency_key(idempotency_key)?;
        let now = now_ms()?;
        let key = scope_key(scope.workspace_id, resource_id);
        let replay_key = (
            scope.workspace_id.as_str().to_owned(),
            idempotency_key.to_owned(),
        );
        let fingerprint = fingerprint(&[scope.auth.actor_id(), resource_id.as_str(), owner_token]);
        let mut state = self.lock()?;
        prune_replays(&mut state, now);
        if let Some(replay) = state.release_replays.get(&replay_key) {
            return if replay.fingerprint == fingerprint {
                Ok(())
            } else {
                Err(idempotency_conflict())
            };
        }
        if let Some(current) = state.leases.get(&key)
            && current.expires_at_ms > now
            && (current.owner_id != scope.auth.actor_id() || current.owner_token != owner_token)
        {
            return Err(lease_not_owner());
        }
        state.leases.remove(&key);
        state.release_replays.insert(
            replay_key,
            TimedReplay {
                fingerprint,
                value: (),
                expires_at_ms: now.saturating_add(IDEMPOTENCY_TTL.as_millis() as u64),
            },
        );
        Ok(())
    }

    async fn lease_status(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
    ) -> Result<Option<LeaseStatus>, Problem> {
        let now = now_ms()?;
        let mut state = self.lock()?;
        let key = scope_key(scope.workspace_id, resource_id);
        let Some(lease) = state.leases.get(&key) else {
            return Ok(None);
        };
        if lease.expires_at_ms <= now {
            state.leases.remove(&key);
            return Ok(None);
        }
        Ok(Some(LeaseStatus {
            resource_id: lease.resource_id.clone(),
            workspace_id: lease.workspace_id.clone(),
            owner_id: lease.owner_id.clone(),
            fencing_token: lease.fencing_token,
            issued_at_ms: lease.issued_at_ms,
            expires_at_ms: lease.expires_at_ms,
        }))
    }

    async fn validate_write_authority(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        fencing_token: u64,
    ) -> Result<(), Problem> {
        validate_owner_token(owner_token)?;
        if fencing_token == 0 {
            return Err(Problem::new(
                "invalid_fencing_token",
                "Invalid fencing token",
                400,
                "sync-coordination",
            ));
        }
        let now = now_ms()?;
        let state = self.lock()?;
        let key = scope_key(scope.workspace_id, resource_id);
        let Some(lease) = state.leases.get(&key) else {
            return Err(lease_missing());
        };
        if lease.expires_at_ms <= now {
            return Err(Problem::new(
                "lease_expired",
                "Resource lease is expired",
                409,
                "sync-coordination",
            ));
        }
        if lease.owner_id != scope.auth.actor_id() || lease.owner_token != owner_token {
            return Err(lease_not_owner());
        }
        if lease.fencing_token != fencing_token {
            return Err(Problem::new(
                "stale_fencing_token",
                "Stale fencing token",
                409,
                "sync-coordination",
            ));
        }
        Ok(())
    }

    async fn heartbeat_presence(
        &self,
        scope: CoordinationScope<'_>,
        session_id: &str,
        metadata: BTreeMap<String, String>,
        ttl: Duration,
    ) -> Result<Presence, Problem> {
        protocol::validate_identifier("sessionId", session_id)?;
        validate_metadata(&metadata)?;
        let ttl_ms = validate_ttl(ttl)?;
        let now = now_ms()?;
        let expires_at_ms = now.checked_add(ttl_ms).ok_or_else(invalid_ttl)?;
        let key = (
            scope.workspace_id.as_str().to_owned(),
            session_id.to_owned(),
        );
        let mut state = self.lock()?;
        let connected_at_ms = state
            .presence
            .get(&key)
            .filter(|presence| presence.expires_at_ms > now)
            .map_or(now, |presence| presence.connected_at_ms);
        let presence = Presence {
            session_id: session_id.to_owned(),
            actor_id: scope.auth.actor_id().to_owned(),
            connected_at_ms,
            last_seen_at_ms: now,
            expires_at_ms,
            metadata,
        };
        state.presence.insert(key, presence.clone());
        Ok(presence)
    }

    async fn list_presence(&self, scope: CoordinationScope<'_>) -> Result<Vec<Presence>, Problem> {
        let now = now_ms()?;
        let mut state = self.lock()?;
        state.presence.retain(|(workspace, _), presence| {
            workspace != scope.workspace_id.as_str() || presence.expires_at_ms > now
        });
        let mut result = state
            .presence
            .iter()
            .filter(|((workspace, _), _)| workspace == scope.workspace_id.as_str())
            .map(|(_, presence)| presence.clone())
            .collect::<Vec<_>>();
        result.sort_by(|left, right| left.session_id.cmp(&right.session_id));
        Ok(result)
    }

    async fn publish_event(
        &self,
        scope: CoordinationScope<'_>,
        idempotency_key: &str,
        event: CoordinationEvent,
    ) -> Result<PublishedEvent, Problem> {
        validate_idempotency_key(idempotency_key)?;
        validate_event(&scope, &event)?;
        let now = now_ms()?;
        let payload = serde_json::to_string(&event).map_err(|_| invalid_event())?;
        let fingerprint = fingerprint(&[&payload]);
        let replay_key = (
            scope.workspace_id.as_str().to_owned(),
            idempotency_key.to_owned(),
        );
        let mut state = self.lock()?;
        prune_replays(&mut state, now);
        if let Some(replay) = state.event_replays.get(&replay_key) {
            return if replay.fingerprint == fingerprint {
                let mut result = replay.value.clone();
                result.duplicate = true;
                Ok(result)
            } else {
                Err(idempotency_conflict())
            };
        }
        let stream_id = append_event(&mut state, scope.workspace_id, event.clone())?;
        let result = PublishedEvent {
            event_id: event.event_id,
            stream_id,
            duplicate: false,
        };
        state.event_replays.insert(
            replay_key,
            TimedReplay {
                fingerprint,
                value: result.clone(),
                expires_at_ms: now.saturating_add(IDEMPOTENCY_TTL.as_millis() as u64),
            },
        );
        Ok(result)
    }

    async fn read_events(
        &self,
        scope: CoordinationScope<'_>,
        after_stream_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StreamEvent>, Problem> {
        if !(1..=1_000).contains(&limit) {
            return Err(protocol::invalid("invalid_event_read_limit"));
        }
        let cursor = after_stream_id.map(parse_stream_id).transpose()?;
        let state = self.lock()?;
        let Some(events) = state.events.get(scope.workspace_id.as_str()) else {
            return Ok(Vec::new());
        };
        if let (Some(cursor), Some(first)) = (cursor, events.front())
            && parse_stream_id(&first.stream_id)? > cursor
        {
            return Err(Problem::new(
                "sync_history_gap_resnapshot_required",
                "Event stream history gap detected. Resnapshot required.",
                409,
                "sync-coordination",
            ));
        }
        Ok(events
            .iter()
            .filter(|entry| {
                cursor.is_none_or(|cursor| {
                    parse_stream_id(&entry.stream_id).is_ok_and(|id| id > cursor)
                })
            })
            .take(limit)
            .cloned()
            .collect())
    }

    async fn current_manifest(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
    ) -> Result<Option<SyncManifest>, Problem> {
        let state = self.lock()?;
        Ok(state
            .manifests
            .get(&scope_key(scope.workspace_id, resource_id))
            .cloned())
    }

    async fn commit_manifest(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        manifest: SyncManifest,
    ) -> Result<(), Problem> {
        validate_owner_token(owner_token)?;
        manifest.validate()?;
        if &manifest.workspace_id != scope.workspace_id || &manifest.resource_id != resource_id {
            return Err(Problem::new(
                "manifest_scope_mismatch",
                "Manifest scope does not match authorization scope",
                409,
                "sync-coordination",
            ));
        }
        let now = now_ms()?;
        let key = scope_key(scope.workspace_id, resource_id);
        let mut state = self.lock()?;
        let lease = state.leases.get(&key).ok_or_else(lease_missing)?;
        if lease.expires_at_ms <= now {
            return Err(Problem::new(
                "lease_expired",
                "Resource lease is expired",
                409,
                "sync-coordination",
            ));
        }
        if lease.owner_id != scope.auth.actor_id() || lease.owner_token != owner_token {
            return Err(lease_not_owner());
        }
        if lease.fencing_token != manifest.fencing_token {
            return Err(Problem::new(
                "stale_manifest_fencing_token",
                "Stale manifest fencing token",
                409,
                "sync-coordination",
            ));
        }
        let payload = serde_json::to_string(&manifest).map_err(|_| invalid_manifest())?;
        if let Some(current) = state.manifests.get(&key) {
            if manifest.generation == current.generation {
                if serde_json::to_string(current).map_err(|_| invalid_manifest())? == payload {
                    return Ok(());
                }
                return Err(Problem::new(
                    "idempotency_conflict",
                    "Manifest commit conflict with different payload",
                    409,
                    "sync-coordination",
                ));
            }
            if current.generation.checked_add(1) != Some(manifest.generation)
                || manifest.parent_generation != Some(current.generation)
            {
                return Err(Problem::new(
                    "stale_manifest_generation",
                    "Stale or gap in manifest generation",
                    409,
                    "sync-coordination",
                ));
            }
        } else if manifest.generation != 1 || manifest.parent_generation.is_some() {
            return Err(Problem::new(
                "manifest_base_missing",
                "Manifest base is missing",
                409,
                "sync-coordination",
            ));
        }

        let event_id = format!("evt_sync_{}", secure_token()?);
        let event = CoordinationEvent {
            event_id: event_id.clone(),
            event_version: "1".to_owned(),
            name: "profile.sync.committed".to_owned(),
            occurred_at_ms: now,
            correlation_id: event_id.clone(),
            causation_id: event_id,
            actor_id: scope.auth.actor_id().to_owned(),
            resource: EventResource {
                resource_type: "profile".to_owned(),
                id: resource_id.as_str().to_owned(),
                revision: manifest.generation,
            },
            data: serde_json::json!({"generation": manifest.generation}),
        };
        validate_event(&scope, &event)?;
        append_event(&mut state, scope.workspace_id, event)?;
        state.manifests.insert(key, manifest);
        Ok(())
    }
}

fn scope_key(workspace_id: &WorkspaceId, resource_id: &ResourceId) -> ScopeKey {
    (
        workspace_id.as_str().to_owned(),
        resource_id.as_str().to_owned(),
    )
}

fn prune_replays(state: &mut LocalState, now: u64) {
    state
        .acquire_replays
        .retain(|_, replay| replay.expires_at_ms > now);
    state
        .renew_replays
        .retain(|_, replay| replay.expires_at_ms > now);
    state
        .release_replays
        .retain(|_, replay| replay.expires_at_ms > now);
    state
        .event_replays
        .retain(|_, replay| replay.expires_at_ms > now);
}

fn append_event(
    state: &mut LocalState,
    workspace_id: &WorkspaceId,
    event: CoordinationEvent,
) -> Result<String, Problem> {
    let workspace = workspace_id.as_str().to_owned();
    let sequence = state.event_sequences.entry(workspace.clone()).or_default();
    *sequence = sequence.checked_add(1).ok_or_else(unavailable)?;
    let stream_id = format!("{}-0", *sequence);
    let events = state.events.entry(workspace).or_default();
    events.push_back(StreamEvent {
        stream_id: stream_id.clone(),
        event,
    });
    while events.len() > MAX_EVENTS {
        events.pop_front();
    }
    Ok(stream_id)
}

fn parse_stream_id(value: &str) -> Result<u64, Problem> {
    let (sequence, suffix) = value
        .split_once('-')
        .ok_or_else(|| protocol::invalid("invalid_event_stream_id"))?;
    if suffix != "0" || sequence.is_empty() || !sequence.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(protocol::invalid("invalid_event_stream_id"));
    }
    sequence
        .parse::<u64>()
        .map_err(|_| protocol::invalid("invalid_event_stream_id"))
}

fn validate_ttl(ttl: Duration) -> Result<u64, Problem> {
    let seconds = ttl.as_secs();
    if ttl.subsec_nanos() != 0 || !(MIN_TTL_SECONDS..=MAX_TTL_SECONDS).contains(&seconds) {
        return Err(invalid_ttl());
    }
    seconds.checked_mul(1_000).ok_or_else(invalid_ttl)
}

fn validate_owner_token(token: &str) -> Result<(), Problem> {
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(protocol::invalid("invalid_lease_owner_token"));
    }
    Ok(())
}

fn validate_idempotency_key(key: &str) -> Result<(), Problem> {
    if !(16..=128).contains(&key.len()) || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(protocol::invalid("invalid_idempotency_key"));
    }
    Ok(())
}

fn validate_metadata(metadata: &BTreeMap<String, String>) -> Result<(), Problem> {
    if metadata.len() > 32
        || metadata
            .iter()
            .any(|(key, value)| key.is_empty() || key.len() > 64 || value.len() > 512)
    {
        return Err(protocol::invalid("invalid_presence_metadata"));
    }
    Ok(())
}

fn validate_event(scope: &CoordinationScope<'_>, event: &CoordinationEvent) -> Result<(), Problem> {
    for (field, value) in [
        ("eventId", event.event_id.as_str()),
        ("correlationId", event.correlation_id.as_str()),
        ("causationId", event.causation_id.as_str()),
        ("actorId", event.actor_id.as_str()),
        ("resourceType", event.resource.resource_type.as_str()),
        ("resourceId", event.resource.id.as_str()),
    ] {
        protocol::validate_identifier(field, value)?;
    }
    if event.actor_id != scope.auth.actor_id()
        || event.event_version.is_empty()
        || event.event_version.len() > 32
        || event.name.is_empty()
        || event.name.len() > 128
        || !event.name.contains('.')
    {
        return Err(invalid_event());
    }
    Ok(())
}

fn fingerprint(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn secure_token() -> Result<String, Problem> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| unavailable())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn now_ms() -> Result<u64, Problem> {
    crate::clock::unix_time_millis()
}

fn unavailable() -> Problem {
    Problem::new(
        "coordination_unavailable",
        "Coordination service is unavailable",
        503,
        "sync-coordination",
    )
    .retryable(true)
}

fn invalid_ttl() -> Problem {
    protocol::invalid("invalid_coordination_ttl")
}
fn invalid_fence() -> Problem {
    Problem::new(
        "fencing_token_exhausted",
        "Fencing token space is exhausted",
        503,
        "sync-coordination",
    )
}
fn invalid_event() -> Problem {
    protocol::invalid("invalid_coordination_event")
}
fn invalid_manifest() -> Problem {
    Problem::new(
        "invalid_coordination_manifest",
        "Stored coordination manifest is invalid",
        500,
        "sync-coordination",
    )
}
fn idempotency_conflict() -> Problem {
    Problem::new(
        "idempotency_conflict",
        "Idempotency key was reused with different input",
        409,
        "sync-coordination",
    )
}
fn lease_missing() -> Problem {
    Problem::new(
        "lease_not_found",
        "Resource lease was not found",
        404,
        "sync-coordination",
    )
}
fn lease_not_owner() -> Problem {
    Problem::new(
        "lease_not_owner",
        "Lease owner token did not match",
        409,
        "sync-coordination",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ManifestObject;
    use buglogin_domain::ids::{ResourceId, WorkspaceId};

    fn auth(actor: &str) -> super::super::CoordinationAuth {
        super::super::CoordinationAuth::new(actor, "local-test-bearer").unwrap()
    }

    fn workspace() -> WorkspaceId {
        WorkspaceId::new("workspace-local").unwrap()
    }
    fn resource() -> ResourceId {
        ResourceId::new("profile-local").unwrap()
    }

    fn scope<'a>(
        workspace: &'a WorkspaceId,
        auth: &'a super::super::CoordinationAuth,
    ) -> CoordinationScope<'a> {
        CoordinationScope {
            workspace_id: workspace,
            auth,
        }
    }

    fn manifest(
        workspace: &WorkspaceId,
        resource: &ResourceId,
        fence: u64,
        generation: u64,
    ) -> SyncManifest {
        SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            fence,
            generation,
            generation.checked_sub(1).filter(|parent| *parent > 0),
            b"root",
            Vec::<ManifestObject>::new(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn lease_acquire_is_idempotent_fenced_and_owner_checked() {
        let backend = LocalCoordination::new();
        let workspace = workspace();
        let resource = resource();
        let alice = auth("alice");
        let bob = auth("bob");
        let first = backend
            .acquire_lease_idempotent(
                scope(&workspace, &alice),
                &resource,
                Duration::from_secs(30),
                "acquire-key-000001",
            )
            .await
            .unwrap();
        let replay = backend
            .acquire_lease_idempotent(
                scope(&workspace, &alice),
                &resource,
                Duration::from_secs(30),
                "acquire-key-000001",
            )
            .await
            .unwrap();
        assert_eq!(first, replay);
        assert_eq!(
            backend
                .acquire_lease_idempotent(
                    scope(&workspace, &bob),
                    &resource,
                    Duration::from_secs(30),
                    "acquire-key-000002"
                )
                .await
                .unwrap_err()
                .code,
            "lease_held"
        );
        assert_eq!(
            backend
                .validate_write_authority(
                    scope(&workspace, &alice),
                    &resource,
                    &first.owner_token,
                    first.fencing_token + 1
                )
                .await
                .unwrap_err()
                .code,
            "stale_fencing_token"
        );
        backend
            .release_lease_idempotent(
                scope(&workspace, &alice),
                &resource,
                &first.owner_token,
                "release-key-000001",
            )
            .await
            .unwrap();
        let second = backend
            .acquire_lease_idempotent(
                scope(&workspace, &bob),
                &resource,
                Duration::from_secs(30),
                "acquire-key-000003",
            )
            .await
            .unwrap();
        assert!(second.fencing_token > first.fencing_token);
    }

    #[tokio::test]
    async fn renew_replays_and_rejects_wrong_owner_or_reused_key() {
        let backend = LocalCoordination::new();
        let workspace = workspace();
        let resource = resource();
        let alice = auth("alice");
        let bob = auth("bob");
        let lease = backend
            .acquire_lease_idempotent(
                scope(&workspace, &alice),
                &resource,
                Duration::from_secs(30),
                "acquire-key-000001",
            )
            .await
            .unwrap();
        assert_eq!(
            backend
                .renew_lease_idempotent(
                    scope(&workspace, &bob),
                    &resource,
                    &lease.owner_token,
                    Duration::from_secs(60),
                    "renew-key-00000001"
                )
                .await
                .unwrap_err()
                .code,
            "lease_not_owner"
        );
        let renewed = backend
            .renew_lease_idempotent(
                scope(&workspace, &alice),
                &resource,
                &lease.owner_token,
                Duration::from_secs(60),
                "renew-key-00000001",
            )
            .await
            .unwrap();
        assert!(renewed.expires_at_ms > lease.expires_at_ms);
        assert_eq!(
            backend
                .renew_lease_idempotent(
                    scope(&workspace, &alice),
                    &resource,
                    &lease.owner_token,
                    Duration::from_secs(60),
                    "renew-key-00000001"
                )
                .await
                .unwrap(),
            renewed
        );
        assert_eq!(
            backend
                .renew_lease_idempotent(
                    scope(&workspace, &alice),
                    &resource,
                    &lease.owner_token,
                    Duration::from_secs(90),
                    "renew-key-00000001"
                )
                .await
                .unwrap_err()
                .code,
            "idempotency_conflict"
        );
    }

    #[tokio::test]
    async fn presence_ttl_and_workspace_isolation_are_enforced() {
        let backend = LocalCoordination::new();
        let workspace = workspace();
        let other = WorkspaceId::new("workspace-other").unwrap();
        let alice = auth("alice");
        let mut metadata = BTreeMap::new();
        metadata.insert("device".to_owned(), "desktop".to_owned());
        backend
            .heartbeat_presence(
                scope(&workspace, &alice),
                "session-001",
                metadata,
                Duration::from_secs(30),
            )
            .await
            .unwrap();
        assert_eq!(
            backend
                .list_presence(scope(&workspace, &alice))
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            backend
                .list_presence(scope(&other, &alice))
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            backend
                .heartbeat_presence(
                    scope(&workspace, &alice),
                    "session-002",
                    BTreeMap::new(),
                    Duration::ZERO
                )
                .await
                .unwrap_err()
                .code,
            "invalid_coordination_ttl"
        );
    }

    #[tokio::test]
    async fn event_order_idempotency_and_manifest_generation_are_enforced() {
        let backend = LocalCoordination::new();
        let workspace = workspace();
        let resource = resource();
        let alice = auth("alice");
        let event = CoordinationEvent {
            event_id: "evt_local_001".to_owned(),
            event_version: "1".to_owned(),
            name: "profile.updated".to_owned(),
            occurred_at_ms: now_ms().unwrap(),
            correlation_id: "corr_local_001".to_owned(),
            causation_id: "cause_local_001".to_owned(),
            actor_id: "alice".to_owned(),
            resource: EventResource {
                resource_type: "profile".to_owned(),
                id: resource.as_str().to_owned(),
                revision: 1,
            },
            data: serde_json::json!({"ok": true}),
        };
        let published = backend
            .publish_event(
                scope(&workspace, &alice),
                "event-key-0000001",
                event.clone(),
            )
            .await
            .unwrap();
        let replay = backend
            .publish_event(
                scope(&workspace, &alice),
                "event-key-0000001",
                event.clone(),
            )
            .await
            .unwrap();
        assert!(replay.duplicate);
        assert_eq!(published.stream_id, replay.stream_id);
        assert_eq!(
            backend
                .read_events(scope(&workspace, &alice), None, 10)
                .await
                .unwrap()
                .len(),
            1
        );

        let lease = backend
            .acquire_lease_idempotent(
                scope(&workspace, &alice),
                &resource,
                Duration::from_secs(30),
                "acquire-key-000001",
            )
            .await
            .unwrap();
        backend
            .commit_manifest(
                scope(&workspace, &alice),
                &resource,
                &lease.owner_token,
                manifest(&workspace, &resource, lease.fencing_token, 1),
            )
            .await
            .unwrap();
        assert_eq!(
            backend
                .current_manifest(scope(&workspace, &alice), &resource)
                .await
                .unwrap()
                .unwrap()
                .generation,
            1
        );
        backend
            .commit_manifest(
                scope(&workspace, &alice),
                &resource,
                &lease.owner_token,
                manifest(&workspace, &resource, lease.fencing_token, 1),
            )
            .await
            .unwrap();
        assert_eq!(
            backend
                .commit_manifest(
                    scope(&workspace, &alice),
                    &resource,
                    &lease.owner_token,
                    manifest(&workspace, &resource, lease.fencing_token, 3)
                )
                .await
                .unwrap_err()
                .code,
            "stale_manifest_generation"
        );
    }

    #[tokio::test]
    async fn expired_lease_cannot_authorize_writes_and_acquire_increments_fence() {
        let backend = LocalCoordination::new();
        let workspace = workspace();
        let resource = resource();
        let alice = auth("alice");
        let first = backend
            .acquire_lease_idempotent(
                scope(&workspace, &alice),
                &resource,
                Duration::from_secs(1),
                "acquire-key-000001",
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(1_050)).await;
        assert_eq!(
            backend
                .validate_write_authority(
                    scope(&workspace, &alice),
                    &resource,
                    &first.owner_token,
                    first.fencing_token
                )
                .await
                .unwrap_err()
                .code,
            "lease_expired"
        );
        let next = backend
            .acquire_lease_idempotent(
                scope(&workspace, &alice),
                &resource,
                Duration::from_secs(1),
                "acquire-key-000002",
            )
            .await
            .unwrap();
        assert!(next.fencing_token > first.fencing_token);
    }

    #[tokio::test]
    async fn expired_lease_cannot_be_renewed() {
        let backend = LocalCoordination::new();
        let workspace = workspace();
        let resource = resource();
        let alice = auth("alice");
        let lease = backend
            .acquire_lease_idempotent(
                scope(&workspace, &alice),
                &resource,
                Duration::from_secs(10),
                "renew-expired-acquire-0001",
            )
            .await
            .unwrap();
        {
            let key = scope_key(&workspace, &resource);
            let mut state = backend.lock().unwrap();
            state.leases.get_mut(&key).unwrap().expires_at_ms = 0;
        }

        let error = backend
            .renew_lease_idempotent(
                scope(&workspace, &alice),
                &resource,
                &lease.owner_token,
                Duration::from_secs(10),
                "renew-expired-key-0001",
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "lease_expired");
        assert_eq!(error.status, 409);
    }

    #[test]
    fn contract_types_are_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LocalCoordination>();
    }
}
