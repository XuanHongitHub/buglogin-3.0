use buglogin_domain::{
    errors::Problem,
    ids::{ProfileId, ResourceId},
    state::{ImportState, ProfileLifecycle},
};
use serde::{Deserialize, Serialize};

pub type CommandError = Problem;
pub type CanonicalResourceId = ResourceId;
pub type CanonicalProfileId = ProfileId;
pub type ProfileStateContract = ProfileLifecycle;
pub type ImportStateContract = ImportState;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandResult<T> {
    pub command_id: String,
    pub correlation_id: String,
    pub data: T,
}
