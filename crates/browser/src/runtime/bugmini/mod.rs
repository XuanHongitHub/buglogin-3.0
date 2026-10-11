use std::path::{Path, PathBuf};

use buglogin_domain::{entities::Runtime, errors::Problem, ids::ProfileId};

use super::{
    LaunchSpecification, RuntimeAdapter, RuntimeBinary, RuntimeCapability, ValidatedRuntimeBinary,
    wayfern::WayfernAdapter,
};

const CAPABILITIES: &[RuntimeCapability] = &[
    RuntimeCapability::ProfileIsolation,
    RuntimeCapability::Proxy,
    RuntimeCapability::Cookies,
    RuntimeCapability::Extensions,
    RuntimeCapability::Cdp,
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BugminiAdapter;

impl RuntimeAdapter for BugminiAdapter {
    fn runtime(&self) -> Runtime {
        Runtime::Bugmini
    }

    fn capabilities(&self) -> &'static [RuntimeCapability] {
        CAPABILITIES
    }

    fn validate_binary(&self, binary: &RuntimeBinary) -> Result<ValidatedRuntimeBinary, Problem> {
        WayfernAdapter.validate_binary(binary)
    }

    fn profile_path(
        &self,
        profiles_root: &Path,
        profile_id: &ProfileId,
    ) -> Result<PathBuf, Problem> {
        WayfernAdapter.profile_path(profiles_root, profile_id)
    }

    fn launch_specification(
        &self,
        binary: &ValidatedRuntimeBinary,
        profile_path: &Path,
        extra_args: &[String],
    ) -> Result<LaunchSpecification, Problem> {
        let mut specification =
            WayfernAdapter.launch_specification(binary, profile_path, extra_args)?;
        let profile_arg = specification.args.get(1).cloned().ok_or_else(|| {
            Problem::new(
                "runtime_launch_specification_invalid",
                "Runtime launch specification is invalid",
                500,
                "runtime-bugmini-launch",
            )
        })?;
        let profile_arg = bugmini_path_argument(&profile_arg);
        specification
            .args
            .splice(0..2, [format!("--user-data-dir={profile_arg}")]);
        specification.runtime = Runtime::Bugmini;
        Ok(specification)
    }
}

fn bugmini_path_argument(path: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        if let Some(path) = path.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{path}");
        }
        if let Some(path) = path.strip_prefix(r"\\?\") {
            return path.to_owned();
        }
    }
    path.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "windows")]
    #[test]
    fn verbatim_windows_profile_paths_are_normalized_for_bugmini() {
        assert_eq!(
            bugmini_path_argument(r"\\?\D:\profiles\one"),
            r"D:\profiles\one"
        );
        assert_eq!(
            bugmini_path_argument(r"\\?\UNC\server\share\one"),
            r"\\server\share\one"
        );
    }

    #[test]
    fn user_data_dir_is_one_argument_for_bugmini_headless() {
        let root = std::env::temp_dir().join("buglogin-bugmini-runtime-args");
        let binary = ValidatedRuntimeBinary {
            path: root.join("chrome.exe"),
            version: "146.0.7680.177.4".to_owned(),
            checksum: "a".repeat(64),
            size_bytes: 1,
        };
        let profile = root.join("profile");
        let specification = BugminiAdapter
            .launch_specification(
                &binary,
                &profile,
                &[
                    "http://127.0.0.1:3000/check".to_owned(),
                    "--headless=new".to_owned(),
                ],
            )
            .unwrap();

        let expected_profile_arg = format!("--user-data-dir={}", profile.to_string_lossy());
        assert_eq!(
            specification.args.first().map(String::as_str),
            Some(expected_profile_arg.as_str())
        );
        assert!(
            !specification
                .args
                .iter()
                .any(|argument| argument == &profile.to_string_lossy())
        );
    }
}
