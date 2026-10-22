use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use buglogin_domain::{
    errors::Problem,
    ids::{ResourceId, WorkspaceId},
    lease::{MAX_TTL_SECONDS, MIN_TTL_SECONDS},
};
use redis::{AsyncCommands, Script, aio::ConnectionManagerConfig, streams::StreamRangeReply};
use sha2::{Digest, Sha256};

use super::{
    AuthorizationRequest, CoordinationAuthorizer, CoordinationBackend, CoordinationEvent,
    CoordinationOperation, CoordinationScope, EventResource, LeaseGrant, LeaseStatus, Presence,
    PublishedEvent, SharedAuthorizer, StreamEvent, protocol,
};
use crate::manifest::SyncManifest;

const MIN_IDEMPOTENCY_TTL: Duration = Duration::from_secs(86_400);
const DEFAULT_STREAM_MAX_LEN: usize = 10_000;

const ACQUIRE_LEASE_IDEMPOTENT: &str = r#"
local replay = redis.call('HGETALL', KEYS[3])
if #replay > 0 then
  local values = {}
  for index = 1, #replay, 2 do values[replay[index]] = replay[index + 1] end
  if not values.fingerprint or not values.owner or values.owner == ''
    or not values.token or string.len(values.token) ~= 64 or not string.match(values.token, '^%x+$')
    or not values.fencingToken or not string.match(values.fencingToken, '^%d+$') or tonumber(values.fencingToken) < 1
    or not values.issuedAt or not string.match(values.issuedAt, '^%d+$')
    or not values.expiresAt or not string.match(values.expiresAt, '^%d+$') then
    return {-2, '', '', '', '', ''}
  end
  if values.fingerprint ~= ARGV[4] then
    return {-3, '', '', '', '', ''}
  end
  local time = redis.call('TIME')
  local now = (time[1] * 1000) + math.floor(time[2] / 1000)
  local lease_matches = redis.call('EXISTS', KEYS[1]) == 1
    and redis.call('HGET', KEYS[1], 'owner') == values.owner
    and redis.call('HGET', KEYS[1], 'token') == values.token
    and redis.call('HGET', KEYS[1], 'fencingToken') == values.fencingToken
  if lease_matches and tonumber(values.expiresAt) > now then
    return {2, values.owner, values.token, values.fencingToken, values.issuedAt, values.expiresAt}
  end
  if redis.call('EXISTS', KEYS[1]) == 1 then
    local current_owner = redis.call('HGET', KEYS[1], 'owner') or ''
    local current_fence = redis.call('HGET', KEYS[1], 'fencingToken') or ''
    local current_issued = redis.call('HGET', KEYS[1], 'issuedAt') or ''
    local current_expires = redis.call('HGET', KEYS[1], 'expiresAt') or ''
    return {0, current_owner, '', current_fence, current_issued, current_expires}
  end
  redis.call('DEL', KEYS[3])
end

local current = redis.call('HGETALL', KEYS[1])
if #current > 0 then
  local values = {}
  for index = 1, #current, 2 do values[current[index]] = current[index + 1] end
  if not values.owner or values.owner == ''
    or not values.token or string.len(values.token) ~= 64 or not string.match(values.token, '^%x+$')
    or not values.fencingToken or not string.match(values.fencingToken, '^%d+$') or tonumber(values.fencingToken) < 1
    or not values.issuedAt or not string.match(values.issuedAt, '^%d+$')
    or not values.expiresAt or not string.match(values.expiresAt, '^%d+$') then
    return {-2, '', '', '', '', ''}
  end
  return {0, values.owner, '', values.fencingToken, values.issuedAt, values.expiresAt}
end

local time = redis.call('TIME')
local now = (time[1] * 1000) + math.floor(time[2] / 1000)
local expires = now + tonumber(ARGV[3])
redis.call('INCR', KEYS[2])
local fence = redis.call('GET', KEYS[2])
redis.call('HSET', KEYS[1], 'owner', ARGV[1], 'token', ARGV[2], 'fencingToken', fence, 'issuedAt', now, 'expiresAt', expires)
redis.call('PEXPIREAT', KEYS[1], string.format('%.0f', expires))
redis.call('HSET', KEYS[3], 'fingerprint', ARGV[4], 'owner', ARGV[1], 'token', ARGV[2], 'fencingToken', fence, 'issuedAt', now, 'expiresAt', expires)
redis.call('PEXPIRE', KEYS[3], ARGV[5])
return {1, ARGV[1], ARGV[2], fence, tostring(now), tostring(expires)}
"#;

const RENEW_LEASE_IDEMPOTENT: &str = r#"
local replay = redis.call('HGETALL', KEYS[2])
if #replay > 0 then
  local values = {}
  for index = 1, #replay, 2 do values[replay[index]] = replay[index + 1] end
  if not values.fingerprint or not values.owner or values.owner == ''
    or not values.token or string.len(values.token) ~= 64 or not string.match(values.token, '^%x+$')
    or not values.fencingToken or not string.match(values.fencingToken, '^%d+$') or tonumber(values.fencingToken) < 1
    or not values.issuedAt or not string.match(values.issuedAt, '^%d+$')
    or not values.expiresAt or not string.match(values.expiresAt, '^%d+$') then
    return {-2, '', '', '', '', ''}
  end
  if values.fingerprint ~= ARGV[4] then return {-3, '', '', '', '', ''} end
  return {2, values.owner, values.token, values.fencingToken, values.issuedAt, values.expiresAt}
