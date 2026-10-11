use std::{
    fs,
    path::{Path, PathBuf},
};

use buglogin_domain::{entities::Runtime, errors::Problem, ids::ProfileId};
use buglogin_storage::{blob::checksum, paths::confined_path};

use super::{
    LaunchSpecification, RuntimeAdapter, RuntimeBinary, RuntimeCapability, ValidatedRuntimeBinary,
    invalid_runtime,
};

const CAPABILITIES: &[RuntimeCapability] = &[
    RuntimeCapability::ProfileIsolation,
    RuntimeCapability::Proxy,
    RuntimeCapability::Cookies,
    RuntimeCapability::Extensions,
    RuntimeCapability::Fingerprint,
    RuntimeCapability::Cdp,
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WayfernAdapter;

impl RuntimeAdapter for WayfernAdapter {
    fn runtime(&self) -> Runtime {
        Runtime::Wayfern
    }

    fn capabilities(&self) -> &'static [RuntimeCapability] {
        CAPABILITIES
    }

    fn validate_binary(&self, binary: &RuntimeBinary) -> Result<ValidatedRuntimeBinary, Problem> {
        if binary.version.trim().is_empty() {
            return Err(invalid_runtime(
                "runtime_version_required",
                "Runtime version required",
            ));
        }
        if !is_sha256(&binary.expected_checksum) {
            return Err(invalid_runtime(
                "runtime_checksum_invalid",
                "Runtime checksum invalid",
            ));
        }
        let metadata = fs::symlink_metadata(&binary.path)
            .map_err(|_| invalid_runtime("runtime_binary_not_found", "Runtime binary not found"))?;
        if !metadata.file_type().is_file() {
            return Err(invalid_runtime(
                "runtime_binary_invalid",
                "Runtime binary is not a file",
            ));
        }
        let bytes = fs::read(&binary.path).map_err(|_| {
            invalid_runtime("runtime_binary_unreadable", "Runtime binary is unreadable")
        })?;
        let actual_checksum = checksum(&bytes);
        if actual_checksum != binary.expected_checksum.to_ascii_lowercase() {
            return Err(invalid_runtime(
                "runtime_checksum_mismatch",
                "Runtime checksum mismatch",
            ));
        }
        Ok(ValidatedRuntimeBinary {
            path: binary.path.clone(),
            version: binary.version.clone(),
            checksum: actual_checksum,
            size_bytes: metadata.len(),
        })
    }

    fn profile_path(
        &self,
        profiles_root: &Path,
        profile_id: &ProfileId,
    ) -> Result<PathBuf, Problem> {
        confined_path(profiles_root, Path::new(profile_id.as_str()))
    }

    fn launch_specification(
        &self,
        binary: &ValidatedRuntimeBinary,
        profile_path: &Path,
        extra_args: &[String],
    ) -> Result<LaunchSpecification, Problem> {
        if !profile_path.is_absolute() {
            return Err(invalid_runtime(
                "runtime_profile_path_invalid",
                "Runtime profile path must be absolute",
            ));
        }
        let mut args = vec![
            "--user-data-dir".to_string(),
            profile_path.to_string_lossy().into_owned(),
        ];
        args.extend(extra_args.iter().cloned());
        Ok(LaunchSpecification {
            runtime: Runtime::Wayfern,
            version: binary.version.clone(),
            executable: binary.path.clone(),
            profile_path: profile_path.to_path_buf(),
            args,
        })
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
