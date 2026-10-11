use super::{
    CookieError, CookieInput, CookieReport, CookieScope, CookieService, CookieSummary,
    PersistedCookieAuthorityState, PersistedCookieGeneration, PersistedCookieReceipt,
};
use buglogin_domain::{
    lease::Lease,
    permissions::{Action, Capability, Grant, Principal, ResourceScope, is_allowed},
};
use buglogin_storage::secrets::OsVault;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt};

const COOKIE_CAPABILITY: &str = super::COOKIE_CAPABILITY;
const STAGED_ID_BYTES: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileRuntimeState {
    Stopped,
    Running,
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct StagedImportId(String);

impl StagedImportId {
    pub fn generate() -> Result<Self, CookieAuthorityError> {
        let mut bytes = [0_u8; STAGED_ID_BYTES];
        getrandom::fill(&mut bytes).map_err(|_| CookieAuthorityError::RandomnessUnavailable)?;
        Ok(Self(hex(&bytes)))
    }

    pub fn from_opaque(value: impl Into<String>) -> Result<Self, CookieAuthorityError> {
        let value = value.into();
        if value.len() != STAGED_ID_BYTES * 2 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(CookieAuthorityError::InvalidStagedImportId);
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for StagedImportId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("StagedImportId")
            .field(&"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    pub fn new(value: impl Into<String>) -> Result<Self, CookieAuthorityError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 256
            || !value.bytes().all(|byte| !byte.is_ascii_control())
        {
            return Err(CookieAuthorityError::InvalidIdempotencyKey);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for IdempotencyKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("IdempotencyKey")
            .field(&"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CookieMutationResult {
    pub report: CookieReport,
    pub generation: u64,
    pub replayed: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CookieStageReceipt {
    pub stage_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CookieListResult {
    pub generation: u64,
    pub cookies: Vec<CookieSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CookieAuthorityError {
    Forbidden,
    InvalidStagedImportId,
    StagedImportNotFound,
    InvalidIdempotencyKey,
    IdempotencyConflict,
    ProfileStateUnknown,
    ProfileRunning,
    LeaseMissing,
    LeaseExpired,
    LeaseMismatch,
    LeaseFencingMismatch,
    InvalidLease,
    GenerationMismatch { expected: u64, actual: u64 },
    GenerationExhausted,
    RandomnessUnavailable,
    Storage,
    LockUnavailable,
    Cookie(CookieError),
}

impl fmt::Display for CookieAuthorityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Forbidden => formatter.write_str("cookie_access_denied"),
            Self::InvalidStagedImportId => formatter.write_str("invalid_staged_cookie_import_id"),
            Self::StagedImportNotFound => formatter.write_str("staged_cookie_import_not_found"),
            Self::InvalidIdempotencyKey => formatter.write_str("invalid_idempotency_key"),
            Self::IdempotencyConflict => formatter.write_str("idempotency_conflict"),
            Self::ProfileStateUnknown => formatter.write_str("cookie_profile_state_unknown"),
            Self::ProfileRunning => formatter.write_str("cookie_profile_running"),
            Self::LeaseMissing => formatter.write_str("cookie_lease_missing"),
            Self::LeaseExpired => formatter.write_str("cookie_lease_expired"),
            Self::LeaseMismatch => formatter.write_str("cookie_lease_mismatch"),
            Self::LeaseFencingMismatch => formatter.write_str("cookie_lease_fencing_mismatch"),
            Self::InvalidLease => formatter.write_str("invalid_cookie_lease"),
            Self::GenerationMismatch { .. } => formatter.write_str("cookie_generation_mismatch"),
            Self::GenerationExhausted => formatter.write_str("cookie_generation_exhausted"),
            Self::RandomnessUnavailable => formatter.write_str("cookie_randomness_unavailable"),
            Self::Storage => formatter.write_str("cookie_authority_storage_error"),
            Self::LockUnavailable => formatter.write_str("cookie_authority_lock_unavailable"),
            Self::Cookie(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for CookieAuthorityError {}

impl From<CookieError> for CookieAuthorityError {
    fn from(error: CookieError) -> Self {
        Self::Cookie(error)
    }
}

#[derive(Clone)]
struct StagedImport {
    scope: CookieScope,
    inputs: Vec<CookieInput>,
}

#[derive(Clone)]
struct Replay {
    scope: CookieScope,
    fingerprint: [u8; 32],
    result: CookieMutationResult,
}

/// Cookie command authority.
///
/// Cookie metadata, generations, and idempotency receipts persist through the
/// service ledger. Staged imports, runtime states, and leases remain process-
/// local by design and must be recreated after restart.
pub struct CookieAuthority<V: OsVault> {
    service: CookieService<V>,
    profile_states: BTreeMap<CookieScope, ProfileRuntimeState>,
    generations: BTreeMap<CookieScope, u64>,
    leases: BTreeMap<CookieScope, Lease>,
    staged: BTreeMap<StagedImportId, StagedImport>,
    idempotency: BTreeMap<IdempotencyKey, Replay>,
}

impl<V: OsVault> CookieAuthority<V> {
    pub fn new(service: CookieService<V>) -> Self {
        Self {
            service,
            profile_states: BTreeMap::new(),
            generations: BTreeMap::new(),
            leases: BTreeMap::new(),
            staged: BTreeMap::new(),
            idempotency: BTreeMap::new(),
        }
    }

    pub fn open(
        vault: V,
        root: impl Into<std::path::PathBuf>,
    ) -> Result<Self, CookieAuthorityError> {
        let service = CookieService::open(vault, root).map_err(CookieAuthorityError::Cookie)?;
        let mut authority = Self::new(service);
        authority.restore_state()?;
        Ok(authority)
    }

    fn restore_state(&mut self) -> Result<(), CookieAuthorityError> {
        let state = self.service.authority_state().clone();
        if state.schema_version == 0 {
            return Ok(());
        }
        if state.schema_version != 1 {
            return Err(CookieAuthorityError::Storage);
        }
        for item in state.generations {
            let scope = CookieScope {
                workspace_id: buglogin_domain::ids::WorkspaceId::from_str(&item.workspace_id)
                    .map_err(|_| CookieAuthorityError::Storage)?,
                profile_id: buglogin_domain::ids::ProfileId::from_str(&item.profile_id)
                    .map_err(|_| CookieAuthorityError::Storage)?,
            };
            self.generations.insert(scope, item.generation);
        }
        for item in state.receipts {
            let scope = CookieScope {
                workspace_id: buglogin_domain::ids::WorkspaceId::from_str(&item.workspace_id)
                    .map_err(|_| CookieAuthorityError::Storage)?,
                profile_id: buglogin_domain::ids::ProfileId::from_str(&item.profile_id)
                    .map_err(|_| CookieAuthorityError::Storage)?,
            };
            let key = IdempotencyKey::new(item.idempotency_key)?;
            let bytes = hex_decode(&item.fingerprint).ok_or(CookieAuthorityError::Storage)?;
            let fingerprint: [u8; 32] = bytes
                .try_into()
                .map_err(|_| CookieAuthorityError::Storage)?;
            self.idempotency.insert(
                key,
                Replay {
                    scope,
                    fingerprint,
                    result: CookieMutationResult {
                        report: item.report,
                        generation: item.generation,
                        replayed: false,
                    },
                },
            );
        }
        Ok(())
    }

    fn persisted_state_with(
        generations: &BTreeMap<CookieScope, u64>,
        idempotency: &BTreeMap<IdempotencyKey, Replay>,
        scope: CookieScope,
        generation: u64,
        idempotency_key: IdempotencyKey,
        fingerprint: [u8; 32],
        report: CookieReport,
    ) -> PersistedCookieAuthorityState {
        let mut generation_map = generations.clone();
        generation_map.insert(scope.clone(), generation);
        let generations = generation_map
            .iter()
            .map(|(item_scope, item_generation)| PersistedCookieGeneration {
                workspace_id: item_scope.workspace_id.as_str().to_owned(),
                profile_id: item_scope.profile_id.as_str().to_owned(),
                generation: *item_generation,
            })
            .collect::<Vec<_>>();
        let mut receipt_map = idempotency.clone();
        receipt_map.insert(
            idempotency_key.clone(),
            Replay {
                scope: scope.clone(),
                fingerprint,
                result: CookieMutationResult {
                    report: report.clone(),
                    generation,
                    replayed: false,
                },
            },
        );
        let receipts = receipt_map
            .iter()
            .map(|(key, replay)| PersistedCookieReceipt {
                idempotency_key: key.as_str().to_owned(),
                workspace_id: replay.scope.workspace_id.as_str().to_owned(),
                profile_id: replay.scope.profile_id.as_str().to_owned(),
                fingerprint: hex(&replay.fingerprint),
                report: replay.result.report.clone(),
                generation: replay.result.generation,
            })
            .collect::<Vec<_>>();
        PersistedCookieAuthorityState {
            schema_version: 1,
            generations,
            receipts,
        }
    }

    pub fn set_profile_state(&mut self, scope: CookieScope, state: ProfileRuntimeState) {
        self.profile_states.insert(scope, state);
    }

    pub fn set_current_lease(
        &mut self,
        scope: CookieScope,
        lease: Lease,
    ) -> Result<(), CookieAuthorityError> {
        validate_lease(&scope, &lease)?;
        if let Some(previous) = self.leases.get(&scope) {
            let same_lease = previous.fencing_token == lease.fencing_token
                && previous.owner_id == lease.owner_id
                && previous.token == lease.token;
            if !same_lease && lease.fencing_token <= previous.fencing_token {
                return Err(CookieAuthorityError::LeaseFencingMismatch);
            }
        }
        self.leases.insert(scope, lease);
        Ok(())
    }

    pub fn generation(&self, scope: &CookieScope) -> u64 {
        self.generations.get(scope).copied().unwrap_or(0)
    }

    pub fn stage_import(
        &mut self,
        scope: CookieScope,
        inputs: impl IntoIterator<Item = CookieInput>,
    ) -> Result<StagedImportId, CookieAuthorityError> {
        let mut unique = BTreeMap::new();
        for input in inputs {
            let input = super::normalize(input)?;
            let identity = super::identity(&input);
            if unique.insert(identity, input).is_some() {
                return Err(CookieError::Duplicate.into());
            }
        }
        let id = loop {
            let candidate = StagedImportId::generate()?;
            if !self.staged.contains_key(&candidate) {
                break candidate;
            }
        };
        self.staged.insert(
            id.clone(),
            StagedImport {
                scope,
                inputs: unique.into_values().collect(),
            },
        );
        Ok(id)
    }

    pub fn import(
        &mut self,
        stage_id: &StagedImportId,
        expected_generation: u64,
        lease: &Lease,
        now_unix_ms: u64,
        principal: &Principal,
        grants: &[Grant],
        idempotency_key: IdempotencyKey,
    ) -> Result<CookieMutationResult, CookieAuthorityError> {
        if let Some(replay) = self.idempotency.get(&idempotency_key) {
            let fingerprint = fingerprint(
                "import",
                &[
                    stage_id.as_str(),
                    replay.scope.workspace_id.as_str(),
                    replay.scope.profile_id.as_str(),
                ],
                expected_generation,
                lease.fencing_token,
            );
            self.validate_replay_authority(&replay.scope, lease, now_unix_ms, principal, grants)?;
            authorize(&replay.scope, Action::Create, principal, grants)?;
            if replay.fingerprint != fingerprint {
                return Err(CookieAuthorityError::IdempotencyConflict);
            }
            let mut result = replay.result.clone();
            result.replayed = true;
            return Ok(result);
        }
        let staged = self
            .staged
            .get(stage_id)
            .ok_or(CookieAuthorityError::StagedImportNotFound)?
            .clone();
        let scope = staged.scope.clone();
        let fingerprint = fingerprint(
            "import",
            &[
                stage_id.as_str(),
                scope.workspace_id.as_str(),
                scope.profile_id.as_str(),
            ],
            expected_generation,
            lease.fencing_token,
        );
        self.validate_mutation(
            &scope,
            expected_generation,
            lease,
            now_unix_ms,
            principal,
            grants,
        )?;
        self.ensure_generation_available(expected_generation)?;
        let inputs = staged.inputs.clone();
        let generation = expected_generation
            .checked_add(1)
            .ok_or(CookieAuthorityError::GenerationExhausted)?;
        let generations = self.generations.clone();
        let idempotency = self.idempotency.clone();
        let preview = self
            .service
            .preview_import(&scope, &inputs, principal, grants)?;
        let authority_state = Self::persisted_state_with(
            &generations,
            &idempotency,
            scope.clone(),
            generation,
            idempotency_key.clone(),
            fingerprint,
            preview,
        );
        let report = self.service.import_with_authority_state(
            scope.clone(),
            inputs,
            principal,
            grants,
            &authority_state,
        )?;
        self.generations.insert(scope.clone(), generation);
        self.staged.remove(stage_id);
        let result = CookieMutationResult {
            report,
            generation,
            replayed: false,
        };
        self.idempotency.insert(
            idempotency_key,
            Replay {
                scope,
                fingerprint,
                result: result.clone(),
            },
        );
        Ok(result)
    }

    pub fn copy(
        &mut self,
        source: &CookieScope,
        destination: CookieScope,
        expected_destination_generation: u64,
        destination_lease: &Lease,
        now_unix_ms: u64,
        principal: &Principal,
        grants: &[Grant],
        idempotency_key: IdempotencyKey,
    ) -> Result<CookieMutationResult, CookieAuthorityError> {
        let fingerprint = fingerprint(
            "copy",
            &[
                source.workspace_id.as_str(),
                source.profile_id.as_str(),
                destination.workspace_id.as_str(),
                destination.profile_id.as_str(),
            ],
            expected_destination_generation,
            destination_lease.fencing_token,
        );
        if let Some(replay) = self.idempotency.get(&idempotency_key) {
            self.require_stopped(source)?;
            self.validate_replay_authority(
                &replay.scope,
                destination_lease,
                now_unix_ms,
                principal,
                grants,
            )?;
            authorize(source, Action::Read, principal, grants)?;
            authorize(&destination, Action::Create, principal, grants)?;
            if replay.scope != destination || replay.fingerprint != fingerprint {
                return Err(CookieAuthorityError::IdempotencyConflict);
            }
            let mut result = replay.result.clone();
            result.replayed = true;
            return Ok(result);
        }
        self.require_stopped(source)?;
        self.validate_mutation(
            &destination,
            expected_destination_generation,
            destination_lease,
            now_unix_ms,
            principal,
            grants,
        )?;
        self.ensure_generation_available(expected_destination_generation)?;
        let generation = expected_destination_generation
            .checked_add(1)
            .ok_or(CookieAuthorityError::GenerationExhausted)?;
        let generations = self.generations.clone();
        let idempotency = self.idempotency.clone();
        let preview = self
            .service
            .preview_copy(source, &destination, principal, grants)?;
        let authority_state = Self::persisted_state_with(
            &generations,
            &idempotency,
            destination.clone(),
            generation,
            idempotency_key.clone(),
            fingerprint,
            preview,
        );
        let report = self.service.copy_with_authority_state(
            source,
            destination.clone(),
            principal,
            grants,
            &authority_state,
        )?;
        self.generations.insert(destination.clone(), generation);
        let result = CookieMutationResult {
            report,
            generation,
            replayed: false,
        };
        self.idempotency.insert(
            idempotency_key,
            Replay {
                scope: destination,
                fingerprint,
                result: result.clone(),
            },
        );
        Ok(result)
    }

    pub fn list(
        &self,
        scope: &CookieScope,
        principal: &Principal,
        grants: &[Grant],
    ) -> Result<CookieListResult, CookieAuthorityError> {
        self.require_stopped(scope)?;
        self.service
            .list(scope, principal, grants)
            .map(|cookies| CookieListResult {
                generation: self.generation(scope),
                cookies,
            })
            .map_err(Into::into)
    }

    fn validate_mutation(
        &self,
        scope: &CookieScope,
        expected_generation: u64,
        lease: &Lease,
        now_unix_ms: u64,
        principal: &Principal,
        grants: &[Grant],
    ) -> Result<(), CookieAuthorityError> {
        self.require_stopped(scope)?;
        authorize(scope, Action::Create, principal, grants)?;
        validate_lease(scope, lease)?;
        let actual = self.generation(scope);
        if actual != expected_generation {
            return Err(CookieAuthorityError::GenerationMismatch {
                expected: expected_generation,
                actual,
            });
        }
        let current = self
            .leases
            .get(scope)
            .ok_or(CookieAuthorityError::LeaseMissing)?;
        if now_unix_ms >= lease.expires_at || now_unix_ms >= current.expires_at {
            return Err(CookieAuthorityError::LeaseExpired);
        }
        if current.fencing_token != lease.fencing_token {
            return Err(CookieAuthorityError::LeaseFencingMismatch);
        }
        if current.workspace_id != lease.workspace_id
            || current.resource_id != lease.resource_id
            || current.owner_id != lease.owner_id
            || current.token != lease.token
        {
            return Err(CookieAuthorityError::LeaseMismatch);
        }
        Ok(())
    }

    fn ensure_generation_available(&self, expected: u64) -> Result<(), CookieAuthorityError> {
        if expected == u64::MAX {
            Err(CookieAuthorityError::GenerationExhausted)
        } else {
            Ok(())
        }
    }

    fn validate_replay_authority(
        &self,
        scope: &CookieScope,
        lease: &Lease,
        now_unix_ms: u64,
        principal: &Principal,
        grants: &[Grant],
    ) -> Result<(), CookieAuthorityError> {
        self.require_stopped(scope)?;
        authorize(scope, Action::Create, principal, grants)?;
        validate_lease(scope, lease)?;
        let current = self
            .leases
            .get(scope)
            .ok_or(CookieAuthorityError::LeaseMissing)?;
        if now_unix_ms >= lease.expires_at || now_unix_ms >= current.expires_at {
            return Err(CookieAuthorityError::LeaseExpired);
        }
        if current.fencing_token != lease.fencing_token {
            return Err(CookieAuthorityError::LeaseFencingMismatch);
        }
        if current.workspace_id != lease.workspace_id
            || current.resource_id != lease.resource_id
            || current.owner_id != lease.owner_id
            || current.token != lease.token
        {
            return Err(CookieAuthorityError::LeaseMismatch);
        }
        Ok(())
    }

    fn require_stopped(&self, scope: &CookieScope) -> Result<(), CookieAuthorityError> {
        match self.profile_states.get(scope) {
            Some(ProfileRuntimeState::Stopped) => Ok(()),
            Some(ProfileRuntimeState::Running) => Err(CookieAuthorityError::ProfileRunning),
            None => Err(CookieAuthorityError::ProfileStateUnknown),
        }
    }

    fn advance_generation(
        &mut self,
        scope: &CookieScope,
        expected: u64,
    ) -> Result<u64, CookieAuthorityError> {
        let actual = self.generation(scope);
        if actual != expected {
            return Err(CookieAuthorityError::GenerationMismatch { expected, actual });
        }
        let next = expected
            .checked_add(1)
            .ok_or(CookieAuthorityError::GenerationExhausted)?;
        self.generations.insert(scope.clone(), next);
        Ok(next)
    }
}

fn authorize(
    scope: &CookieScope,
    action: Action,
    principal: &Principal,
    grants: &[Grant],
) -> Result<(), CookieAuthorityError> {
    let capability =
        Capability::new(COOKIE_CAPABILITY).map_err(|_| CookieAuthorityError::Forbidden)?;
    let resource_scope = ResourceScope {
        workspace_id: Some(scope.workspace_id.clone()),
        profile_id: Some(scope.profile_id.clone()),
        ..ResourceScope::default()
    };
    if is_allowed(principal, &capability, action, &resource_scope, grants) {
        Ok(())
    } else {
        Err(CookieAuthorityError::Forbidden)
    }
}

fn validate_lease(scope: &CookieScope, lease: &Lease) -> Result<(), CookieAuthorityError> {
    if lease.workspace_id != scope.workspace_id
        || lease.resource_id.as_str().is_empty()
        || lease.owner_id.is_empty()
        || lease.token.is_empty()
        || lease.fencing_token == 0
        || lease.expires_at <= lease.issued_at
    {
        return Err(CookieAuthorityError::InvalidLease);
    }
    Ok(())
}

fn fingerprint(
    operation: &str,
    values: &[&str],
    expected_generation: u64,
    fencing_token: u64,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(operation.as_bytes());
    hasher.update([0]);
    for value in values {
        hasher.update(value.as_bytes());
        hasher.update([0]);
    }
    hasher.update(expected_generation.to_be_bytes());
    hasher.update(fencing_token.to_be_bytes());
    hasher.finalize().into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_decode(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect()
}