end
if redis.call('EXISTS', KEYS[1]) == 0 then return {0, '', '', '', '', ''} end
local owner = redis.call('HGET', KEYS[1], 'owner')
local token = redis.call('HGET', KEYS[1], 'token')
local fence = redis.call('HGET', KEYS[1], 'fencingToken')
local issued = redis.call('HGET', KEYS[1], 'issuedAt')
local currentExpires = redis.call('HGET', KEYS[1], 'expiresAt')
if not owner or owner == ''
  or not token or string.len(token) ~= 64 or not string.match(token, '^%x+$')
  or not fence or not string.match(fence, '^%d+$') or tonumber(fence) < 1
  or not issued or not string.match(issued, '^%d+$')
  or not currentExpires or not string.match(currentExpires, '^%d+$') then
  return {-2, '', '', '', '', ''}
end
if owner ~= ARGV[1] or token ~= ARGV[2] then return {-1, owner, '', '', '', ''} end
local time = redis.call('TIME')
local now = (time[1] * 1000) + math.floor(time[2] / 1000)
if tonumber(currentExpires) <= now then return {-4, owner, '', fence, issued, currentExpires} end
local expires = now + tonumber(ARGV[3])
redis.call('HSET', KEYS[1], 'expiresAt', expires)
redis.call('PEXPIREAT', KEYS[1], string.format('%.0f', expires))
redis.call('HSET', KEYS[2], 'fingerprint', ARGV[4], 'owner', owner, 'token', token, 'fencingToken', fence, 'issuedAt', issued, 'expiresAt', expires)
redis.call('PEXPIRE', KEYS[2], ARGV[5])
return {1, owner, token, fence, issued, tostring(expires)}
"#;

const VALIDATE_WRITE_AUTHORITY: &str = r#"
if redis.call('EXISTS', KEYS[1]) == 0 then return {-1, 'lease_not_found'} end
local token = redis.call('HGET', KEYS[1], 'token')
local fence = redis.call('HGET', KEYS[1], 'fencingToken')
local expires = redis.call('HGET', KEYS[1], 'expiresAt')
if not token or string.len(token) ~= 64 or not string.match(token, '^%x+$')
  or not fence or not string.match(fence, '^%d+$') or tonumber(fence) < 1
  or not expires or not string.match(expires, '^%d+$') then
  return {-4, 'lease_corrupt'}
end
local time = redis.call('TIME')
local now = (time[1] * 1000) + math.floor(time[2] / 1000)
if tonumber(expires) <= now then return {-1, 'lease_expired'} end
if token ~= ARGV[1] then return {-2, 'lease_not_owner'} end
if fence ~= ARGV[2] then return {-3, 'stale_fencing_token'} end
return {1, 'ok'}
"#;

const RELEASE_LEASE_IDEMPOTENT: &str = r#"
local replay = redis.call('HGETALL', KEYS[2])
if #replay > 0 then
  local values = {}
  for index = 1, #replay, 2 do values[replay[index]] = replay[index + 1] end
  if not values.fingerprint or values.released ~= '1' then return -3 end
  if values.fingerprint ~= ARGV[3] then return -2 end
  return 2
end
if redis.call('EXISTS', KEYS[1]) == 1 then
  if redis.call('HGET', KEYS[1], 'owner') ~= ARGV[1]
    or redis.call('HGET', KEYS[1], 'token') ~= ARGV[2] then return -1 end
  redis.call('DEL', KEYS[1])
end
redis.call('HSET', KEYS[2], 'fingerprint', ARGV[3], 'released', '1')
redis.call('PEXPIRE', KEYS[2], ARGV[4])
return 1
"#;

const HEARTBEAT_PRESENCE: &str = r#"
local time = redis.call('TIME')
local now = (time[1] * 1000) + math.floor(time[2] / 1000)
local connected = redis.call('HGET', KEYS[1], 'connectedAt')
if not connected then connected = now end
local expires = now + tonumber(ARGV[4])
redis.call('HSET', KEYS[1], 'sessionId', ARGV[1], 'actorId', ARGV[2], 'metadata', ARGV[3], 'connectedAt', connected, 'lastSeenAt', now, 'expiresAt', expires)
redis.call('PEXPIRE', KEYS[1], ARGV[4])
return {connected, now, expires}
"#;

const PUBLISH_EVENT: &str = r#"
if redis.call('EXISTS', KEYS[2]) == 1 then
  local previous = redis.call('HGET', KEYS[2], 'payload')
  local streamId = redis.call('HGET', KEYS[2], 'streamId')
  if previous == ARGV[1] then return {0, streamId} end
  return {-1, streamId}
end
local streamId = redis.call('XADD', KEYS[1], 'MAXLEN', '~', ARGV[3], '*', 'event', ARGV[1])
redis.call('HSET', KEYS[2], 'payload', ARGV[1], 'streamId', streamId)
redis.call('EXPIRE', KEYS[2], ARGV[2])
return {1, streamId}
"#;

const COMMIT_MANIFEST: &str = r#"
local function is_positive_u64(value)
  if not value or not string.match(value, '^[1-9]%d*$') or string.len(value) > 20 then
    return false
  end
  return string.len(value) < 20 or value <= '18446744073709551615'
end

local function increment_u64(value)
  local digits = {string.byte(value, 1, string.len(value))}
  for index = #digits, 1, -1 do
    if digits[index] < 57 then
      digits[index] = digits[index] + 1
      return string.char(unpack(digits))
    end
    digits[index] = 48
  end
  return nil
end

local manifest_key = KEYS[1]
local lease_key = KEYS[2]

local owner_token = ARGV[1]
local fencing_token = ARGV[2]
local workspace_id = ARGV[3]
local resource_id = ARGV[4]
local incoming_gen = ARGV[5]
local incoming_parent = ARGV[6]
local payload = ARGV[7]

if not is_positive_u64(incoming_gen)
  or (incoming_parent ~= 'none' and not is_positive_u64(incoming_parent)) then
  return {-8, 'manifest_generation_invalid'}
end

if redis.call('EXISTS', lease_key) == 0 then
  return {-1, 'lease_missing'}
end

