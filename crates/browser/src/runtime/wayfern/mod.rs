use std::{
    fs,
    path::{Path, PathBuf},
};

use buglogin_domain::{entities::Runtime, errors::Problem, ids::ProfileId};
use buglogin_storage::paths::confined_path;

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
        let file = fs::File::open(&binary.path).map_err(|_| {
            invalid_runtime("runtime_binary_unreadable", "Runtime binary is unreadable")
        })?;
        let (actual_checksum, actual_size) = buglogin_storage::blob::checksum_reader(file)
            .map_err(|_| {
                invalid_runtime("runtime_binary_unreadable", "Runtime binary is unreadable")
            })?;
        if actual_size != metadata.len() {
            return Err(invalid_runtime(
                "runtime_size_mismatch",
                "Runtime size changed during validation",
            ));
        }
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
        if extra_args
            .iter()
            .any(|argument| is_user_data_dir_override(argument))
        {
            return Err(invalid_runtime(
                "runtime_profile_path_override_forbidden",
                "Runtime arguments cannot override the isolated profile data directory",
            ));
        }
        if !profile_path.is_absolute() {
            return Err(invalid_runtime(
                "runtime_profile_path_invalid",
                "Runtime profile path must be absolute",
            ));
        }
        let mut args = vec![format!(
            "--user-data-dir={}",
            profile_path.to_string_lossy()
        )];
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

fn is_user_data_dir_override(argument: &str) -> bool {
    argument
        .split_once('=')
        .map_or(argument, |(name, _)| name)
        .eq_ignore_ascii_case("--user-data-dir")
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binary() -> ValidatedRuntimeBinary {
        ValidatedRuntimeBinary {
            path: PathBuf::from(r"C:\BugLogin\runtimes\wayfern\chrome.exe"),
            version: "154.0.8037.57".into(),
            checksum: "a".repeat(64),
            size_bytes: 1,
        }
    }

    #[test]
    fn launch_specification_uses_the_profile_specific_data_directory() {
        let profile_path = Path::new(r"C:\BugLogin\profiles\profile-one");
        let specification = WayfernAdapter
            .launch_specification(&binary(), profile_path, &["--disable-sync".into()])
            .unwrap();

        assert_eq!(
            specification.args,
            vec![
                format!("--user-data-dir={}", profile_path.to_string_lossy()),
                "--disable-sync".to_owned(),
            ]
        );
    }

    #[test]
    fn launch_specification_rejects_runtime_data_directory_overrides() {
        let error = WayfernAdapter
            .launch_specification(
                &binary(),
                Path::new(r"C:\BugLogin\profiles\profile-one"),
                &["--user-data-dir=C:\\Users\\Acer\\AppData\\Local\\Wayfern".into()],
            )
            .unwrap_err();

        assert_eq!(error.code, "runtime_profile_path_override_forbidden");
    }
}
