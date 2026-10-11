use crate::ids::{MemberId, ProfileId, ResourceId, WorkspaceId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileArchiveState {
    Active,
    Archived,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileResource {
    pub profile_id: ProfileId,
    pub resource_id: ResourceId,
    pub workspace_id: WorkspaceId,
    pub archive_state: ProfileArchiveState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileSharePermission {
    Read,
    Operate,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileShare {
    pub member_id: MemberId,
    pub permission: ProfileSharePermission,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileShareState {
    pub profile_id: ProfileId,
    pub revision: u64,
    pub items: Vec<ProfileShare>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileCloneInput {
    pub source_profile_id: ProfileId,
    pub target_profile_id: ProfileId,
    pub name: String,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileCloneReceipt {
    pub source_profile_id: ProfileId,
    pub profile: crate::entities::Profile,
    pub resource_id: ResourceId,
    pub metadata_only: bool,
    pub copied_fields: Vec<String>,
    pub excluded_fields: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileShareInput {
    pub profile_id: ProfileId,
    pub member_id: MemberId,
    pub permission: ProfileSharePermission,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileShareRevokeInput {
    pub profile_id: ProfileId,
    pub member_id: MemberId,
    pub permission: ProfileSharePermission,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileShareReceipt {
    pub profile_id: ProfileId,
    pub member_id: MemberId,
    pub permission: Option<ProfileSharePermission>,
    pub revision: u64,
    pub shared: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileShareRevocationReceipt {
    pub profile_id: ProfileId,
    pub member_id: MemberId,
    pub revision: u64,
    pub revoked: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_resource_serializes_canonical_lease_binding() {
        let binding = ProfileResource {
            profile_id: ProfileId::from_str("prf_1").unwrap(),
            resource_id: ResourceId::from_str("res_1").unwrap(),
            workspace_id: WorkspaceId::from_str("ws_1").unwrap(),
            archive_state: ProfileArchiveState::Active,
        };

        let value = serde_json::to_value(binding).unwrap();
        assert_eq!(value["profileId"], "prf_1");
        assert_eq!(value["resourceId"], "res_1");
        assert_eq!(value["workspaceId"], "ws_1");
    }
}