local lease_token = redis.call('HGET', lease_key, 'token')
local lease_fence = redis.call('HGET', lease_key, 'fencingToken')
local lease_expires = redis.call('HGET', lease_key, 'expiresAt')

if not lease_expires then
  return {-1, 'lease_missing'}
end

local time = redis.call('TIME')
local now = (time[1] * 1000) + math.floor(time[2] / 1000)
if now > tonumber(lease_expires) then
  return {-1, 'lease_expired'}
end

if lease_token ~= owner_token then
  return {-2, 'lease_not_owner'}
end

if lease_fence ~= fencing_token then
  return {-3, 'fencing_token_mismatch'}
end

local current_exists = redis.call('EXISTS', manifest_key)
if current_exists == 0 then
  if incoming_gen ~= '1' or incoming_parent ~= 'none' then
    return {-5, 'manifest_base_missing'}
  end
else
  local curr_gen = redis.call('HGET', manifest_key, 'generation')
  local curr_ws = redis.call('HGET', manifest_key, 'workspace_id')
  local curr_res = redis.call('HGET', manifest_key, 'resource_id')
  local curr_payload = redis.call('HGET', manifest_key, 'payload')

  if not is_positive_u64(curr_gen) then
    return {-8, 'manifest_generation_invalid'}
  end

  if curr_ws ~= workspace_id or curr_res ~= resource_id then
    return {-4, 'workspace_resource_mismatch'}
  end

  if incoming_gen == curr_gen then
    if curr_payload == payload then
      return {2, 'idempotent_retry'}
    else
      return {-7, 'idempotency_conflict'}
    end
  end

  local next_gen = increment_u64(curr_gen)
  if not next_gen or not is_positive_u64(next_gen) or incoming_gen ~= next_gen then
    return {-6, 'generation_gap_or_stale'}
  end

  local expected_parent = tostring(curr_gen)
  if incoming_parent ~= expected_parent then
    return {-6, 'generation_parent_mismatch'}
  end
end

redis.call('HSET', manifest_key, 
  'generation', ARGV[5], 
  'parent_generation', incoming_parent, 
  'workspace_id', workspace_id, 
  'resource_id', resource_id, 
  'fencing_token', fencing_token, 
  'payload', payload
)

redis.call('XADD', KEYS[3], 'MAXLEN', '~', ARGV[9], '*', 'event', ARGV[8])

return {1, 'success'}
"#;

#[derive(Clone)]
pub struct RedisCoordinationConfig {
    pub url: String,
    pub namespace: String,
    tls_root_cert: Option<Vec<u8>>,
    pub reconnect_min_delay: Duration,
    pub reconnect_max_delay: Duration,
    pub reconnect_attempts: usize,
    pub connection_timeout: Duration,
    pub response_timeout: Duration,
    pub idempotency_ttl: Duration,
    pub stream_max_len: usize,
}

impl RedisCoordinationConfig {
    pub fn new(url: impl Into<String>, namespace: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            namespace: namespace.into(),
            tls_root_cert: None,
            reconnect_min_delay: Duration::from_millis(100),
            reconnect_max_delay: Duration::from_secs(5),
            reconnect_attempts: 8,
            connection_timeout: Duration::from_secs(3),
            response_timeout: Duration::from_secs(5),
            idempotency_ttl: MIN_IDEMPOTENCY_TTL,
            stream_max_len: DEFAULT_STREAM_MAX_LEN,
        }
    }

    pub fn with_tls_root_cert(mut self, certificate_pem: Vec<u8>) -> Self {
        self.tls_root_cert = Some(certificate_pem);
        self
    }

    fn validate(&self) -> Result<(), Problem> {
        protocol::validate_namespace(&self.namespace)?;
        if self.url.is_empty()
            || self.reconnect_attempts == 0
            || self.reconnect_min_delay > self.reconnect_max_delay
            || self.connection_timeout.is_zero()
            || self.response_timeout.is_zero()
            || self.idempotency_ttl < MIN_IDEMPOTENCY_TTL
            || self.stream_max_len == 0
        {
            return Err(protocol::invalid("invalid_redis_coordination_config"));
        }
        Ok(())
    }
}

impl fmt::Debug for RedisCoordinationConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RedisCoordinationConfig")
            .field("url", &"<redacted>")
            .field("namespace", &self.namespace)
            .field(
                "tls_root_cert",
                &self.tls_root_cert.as_ref().map(|_| "<configured>"),
            )
            .field("reconnect_min_delay", &self.reconnect_min_delay)
            .field("reconnect_max_delay", &self.reconnect_max_delay)
            .field("reconnect_attempts", &self.reconnect_attempts)
            .field("connection_timeout", &self.connection_timeout)
            .field("response_timeout", &self.response_timeout)
            .field("idempotency_ttl", &self.idempotency_ttl)
            .field("stream_max_len", &self.stream_max_len)
            .finish()
    }
}

#[derive(Clone)]
pub struct RedisCoordination {
    connection: redis::aio::ConnectionManager,
    namespace: String,
    authorizer: SharedAuthorizer,
    idempotency_ttl_seconds: u64,
    stream_max_len: usize,
}

impl RedisCoordination {
    pub async fn connect(
        config: RedisCoordinationConfig,
        authorizer: Arc<dyn CoordinationAuthorizer>,
    ) -> Result<Self, Problem> {
        config.validate()?;
        let client = match config.tls_root_cert.as_ref() {
            Some(root_cert) => redis::Client::build_with_tls(
                config.url.as_str(),
                redis::TlsCertificates {
                    client_tls: None,
                    root_cert: Some(root_cert.clone()),
                },
            ),
            None => redis::Client::open(config.url.as_str()),
        }
        .map_err(redis_problem)?;
        let manager_config = ConnectionManagerConfig::new()
            .set_min_delay(config.reconnect_min_delay)
            .set_max_delay(config.reconnect_max_delay)
            .set_number_of_retries(config.reconnect_attempts)
            .set_connection_timeout(Some(config.connection_timeout))
            .set_response_timeout(Some(config.response_timeout));
        let connection = client
            .get_connection_manager_with_config(manager_config)
            .await
            .map_err(redis_problem)?;
        Ok(Self {
            connection,
            namespace: config.namespace,
            authorizer,
            idempotency_ttl_seconds: config.idempotency_ttl.as_secs(),
            stream_max_len: config.stream_max_len,
        })
    }

