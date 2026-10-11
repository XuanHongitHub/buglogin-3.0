use std::path::{Path, PathBuf};

use buglogin_domain::{entities::Runtime, errors::Problem, ids::ProfileId};
use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeCapability {
    ProfileIsolation,
    Proxy,
    Cookies,
    Extensions,
    Fingerprint,
    Cdp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeBinary {
    pub path: PathBuf,
    pub version: String,
    pub expected_checksum: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidatedRuntimeBinary {
    pub path: PathBuf,
    pub version: String,
    pub checksum: String,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchSpecification {
    pub runtime: Runtime,
    pub version: String,
    pub executable: PathBuf,
    pub profile_path: PathBuf,
    pub args: Vec<String>,
}

pub trait RuntimeAdapter {
    fn runtime(&self) -> Runtime;
    fn capabilities(&self) -> &'static [RuntimeCapability];
    fn validate_binary(&self, binary: &RuntimeBinary) -> Result<ValidatedRuntimeBinary, Problem>;
    fn profile_path(
        &self,
        profiles_root: &Path,
        profile_id: &ProfileId,
    ) -> Result<PathBuf, Problem>;
    fn launch_specification(
        &self,
        binary: &ValidatedRuntimeBinary,
        profile_path: &Path,
        extra_args: &[String],
    ) -> Result<LaunchSpecification, Problem>;
}

pub(crate) fn invalid_runtime(code: &'static str, title: &'static str) -> Problem {
    Problem::new(code, title, 400, "runtime")
}
