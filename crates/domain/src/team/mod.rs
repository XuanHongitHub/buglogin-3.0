use crate::ids::{TeamId, WorkspaceId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    pub id: TeamId,
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub member_ids: Vec<crate::ids::MemberId>,
}