    async fn authorize(
        &self,
        scope: CoordinationScope<'_>,
        operation: CoordinationOperation,
        resource_id: Option<&ResourceId>,
    ) -> Result<(), Problem> {
        self.authorizer
            .authorize(
                scope.auth,
                AuthorizationRequest {
                    operation,
                    workspace_id: scope.workspace_id,
                    resource_id,
                },
            )
            .await
    }

    fn workspace_prefix(&self, scope: CoordinationScope<'_>) -> String {
        workspace_prefix(&self.namespace, scope.workspace_id)
    }
    fn lease_key(&self, scope: CoordinationScope<'_>, resource_id: &ResourceId) -> String {
        lease_key(&self.namespace, scope.workspace_id, resource_id)
    }
    fn fence_key(&self, scope: CoordinationScope<'_>, resource_id: &ResourceId) -> String {
        fence_key(&self.namespace, scope.workspace_id, resource_id)
    }
    fn manifest_key(&self, scope: CoordinationScope<'_>, resource_id: &ResourceId) -> String {
        manifest_key(&self.namespace, scope.workspace_id, resource_id)
    }
    fn presence_key(&self, scope: CoordinationScope<'_>, session_id: &str) -> String {
        format!("{}:presence:{session_id}", self.workspace_prefix(scope))
    }
    fn stream_key(&self, scope: CoordinationScope<'_>) -> String {
        format!("{}:events", self.workspace_prefix(scope))
    }
    fn idempotency_key(&self, scope: CoordinationScope<'_>, key: &str) -> String {
        format!(
            "{}:event-idempotency:{}:{}",
            self.workspace_prefix(scope.clone()),
            key_segment(scope.auth.actor_id()),
            key_segment(key)
        )
    }
    fn lease_idempotency_key(&self, scope: CoordinationScope<'_>, key: &str) -> String {
        format!(
            "{}:lease-idempotency:{}",
            self.workspace_prefix(scope.clone()),
            key_segment(key)
        )
    }
    fn lease_renew_idempotency_key(&self, scope: CoordinationScope<'_>, key: &str) -> String {
        format!(
            "{}:lease-renew-idempotency:{}",
            self.workspace_prefix(scope.clone()),
            key_segment(key)
        )
    }
    fn lease_release_idempotency_key(&self, scope: CoordinationScope<'_>, key: &str) -> String {
        format!(
            "{}:lease-release-idempotency:{}",
            self.workspace_prefix(scope),
            key_segment(key)
        )
    }
}

#[async_trait]
impl CoordinationBackend for RedisCoordination {
    async fn readiness(&self) -> Result<(), Problem> {
        let mut connection = self.connection.clone();
        let response: String = redis::cmd("PING")
            .query_async(&mut connection)
            .await
            .map_err(redis_problem)?;
        if response == "PONG" {
            Ok(())
        } else {
            Err(Problem::new(
                "redis_unavailable",
                "Redis coordination is unavailable",
                503,
                "readiness",
            ))
        }
    }

    async fn acquire_lease_idempotent(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        ttl: Duration,
        idempotency_key: &str,
    ) -> Result<LeaseGrant, Problem> {
        self.authorize(
            scope.clone(),
            CoordinationOperation::LeaseAcquire,
            Some(resource_id),
        )
        .await?;
        validate_idempotency_key(idempotency_key)?;
        let ttl_ms = validate_ttl(ttl)?;
        let idempotency_ttl_ms = self
            .idempotency_ttl_seconds
            .checked_mul(1_000)
            .ok_or_else(|| protocol::invalid("invalid_idempotency_ttl"))?;
        let owner_token = secure_token()?;
        let ttl_fingerprint = ttl_ms.to_string();
        let fingerprint = lease_fingerprint(&[
            scope.auth.actor_id(),
            resource_id.as_str(),
            &ttl_fingerprint,
        ]);
        let mut connection = self.connection.clone();
        let (status, owner_id, token, fencing_token, issued_at_ms, expires_at_ms): (
            i64,
            String,
            String,
            String,
            String,
            String,
        ) = Script::new(ACQUIRE_LEASE_IDEMPOTENT)
            .key(self.lease_key(scope.clone(), resource_id))
            .key(self.fence_key(scope.clone(), resource_id))
            .key(self.lease_idempotency_key(scope.clone(), idempotency_key))
            .arg(scope.auth.actor_id())
            .arg(&owner_token)
            .arg(ttl_ms)
            .arg(fingerprint)
            .arg(idempotency_ttl_ms)
            .invoke_async(&mut connection)
            .await
            .map_err(redis_problem)?;
        match status {
            1 | 2 => Ok(LeaseGrant {
                resource_id: resource_id.clone(),
                workspace_id: scope.workspace_id.clone(),
                owner_id,
                owner_token: token,
                fencing_token: parse_lease_number(&fencing_token)?,
                issued_at_ms: parse_lease_number(&issued_at_ms)?,
                expires_at_ms: parse_lease_number(&expires_at_ms)?,
            }),
            0 => Err(Problem::new(
                "lease_held",
                "Resource lease is already held",
                409,
                "sync-coordination",
            )
            .with_field("ownerId", owner_id)
            .with_field("expiresAtMs", parse_lease_number(&expires_at_ms)?)),
            -2 => Err(invalid_stored_lease()),
            -3 => Err(Problem::new(
                "idempotency_conflict",
                "Idempotency key was reused with different input",
                409,
                "sync-coordination",
            )),
            _ => Err(invalid_stored_lease()),
        }
    }

