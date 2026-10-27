use super::{
    CookieError, CookieInput, CookieReport, CookieScope, CookieService, CookieSummary,
    PersistedCookieAuthorityState, PersistedCookieGeneration, PersistedCookieMaterialization,
    PersistedCookieReceipt, PersistedCookieStage, PersistedHostedCookieJobBinding,
};
use buglogin_domain::{
    errors::Problem,
    lease::Lease,
    permissions::{Action, Capability, Grant, Principal, ResourceScope, is_allowed},
};
use buglogin_storage::secrets::OsVault;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    time::{SystemTime, UNIX_EPOCH},
};

const COOKIE_CAPABILITY: &str = super::COOKIE_CAPABILITY;
const STAGED_ID_BYTES: usize = 32;
const STAGED_IMPORT_TTL_MS: u64 = 15 * 60 * 1000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostedCookieJobBinding {
    pub job_id: String,
    pub operation_id: String,
    pub device_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostedCookieRecoveryRecord {
    pub scope: CookieScope,
    pub idempotency_key: String,
    pub job: HostedCookieJobBinding,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostedCookieJobReceipt {
    pub scope: CookieScope,
    pub idempotency_key: String,
    pub job: HostedCookieJobBinding,
    pub mutation: CookieMutationResult,
    pub materialization_pending: bool,
}

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
    StagedImportExpired,
    MaterializationPending,
    MaterializationNotFound,
    InvalidIdempotencyKey,
    IdempotencyConflict,
    ProfileStateUnknown,
    ProfileRunning,
    LeaseMissing,
    LeaseExpired,
    LeaseMismatch,
    LeaseFencingMismatch,
    InvalidLease,
    InvalidHostedJobBinding,
    GenerationMismatch { expected: u64, actual: u64 },
    GenerationExhausted,
    ClockUnavailable,
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
            Self::StagedImportExpired => formatter.write_str("staged_cookie_import_expired"),
            Self::MaterializationPending => {
                formatter.write_str("cookie_materialization_recovery_pending")
            }
            Self::MaterializationNotFound => {
                formatter.write_str("cookie_materialization_not_found")
            }
            Self::InvalidIdempotencyKey => formatter.write_str("invalid_idempotency_key"),
            Self::IdempotencyConflict => formatter.write_str("idempotency_conflict"),
            Self::ProfileStateUnknown => formatter.write_str("cookie_profile_state_unknown"),
            Self::ProfileRunning => formatter.write_str("cookie_profile_running"),
            Self::LeaseMissing => formatter.write_str("cookie_lease_missing"),
            Self::LeaseExpired => formatter.write_str("cookie_lease_expired"),
            Self::LeaseMismatch => formatter.write_str("cookie_lease_mismatch"),
            Self::LeaseFencingMismatch => formatter.write_str("cookie_lease_fencing_mismatch"),
            Self::InvalidLease => formatter.write_str("invalid_cookie_lease"),
            Self::InvalidHostedJobBinding => {
                formatter.write_str("invalid_hosted_cookie_job_binding")
            }
            Self::GenerationMismatch { .. } => formatter.write_str("cookie_generation_mismatch"),
            Self::GenerationExhausted => formatter.write_str("cookie_generation_exhausted"),
            Self::ClockUnavailable => formatter.write_str("clock_unavailable"),
            Self::RandomnessUnavailable => formatter.write_str("cookie_randomness_unavailable"),
            Self::Storage => formatter.write_str("cookie_authority_storage_error"),
            Self::LockUnavailable => formatter.write_str("cookie_authority_lock_unavailable"),
            Self::Cookie(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for CookieAuthorityError {}

impl CookieAuthorityError {
    pub fn as_problem(&self, correlation_id: &str) -> Problem {
        let (status, title) = match self {
            Self::Forbidden | Self::Cookie(CookieError::Forbidden) => {
                (403, "Cookie access is not allowed")
            }
            Self::InvalidStagedImportId
            | Self::InvalidIdempotencyKey
            | Self::InvalidLease
            | Self::InvalidHostedJobBinding
            | Self::Cookie(
                CookieError::InvalidName
                | CookieError::InvalidDomain
                | CookieError::InvalidPath
                | CookieError::InvalidValue
                | CookieError::Duplicate,
            ) => (400, "Cookie request is invalid"),
            Self::StagedImportNotFound | Self::MaterializationNotFound => {
                (404, "Cookie stage or recovery record was not found")
            }
            Self::StagedImportExpired => (410, "Cookie stage has expired"),
            Self::ClockUnavailable => (503, "System clock is unavailable"),
            Self::RandomnessUnavailable
            | Self::Storage
            | Self::Cookie(CookieError::Storage | CookieError::Vault(_)) => {
                (503, "Cookie storage is unavailable")
            }
            Self::LockUnavailable | Self::Cookie(CookieError::Locked) => {
                (423, "Cookie storage is locked")
            }
            Self::Cookie(CookieError::NotFound) => (404, "Cookie was not found"),
            Self::Cookie(CookieError::Rollback) => (409, "Cookie storage requires recovery"),
            Self::MaterializationPending
            | Self::IdempotencyConflict
            | Self::ProfileStateUnknown
            | Self::ProfileRunning
            | Self::LeaseMissing
            | Self::LeaseExpired
            | Self::LeaseMismatch
            | Self::LeaseFencingMismatch
            | Self::GenerationMismatch { .. }
            | Self::GenerationExhausted => (409, "Cookie operation conflicts with current state"),
        };
        let problem = Problem::new(self.to_string(), title, status, correlation_id);
        if matches!(self, Self::ClockUnavailable) {
            problem.retryable(true)
        } else {
            problem
        }
    }
}

impl From<CookieError> for CookieAuthorityError {
    fn from(error: CookieError) -> Self {
        Self::Cookie(error)
    }
}

#[derive(Clone)]
struct StagedImport {
    scope: CookieScope,
    expires_at_unix_ms: u64,
    inputs: Vec<CookieInput>,
}

#[derive(Clone)]
struct Replay {
    scope: CookieScope,
    fingerprint: [u8; 32],
    result: CookieMutationResult,
    hosted_job: Option<HostedCookieJobBinding>,
}

/// Cookie command authority.
///
/// Cookie metadata, generations, idempotency receipts, and encrypted staged
/// values persist through the service ledger and OS vault. Runtime states and
/// leases remain process-local and must be re-established after restart.
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
        let now_ms = unix_now_ms()?;
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
            let hosted_job = item.hosted_job.map(|job| HostedCookieJobBinding {
                job_id: job.job_id,
                operation_id: job.operation_id,
                device_id: job.device_id,
            });
            if let Some(binding) = &hosted_job {
                validate_hosted_job_binding(binding).map_err(|_| CookieAuthorityError::Storage)?;
            }
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
                    hosted_job,
                },
            );
        }
        validate_persisted_stage_ids(&state.stages)?;
        let mut pending_scopes = BTreeSet::new();
        let mut pending_keys = BTreeSet::new();
        for pending in &state.pending_materializations {
            let scope = CookieScope {
                workspace_id: buglogin_domain::ids::WorkspaceId::from_str(&pending.workspace_id)
                    .map_err(|_| CookieAuthorityError::Storage)?,
                profile_id: buglogin_domain::ids::ProfileId::from_str(&pending.profile_id)
                    .map_err(|_| CookieAuthorityError::Storage)?,
            };
            let idempotency_key = IdempotencyKey::new(pending.idempotency_key.clone())?;
            let Some(receipt) = self.idempotency.get(&idempotency_key) else {
                return Err(CookieAuthorityError::Storage);
            };
            if pending.generation != self.generation(&scope)
                || pending.expected_generation.checked_add(1) != Some(pending.generation)
                || receipt.scope != scope
                || receipt.result.generation != pending.generation
                || !pending_scopes.insert(scope.clone())
                || !pending_keys.insert(idempotency_key)
            {
                return Err(CookieAuthorityError::Storage);
            }
            validate_pending_materialization_binding(pending, &scope, receipt, &state.stages)?;
            match pending.command.as_str() {
                "cookie.import"
                    if pending.source_workspace_id.is_none()
                        && pending.source_profile_id.is_none() => {}
                "cookie.copy" => {
                    let source_workspace = pending
                        .source_workspace_id
                        .as_deref()
                        .and_then(|value| buglogin_domain::ids::WorkspaceId::from_str(value).ok());
                    let source_profile = pending
                        .source_profile_id
                        .as_deref()
                        .and_then(|value| buglogin_domain::ids::ProfileId::from_str(value).ok());
                    if source_workspace.is_none() || source_profile.is_none() {
                        return Err(CookieAuthorityError::Storage);
                    }
                }
                _ => return Err(CookieAuthorityError::Storage),
            }
        }
        let pending_stage_ids = state
            .pending_materializations
            .iter()
            .filter_map(|pending| pending.staged_import_id.clone())
            .collect::<BTreeSet<_>>();
        for stage in state.stages {
            let id = StagedImportId::from_opaque(stage.stage_id.clone())?;
            let scope = CookieScope {
                workspace_id: buglogin_domain::ids::WorkspaceId::from_str(&stage.workspace_id)
                    .map_err(|_| CookieAuthorityError::Storage)?,
                profile_id: buglogin_domain::ids::ProfileId::from_str(&stage.profile_id)
                    .map_err(|_| CookieAuthorityError::Storage)?,
            };
            if stage.cleanup_pending {
                if pending_stage_ids.contains(&stage.stage_id) {
                    let _ = self.service.cleanup_pending_stage(&scope, &stage);
                } else {
                    self.service.discard_stage(&scope, &stage)?;
                }
                continue;
            }
            if stage.expires_at_unix_ms <= now_ms {
                self.service.discard_stage(&scope, &stage)?;
                continue;
            }
            let inputs = match self.service.restore_stage_inputs(&scope, &stage) {
                Ok(inputs) => inputs,
                Err(_) => {
                    self.service.discard_stage(&scope, &stage)?;
                    continue;
                }
            };
            self.staged.insert(
                id,
                StagedImport {
                    scope,
                    expires_at_unix_ms: stage.expires_at_unix_ms,
                    inputs,
                },
            );
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn persisted_state_with(
        generations: &BTreeMap<CookieScope, u64>,
        idempotency: &BTreeMap<IdempotencyKey, Replay>,
        scope: CookieScope,
        generation: u64,
        idempotency_key: IdempotencyKey,
        fingerprint: [u8; 32],
        report: CookieReport,
        hosted_job: Option<&HostedCookieJobBinding>,
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
                hosted_job: hosted_job.cloned(),
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
                hosted_job: replay.hosted_job.as_ref().map(persisted_hosted_job_binding),
            })
            .collect::<Vec<_>>();
        PersistedCookieAuthorityState {
            schema_version: 1,
            generations,
            receipts,
            stages: Vec::new(),
            pending_materializations: Vec::new(),
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

    pub fn has_idempotency_receipt(&self, key: &IdempotencyKey) -> bool {
        self.idempotency.contains_key(key)
    }

    pub fn pending_materializations(
        &self,
    ) -> Result<Vec<(CookieScope, String)>, CookieAuthorityError> {
        self.service
            .authority_state()
            .pending_materializations
            .iter()
            .map(|entry| {
                Ok((
                    CookieScope {
                        workspace_id: buglogin_domain::ids::WorkspaceId::from_str(
                            &entry.workspace_id,
                        )
                        .map_err(|_| CookieAuthorityError::Storage)?,
                        profile_id: buglogin_domain::ids::ProfileId::from_str(&entry.profile_id)
                            .map_err(|_| CookieAuthorityError::Storage)?,
                    },
                    entry.idempotency_key.clone(),
                ))
            })
            .collect()
    }

    pub fn pending_hosted_recoveries(
        &self,
    ) -> Result<Vec<HostedCookieRecoveryRecord>, CookieAuthorityError> {
        self.service
            .authority_state()
            .pending_materializations
            .iter()
            .filter_map(|pending| {
                let job = pending.hosted_job.as_ref()?;
                Some((pending, job))
            })
            .map(|(pending, job)| {
                Ok(HostedCookieRecoveryRecord {
                    scope: CookieScope {
                        workspace_id: buglogin_domain::ids::WorkspaceId::from_str(
                            &pending.workspace_id,
                        )
                        .map_err(|_| CookieAuthorityError::Storage)?,
                        profile_id: buglogin_domain::ids::ProfileId::from_str(&pending.profile_id)
                            .map_err(|_| CookieAuthorityError::Storage)?,
                    },
                    idempotency_key: pending.idempotency_key.clone(),
                    job: HostedCookieJobBinding {
                        job_id: job.job_id.clone(),
                        operation_id: job.operation_id.clone(),
                        device_id: job.device_id.clone(),
                    },
                })
            })
            .collect()
    }

    pub fn hosted_job_receipts(&self) -> Result<Vec<HostedCookieJobReceipt>, CookieAuthorityError> {
        self.idempotency
            .iter()
            .filter_map(|(key, replay)| {
                let job = replay.hosted_job.as_ref()?;
                Some((key, replay, job))
            })
            .map(|(key, replay, job)| {
                let scope = replay.scope.clone();
                Ok(HostedCookieJobReceipt {
                    materialization_pending: self.has_pending_materialization(&scope),
                    scope,
                    idempotency_key: key.as_str().to_owned(),
                    job: job.clone(),
                    mutation: replay.result.clone(),
                })
            })
            .collect()
    }

    fn has_pending_materialization(&self, scope: &CookieScope) -> bool {
        self.service
            .authority_state()
            .pending_materializations
            .iter()
            .any(|entry| {
                entry.workspace_id == scope.workspace_id.as_str()
                    && entry.profile_id == scope.profile_id.as_str()
            })
    }

    pub fn has_pending_materialization_for_profile(&self, profile_id: &str) -> bool {
        self.service
            .authority_state()
            .pending_materializations
            .iter()
            .any(|entry| entry.profile_id == profile_id)
    }

    pub fn materialization_inputs(
        &self,
        scope: &CookieScope,
        principal: &Principal,
        grants: &[Grant],
    ) -> Result<Vec<CookieInput>, CookieAuthorityError> {
        self.require_stopped(scope)?;
        self.service
            .materialization_inputs(scope, principal, grants)
            .map_err(Into::into)
    }

    pub fn complete_materialization(
        &mut self,
        scope: &CookieScope,
        idempotency_key: &str,
    ) -> Result<(), CookieAuthorityError> {
        self.require_stopped(scope)?;
        let mut authority_state = self.service.authority_state().clone();
        let Some(index) = authority_state
            .pending_materializations
            .iter()
            .position(|entry| {
                entry.workspace_id == scope.workspace_id.as_str()
                    && entry.profile_id == scope.profile_id.as_str()
                    && entry.idempotency_key == idempotency_key
            })
        else {
            return Err(CookieAuthorityError::MaterializationNotFound);
        };
        if authority_state.pending_materializations[index].generation != self.generation(scope) {
            return Err(CookieAuthorityError::GenerationMismatch {
                expected: authority_state.pending_materializations[index].generation,
                actual: self.generation(scope),
            });
        }
        authority_state.pending_materializations.remove(index);
        self.service.commit_authority_state(&authority_state)?;
        let cleanup_stages = authority_state
            .stages
            .iter()
            .filter(|stage| {
                stage.cleanup_pending
                    && stage.workspace_id == scope.workspace_id.as_str()
                    && stage.profile_id == scope.profile_id.as_str()
            })
            .cloned()
            .collect::<Vec<_>>();
        for stage in cleanup_stages {
            let _ = self.service.discard_stage(scope, &stage);
        }
        Ok(())
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
        let expires_at_unix_ms = unix_now_ms()?
            .checked_add(STAGED_IMPORT_TTL_MS)
            .ok_or(CookieAuthorityError::ClockUnavailable)?;
        let inputs = unique.into_values().collect::<Vec<_>>();
        let persisted =
            self.service
                .persist_stage(&scope, id.as_str(), expires_at_unix_ms, inputs.clone())?;
        self.staged.insert(
            id.clone(),
            StagedImport {
                scope,
                expires_at_unix_ms: persisted.expires_at_unix_ms,
                inputs,
            },
        );
        Ok(id)
    }

    #[allow(clippy::too_many_arguments)]
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
        self.import_with_hosted_binding(
            stage_id,
            expected_generation,
            lease,
            now_unix_ms,
            principal,
            grants,
            idempotency_key,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn import_hosted(
        &mut self,
        stage_id: &StagedImportId,
        expected_generation: u64,
        lease: &Lease,
        now_unix_ms: u64,
        principal: &Principal,
        grants: &[Grant],
        idempotency_key: IdempotencyKey,
        hosted_job: &HostedCookieJobBinding,
    ) -> Result<CookieMutationResult, CookieAuthorityError> {
        validate_hosted_job_binding(hosted_job)?;
        self.import_with_hosted_binding(
            stage_id,
            expected_generation,
            lease,
            now_unix_ms,
            principal,
            grants,
            idempotency_key,
            Some(hosted_job),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn import_with_hosted_binding(
        &mut self,
        stage_id: &StagedImportId,
        expected_generation: u64,
        lease: &Lease,
        now_unix_ms: u64,
        principal: &Principal,
        grants: &[Grant],
        idempotency_key: IdempotencyKey,
        hosted_job: Option<&HostedCookieJobBinding>,
    ) -> Result<CookieMutationResult, CookieAuthorityError> {
        if let Some(replay) = self.idempotency.get(&idempotency_key) {
            if replay.hosted_job.as_ref() != hosted_job {
                return Err(CookieAuthorityError::IdempotencyConflict);
            }
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
        if unix_now_ms()? >= staged.expires_at_unix_ms {
            let stage = self
                .service
                .authority_state()
                .stages
                .iter()
                .find(|stage| stage.stage_id == stage_id.as_str())
                .cloned()
                .ok_or(CookieAuthorityError::StagedImportNotFound)?;
            self.service.discard_stage(&staged.scope, &stage)?;
            self.staged.remove(stage_id);
            return Err(CookieAuthorityError::StagedImportExpired);
        }
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
        let mut authority_state = Self::persisted_state_with(
            &generations,
            &idempotency,
            scope.clone(),
            generation,
            idempotency_key.clone(),
            fingerprint,
            preview,
            hosted_job,
        );
        authority_state.stages = self.service.authority_state().stages.clone();
        authority_state.pending_materializations = self
            .service
            .authority_state()
            .pending_materializations
            .clone();
        authority_state
            .pending_materializations
            .push(PersistedCookieMaterialization {
                workspace_id: scope.workspace_id.as_str().to_owned(),
                profile_id: scope.profile_id.as_str().to_owned(),
                command: "cookie.import".to_owned(),
                idempotency_key: idempotency_key.as_str().to_owned(),
                expected_generation,
                generation,
                fencing_token: Some(lease.fencing_token),
                staged_import_id: Some(stage_id.as_str().to_owned()),
                source_workspace_id: None,
                source_profile_id: None,
                hosted_job: hosted_job.map(persisted_hosted_job_binding),
            });
        let Some(staged_entry) = authority_state
            .stages
            .iter_mut()
            .find(|stage| stage.stage_id == stage_id.as_str())
        else {
            return Err(CookieAuthorityError::StagedImportNotFound);
        };
        staged_entry.cleanup_pending = true;
        let report = self.service.import_with_authority_state(
            scope.clone(),
            inputs,
            principal,
            grants,
            &authority_state,
            stage_id.as_str(),
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
                hosted_job: hosted_job.cloned(),
            },
        );
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
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
        self.copy_with_hosted_binding(
            source,
            destination,
            expected_destination_generation,
            destination_lease,
            now_unix_ms,
            principal,
            grants,
            idempotency_key,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn copy_hosted(
        &mut self,
        source: &CookieScope,
        destination: CookieScope,
        expected_destination_generation: u64,
        destination_lease: &Lease,
        now_unix_ms: u64,
        principal: &Principal,
        grants: &[Grant],
        idempotency_key: IdempotencyKey,
        hosted_job: &HostedCookieJobBinding,
    ) -> Result<CookieMutationResult, CookieAuthorityError> {
        validate_hosted_job_binding(hosted_job)?;
        self.copy_with_hosted_binding(
            source,
            destination,
            expected_destination_generation,
            destination_lease,
            now_unix_ms,
            principal,
            grants,
            idempotency_key,
            Some(hosted_job),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn copy_with_hosted_binding(
        &mut self,
        source: &CookieScope,
        destination: CookieScope,
        expected_destination_generation: u64,
        destination_lease: &Lease,
        now_unix_ms: u64,
        principal: &Principal,
        grants: &[Grant],
        idempotency_key: IdempotencyKey,
        hosted_job: Option<&HostedCookieJobBinding>,
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
            if replay.hosted_job.as_ref() != hosted_job {
                return Err(CookieAuthorityError::IdempotencyConflict);
            }
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
        if self.has_pending_materialization(source) {
            return Err(CookieAuthorityError::MaterializationPending);
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
        let mut authority_state = Self::persisted_state_with(
            &generations,
            &idempotency,
            destination.clone(),
            generation,
            idempotency_key.clone(),
            fingerprint,
            preview,
            hosted_job,
        );
        authority_state.stages = self.service.authority_state().stages.clone();
        authority_state.pending_materializations = self
            .service
            .authority_state()
            .pending_materializations
            .clone();
        authority_state
            .pending_materializations
            .push(PersistedCookieMaterialization {
                workspace_id: destination.workspace_id.as_str().to_owned(),
                profile_id: destination.profile_id.as_str().to_owned(),
                command: "cookie.copy".to_owned(),
                idempotency_key: idempotency_key.as_str().to_owned(),
                expected_generation: expected_destination_generation,
                generation,
                fencing_token: Some(destination_lease.fencing_token),
                staged_import_id: None,
                source_workspace_id: Some(source.workspace_id.as_str().to_owned()),
                source_profile_id: Some(source.profile_id.as_str().to_owned()),
                hosted_job: hosted_job.map(persisted_hosted_job_binding),
            });
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
                hosted_job: hosted_job.cloned(),
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
        if self.has_pending_materialization(scope) {
            return Err(CookieAuthorityError::MaterializationPending);
        }
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
}

fn validate_persisted_stage_ids(
    stages: &[PersistedCookieStage],
) -> Result<(), CookieAuthorityError> {
    let mut stage_ids = BTreeSet::new();
    for stage in stages {
        let id = StagedImportId::from_opaque(stage.stage_id.clone())?;
        if !stage_ids.insert(id) {
            return Err(CookieAuthorityError::Storage);
        }
    }
    Ok(())
}

fn validate_pending_materialization_binding(
    pending: &PersistedCookieMaterialization,
    scope: &CookieScope,
    receipt: &Replay,
    stages: &[PersistedCookieStage],
) -> Result<(), CookieAuthorityError> {
    let hosted_identity_matches = match (&pending.hosted_job, &receipt.hosted_job) {
        (None, None) => true,
        (Some(pending), Some(receipt)) => {
            pending.job_id == receipt.job_id
                && pending.operation_id == receipt.operation_id
                && pending.device_id == receipt.device_id
        }
        _ => false,
    };
    if !hosted_identity_matches {
        return Err(CookieAuthorityError::Storage);
    }
    if pending.hosted_job.as_ref().is_some_and(|binding| {
        [&binding.job_id, &binding.operation_id, &binding.device_id]
            .into_iter()
            .any(|value| {
                value.is_empty() || value.len() > 256 || value.chars().any(char::is_control)
            })
    }) {
        return Err(CookieAuthorityError::Storage);
    }
    let (Some(fencing_token), staged_import_id) =
        (pending.fencing_token, &pending.staged_import_id)
    else {
        if pending.fencing_token.is_none() && pending.staged_import_id.is_none() {
            return Ok(());
        }
        return Err(CookieAuthorityError::Storage);
    };
    if fencing_token == 0 {
        return Err(CookieAuthorityError::Storage);
    }

    let expected = match pending.command.as_str() {
        "cookie.import" => {
            let stage_id = StagedImportId::from_opaque(
                staged_import_id
                    .as_deref()
                    .ok_or(CookieAuthorityError::Storage)?,
            )?;
            if pending.source_workspace_id.is_some() || pending.source_profile_id.is_some() {
                return Err(CookieAuthorityError::Storage);
            }
            let stage_matches = stages.iter().any(|stage| {
                StagedImportId::from_opaque(stage.stage_id.clone()).is_ok_and(|candidate| {
                    candidate == stage_id
                        && stage.cleanup_pending
                        && stage.workspace_id == scope.workspace_id.as_str()
                        && stage.profile_id == scope.profile_id.as_str()
                })
            });
            if !stage_matches {
                return Err(CookieAuthorityError::Storage);
            }
            fingerprint(
                "import",
                &[
                    stage_id.as_str(),
                    scope.workspace_id.as_str(),
                    scope.profile_id.as_str(),
                ],
                pending.expected_generation,
                fencing_token,
            )
        }
        "cookie.copy" => {
            if staged_import_id.is_some() {
                return Err(CookieAuthorityError::Storage);
            }
            let source_workspace = pending
                .source_workspace_id
                .as_deref()
                .and_then(|value| buglogin_domain::ids::WorkspaceId::from_str(value).ok())
                .ok_or(CookieAuthorityError::Storage)?;
            let source_profile = pending
                .source_profile_id
                .as_deref()
                .and_then(|value| buglogin_domain::ids::ProfileId::from_str(value).ok())
                .ok_or(CookieAuthorityError::Storage)?;
            fingerprint(
                "copy",
                &[
                    source_workspace.as_str(),
                    source_profile.as_str(),
                    scope.workspace_id.as_str(),
                    scope.profile_id.as_str(),
                ],
                pending.expected_generation,
                fencing_token,
            )
        }
        _ => return Err(CookieAuthorityError::Storage),
    };
    if receipt.fingerprint != expected {
        return Err(CookieAuthorityError::Storage);
    }
    Ok(())
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

fn validate_hosted_job_binding(
    binding: &HostedCookieJobBinding,
) -> Result<(), CookieAuthorityError> {
    for value in [&binding.job_id, &binding.operation_id, &binding.device_id] {
        if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
            return Err(CookieAuthorityError::InvalidHostedJobBinding);
        }
    }
    Ok(())
}

fn persisted_hosted_job_binding(
    binding: &HostedCookieJobBinding,
) -> PersistedHostedCookieJobBinding {
    PersistedHostedCookieJobBinding {
        job_id: binding.job_id.clone(),
        operation_id: binding.operation_id.clone(),
        device_id: binding.device_id.clone(),
    }
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
    if !value.len().is_multiple_of(2) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect()
}

fn unix_now_ms() -> Result<u64, CookieAuthorityError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| CookieAuthorityError::ClockUnavailable)
        .and_then(|duration| {
            u64::try_from(duration.as_millis()).map_err(|_| CookieAuthorityError::ClockUnavailable)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn persisted_stage(stage_id: &str) -> PersistedCookieStage {
        PersistedCookieStage {
            stage_id: stage_id.to_owned(),
            workspace_id: "ws_1".to_owned(),
            profile_id: "prf_1".to_owned(),
            expires_at_unix_ms: 1,
            cleanup_pending: false,
            cookies: Vec::new(),
        }
    }

    #[test]
    fn duplicate_persisted_stage_ids_fail_closed() {
        let duplicate = "ab".repeat(STAGED_ID_BYTES);
        assert_eq!(
            validate_persisted_stage_ids(&[
                persisted_stage(&duplicate),
                persisted_stage(&duplicate),
            ]),
            Err(CookieAuthorityError::Storage)
        );
    }

    #[test]
    fn pending_copy_receipt_is_bound_to_its_source_and_fence() {
        let scope = CookieScope {
            workspace_id: buglogin_domain::ids::WorkspaceId::from_str("ws_1").unwrap(),
            profile_id: buglogin_domain::ids::ProfileId::from_str("prf_1").unwrap(),
        };
        let mut pending = PersistedCookieMaterialization {
            workspace_id: scope.workspace_id.as_str().to_owned(),
            profile_id: scope.profile_id.as_str().to_owned(),
            command: "cookie.copy".to_owned(),
            idempotency_key: "copy-recovery-1".to_owned(),
            expected_generation: 4,
            generation: 5,
            fencing_token: Some(9),
            staged_import_id: None,
            source_workspace_id: Some("ws_source".to_owned()),
            source_profile_id: Some("prf_source".to_owned()),
            hosted_job: None,
        };
        let source_workspace = buglogin_domain::ids::WorkspaceId::from_str("ws_source").unwrap();
        let source_profile = buglogin_domain::ids::ProfileId::from_str("prf_source").unwrap();
        let receipt = Replay {
            scope: scope.clone(),
            fingerprint: fingerprint(
                "copy",
                &[
                    source_workspace.as_str(),
                    source_profile.as_str(),
                    scope.workspace_id.as_str(),
                    scope.profile_id.as_str(),
                ],
                4,
                9,
            ),
            result: CookieMutationResult {
                report: CookieReport::default(),
                generation: 5,
                replayed: false,
            },
            hosted_job: None,
        };

        assert!(validate_pending_materialization_binding(&pending, &scope, &receipt, &[]).is_ok());
        pending.source_profile_id = Some("prf_other".to_owned());
        assert_eq!(
            validate_pending_materialization_binding(&pending, &scope, &receipt, &[]),
            Err(CookieAuthorityError::Storage)
        );
    }

    #[test]
    fn hosted_cookie_job_binding_rejects_empty_or_controlled_identity() {
        let valid = HostedCookieJobBinding {
            job_id: "job_123".to_owned(),
            operation_id: "operation_123".to_owned(),
            device_id: "device_123".to_owned(),
        };
        assert_eq!(validate_hosted_job_binding(&valid), Ok(()));

        let invalid = HostedCookieJobBinding {
            device_id: "device\nforged".to_owned(),
            ..valid
        };
        assert_eq!(
            validate_hosted_job_binding(&invalid),
            Err(CookieAuthorityError::InvalidHostedJobBinding)
        );
    }

    #[test]
    fn pending_hosted_materialization_must_match_its_durable_receipt() {
        let mut pending = PersistedCookieMaterialization {
            workspace_id: "ws_1".to_owned(),
            profile_id: "prf_1".to_owned(),
            command: "cookie.copy".to_owned(),
            idempotency_key: "copy-hosted-recovery".to_owned(),
            expected_generation: 4,
            generation: 5,
            fencing_token: Some(9),
            staged_import_id: None,
            source_workspace_id: Some("ws_source".to_owned()),
            source_profile_id: Some("prf_source".to_owned()),
            hosted_job: Some(PersistedHostedCookieJobBinding {
                job_id: "job_1".to_owned(),
                operation_id: "operation_1".to_owned(),
                device_id: "device_1".to_owned(),
            }),
        };
        let scope = CookieScope {
            workspace_id: buglogin_domain::ids::WorkspaceId::from_str("ws_1").unwrap(),
            profile_id: buglogin_domain::ids::ProfileId::from_str("prf_1").unwrap(),
        };
        let hosted_job = HostedCookieJobBinding {
            job_id: "job_1".to_owned(),
            operation_id: "operation_1".to_owned(),
            device_id: "device_1".to_owned(),
        };
        let receipt = Replay {
            scope: scope.clone(),
            fingerprint: fingerprint("copy", &["ws_source", "prf_source", "ws_1", "prf_1"], 4, 9),
            result: CookieMutationResult {
                report: CookieReport::default(),
                generation: 5,
                replayed: false,
            },
            hosted_job: Some(hosted_job),
        };
        assert!(validate_pending_materialization_binding(&pending, &scope, &receipt, &[]).is_ok());
        pending.hosted_job.as_mut().unwrap().job_id = "job_other".to_owned();
        assert_eq!(
            validate_pending_materialization_binding(&pending, &scope, &receipt, &[]),
            Err(CookieAuthorityError::Storage)
        );
    }
}
