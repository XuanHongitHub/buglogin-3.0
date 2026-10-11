use crate::{
    ids::{GrantId, MemberId, ResourceId, WorkspaceId},
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