    async fn renew_lease_idempotent(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        ttl: Duration,
        idempotency_key: &str,
    ) -> Result<LeaseGrant, Problem> {
        self.authorize(
            scope.clone(),
            CoordinationOperation::LeaseRenew,
            Some(resource_id),
        )
        .await?;
        validate_owner_token(owner_token)?;
        validate_idempotency_key(idempotency_key)?;
        let ttl_ms = validate_ttl(ttl)?;
        let idempotency_ttl_ms = self
            .idempotency_ttl_seconds
            .checked_mul(1_000)
            .ok_or_else(|| protocol::invalid("invalid_idempotency_ttl"))?;
        let ttl_fingerprint = ttl_ms.to_string();
        let fingerprint = lease_fingerprint(&[
            scope.auth.actor_id(),
            resource_id.as_str(),
            owner_token,
            &ttl_fingerprint,
        ]);
        let mut connection = self.connection.clone();
        let result: (i64, String, String, String, String, String) =
            Script::new(RENEW_LEASE_IDEMPOTENT)
                .key(self.lease_key(scope.clone(), resource_id))
                .key(self.lease_renew_idempotency_key(scope.clone(), idempotency_key))
                .arg(scope.auth.actor_id())
                .arg(owner_token)
                .arg(ttl_ms)
                .arg(fingerprint)
                .arg(idempotency_ttl_ms)
                .invoke_async(&mut connection)
                .await
                .map_err(redis_problem)?;
        match result {
            (1 | 2, owner_id, token, fencing_token, issued_at_ms, expires_at_ms) => {
                Ok(LeaseGrant {
                    resource_id: resource_id.clone(),
                    workspace_id: scope.workspace_id.clone(),
                    owner_id,
                    owner_token: token,
                    fencing_token: parse_lease_number(&fencing_token)?,
                    issued_at_ms: parse_lease_number(&issued_at_ms)?,
                    expires_at_ms: parse_lease_number(&expires_at_ms)?,
                })
            }
            (0, _, _, _, _, _) => Err(lease_missing()),
            (-1, owner_id, _, _, _, _) => Err(lease_not_owner().with_field("ownerId", owner_id)),
            (-2, _, _, _, _, _) => Err(invalid_stored_lease()),
            (-3, _, _, _, _, _) => Err(Problem::new(
                "idempotency_conflict",
                "Idempotency key was reused with different input",
                409,
                "sync-coordination",
            )),
            (-4, _, _, _, _, _) => Err(lease_expired()),
            _ => Err(invalid_stored_lease()),
        }
    }

    async fn release_lease_idempotent(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        idempotency_key: &str,
    ) -> Result<(), Problem> {
        self.authorize(
            scope.clone(),
            CoordinationOperation::LeaseRelease,
            Some(resource_id),
        )
        .await?;
        validate_owner_token(owner_token)?;
        validate_idempotency_key(idempotency_key)?;
        let idempotency_ttl_ms = self
            .idempotency_ttl_seconds
            .checked_mul(1_000)
            .ok_or_else(|| protocol::invalid("invalid_idempotency_ttl"))?;
        let fingerprint =
            lease_fingerprint(&[scope.auth.actor_id(), resource_id.as_str(), owner_token]);
        let mut connection = self.connection.clone();
        let status: i64 = Script::new(RELEASE_LEASE_IDEMPOTENT)
            .key(self.lease_key(scope.clone(), resource_id))
            .key(self.lease_release_idempotency_key(scope.clone(), idempotency_key))
            .arg(scope.auth.actor_id())
            .arg(owner_token)
            .arg(fingerprint)
            .arg(idempotency_ttl_ms)
            .invoke_async(&mut connection)
            .await
            .map_err(redis_problem)?;
        match status {
            1 | 2 => Ok(()),
            -1 => Err(lease_not_owner()),
            -2 => Err(Problem::new(
                "idempotency_conflict",
                "Idempotency key was reused with different input",
                409,
                "sync-coordination",
            )),
            -3 => Err(invalid_stored_lease()),
            _ => Err(invalid_stored_lease()),
        }
    }

    async fn lease_status(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
    ) -> Result<Option<LeaseStatus>, Problem> {
        self.authorize(
            scope.clone(),
            CoordinationOperation::LeaseRead,
            Some(resource_id),
        )
        .await?;
        let mut connection = self.connection.clone();
        let fields: HashMap<String, String> = connection
            .hgetall(self.lease_key(scope.clone(), resource_id))
            .await
            .map_err(redis_problem)?;
        if fields.is_empty() {
            return Ok(None);
        }
        let owner_id = fields
            .get("owner")
            .filter(|value| !value.is_empty())
            .ok_or_else(invalid_stored_lease)?;
        let owner_token = fields.get("token").ok_or_else(invalid_stored_lease)?;
        validate_owner_token(owner_token).map_err(|_| invalid_stored_lease())?;
        let fencing_token = fields
            .get("fencingToken")
            .ok_or_else(invalid_stored_lease)?;
        let issued_at_ms = fields.get("issuedAt").ok_or_else(invalid_stored_lease)?;
        let expires_at_ms = fields.get("expiresAt").ok_or_else(invalid_stored_lease)?;
        let status = LeaseStatus {
            resource_id: resource_id.clone(),
            workspace_id: scope.workspace_id.clone(),
            owner_id: owner_id.clone(),
            fencing_token: parse_lease_number(fencing_token)?,
            issued_at_ms: parse_lease_number(issued_at_ms)?,
            expires_at_ms: parse_lease_number(expires_at_ms)?,
        };
        if status.fencing_token == 0 || status.expires_at_ms <= crate::clock::unix_time_millis()? {
            return Ok(None);
        }
        Ok(Some(status))
    }

