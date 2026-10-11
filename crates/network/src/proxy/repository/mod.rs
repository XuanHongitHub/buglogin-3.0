use super::assignment::ProxyAssignmentRepository;
use buglogin_domain::entities::Proxy;
use buglogin_domain::ids::{ProxyId, WorkspaceId};
use buglogin_domain::permissions::{
    Action, Capability, Grant, Principal, ResourceScope, is_allowed,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const PROXY_CAPABILITY: &str = "CAP-NET-001";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RepositoryError {
    Forbidden,
    InvalidProxy,
    ProxyIdConflict,
    ProxyNotFound,
    WorkspaceMismatch,
    RevisionConflict { expected: u64, actual: u64 },
    RevisionExhausted,
    ProxyAssigned,
    SnapshotVersionUnsupported { expected: u32, actual: u32 },
    SnapshotInvalidWorkspace,
    SnapshotInvalidRevision,
    SnapshotDuplicateProxyId,
    SnapshotDuplicateEndpoint,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProxyRecord {
    pub workspace_id: WorkspaceId,
    pub proxy: Proxy,
    pub revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProxySnapshotRecord {
    pub workspace_id: String,
    pub proxy: Proxy,
    pub revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProxyRepositorySnapshot {
    pub schema_version: u32,
    pub records: Vec<ProxySnapshotRecord>,
}

impl Default for ProxyRepositorySnapshot {
    fn default() -> Self {
        Self {
            schema_version: ProxyRepository::SNAPSHOT_SCHEMA_VERSION,
            records: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProxyCreateResult {
    pub record: ProxyRecord,
    pub deduplicated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProxyUpdate {
    pub proxy: Proxy,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProxyDelete {
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct EndpointKey {
    workspace_id: WorkspaceId,
    protocol: String,
    host: String,
    port: u16,
}

#[derive(Clone, Default)]
pub struct ProxyRepository {
    proxies: BTreeMap<ProxyId, ProxyRecord>,
    endpoint_index: BTreeMap<EndpointKey, ProxyId>,
}

impl ProxyRepository {
    pub const SNAPSHOT_SCHEMA_VERSION: u32 = 1;

    pub fn snapshot(&self) -> ProxyRepositorySnapshot {
        ProxyRepositorySnapshot {
            schema_version: Self::SNAPSHOT_SCHEMA_VERSION,
            records: self
                .proxies
                .values()
                .map(|record| ProxySnapshotRecord {
                    workspace_id: record.workspace_id.as_str().to_owned(),
                    proxy: record.proxy.clone(),
                    revision: record.revision,
                })
                .collect(),
        }
    }

    pub fn restore(snapshot: ProxyRepositorySnapshot) -> Result<Self, RepositoryError> {
        if snapshot.schema_version != Self::SNAPSHOT_SCHEMA_VERSION {
            return Err(RepositoryError::SnapshotVersionUnsupported {
                expected: Self::SNAPSHOT_SCHEMA_VERSION,
                actual: snapshot.schema_version,
            });
        }

        let mut repository = Self::default();
        for record in snapshot.records {
            let workspace_id = WorkspaceId::from_str(&record.workspace_id)
                .map_err(|_| RepositoryError::SnapshotInvalidWorkspace)?;
            if record.revision == 0 {
                return Err(RepositoryError::SnapshotInvalidRevision);
            }
            if repository.proxies.contains_key(&record.proxy.id) {
                return Err(RepositoryError::SnapshotDuplicateProxyId);
            }

            let mut proxy = record.proxy;
            normalize_and_validate(&mut proxy)?;
            let endpoint = EndpointKey::new(workspace_id.clone(), &proxy);
            if repository.endpoint_index.contains_key(&endpoint) {
                return Err(RepositoryError::SnapshotDuplicateEndpoint);
            }
            let proxy_id = proxy.id.clone();
            let record = ProxyRecord {
                workspace_id,
                proxy,
                revision: record.revision,
            };
            repository.endpoint_index.insert(endpoint, proxy_id.clone());
            repository.proxies.insert(proxy_id, record);
        }
        Ok(repository)
    }

    pub fn create(
        &mut self,
        workspace_id: WorkspaceId,
        mut proxy: Proxy,
        principal: &Principal,
        grants: &[Grant],
    ) -> Result<ProxyCreateResult, RepositoryError> {
        authorize(principal, grants, Action::Create, &workspace_id)?;
        normalize_and_validate(&mut proxy)?;
        let endpoint = EndpointKey::new(workspace_id.clone(), &proxy);
        if let Some(existing_id) = self.endpoint_index.get(&endpoint) {
            let record = self
                .proxies
                .get(existing_id)
                .expect("endpoint index must reference a proxy")
                .clone();
            return Ok(ProxyCreateResult {
                record,
                deduplicated: true,
            });
        }
        if self.proxies.contains_key(&proxy.id) {
            return Err(RepositoryError::ProxyIdConflict);
        }
        let record = ProxyRecord {
            workspace_id,
            proxy,
            revision: 1,
        };
        self.endpoint_index
            .insert(endpoint, record.proxy.id.clone());
        self.proxies.insert(record.proxy.id.clone(), record.clone());
        Ok(ProxyCreateResult {
            record,
            deduplicated: false,
        })
    }

    pub fn get(
        &self,
        workspace_id: &WorkspaceId,
        proxy_id: &ProxyId,
        principal: &Principal,
        grants: &[Grant],
    ) -> Result<ProxyRecord, RepositoryError> {
        authorize(principal, grants, Action::Read, workspace_id)?;
        let record = self
            .proxies
            .get(proxy_id)
            .ok_or(RepositoryError::ProxyNotFound)?;
        ensure_workspace(workspace_id, record)?;
        Ok(record.clone())
    }

    pub fn list(
        &self,
        workspace_id: &WorkspaceId,
        principal: &Principal,
        grants: &[Grant],
    ) -> Result<Vec<ProxyRecord>, RepositoryError> {
        authorize(principal, grants, Action::Read, workspace_id)?;
        Ok(self
            .proxies
            .values()
            .filter(|record| &record.workspace_id == workspace_id)
            .cloned()
            .collect())
    }

    pub fn update(
        &mut self,
        workspace_id: &WorkspaceId,
        proxy_id: &ProxyId,
        mut update: ProxyUpdate,
        principal: &Principal,
        grants: &[Grant],
    ) -> Result<ProxyRecord, RepositoryError> {
        authorize(principal, grants, Action::Update, workspace_id)?;
        if &update.proxy.id != proxy_id {
            return Err(RepositoryError::InvalidProxy);
        }
        normalize_and_validate(&mut update.proxy)?;
        let current = self
            .proxies
            .get(proxy_id)
            .ok_or(RepositoryError::ProxyNotFound)?;
        ensure_workspace(workspace_id, current)?;
        check_revision(current.revision, update.expected_revision)?;
        let old_endpoint = EndpointKey::new(workspace_id.clone(), &current.proxy);
        let new_endpoint = EndpointKey::new(workspace_id.clone(), &update.proxy);
        if old_endpoint != new_endpoint
            && self
                .endpoint_index
                .get(&new_endpoint)
                .is_some_and(|existing_id| existing_id != proxy_id)
        {
            return Err(RepositoryError::ProxyIdConflict);
        }
        let revision = current
            .revision
            .checked_add(1)
            .ok_or(RepositoryError::RevisionExhausted)?;
        let record = ProxyRecord {
            workspace_id: workspace_id.clone(),
            proxy: update.proxy,
            revision,
        };
        if old_endpoint != new_endpoint {
            self.endpoint_index.remove(&old_endpoint);
            self.endpoint_index.insert(new_endpoint, proxy_id.clone());
        }
        self.proxies.insert(proxy_id.clone(), record.clone());
        Ok(record)
    }

    pub fn delete(
        &mut self,
        workspace_id: &WorkspaceId,
        proxy_id: &ProxyId,
        input: ProxyDelete,
        assignments: &ProxyAssignmentRepository,
        principal: &Principal,
        grants: &[Grant],
    ) -> Result<ProxyRecord, RepositoryError> {
        authorize(principal, grants, Action::Delete, workspace_id)?;
        let record = self
            .proxies
            .get(proxy_id)
            .ok_or(RepositoryError::ProxyNotFound)?;
        ensure_workspace(workspace_id, record)?;
        check_revision(record.revision, input.expected_revision)?;
        if assignments.is_proxy_assigned(workspace_id, proxy_id) {
            return Err(RepositoryError::ProxyAssigned);
        }
        let endpoint = EndpointKey::new(workspace_id.clone(), &record.proxy);
        let removed = self
            .proxies
            .remove(proxy_id)
            .ok_or(RepositoryError::ProxyNotFound)?;
        self.endpoint_index.remove(&endpoint);
        Ok(removed)
    }

    pub(crate) fn get_in_workspace(
        &self,
        proxy_id: &ProxyId,
        workspace_id: &WorkspaceId,
    ) -> Option<&ProxyRecord> {
        self.proxies
            .get(proxy_id)
            .filter(|record| &record.workspace_id == workspace_id)
    }
}

impl EndpointKey {
    fn new(workspace_id: WorkspaceId, proxy: &Proxy) -> Self {
        Self {
            workspace_id,
            protocol: format!("{:?}", proxy.protocol).to_ascii_lowercase(),
            host: proxy.host.trim().to_ascii_lowercase(),
            port: proxy.port,
        }
    }
}

fn normalize_and_validate(proxy: &mut Proxy) -> Result<(), RepositoryError> {
    proxy.host = proxy.host.trim().to_ascii_lowercase();
    if proxy.host.is_empty()
        || proxy.host.len() > 255
        || proxy.port == 0
        || proxy.created_at.trim().is_empty()
        || proxy.updated_at.trim().is_empty()
    {
        return Err(RepositoryError::InvalidProxy);
    }
    Ok(())
}

fn ensure_workspace(
    workspace_id: &WorkspaceId,
    record: &ProxyRecord,
) -> Result<(), RepositoryError> {
    if &record.workspace_id == workspace_id {
        Ok(())
    } else {
        Err(RepositoryError::WorkspaceMismatch)
    }
}

fn check_revision(actual: u64, expected: u64) -> Result<(), RepositoryError> {
    if actual == expected {
        Ok(())
    } else {
        Err(RepositoryError::RevisionConflict { expected, actual })
    }
}

fn authorize(
    principal: &Principal,
    grants: &[Grant],
    action: Action,
    workspace_id: &WorkspaceId,
) -> Result<(), RepositoryError> {
    let capability = Capability::new(PROXY_CAPABILITY).map_err(|_| RepositoryError::Forbidden)?;
    if is_allowed(
        principal,
        &capability,
        action,
        &ResourceScope::workspace(workspace_id.clone()),
        grants,
    ) {
        Ok(())
    } else {
        Err(RepositoryError::Forbidden)
    }
}
