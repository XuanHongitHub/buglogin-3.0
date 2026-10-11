use crate::ids::{AccountId, OrganizationId, WorkspaceId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Organization {
    pub id: OrganizationId,
    pub account_id: AccountId,
    pub name: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationPolicy {
    pub max_workspaces: u32,
    pub offline_entitled: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationSnapshot {
    pub organization: Organization,
    pub policy: OrganizationPolicy,
    pub workspace_ids: Vec<WorkspaceId>,
}