    async fn validate_write_authority(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        fencing_token: u64,
    ) -> Result<(), Problem> {
        self.authorize(
            scope.clone(),
            CoordinationOperation::PayloadWrite,
            Some(resource_id),
        )
        .await?;
        validate_owner_token(owner_token)?;
        if fencing_token == 0 {
            return Err(Problem::new(
                "invalid_fencing_token",
                "Invalid fencing token",
                400,
                "sync-coordination",
            ));
        }
        let mut connection = self.connection.clone();
        let (status, message): (i64, String) = Script::new(VALIDATE_WRITE_AUTHORITY)
            .key(self.lease_key(scope, resource_id))
            .arg(owner_token)
            .arg(fencing_token.to_string())
            .invoke_async(&mut connection)
            .await
            .map_err(redis_problem)?;
        match status {
            1 => Ok(()),
            -1 => Err(Problem::new(
                if message == "lease_expired" {
                    "lease_expired"
                } else {
                    "lease_not_found"
                },
                if message == "lease_expired" {
                    "Resource lease is expired"
                } else {
                    "Resource lease not found"
                },
                409,
                "sync-coordination",
            )),
            -2 => Err(lease_not_owner()),
            -3 => Err(Problem::new(
                "stale_fencing_token",
                "Stale fencing token",
                409,
                "sync-coordination",
            )),
            _ => Err(invalid_stored_lease()),
        }
    }

    async fn heartbeat_presence(
        &self,
        scope: CoordinationScope<'_>,
        session_id: &str,
        metadata: BTreeMap<String, String>,
        ttl: Duration,
    ) -> Result<Presence, Problem> {
        self.authorize(scope.clone(), CoordinationOperation::PresenceWrite, None)
            .await?;
        protocol::validate_identifier("sessionId", session_id)?;
        validate_metadata(&metadata)?;
        let ttl_ms = validate_ttl(ttl)?;
        let metadata_json = protocol::encode(&metadata)?;
        let mut connection = self.connection.clone();
        let (connected_at_ms, last_seen_at_ms, expires_at_ms): (u64, u64, u64) =
            Script::new(HEARTBEAT_PRESENCE)
                .key(self.presence_key(scope.clone(), session_id))
                .arg(session_id)
                .arg(scope.auth.actor_id())
                .arg(metadata_json)
                .arg(ttl_ms)
                .invoke_async(&mut connection)
                .await
                .map_err(redis_problem)?;
        Ok(Presence {
            session_id: session_id.to_owned(),
            actor_id: scope.auth.actor_id().to_owned(),
            connected_at_ms,
            last_seen_at_ms,
            expires_at_ms,
            metadata,
        })
    }

    async fn list_presence(&self, scope: CoordinationScope<'_>) -> Result<Vec<Presence>, Problem> {
        self.authorize(scope.clone(), CoordinationOperation::PresenceRead, None)
            .await?;
        let pattern = format!("{}:presence:*", self.workspace_prefix(scope.clone()));
        let mut connection = self.connection.clone();
        let mut cursor = 0_u64;
        let mut presences = Vec::new();
        loop {
            let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(100)
                .query_async(&mut connection)
                .await
                .map_err(redis_problem)?;
            for key in keys {
                let fields: HashMap<String, String> =
                    connection.hgetall(key).await.map_err(redis_problem)?;
                if let Some(presence) = presence_from_fields(fields)? {
                    presences.push(presence);
                }
            }
            if next == 0 {
                break;
            }
            cursor = next;
        }
        presences.sort_by(|left, right| left.session_id.cmp(&right.session_id));
        Ok(presences)
    }

    async fn publish_event(
        &self,
        scope: CoordinationScope<'_>,
        idempotency_key: &str,
        event: CoordinationEvent,
    ) -> Result<PublishedEvent, Problem> {
        self.authorize(scope.clone(), CoordinationOperation::EventPublish, None)
            .await?;
        validate_idempotency_key(idempotency_key)?;
        validate_event(scope.clone(), &event)?;
        let encoded = event.encode()?;
        let mut connection = self.connection.clone();
        let (status, stream_id): (i64, String) = Script::new(PUBLISH_EVENT)
            .key(self.stream_key(scope.clone()))
            .key(self.idempotency_key(scope, idempotency_key))
            .arg(encoded)
            .arg(self.idempotency_ttl_seconds)
            .arg(self.stream_max_len)
            .invoke_async(&mut connection)
            .await
            .map_err(redis_problem)?;
        if status < 0 {
            return Err(Problem::new(
                "idempotency_conflict",
                "Idempotency key was reused with different input",
                409,
                "sync-coordination",
            ));
        }
        Ok(PublishedEvent {
            event_id: event.event_id,
            stream_id,
            duplicate: status == 0,
        })
    }

