#![forbid(unsafe_code)]

use buglogin_domain::{
    ids::{ProfileId, WorkspaceId},
    permissions::{Action, Capability, Grant, Principal, ResourceScope, is_allowed},
};
use buglogin_storage::secrets::{OsVault, SecretError, SecretKey, SecretValue, SecretVersion};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fmt,
    fs::{self, File, OpenOptions},
    io::{BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

pub const COOKIE_CAPABILITY: &str = "CAP-COOK-001";

pub mod authority;
pub mod wayfern;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CookieScope {
    pub workspace_id: WorkspaceId,
    pub profile_id: ProfileId,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CookieIdentity {
    pub name: String,
    pub domain: String,
    pub path: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CookieSameSite {
    #[default]
    Unspecified,
    None,
    Lax,
    Strict,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CookieInput {
    pub name: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub expires_at: Option<i64>,
    pub same_site: CookieSameSite,
    pub value: SecretValue,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CookieSummary {
    pub name: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub expires_at: Option<i64>,
    pub same_site: CookieSameSite,
    pub value_present: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CookieReport {
    pub inserted: usize,
    pub replaced: usize,
    pub deduplicated: usize,
    pub copied: usize,
    pub cleanup_pending: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CookieError {
    Forbidden,
    InvalidName,
    InvalidDomain,
    InvalidPath,
    InvalidValue,
    Duplicate,
    NotFound,
    Vault(SecretError),
    Storage,
    Locked,
    Rollback,
}

impl fmt::Display for CookieError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Forbidden => "cookie_access_denied",
            Self::InvalidName => "invalid_cookie_name",
            Self::InvalidDomain => "invalid_cookie_domain",
            Self::InvalidPath => "invalid_cookie_path",
            Self::InvalidValue => "invalid_cookie_value",
            Self::Duplicate => "duplicate_cookie",
            Self::NotFound => "cookie_not_found",
            Self::Vault(error) => error.code(),
            Self::Storage => "cookie_storage_error",
            Self::Locked => "cookie_storage_locked",
            Self::Rollback => "cookie_rollback_required",
        })
    }
}

impl std::error::Error for CookieError {}

impl From<SecretError> for CookieError {
    fn from(error: SecretError) -> Self {
        Self::Vault(error)
    }
}

#[derive(Clone, Debug)]
struct CookieRecord {
    metadata: CookieMetadata,
    version: SecretVersion,
    slot: u8,
}

#[derive(Clone, Debug)]
struct CookieMetadata {
    name: String,
    domain: String,
    path: String,
    secure: bool,
    http_only: bool,
    expires_at: Option<i64>,
    same_site: CookieSameSite,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedCookieLedger {
    schema_version: u32,
    revision: u64,
    records: Vec<PersistedCookieRecord>,
    #[serde(default)]
    authority: PersistedCookieAuthorityState,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PersistedCookieAuthorityState {
    pub(crate) schema_version: u32,
    pub(crate) generations: Vec<PersistedCookieGeneration>,
    pub(crate) receipts: Vec<PersistedCookieReceipt>,
    #[serde(default)]
    pub(crate) stages: Vec<PersistedCookieStage>,
    #[serde(default)]
    pub(crate) pending_materializations: Vec<PersistedCookieMaterialization>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PersistedCookieMaterialization {
    pub(crate) workspace_id: String,
    pub(crate) profile_id: String,
    pub(crate) command: String,
    pub(crate) idempotency_key: String,
    pub(crate) expected_generation: u64,
    pub(crate) generation: u64,
    #[serde(default)]
    pub(crate) fencing_token: Option<u64>,
    #[serde(default)]
    pub(crate) staged_import_id: Option<String>,
    pub(crate) source_workspace_id: Option<String>,
    pub(crate) source_profile_id: Option<String>,
    #[serde(default)]
    pub(crate) hosted_job: Option<PersistedHostedCookieJobBinding>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PersistedHostedCookieJobBinding {
    pub(crate) job_id: String,
    pub(crate) operation_id: String,
    pub(crate) device_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PersistedCookieStage {
    pub(crate) stage_id: String,
    pub(crate) workspace_id: String,
    pub(crate) profile_id: String,
    pub(crate) expires_at_unix_ms: u64,
    #[serde(default)]
    pub(crate) cleanup_pending: bool,
    pub(crate) cookies: Vec<PersistedCookieStageItem>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PersistedCookieStageItem {
    pub(crate) name: String,
    pub(crate) domain: String,
    pub(crate) path: String,
    pub(crate) secure: bool,
    pub(crate) http_only: bool,
    pub(crate) expires_at: Option<i64>,
    pub(crate) same_site: i32,
    pub(crate) version: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PersistedCookieGeneration {
    pub(crate) workspace_id: String,
    pub(crate) profile_id: String,
    pub(crate) generation: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PersistedCookieReceipt {
    pub(crate) idempotency_key: String,
    pub(crate) workspace_id: String,
    pub(crate) profile_id: String,
    pub(crate) fingerprint: String,
    pub(crate) report: CookieReport,
    pub(crate) generation: u64,
    #[serde(default)]
    pub(crate) hosted_job: Option<PersistedHostedCookieJobBinding>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedCookieRecord {
    workspace_id: String,
    profile_id: String,
    name: String,
    domain: String,
    path: String,
    secure: bool,
    http_only: bool,
    expires_at: Option<i64>,
    same_site: i32,
    version: u64,
    slot: u8,
}

const COOKIE_SCHEMA_VERSION: u32 = 1;
const COOKIE_AUTHORITY_SCHEMA_VERSION: u32 = 1;

pub struct CookieService<V: OsVault> {
    vault: V,
    records: BTreeMap<CookieScope, BTreeMap<CookieIdentity, CookieRecord>>,
    metadata_root: Option<PathBuf>,
    authority_state: PersistedCookieAuthorityState,
    _lock: Option<File>,
}

impl<V: OsVault> CookieService<V> {
    pub fn new(vault: V) -> Self {
        Self {
            vault,
            records: BTreeMap::new(),
            metadata_root: None,
            authority_state: default_authority_state(),
            _lock: None,
        }
    }

    pub fn open(vault: V, metadata_root: impl Into<PathBuf>) -> Result<Self, CookieError> {
        let metadata_root = metadata_root.into();
        if metadata_root.is_file() {
            return Ok(Self {
                vault,
                records: BTreeMap::new(),
                metadata_root: Some(metadata_root),
                authority_state: default_authority_state(),
                _lock: None,
            });
        }
        fs::create_dir_all(&metadata_root).map_err(|_| CookieError::Storage)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(metadata_root.join(".cookie-authority.lock"))
            .map_err(|_| CookieError::Storage)?;
        lock.try_lock_exclusive().map_err(|_| CookieError::Locked)?;
        let (records, authority_state) = load_ledger(&metadata_root)?;
        Ok(Self {
            vault,
            records,
            metadata_root: Some(metadata_root),
            authority_state,
            _lock: Some(lock),
        })
    }

    pub(crate) fn authority_state(&self) -> &PersistedCookieAuthorityState {
        &self.authority_state
    }

    pub(crate) fn commit_authority_state(
        &mut self,
        authority_state: &PersistedCookieAuthorityState,
    ) -> Result<(), CookieError> {
        persist_records(
            self.metadata_root.as_deref(),
            &self.records,
            authority_state,
        )?;
        self.authority_state = authority_state.clone();
        Ok(())
    }

    pub(crate) fn persist_stage(
        &mut self,
        scope: &CookieScope,
        stage_id: &str,
        expires_at_unix_ms: u64,
        inputs: Vec<CookieInput>,
    ) -> Result<PersistedCookieStage, CookieError> {
        if self
            .authority_state
            .stages
            .iter()
            .any(|stage| stage.stage_id == stage_id)
        {
            return Err(CookieError::Duplicate);
        }
        let mut secret_values = Vec::with_capacity(inputs.len());
        let mut stage = PersistedCookieStage {
            stage_id: stage_id.to_owned(),
            workspace_id: scope.workspace_id.as_str().to_owned(),
            profile_id: scope.profile_id.as_str().to_owned(),
            expires_at_unix_ms,
            cleanup_pending: false,
            cookies: Vec::with_capacity(inputs.len()),
        };
        for input in inputs {
            stage.cookies.push(PersistedCookieStageItem {
                name: input.name.clone(),
                domain: input.domain.clone(),
                path: input.path.clone(),
                secure: input.secure,
                http_only: input.http_only,
                expires_at: input.expires_at,
                same_site: same_site_to_db(input.same_site),
                version: 0,
            });
            secret_values.push(input.value);
        }
        let mut authority_state = self.authority_state.clone();
        authority_state.stages.push(stage.clone());
        persist_records(
            self.metadata_root.as_deref(),
            &self.records,
            &authority_state,
        )?;
        self.authority_state = authority_state;

        for (index, value) in secret_values.into_iter().enumerate() {
            let item = &stage.cookies[index];
            let cookie_identity = CookieIdentity {
                name: item.name.clone(),
                domain: item.domain.clone(),
                path: item.path.clone(),
            };
            let key = key_for_stage(stage_id, scope, &cookie_identity);
            let version = match self.vault.create(&key, value) {
                Ok(version) => version,
                Err(error) => {
                    self.discard_stage(scope, &stage)?;
                    return Err(error.into());
                }
            };
            stage.cookies[index].version = version.generation();
        }
        let mut authority_state = self.authority_state.clone();
        if let Some(entry) = authority_state
            .stages
            .iter_mut()
            .find(|entry| entry.stage_id == stage_id)
        {
            *entry = stage.clone();
        } else {
            return Err(CookieError::Storage);
        }
        if let Err(error) = persist_records(
            self.metadata_root.as_deref(),
            &self.records,
            &authority_state,
        ) {
            self.discard_stage(scope, &stage)?;
            return Err(error);
        }
        self.authority_state = authority_state;
        Ok(stage)
    }

    pub(crate) fn restore_stage_inputs(
        &self,
        scope: &CookieScope,
        stage: &PersistedCookieStage,
    ) -> Result<Vec<CookieInput>, CookieError> {
        stage
            .cookies
            .iter()
            .map(|item| {
                let cookie_identity = CookieIdentity {
                    name: item.name.clone(),
                    domain: item.domain.clone(),
                    path: item.path.clone(),
                };
                let key = key_for_stage(&stage.stage_id, scope, &cookie_identity);
                let (value, version) = self.vault.read(&key)?;
                if version.generation() != item.version {
                    return Err(CookieError::Rollback);
                }
                Ok(CookieInput {
                    name: item.name.clone(),
                    domain: item.domain.clone(),
                    path: item.path.clone(),
                    secure: item.secure,
                    http_only: item.http_only,
                    expires_at: item.expires_at,
                    same_site: same_site_from_db(item.same_site)?,
                    value,
                })
            })
            .collect()
    }

    pub(crate) fn materialization_inputs(
        &self,
        scope: &CookieScope,
        principal: &Principal,
        grants: &[Grant],
    ) -> Result<Vec<CookieInput>, CookieError> {
        authorize(scope, Action::Read, principal, grants)?;
        self.records
            .get(scope)
            .into_iter()
            .flat_map(BTreeMap::iter)
            .map(|(cookie_identity, record)| {
                let key = key_for_slot(scope, cookie_identity, record.slot);
                let (value, version) = self.vault.read(&key)?;
                if version != record.version {
                    return Err(CookieError::Rollback);
                }
                Ok(CookieInput {
                    name: record.metadata.name.clone(),
                    domain: record.metadata.domain.clone(),
                    path: record.metadata.path.clone(),
                    secure: record.metadata.secure,
                    http_only: record.metadata.http_only,
                    expires_at: record.metadata.expires_at,
                    same_site: record.metadata.same_site,
                    value,
                })
            })
            .collect()
    }

    pub(crate) fn discard_stage(
        &mut self,
        scope: &CookieScope,
        stage: &PersistedCookieStage,
    ) -> Result<(), CookieError> {
        for item in &stage.cookies {
            let identity = CookieIdentity {
                name: item.name.clone(),
                domain: item.domain.clone(),
                path: item.path.clone(),
            };
            let key = key_for_stage(&stage.stage_id, scope, &identity);
            match self.vault.read(&key) {
                Ok((_, version)) => self.vault.remove(&key, version)?,
                Err(SecretError::NotFound) => {}
                Err(error) => return Err(error.into()),
            }
        }
        let mut authority_state = self.authority_state.clone();
        authority_state
            .stages
            .retain(|item| item.stage_id != stage.stage_id);
        persist_records(
            self.metadata_root.as_deref(),
            &self.records,
            &authority_state,
        )?;
        self.authority_state = authority_state;
        Ok(())
    }

    pub(crate) fn cleanup_pending_stage(
        &mut self,
        scope: &CookieScope,
        stage: &PersistedCookieStage,
    ) -> Result<(), CookieError> {
        for item in &stage.cookies {
            let identity = CookieIdentity {
                name: item.name.clone(),
                domain: item.domain.clone(),
                path: item.path.clone(),
            };
            let key = key_for_stage(&stage.stage_id, scope, &identity);
            match self.vault.read(&key) {
                Ok((_, version)) => self.vault.remove(&key, version)?,
                Err(SecretError::NotFound) => {}
                Err(error) => return Err(error.into()),
            }
        }
        let mut authority_state = self.authority_state.clone();
        let Some(entry) = authority_state
            .stages
            .iter_mut()
            .find(|entry| entry.stage_id == stage.stage_id)
        else {
            return Err(CookieError::Storage);
        };
        entry.cookies.clear();
        entry.cleanup_pending = true;
        persist_records(
            self.metadata_root.as_deref(),
            &self.records,
            &authority_state,
        )?;
        self.authority_state = authority_state;
        Ok(())
    }

    pub(crate) fn preview_import(
        &self,
        scope: &CookieScope,
        inputs: &[CookieInput],
        principal: &Principal,
        grants: &[Grant],
    ) -> Result<CookieReport, CookieError> {
        authorize(scope, Action::Create, principal, grants)?;
        let mut identities = BTreeMap::new();
        for input in inputs {
            let input = normalize(input.clone())?;
            let identity = identity(&input);
            if identities.insert(identity, input).is_some() {
                return Err(CookieError::Duplicate);
            }
        }
        Ok(self.preview_batch(scope, &identities))
    }

    pub(crate) fn preview_copy(
        &self,
        source: &CookieScope,
        destination: &CookieScope,
        principal: &Principal,
        grants: &[Grant],
    ) -> Result<CookieReport, CookieError> {
        authorize(source, Action::Read, principal, grants)?;
        authorize(destination, Action::Create, principal, grants)?;
        let source_records = self
            .records
            .get(source)
            .cloned()
            .ok_or(CookieError::NotFound)?;
        let mut inputs = BTreeMap::new();
        for (cookie_identity, record) in source_records {
            let (value, version) =
                self.vault
                    .read(&key_for_slot(source, &cookie_identity, record.slot))?;
            if version != record.version {
                return Err(CookieError::Rollback);
            }
            let input = CookieInput {
                name: record.metadata.name,
                domain: record.metadata.domain,
                path: record.metadata.path,
                secure: record.metadata.secure,
                http_only: record.metadata.http_only,
                expires_at: record.metadata.expires_at,
                same_site: record.metadata.same_site,
                value,
            };
            inputs.insert(identity(&input), input);
        }
        let mut report = self.preview_batch(destination, &inputs);
        report.copied = report.inserted + report.replaced + report.deduplicated;
        Ok(report)
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn into_vault(self) -> V {
        self.vault
    }

    fn preview_batch(
        &self,
        scope: &CookieScope,
        inputs: &BTreeMap<CookieIdentity, CookieInput>,
    ) -> CookieReport {
        let mut report = CookieReport::default();
        for identity in inputs.keys() {
            if self
                .records
                .get(scope)
                .and_then(|records| records.get(identity))
                .is_some()
            {
                report.replaced += 1;
            } else {
                report.inserted += 1;
            }
        }
        report
    }

    pub(crate) fn import_with_authority_state(
        &mut self,
        scope: CookieScope,
        inputs: impl IntoIterator<Item = CookieInput>,
        principal: &Principal,
        grants: &[Grant],
        authority_state: &PersistedCookieAuthorityState,
        stage_id: &str,
    ) -> Result<CookieReport, CookieError> {
        authorize(&scope, Action::Create, principal, grants)?;
        let inputs = inputs
            .into_iter()
            .map(normalize)
            .collect::<Result<Vec<_>, _>>()?;
        let mut identities = BTreeMap::new();
        for input in inputs {
            let identity = identity(&input);
            if identities.insert(identity, input).is_some() {
                return Err(CookieError::Duplicate);
            }
        }
        // The commit payload marks this stage for cleanup while the pending
        // materialization receipt keeps its identity for restart recovery.
        // Read that payload instead of the pre-commit in-memory copy.
        let stage = authority_state
            .stages
            .iter()
            .find(|stage| stage.stage_id == stage_id)
            .cloned()
            .ok_or(CookieError::NotFound)?;
        let mut report = self.apply_batch_with_authority(&scope, identities, authority_state)?;
        if stage.cleanup_pending {
            if self.cleanup_pending_stage(&scope, &stage).is_err() {
                report.cleanup_pending += 1;
            }
        } else if self.discard_stage(&scope, &stage).is_err() {
            report.cleanup_pending += 1;
        }
        Ok(report)
    }

    pub(crate) fn copy_with_authority_state(
        &mut self,
        source: &CookieScope,
        destination: CookieScope,
        principal: &Principal,
        grants: &[Grant],
        authority_state: &PersistedCookieAuthorityState,
    ) -> Result<CookieReport, CookieError> {
        authorize(source, Action::Read, principal, grants)?;
        authorize(&destination, Action::Create, principal, grants)?;
        let source_records = self
            .records
            .get(source)
            .cloned()
            .ok_or(CookieError::NotFound)?;
        let mut inputs = BTreeMap::new();
        for (cookie_identity, record) in source_records {
            let (value, version) =
                self.vault
                    .read(&key_for_slot(source, &cookie_identity, record.slot))?;
            if version != record.version {
                return Err(CookieError::Rollback);
            }
            let input = CookieInput {
                name: record.metadata.name,
                domain: record.metadata.domain,
                path: record.metadata.path,
                secure: record.metadata.secure,
                http_only: record.metadata.http_only,
                expires_at: record.metadata.expires_at,
                same_site: record.metadata.same_site,
                value,
            };
            inputs.insert(identity(&input), input);
        }
        let mut report = self.apply_batch_with_authority(&destination, inputs, authority_state)?;
        report.copied = report.inserted + report.replaced + report.deduplicated;
        Ok(report)
    }

    pub fn import(
        &mut self,
        scope: CookieScope,
        inputs: impl IntoIterator<Item = CookieInput>,
        principal: &Principal,
        grants: &[Grant],
    ) -> Result<CookieReport, CookieError> {
        authorize(&scope, Action::Create, principal, grants)?;
        let inputs = inputs
            .into_iter()
            .map(normalize)
            .collect::<Result<Vec<_>, _>>()?;
        let mut identities = BTreeMap::new();
        for input in inputs {
            let identity = identity(&input);
            if identities.insert(identity, input).is_some() {
                return Err(CookieError::Duplicate);
            }
        }
        let authority = self.authority_state.clone();
        self.apply_batch_with_authority(&scope, identities, &authority)
    }

    pub fn copy(
        &mut self,
        source: &CookieScope,
        destination: CookieScope,
        principal: &Principal,
        grants: &[Grant],
    ) -> Result<CookieReport, CookieError> {
        authorize(source, Action::Read, principal, grants)?;
        authorize(&destination, Action::Create, principal, grants)?;
        let source_records = self
            .records
            .get(source)
            .cloned()
            .ok_or(CookieError::NotFound)?;
        let mut inputs = BTreeMap::new();
        for (cookie_identity, record) in source_records {
            let (value, version) =
                self.vault
                    .read(&key_for_slot(source, &cookie_identity, record.slot))?;
            if version != record.version {
                return Err(CookieError::Rollback);
            }
            let input = CookieInput {
                name: record.metadata.name,
                domain: record.metadata.domain,
                path: record.metadata.path,
                secure: record.metadata.secure,
                http_only: record.metadata.http_only,
                expires_at: record.metadata.expires_at,
                same_site: record.metadata.same_site,
                value,
            };
            inputs.insert(identity(&input), input);
        }
        let authority = self.authority_state.clone();
        let mut report = self.apply_batch_with_authority(&destination, inputs, &authority)?;
        report.copied = report.inserted + report.replaced + report.deduplicated;
        Ok(report)
    }

    pub fn clone_to(
        &mut self,
        source: &CookieScope,
        destination: CookieScope,
        principal: &Principal,
        grants: &[Grant],
    ) -> Result<CookieReport, CookieError> {
        self.copy(source, destination, principal, grants)
    }

    pub fn list(
        &self,
        scope: &CookieScope,
        principal: &Principal,
        grants: &[Grant],
    ) -> Result<Vec<CookieSummary>, CookieError> {
        authorize(scope, Action::Read, principal, grants)?;
        Ok(self
            .records
            .get(scope)
            .into_iter()
            .flat_map(BTreeMap::values)
            .map(|record| CookieSummary {
                name: record.metadata.name.clone(),
                domain: record.metadata.domain.clone(),
                path: record.metadata.path.clone(),
                secure: record.metadata.secure,
                http_only: record.metadata.http_only,
                expires_at: record.metadata.expires_at,
                same_site: record.metadata.same_site,
                value_present: self
                    .vault
                    .read(&key_for_slot(
                        scope,
                        &identity_from_metadata(&record.metadata),
                        record.slot,
                    ))
                    .map(|(_, version)| version == record.version)
                    .unwrap_or(false),
            })
            .collect())
    }

    fn apply_batch_with_authority(
        &mut self,
        scope: &CookieScope,
        inputs: BTreeMap<CookieIdentity, CookieInput>,
        authority_state: &PersistedCookieAuthorityState,
    ) -> Result<CookieReport, CookieError> {
        let mut working = self.records.clone();
        let mut staged = Vec::new();
        let mut old_keys = Vec::new();
        let mut report = CookieReport::default();
        for (identity, input) in inputs {
            let previous = working
                .get(scope)
                .and_then(|records| records.get(&identity).cloned());
            let slot = previous.as_ref().map_or(0, |record| 1 - record.slot);
            let key = key_for_slot(scope, &identity, slot);
            match self.vault.read(&key) {
                Ok((_, version)) => self.vault.remove(&key, version)?,
                Err(SecretError::NotFound) => {}
                Err(error) => return Err(error.into()),
            }
            let version = match self.vault.create(&key, input.value) {
                Ok(version) => version,
                Err(error) => {
                    cleanup_staged(&mut self.vault, &staged)?;
                    return Err(error.into());
                }
            };
            staged.push((key, version));
            if let Some(previous) = previous {
                old_keys.push((
                    key_for_slot(scope, &identity, previous.slot),
                    previous.version,
                ));
                report.replaced += 1;
            } else {
                report.inserted += 1;
            }
            working.entry(scope.clone()).or_default().insert(
                identity,
                CookieRecord {
                    metadata: CookieMetadata {
                        name: input.name,
                        domain: input.domain,
                        path: input.path,
                        secure: input.secure,
                        http_only: input.http_only,
                        expires_at: input.expires_at,
                        same_site: input.same_site,
                    },
                    version,
                    slot,
                },
            );
        }
        if let Err(error) =
            persist_records(self.metadata_root.as_deref(), &working, authority_state)
        {
            cleanup_staged(&mut self.vault, &staged)?;
            return Err(error);
        }
        self.records = working;
        self.authority_state = authority_state.clone();
        for (key, version) in old_keys {
            if self.vault.remove(&key, version).is_err() {
                report.cleanup_pending += 1;
            }
        }
        Ok(report)
    }
}

fn identity_from_metadata(metadata: &CookieMetadata) -> CookieIdentity {
    CookieIdentity {
        name: metadata.name.clone(),
        domain: metadata.domain.clone(),
        path: metadata.path.clone(),
    }
}

fn cleanup_staged<V: OsVault>(
    vault: &mut V,
    staged: &[(SecretKey, SecretVersion)],
) -> Result<(), CookieError> {
    for (key, version) in staged {
        vault
            .remove(key, *version)
            .map_err(|_| CookieError::Rollback)?;
    }
    Ok(())
}

type LoadedCookieLedger = (
    BTreeMap<CookieScope, BTreeMap<CookieIdentity, CookieRecord>>,
    PersistedCookieAuthorityState,
);

fn load_ledger(root: &Path) -> Result<LoadedCookieLedger, CookieError> {
    let Some(path) = latest_ledger_path(root)? else {
        return Ok((BTreeMap::new(), default_authority_state()));
    };
    let file = File::open(path).map_err(|_| CookieError::Storage)?;
    let ledger: PersistedCookieLedger =
        serde_json::from_reader(BufReader::new(file)).map_err(|_| CookieError::Storage)?;
    if ledger.schema_version != COOKIE_SCHEMA_VERSION {
        return Err(CookieError::Storage);
    }
    let mut records: BTreeMap<CookieScope, BTreeMap<CookieIdentity, CookieRecord>> =
        BTreeMap::new();
    for item in ledger.records {
        let scope = CookieScope {
            workspace_id: WorkspaceId::from_str(&item.workspace_id)
                .map_err(|_| CookieError::Storage)?,
            profile_id: ProfileId::from_str(&item.profile_id).map_err(|_| CookieError::Storage)?,
        };
        let identity = CookieIdentity {
            name: item.name.clone(),
            domain: item.domain.clone(),
            path: item.path.clone(),
        };
        records.entry(scope).or_default().insert(
            identity,
            CookieRecord {
                metadata: CookieMetadata {
                    name: item.name,
                    domain: item.domain,
                    path: item.path,
                    secure: item.secure,
                    http_only: item.http_only,
                    expires_at: item.expires_at,
                    same_site: same_site_from_db(item.same_site)?,
                },
                version: SecretVersion::new(item.version).ok_or(CookieError::Storage)?,
                slot: item.slot,
            },
        );
    }
    let mut authority = ledger.authority;
    if authority.schema_version == 0 {
        authority.schema_version = COOKIE_AUTHORITY_SCHEMA_VERSION;
    }
    if authority.schema_version != COOKIE_AUTHORITY_SCHEMA_VERSION {
        return Err(CookieError::Storage);
    }
    Ok((records, authority))
}

fn persist_records(
    root: Option<&Path>,
    records: &BTreeMap<CookieScope, BTreeMap<CookieIdentity, CookieRecord>>,
    authority_state: &PersistedCookieAuthorityState,
) -> Result<(), CookieError> {
    let Some(root) = root else {
        return Ok(());
    };
    let revisions = root.join("revisions");
    fs::create_dir_all(&revisions).map_err(|_| CookieError::Storage)?;
    let revision = latest_revision(root)?.saturating_add(1);
    let ledger = PersistedCookieLedger {
        schema_version: COOKIE_SCHEMA_VERSION,
        revision,
        records: records
            .iter()
            .flat_map(|(scope, values)| {
                values
                    .iter()
                    .map(|(identity, record)| PersistedCookieRecord {
                        workspace_id: scope.workspace_id.as_str().to_owned(),
                        profile_id: scope.profile_id.as_str().to_owned(),
                        name: identity.name.clone(),
                        domain: identity.domain.clone(),
                        path: identity.path.clone(),
                        secure: record.metadata.secure,
                        http_only: record.metadata.http_only,
                        expires_at: record.metadata.expires_at,
                        same_site: same_site_to_db(record.metadata.same_site),
                        version: record.version.generation(),
                        slot: record.slot,
                    })
            })
            .collect(),
        authority: authority_state.clone(),
    };
    let temporary = revisions.join(format!(".tmp-{}-{}.json", std::process::id(), revision));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|_| CookieError::Storage)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, &ledger).map_err(|_| CookieError::Storage)?;
    writer.write_all(b"\n").map_err(|_| CookieError::Storage)?;
    writer.flush().map_err(|_| CookieError::Storage)?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|_| CookieError::Storage)?;
    fs::rename(temporary, revisions.join(format!("{revision:020}.json")))
        .map_err(|_| CookieError::Storage)
}

fn default_authority_state() -> PersistedCookieAuthorityState {
    PersistedCookieAuthorityState {
        schema_version: COOKIE_AUTHORITY_SCHEMA_VERSION,
        ..PersistedCookieAuthorityState::default()
    }
}

fn latest_ledger_path(root: &Path) -> Result<Option<PathBuf>, CookieError> {
    let revisions = root.join("revisions");
    let entries = match fs::read_dir(revisions) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(CookieError::Storage),
    };
    let mut latest = None;
    for entry in entries {
        let entry = entry.map_err(|_| CookieError::Storage)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(number) = name.strip_suffix(".json") else {
            continue;
        };
        let Ok(revision) = number.parse::<u64>() else {
            continue;
        };
        if latest
            .as_ref()
            .is_none_or(|(current, _): &(u64, PathBuf)| revision > *current)
        {
            latest = Some((revision, entry.path()));
        }
    }
    Ok(latest.map(|(_, path)| path))
}

fn latest_revision(root: &Path) -> Result<u64, CookieError> {
    Ok(latest_ledger_path(root)?
        .and_then(|path| path.file_stem()?.to_str()?.parse().ok())
        .unwrap_or(0))
}

fn same_site_to_db(value: CookieSameSite) -> i32 {
    match value {
        CookieSameSite::Unspecified => -1,
        CookieSameSite::None => 0,
        CookieSameSite::Lax => 1,
        CookieSameSite::Strict => 2,
    }
}

fn same_site_from_db(value: i32) -> Result<CookieSameSite, CookieError> {
    match value {
        -1 => Ok(CookieSameSite::Unspecified),
        0 => Ok(CookieSameSite::None),
        1 => Ok(CookieSameSite::Lax),
        2 => Ok(CookieSameSite::Strict),
        _ => Err(CookieError::Storage),
    }
}

fn normalize(input: CookieInput) -> Result<CookieInput, CookieError> {
    if input.name.is_empty() || input.name.len() > 256 || !valid_text(&input.name) {
        return Err(CookieError::InvalidName);
    }
    let domain = input
        .domain
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    if domain.is_empty() || domain.len() > 253 || !valid_domain(&domain) {
        return Err(CookieError::InvalidDomain);
    }
    if input.path.is_empty() || input.path.len() > 4096 || !input.path.starts_with('/') {
        return Err(CookieError::InvalidPath);
    }
    if input.value.is_empty() || input.expires_at.is_some_and(|value| value < 0) {
        return Err(CookieError::InvalidValue);
    }
    Ok(CookieInput { domain, ..input })
}

fn identity(input: &CookieInput) -> CookieIdentity {
    CookieIdentity {
        name: input.name.clone(),
        domain: input.domain.clone(),
        path: input.path.clone(),
    }
}

fn valid_text(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| !byte.is_ascii_control() && byte != b';')
}

fn valid_domain(value: &str) -> bool {
    value.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    })
}

fn authorize(
    scope: &CookieScope,
    action: Action,
    principal: &Principal,
    grants: &[Grant],
) -> Result<(), CookieError> {
    let capability = Capability::new(COOKIE_CAPABILITY).map_err(|_| CookieError::Forbidden)?;
    let resource_scope = ResourceScope {
        workspace_id: Some(scope.workspace_id.clone()),
        profile_id: Some(scope.profile_id.clone()),
        ..ResourceScope::default()
    };
    if is_allowed(principal, &capability, action, &resource_scope, grants) {
        Ok(())
    } else {
        Err(CookieError::Forbidden)
    }
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn key_for(scope: &CookieScope, identity: &CookieIdentity) -> SecretKey {
    key_for_slot(scope, identity, 0)
}

fn key_for_slot(scope: &CookieScope, identity: &CookieIdentity, slot: u8) -> SecretKey {
    let mut hasher = Sha256::new();
    for value in [
        scope.workspace_id.as_str(),
        scope.profile_id.as_str(),
        &identity.name,
        &identity.domain,
        &identity.path,
    ] {
        hasher.update(value.as_bytes());
        hasher.update([0]);
    }
    if slot != 0 {
        hasher.update([slot]);
    }
    SecretKey::new("cookies", format!("{:x}", hasher.finalize())).expect("fixed vault key format")
}

fn key_for_stage(stage_id: &str, scope: &CookieScope, identity: &CookieIdentity) -> SecretKey {
    let mut hasher = Sha256::new();
    for value in [
        stage_id,
        scope.workspace_id.as_str(),
        scope.profile_id.as_str(),
        &identity.name,
        &identity.domain,
        &identity.path,
    ] {
        hasher.update(value.as_bytes());
        hasher.update([0]);
    }
    SecretKey::new("cookie-staging", format!("{:x}", hasher.finalize()))
        .expect("fixed vault key format")
}
