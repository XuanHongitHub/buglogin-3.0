use crate::{
    ids::{GrantId, GroupId, MemberId, ResourceId, WorkspaceId},
    permissions::{Action, Effect},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceGrant {
    pub id: GrantId,
    pub workspace_id: WorkspaceId,
    pub resource_id: ResourceId,
    pub member_id: MemberId,
    pub action: Action,
    pub effect: Effect,
}

/// A dynamic share from a workspace group to a workspace member.
///
/// It is evaluated against a profile's current group assignment on every
/// protected request, so moving a profile changes access immediately.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupShareGrant {
    pub id: GrantId,
    pub workspace_id: WorkspaceId,
    pub group_id: GroupId,
    pub member_id: MemberId,
    pub action: Action,
    pub effect: Effect,
    pub revision: u64,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
}
