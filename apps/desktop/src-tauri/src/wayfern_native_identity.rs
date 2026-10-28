use buglogin_browser::runtime::LaunchSpecification;
use buglogin_domain::{
    entities::{
        FingerprintOperatingSystem, PROFILE_RUNTIME_CONFIG_SCHEMA_VERSION,
        ProfileFingerprintTarget, WayfernProfileRuntimeConfig, validate_wayfern_fingerprint_target,
    },
    errors::Problem,
};
use serde_json::Value;

pub struct WayfernNativeIdentityAdapter;

impl WayfernNativeIdentityAdapter {
    /// Let Wayfern provide its native identity. Cross-OS and OS-version
    /// fingerprint targets require a Wayfern server-side entitlement, so keep
    /// the profile usable and report that the configured target was not applied.
    pub fn prepare(
        specification: &mut LaunchSpecification,
        target: &ProfileFingerprintTarget,
        correlation_id: &str,
    ) -> Option<Problem> {
        specification.args.retain(|arg| {
            !arg.starts_with("--wayfern-identity-file=")
                && !arg.starts_with("--wayfern-identity-file ")
        });

        let host = host_operating_system();
        if target.operating_system != host {
            return Some(not_applied_warning(
                correlation_id,
                "wayfern_cross_os_requires_plan",
            ));
        }
        if target.operating_system_version.is_some() {
            return Some(not_applied_warning(
                correlation_id,
                "wayfern_os_version_requires_plan",
            ));
        }

        None
    }
}

pub fn not_applied_warning(correlation_id: &str, cause: &str) -> Problem {
    Problem::new(
        "wayfern_fingerprint_unverified",
        "Profile opened with the runtime's native fingerprint; configured fingerprint could not be applied",
        200,
        correlation_id,
    )
    .with_field("causeCode", cause)
}

/// Resolve optional fingerprint configuration for a launch. Invalid or stale
/// fingerprint data must not prevent an otherwise authorized profile from
/// opening with Wayfern's native identity.
pub fn target_for_launch(
    runtime_config: Option<&Value>,
    correlation_id: &str,
) -> (Option<ProfileFingerprintTarget>, Option<Problem>) {
    match target_from_runtime_config(runtime_config, correlation_id) {
        Ok(target) => (target, None),
        Err(error) => (
            None,
            Some(
                not_applied_warning(correlation_id, &error.code)
                    .with_field("causeDetail", error.title),
            ),
        ),
    }
}

fn host_operating_system() -> FingerprintOperatingSystem {
    if cfg!(target_os = "windows") {
        FingerprintOperatingSystem::Windows
    } else if cfg!(target_os = "macos") {
        FingerprintOperatingSystem::Macos
    } else {
        FingerprintOperatingSystem::Linux
    }
}

pub fn target_from_runtime_config(
    runtime_config: Option<&Value>,
    correlation_id: &str,
) -> Result<Option<ProfileFingerprintTarget>, Problem> {
    let Some(value) = runtime_config else {
        return Ok(None);
    };
    if value.get("wayfern").is_none() {
        return Ok(None);
    }
    let config: WayfernProfileRuntimeConfig =
        serde_json::from_value(value.clone()).map_err(|_| {
            Problem::new(
                "profile_runtime_config_invalid",
                "Wayfern profile configuration is invalid",
                409,
                correlation_id,
            )
        })?;
    if config.schema_version != PROFILE_RUNTIME_CONFIG_SCHEMA_VERSION {
        return Err(Problem::new(
            "profile_runtime_config_version_unsupported",
            "Wayfern profile configuration version is unsupported",
            409,
            correlation_id,
        ));
    }
    let target = config.wayfern.fingerprint_target;
    validate_wayfern_fingerprint_target(&target).map_err(|error| {
        let (code, title, status) = error.problem_details();
        Problem::new(code, title, status, correlation_id)
    })?;
    Ok(Some(target))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_target() -> ProfileFingerprintTarget {
        ProfileFingerprintTarget {
            operating_system: host_operating_system(),
            operating_system_version: None,
        }
    }
    fn specification() -> LaunchSpecification {
        LaunchSpecification {
            runtime: buglogin_domain::entities::Runtime::Wayfern,
            version: "155.0.8059.79".into(),
            executable: std::env::current_exe().unwrap(),
            profile_path: "C:\\profiles\\test".into(),
            args: vec!["--wayfern-identity-file=stale-path".into()],
        }
    }

    #[test]
    fn host_native_identity_uses_wayfern_default_without_extra_args() {
        let mut specification = specification();

        let warning =
            WayfernNativeIdentityAdapter::prepare(&mut specification, &host_target(), "test");

        assert!(warning.is_none());
        assert!(specification.args.is_empty());
    }

    #[test]
    fn cross_os_target_falls_back_to_native_default_with_warning() {
        let mut specification = specification();
        let cross_os = ProfileFingerprintTarget {
            operating_system: FingerprintOperatingSystem::Android,
            operating_system_version: None,
        };
        let warning =
            WayfernNativeIdentityAdapter::prepare(&mut specification, &cross_os, "test").unwrap();
        assert_eq!(warning.code, "wayfern_fingerprint_unverified");
        assert_eq!(
            warning.fields["causeCode"],
            "wayfern_cross_os_requires_plan"
        );
        assert!(specification.args.is_empty());
    }

    #[test]
    fn os_version_target_falls_back_to_native_default_with_warning() {
        let mut specification = specification();
        let target = ProfileFingerprintTarget {
            operating_system: host_operating_system(),
            operating_system_version: Some("10.0".into()),
        };
        let warning =
            WayfernNativeIdentityAdapter::prepare(&mut specification, &target, "test").unwrap();
        assert_eq!(
            warning.fields["causeCode"],
            "wayfern_os_version_requires_plan"
        );
    }

    #[test]
    fn invalid_optional_config_is_a_warning_not_a_launch_error() {
        let (target, warning) = target_for_launch(
            Some(&serde_json::json!({
                "schemaVersion": 2,
                "wayfern": { "fingerprintTarget": { "operatingSystem": "windows" } }
            })),
            "test",
        );

        assert!(target.is_none());
        let warning = warning.unwrap();
        assert_eq!(warning.code, "wayfern_fingerprint_unverified");
        assert_eq!(
            warning.fields["causeCode"],
            "profile_runtime_config_version_unsupported"
        );
    }

    #[test]
    fn valid_native_config_resolves_without_warning() {
        let (target, warning) = target_for_launch(
            Some(&serde_json::json!({
                "schemaVersion": 1,
                "wayfern": {
                    "fingerprintTarget": {
                        "operatingSystem": match host_operating_system() {
                            FingerprintOperatingSystem::Windows => "windows",
                            FingerprintOperatingSystem::Macos => "macos",
                            FingerprintOperatingSystem::Linux => "linux",
                            FingerprintOperatingSystem::Android => "android",
                            FingerprintOperatingSystem::Ios => "ios",
                        }
                    }
                }
            })),
            "test",
        );

        assert_eq!(target, Some(host_target()));
        assert!(warning.is_none());
    }
}
