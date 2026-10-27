use crate::ids::{AccountId, ExtensionGroupId, OrganizationId, OrganizationMemberId, WorkspaceId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Organization {
    pub id: OrganizationId,
    pub account_id: AccountId,
    pub name: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OrganizationMemberStatus {
    Active,
    Suspended,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationMember {
    pub id: OrganizationMemberId,
    pub organization_id: OrganizationId,
    pub account_id: AccountId,
    pub status: OrganizationMemberStatus,
    pub revision: u64,
}

impl OrganizationMember {
    pub fn active(organization_id: OrganizationId, account_id: AccountId) -> Self {
        Self {
            id: organization_member_id(&organization_id, &account_id),
            organization_id,
            account_id,
            status: OrganizationMemberStatus::Active,
            revision: 1,
        }
    }

    pub fn transition_status(
        &self,
        expected_revision: u64,
        status: OrganizationMemberStatus,
    ) -> Result<Self, OrganizationMemberTransitionError> {
        if self.revision != expected_revision {
            return Err(OrganizationMemberTransitionError::RevisionConflict);
        }
        if self.revision == u64::MAX {
            return Err(OrganizationMemberTransitionError::RevisionExhausted);
        }
        let mut next = self.clone();
        next.status = status;
        next.revision += 1;
        Ok(next)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrganizationMemberTransitionError {
    RevisionConflict,
    RevisionExhausted,
}

pub fn organization_member_id(
    organization_id: &OrganizationId,
    account_id: &AccountId,
) -> OrganizationMemberId {
    use sha2::{Digest, Sha256};

    let digest =
        Sha256::digest(format!("{}\0{}", organization_id.as_str(), account_id.as_str()).as_bytes());
    let suffix = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    OrganizationMemberId::new(format!("orgmem_{suffix}"))
        .expect("SHA-256 organization membership IDs are valid")
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationPolicy {
    pub max_workspaces: u32,
    pub offline_entitled: bool,
    /// Workspace-scoped default extension groups selected by this organization.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub default_extension_groups_by_workspace: BTreeMap<WorkspaceId, ExtensionGroupId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationSnapshot {
    pub organization: Organization,
    pub policy: OrganizationPolicy,
    pub workspace_ids: Vec<WorkspaceId>,
}