    async fn read_events(
        &self,
        scope: CoordinationScope<'_>,
        after_stream_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StreamEvent>, Problem> {
        self.authorize(scope.clone(), CoordinationOperation::EventRead, None)
            .await?;
        if !(1..=1_000).contains(&limit) {
            return Err(protocol::invalid("invalid_event_read_limit"));
        }
        let mut connection = self.connection.clone();
        if let Some(cursor) = after_stream_id {
            if let Some(cursor_parsed) = parse_stream_id(cursor) {
                let first_reply: StreamRangeReply = redis::cmd("XRANGE")
                    .arg(self.stream_key(scope.clone()))
                    .arg("-")
                    .arg("+")
                    .arg("COUNT")
                    .arg(1)
                    .query_async(&mut connection)
                    .await
                    .map_err(redis_problem)?;
                if let Some(first_entry) = first_reply.ids.first() {
                    if let Some(first_parsed) = parse_stream_id(&first_entry.id) {
                        if first_parsed > cursor_parsed {
                            return Err(Problem::new(
                                "sync_history_gap_resnapshot_required",
                                "Event stream history gap detected. Resnapshot required.",
                                409,
                                "sync-coordination",
                            ));
                        }
                    }
                }
            }
        }
        let start = after_stream_id.map_or_else(|| "-".to_owned(), |id| format!("({id}"));
        let reply: StreamRangeReply = redis::cmd("XRANGE")
            .arg(self.stream_key(scope))
            .arg(start)
            .arg("+")
            .arg("COUNT")
            .arg(limit)
            .query_async(&mut connection)
            .await
            .map_err(redis_problem)?;
        reply
            .ids
            .into_iter()
            .map(|entry| {
                let encoded = entry.get::<String>("event").ok_or_else(|| {
                    Problem::new(
                        "invalid_coordination_event",
                        "Stored coordination event is invalid",
                        500,
                        "sync-coordination",
                    )
                })?;
                Ok(StreamEvent {
                    stream_id: entry.id,
                    event: CoordinationEvent::decode(&encoded)?,
                })
            })
            .collect()
    }

    async fn current_manifest(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
    ) -> Result<Option<SyncManifest>, Problem> {
        self.authorize(
            scope.clone(),
            CoordinationOperation::ManifestRead,
            Some(resource_id),
        )
        .await?;
        let mut connection = self.connection.clone();
        let payload: Option<String> = connection
            .hget(self.manifest_key(scope, resource_id), "payload")
            .await
            .map_err(redis_problem)?;
        match payload {
            Some(encoded) if !encoded.is_empty() => {
                let manifest: SyncManifest =
                    protocol::decode(&encoded).map_err(|_| invalid_stored_manifest())?;
                manifest.validate()?;
                Ok(Some(manifest))
            }
            _ => Ok(None),
        }
    }

    async fn commit_manifest(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        manifest: SyncManifest,
    ) -> Result<(), Problem> {
        self.authorize(
            scope.clone(),
            CoordinationOperation::ManifestWrite,
            Some(resource_id),
        )
        .await?;
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

        let payload = protocol::encode(&manifest)?;
        let mut event_id_bytes = [0_u8; 16];
        getrandom::fill(&mut event_id_bytes).map_err(|_| {
            Problem::new(
                "event_id_unavailable",
                "Event identifier generation failed",
                503,
                "sync-coordination",
            )
        })?;
        let event_id = format!("evt_sync_{}", hex(&event_id_bytes));
        let occurred_at_ms = crate::clock::unix_time_millis()?;
        let event = CoordinationEvent {
            event_id: event_id.clone(),
            event_version: "1".to_owned(),
            name: "profile.sync.committed".to_owned(),
            occurred_at_ms,
            correlation_id: event_id.clone(),
            causation_id: event_id,
            actor_id: scope.auth.actor_id().to_owned(),
            resource: EventResource {
                resource_type: "profile".to_owned(),
                id: resource_id.as_str().to_owned(),
                revision: manifest.generation,
            },
            data: serde_json::json!({ "generation": manifest.generation }),
        };
        validate_event(scope.clone(), &event)?;
        let event_payload = event.encode()?;
        let parent_gen_str = manifest
            .parent_generation
            .map_or_else(|| "none".to_owned(), |v| v.to_string());

        let mut connection = self.connection.clone();
        let (status, msg): (i64, String) = Script::new(COMMIT_MANIFEST)
            .key(self.manifest_key(scope.clone(), resource_id))
            .key(self.lease_key(scope.clone(), resource_id))
            .key(self.stream_key(scope.clone()))
            .arg(owner_token)
            .arg(manifest.fencing_token.to_string())
            .arg(scope.workspace_id.as_str())
            .arg(resource_id.as_str())
            .arg(manifest.generation.to_string())
            .arg(parent_gen_str)
            .arg(payload)
            .arg(event_payload)
            .arg(self.stream_max_len)
            .invoke_async(&mut connection)
            .await
            .map_err(redis_problem)?;

        match status {
            1 | 2 => Ok(()),
            -1 => {
                if msg == "lease_expired" {
                    Err(Problem::new(
                        "lease_expired",
                        "Resource lease is expired",
                        409,
                        "sync-coordination",
                    ))
                } else {
                    Err(lease_missing())
                }
            }
            -2 => Err(lease_not_owner()),
            -3 => Err(Problem::new(
                "stale_manifest_fencing_token",
                "Stale manifest fencing token",
                409,
                "sync-coordination",
            )),
            -4 => Err(Problem::new(
                "manifest_workspace_mismatch",
                "Manifest workspace or resource mismatch",
                409,
                "sync-coordination",
            )),
            -5 => Err(Problem::new(
                "manifest_base_missing",
                "Manifest base is missing",
                409,
                "sync-coordination",
            )),
            -6 => Err(Problem::new(
                "stale_manifest_generation",
                "Stale or gap in manifest generation",
                409,
                "sync-coordination",
            )),
            -7 => Err(Problem::new(
                "idempotency_conflict",
                "Manifest commit conflict with different payload",
                409,
                "sync-coordination",
            )),
            _ => Err(invalid_stored_manifest()),
        }
    }
}

fn validate_ttl(ttl: Duration) -> Result<u64, Problem> {
    let seconds = ttl.as_secs();
    if ttl.subsec_nanos() != 0 || !(MIN_TTL_SECONDS..=MAX_TTL_SECONDS).contains(&seconds) {
        return Err(protocol::invalid("invalid_coordination_ttl"));
    }
    seconds
        .checked_mul(1_000)
        .ok_or_else(|| protocol::invalid("invalid_coordination_ttl"))
}

fn validate_owner_token(token: &str) -> Result<(), Problem> {
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(protocol::invalid("invalid_lease_owner_token"));
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

fn validate_idempotency_key(key: &str) -> Result<(), Problem> {
    if !(16..=128).contains(&key.len()) || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(protocol::invalid("invalid_idempotency_key"));
    }
    Ok(())
}

fn lease_fingerprint(parts: &[&str]) -> String {
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

fn key_segment(value: &str) -> String {
    value.bytes().map(|byte| format!("{byte:02x}")).collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn parse_stream_id(id: &str) -> Option<(u64, u64)> {
    let mut parts = id.split('-');
    let ms = parts.next()?.parse::<u64>().ok()?;
    let seq = parts.next()?.parse::<u64>().ok()?;
    Some((ms, seq))
}

fn workspace_prefix(namespace: &str, workspace_id: &WorkspaceId) -> String {
    format!("buglogin:{{{namespace}:{}}}", workspace_id.as_str())
}

fn lease_key(namespace: &str, workspace_id: &WorkspaceId, resource_id: &ResourceId) -> String {
    format!(
        "{}:lease:{}",
        workspace_prefix(namespace, workspace_id),
        resource_id.as_str()
    )
}

fn fence_key(namespace: &str, workspace_id: &WorkspaceId, resource_id: &ResourceId) -> String {
    format!(
        "{}:lease-fence:{}",
        workspace_prefix(namespace, workspace_id),
        resource_id.as_str()
    )
}

fn manifest_key(namespace: &str, workspace_id: &WorkspaceId, resource_id: &ResourceId) -> String {
    format!(
        "{}:manifest:{}",
        workspace_prefix(namespace, workspace_id),
        resource_id.as_str()
    )
}

fn validate_event(scope: CoordinationScope<'_>, event: &CoordinationEvent) -> Result<(), Problem> {
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
        return Err(protocol::invalid("invalid_coordination_event"));
    }
    Ok(())
}

fn presence_from_fields(fields: HashMap<String, String>) -> Result<Option<Presence>, Problem> {
    if fields.is_empty() {
        return Ok(None);
    }
    let get = |name: &str| {
        fields.get(name).ok_or_else(|| {
            Problem::new(
                "invalid_coordination_presence",
                "Stored coordination presence is invalid",
                500,
                "sync-coordination",
            )
        })
    };
    Ok(Some(Presence {
        session_id: get("sessionId")?.clone(),
        actor_id: get("actorId")?.clone(),
        connected_at_ms: parse_u64(get("connectedAt")?)?,
        last_seen_at_ms: parse_u64(get("lastSeenAt")?)?,
        expires_at_ms: parse_u64(get("expiresAt")?)?,
        metadata: protocol::decode(get("metadata")?)?,
    }))
}

fn parse_u64(value: &str) -> Result<u64, Problem> {
    value.parse().map_err(|_| {
        Problem::new(
            "invalid_coordination_number",
            "Stored coordination value is invalid",
            500,
            "sync-coordination",
        )
    })
}

fn parse_lease_number(value: &str) -> Result<u64, Problem> {
    value.parse().map_err(|_| invalid_stored_lease())
}

fn secure_token() -> Result<String, Problem> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| {
        Problem::new(
            "lease_token_generation_failed",
            "Lease token generation failed",
            500,
            "sync-coordination",
        )
    })?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn lease_missing() -> Problem {
    Problem::new(
        "lease_not_found",
        "Resource lease was not found",
        404,
        "sync-coordination",
    )
}
fn lease_expired() -> Problem {
    Problem::new(
        "lease_expired",
        "Resource lease is expired",
        409,
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
fn invalid_stored_lease() -> Problem {
    Problem::new(
        "invalid_coordination_lease",
        "Stored coordination lease is invalid",
        500,
        "sync-coordination",
    )
}
fn invalid_stored_manifest() -> Problem {
    Problem::new(
        "invalid_coordination_manifest",
        "Stored coordination manifest is invalid",
        500,
        "sync-coordination",
    )
}
fn redis_problem(_error: redis::RedisError) -> Problem {
    Problem::new(
        "coordination_unavailable",
        "Coordination service is unavailable",
        503,
        "sync-coordination",
    )
    .retryable(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_and_fence_keys_share_workspace_hash_tag() {
        let workspace = WorkspaceId::new("workspace-1").unwrap();
        let resource = ResourceId::new("resource-1").unwrap();

        assert_eq!(
            lease_key("sync", &workspace, &resource),
            "buglogin:{sync:workspace-1}:lease:resource-1"
        );
        assert_eq!(
            fence_key("sync", &workspace, &resource),
            "buglogin:{sync:workspace-1}:lease-fence:resource-1"
        );
        assert_eq!(
            manifest_key("sync", &workspace, &resource),
            "buglogin:{sync:workspace-1}:manifest:resource-1"
        );
    }

    #[test]
    fn lease_receipt_fingerprint_is_stable_and_does_not_store_owner_token() {
        let owner_token = "a1b2c3d4e5f6a7b8";
        let fingerprint = lease_fingerprint(&["actor-1", "resource-1", owner_token, "30000"]);

        assert_eq!(
            fingerprint,
            lease_fingerprint(&["actor-1", "resource-1", owner_token, "30000"])
        );
        assert!(!fingerprint.contains(owner_token));
        assert_ne!(
            fingerprint,
            lease_fingerprint(&["actor-1", "resource-1", "different-token", "30000"])
        );
    }

    #[test]
    fn redis_connection_debug_redacts_credentials_and_ca_material() {
        let config = RedisCoordinationConfig::new(
            "rediss://buglogin3:private-password@127.0.0.1:6380",
            "buglogin3-prod",
        )
        .with_tls_root_cert(b"private-ca-material".to_vec());
        let debug = format!("{config:?}");

        assert!(!debug.contains("private-password"));
        assert!(!debug.contains("private-ca-material"));
        assert!(debug.contains("<redacted>"));
        assert!(debug.contains("<configured>"));
    }
}
