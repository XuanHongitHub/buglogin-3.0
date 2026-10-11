use crate::ids::{AccountId, MemberId, TeamId, WorkspaceId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MemberStatus {
    Invited,
    Active,
    Suspended,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemberRole {
    Owner,
    Admin,
    Operator,
    Member,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub id: MemberId,
    pub account_id: AccountId,
    pub workspace_id: WorkspaceId,
    pub team_id: Option<TeamId>,
    pub role: MemberRole,
    pub status: MemberStatus,
    pub invite_expires_at: Option<u64>,
}
