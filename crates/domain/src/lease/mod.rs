use crate::ids::{ResourceId, WorkspaceId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MIN_TTL_SECONDS: u64 = 1;
pub const MAX_TTL_SECONDS: u64 = 3_600;

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Lease {
    pub lease_id: String,
    pub fencing_token: u64,
    pub resource_id: ResourceId,
    pub workspace_id: WorkspaceId,
    pub owner_id: String,
    pub token: String,
    pub issued_at: u64,
    pub expires_at: u64,
}

impl std::fmt::Debug for Lease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Lease")
            .field("lease_id", &self.lease_id)
            .field("fencing_token", &self.fencing_token)
            .field("resource_id", &self.resource_id)
            .field("workspace_id", &self.workspace_id)
            .field("owner_id", &self.owner_id)
            .field("token", &"[REDACTED]")
            .field("issued_at", &self.issued_at)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl Lease {
    pub fn is_expired(&self, now: u64) -> bool {
        now >= self.expires_at
    }

    fn owns(&self, owner_id: &str, token: &str) -> bool {
        self.owner_id == owner_id && self.token == token
    }
}

#[derive(Default)]
pub struct LeaseStore {
    leases: BTreeMap<ResourceId, Lease>,
    sequence: u64,
}

impl LeaseStore {
    pub fn acquire(
        &mut self,
        resource_id: ResourceId,
        workspace_id: WorkspaceId,
        owner_id: impl Into<String>,
        now: u64,
        ttl_seconds: u64,
    ) -> Result<Lease, LeaseError> {
        validate_ttl(ttl_seconds)?;
        let owner_id = validate_owner(owner_id.into())?;
        if let Some(existing) = self.leases.get(&resource_id) {
            if existing.workspace_id != workspace_id {
                return Err(LeaseError::TenantMismatch);
            }
            if !existing.is_expired(now) {
                return Err(LeaseError::Held {
                    owner_id: existing.owner_id.clone(),
                    expires_at: existing.expires_at,
                });
            }
        }
        self.sequence = self.sequence.saturating_add(1);
        let lease = Lease {
            lease_id: format!("lease-{}", self.sequence),
            fencing_token: self.sequence,
            resource_id: resource_id.clone(),
            workspace_id,
            owner_id,
            token: secure_token().map_err(|_| LeaseError::TokenGeneration)?,
            issued_at: now,
            expires_at: now.saturating_add(ttl_seconds),
        };
        self.leases.insert(resource_id, lease.clone());
        Ok(lease)
    }

    pub fn renew(
        &mut self,
        resource_id: &ResourceId,
        owner_id: &str,
        token: &str,
        now: u64,
        ttl_seconds: u64,
    ) -> Result<Lease, LeaseError> {
        validate_ttl(ttl_seconds)?;
        let lease = self
            .leases
            .get_mut(resource_id)
            .ok_or(LeaseError::NotFound)?;
        if lease.is_expired(now) {
            return Err(LeaseError::Expired);
        }
        if !lease.owns(owner_id, token) {
            return Err(LeaseError::NotOwner);
        }
        lease.expires_at = now.saturating_add(ttl_seconds);
        Ok(lease.clone())
    }

    pub fn release(
        &mut self,
        resource_id: &ResourceId,
        owner_id: &str,
        token: &str,
        now: u64,
    ) -> Result<(), LeaseError> {
        let lease = self.leases.get(resource_id).ok_or(LeaseError::NotFound)?;
        if lease.is_expired(now) {
            self.leases.remove(resource_id);
            return Err(LeaseError::Expired);
        }
        if !lease.owns(owner_id, token) {
            return Err(LeaseError::NotOwner);
        }
        self.leases.remove(resource_id);
        Ok(())
    }

    pub fn get(&self, resource_id: &ResourceId, now: u64) -> Option<&Lease> {
        self.leases
            .get(resource_id)
            .filter(|lease| !lease.is_expired(now))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LeaseError {
    InvalidOwner,
    InvalidTtl,
    Held { owner_id: String, expires_at: u64 },
    NotFound,
    NotOwner,
    Expired,
    TokenGeneration,
    TenantMismatch,
}

fn validate_owner(owner_id: String) -> Result<String, LeaseError> {
    if owner_id.is_empty() || owner_id.len() > 128 || owner_id.contains(char::is_whitespace) {
        return Err(LeaseError::InvalidOwner);
    }
    Ok(owner_id)
}

fn validate_ttl(ttl_seconds: u64) -> Result<(), LeaseError> {
    if !(MIN_TTL_SECONDS..=MAX_TTL_SECONDS).contains(&ttl_seconds) {
        return Err(LeaseError::InvalidTtl);
    }
    Ok(())
}
fn secure_token() -> Result<String, getrandom::Error> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> (ResourceId, WorkspaceId) {
        (
            ResourceId::from_str("res_1").unwrap(),
            WorkspaceId::from_str("ws_1").unwrap(),
        )
    }

    #[test]
    fn expires_and_allows_recovery() {
        let (resource, workspace) = ids();
        let mut store = LeaseStore::default();
        let old = store
            .acquire(resource.clone(), workspace.clone(), "offline", 100, 10)
            .unwrap();
        assert!(matches!(
            store.acquire(resource.clone(), workspace, "recovered", 109, 10),
            Err(LeaseError::Held { .. })
        ));
        let recovered = store
            .acquire(
                resource,
                WorkspaceId::from_str("ws_1").unwrap(),
                "recovered",
                110,
                10,
            )
            .unwrap();
        assert_ne!(old.lease_id, recovered.lease_id);
        assert_ne!(old.token, recovered.token);
        assert!(recovered.fencing_token > old.fencing_token);
    }

    #[test]
    fn renew_and_release_require_current_owner() {
        let (resource, workspace) = ids();
        let mut store = LeaseStore::default();
        let lease = store
            .acquire(resource.clone(), workspace, "owner", 1, 30)
            .unwrap();
        assert_eq!(
            store.renew(&resource, "other", &lease.token, 2, 30),
            Err(LeaseError::NotOwner)
        );
        let renewed = store
            .renew(&resource, "owner", &lease.token, 2, 40)
            .unwrap();
        assert_eq!(renewed.expires_at, 42);
        store
            .release(&resource, "owner", &renewed.token, 3)
            .unwrap();
        assert!(store.get(&resource, 3).is_none());
    }

    #[test]
    fn token_is_redacted_and_workspace_cannot_change() {
        let (resource, workspace) = ids();
        let mut store = LeaseStore::default();
        let lease = store
            .acquire(resource.clone(), workspace, "owner", 1, 1)
            .unwrap();
        let debug = format!("{lease:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains(&lease.token));
        assert_eq!(
            store.acquire(
                resource,
                WorkspaceId::from_str("ws_other").unwrap(),
                "other",
                2,
                10,
            ),
            Err(LeaseError::TenantMismatch)
        );
    }

    #[test]
    fn expiry_cannot_be_renewed_and_ttl_is_bounded() {
        let (resource, workspace) = ids();
        let mut store = LeaseStore::default();
        assert_eq!(
            store.acquire(resource.clone(), workspace.clone(), "owner", 1, 0),
            Err(LeaseError::InvalidTtl)
        );
        let lease = store.acquire(resource, workspace, "owner", 1, 1).unwrap();
        assert_eq!(
            store.renew(&lease.resource_id, "owner", &lease.token, 2, 10),
            Err(LeaseError::Expired)
        );
        assert!(
            store
                .acquire(lease.resource_id, lease.workspace_id, "new", 2, 10)
                .is_ok()
        );
    }
}
