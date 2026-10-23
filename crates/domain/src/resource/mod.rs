use crate::ids::{ResourceId, WorkspaceId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceKind {
    Profile,
    Proxy,
    Vpn,
    CookieSet,
    Extension,
    Runtime,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceState {
    Active,
    Archived,
    Locked,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    pub id: ResourceId,
    pub workspace_id: WorkspaceId,
    pub kind: ResourceKind,
    pub state: ResourceState,
    pub display_name: String,
}
