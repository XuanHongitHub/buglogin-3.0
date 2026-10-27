use crate::ids::{ExtensionGroupId, OrganizationId, WorkspaceId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePolicy {
    pub max_profiles: u32,
    pub max_members: u32,
    pub max_storage_bytes: u64,
    pub offline_entitled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_extension_group_id: Option<ExtensionGroupId>,
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

#[cfg(test)]
mod tests {
    use super::WorkspacePolicy;

    #[test]
    fn default_extension_group_is_backward_compatible_and_omitted_when_unset() {
        let legacy = serde_json::json!({
            "maxProfiles": 10,
            "maxMembers": 5,
            "maxStorageBytes": 1024,
            "offlineEntitled": false
        });
        let policy: WorkspacePolicy = serde_json::from_value(legacy).unwrap();
        assert!(policy.default_extension_group_id.is_none());
        assert!(
            serde_json::to_value(policy)
                .unwrap()
                .get("defaultExtensionGroupId")
                .is_none()
        );
    }
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
