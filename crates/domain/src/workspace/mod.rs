use crate::ids::{OrganizationId, WorkspaceId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePolicy {
    pub max_profiles: u32,
    pub max_members: u32,
    pub max_storage_bytes: u64,
    pub offline_entitled: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: WorkspaceId,
    pub organization_id: OrganizationId,
    pub name: String,
    pub policy: WorkspacePolicy,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceUsage {
    pub profiles: u32,
    pub members: u32,
    pub storage_bytes: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OfflineEntitlement {
    Enabled,
    Disabled,
}
impl OfflineEntitlement {
    pub fn from_flag(enabled: bool) -> Self {
        if enabled {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }
}
