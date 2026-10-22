use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::ids::{
    ArtifactId, CredentialId, ExtensionGroupId, ExtensionId, GroupId, ProfileId, ProxyId,
    RuntimeInstallId, VpnId,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    Wayfern,
    Bugox,
    Bugium,
    Bugmini,
    Bugthor,
    Bughelium,
    /// Migration-only marker for a legacy runtime with no approved local adapter.
    Preserved,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileCreateRuntime {
    Wayfern,
    Bugmini,
}

impl From<ProfileCreateRuntime> for Runtime {
    fn from(runtime: ProfileCreateRuntime) -> Self {
        match runtime {
            ProfileCreateRuntime::Wayfern => Self::Wayfern,
            ProfileCreateRuntime::Bugmini => Self::Bugmini,
        }
    }
}

fn deserialize_profile_create_runtime<'de, D>(deserializer: D) -> Result<Runtime, D::Error>
where
    D: Deserializer<'de>,
{
    ProfileCreateRuntime::deserialize(deserializer).map(Into::into)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileState {
    #[serde(alias = "idle")]
    Created,
    Ready,
    Launching,
    Running,
    Stopping,
    Failed,
    Recovering,
}

#[cfg(test)]
mod profile_state_compat_tests {
    use super::ProfileState;

    #[test]
    fn legacy_idle_state_deserializes_as_created() {
        assert_eq!(
            serde_json::from_str::<ProfileState>("\"idle\"").unwrap(),
            ProfileState::Created
        );
        assert_eq!(
            serde_json::to_string(&ProfileState::Created).unwrap(),
            "\"created\""
        );
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncMode {
    Local,
    Managed,
    Disabled,
}

/// Stable command input shared by REST, MCP, and the native desktop adapter.
/// Identity, runtime state, timestamps, and sync authority are assigned by the
/// owning backend rather than accepted from callers.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileCreateInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    #[serde(deserialize_with = "deserialize_profile_create_runtime")]
    pub runtime: Runtime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint_target: Option<ProfileFingerprintTarget>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FingerprintOperatingSystem {
    Windows,
    Macos,
    Linux,
    Android,
    Ios,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileFingerprintTarget {
    pub operating_system: FingerprintOperatingSystem,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operating_system_version: Option<String>,
}

pub const PROFILE_RUNTIME_CONFIG_SCHEMA_VERSION: u8 = 1;

/// Versioned runtime configuration used by the Wayfern profile adapter.
/// Keep this DTO shared across API validation and desktop launch parsing.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WayfernProfileRuntimeConfig {
    pub schema_version: u8,
    pub wayfern: WayfernRuntimeConfig,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WayfernRuntimeConfig {
    pub fingerprint_target: ProfileFingerprintTarget,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WayfernRuntimeConfigError {
    Invalid,
    UnsupportedSchema,
    FingerprintRuntimeUnsupported,
    MobileFingerprintUnavailable,
    FingerprintVersionInvalid,
    FingerprintVersionUnsupported,
}

impl WayfernRuntimeConfigError {
    pub fn problem_details(self) -> (&'static str, &'static str, u16) {
        match self {
            Self::Invalid => (
                "profile_runtime_config_invalid",
                "Wayfern profile configuration is invalid",
                409,
            ),
            Self::UnsupportedSchema => (
                "profile_runtime_config_version_unsupported",
                "Wayfern profile configuration version is unsupported",
                409,
            ),
            Self::FingerprintRuntimeUnsupported => (
                "fingerprint_runtime_unsupported",
                "Fingerprint targets are supported only by the Wayfern runtime",
                400,
            ),
            Self::MobileFingerprintUnavailable => (
                "fingerprint_provider_unavailable",
                "This BugLogin build cannot generate mobile fingerprints",
                409,
            ),
            Self::FingerprintVersionInvalid => (
                "fingerprint_version_invalid",
                "Fingerprint operating system version must use numeric components",
                400,
            ),
            Self::FingerprintVersionUnsupported => (
                "fingerprint_version_unsupported",
                "The installed Wayfern provider cannot select an operating system version",
                409,
            ),
        }
    }
}

pub fn validate_wayfern_fingerprint_target(
    target: &ProfileFingerprintTarget,
) -> Result<(), WayfernRuntimeConfigError> {
    if matches!(
        target.operating_system,
        FingerprintOperatingSystem::Android | FingerprintOperatingSystem::Ios
    ) {
        return Err(WayfernRuntimeConfigError::MobileFingerprintUnavailable);
    }
    if let Some(version) = target.operating_system_version.as_deref() {
        let valid = !version.is_empty()
            && version.len() <= 12
            && version
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'.')
            && version.as_bytes()[0].is_ascii_digit()
            && version.as_bytes()[version.len() - 1].is_ascii_digit()
            && !version.contains("..");
        return Err(if valid {
            WayfernRuntimeConfigError::FingerprintVersionUnsupported
        } else {
            WayfernRuntimeConfigError::FingerprintVersionInvalid
        });
    }
    Ok(())
}

pub fn validate_wayfern_runtime_config(
    runtime: &Runtime,
    value: &Value,
) -> Result<(), WayfernRuntimeConfigError> {
    if value.get("wayfern").is_none() {
        return Ok(());
    }
    if runtime != &Runtime::Wayfern {
        return Err(WayfernRuntimeConfigError::FingerprintRuntimeUnsupported);
    }
    let config: WayfernProfileRuntimeConfig =
        serde_json::from_value(value.clone()).map_err(|_| WayfernRuntimeConfigError::Invalid)?;
    if config.schema_version != PROFILE_RUNTIME_CONFIG_SCHEMA_VERSION {
        return Err(WayfernRuntimeConfigError::UnsupportedSchema);
    }
    validate_wayfern_fingerprint_target(&config.wayfern.fingerprint_target)
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: ProfileId,
    pub name: String,
    pub runtime: Runtime,
    pub runtime_version: Option<String>,
    pub runtime_config: Option<Value>,
    pub state: ProfileState,
    pub archived: bool,
    pub fingerprint: Option<Value>,
    pub note: Option<String>,
    pub proxy_id: Option<ProxyId>,
    pub vpn_id: Option<VpnId>,
    pub group_ids: Vec<GroupId>,
    pub tags: Vec<String>,
    pub extension_group_id: Option<ExtensionGroupId>,
    pub sync_mode: Option<SyncMode>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRecord {
    #[serde(flatten)]
    pub profile: Profile,
    pub revision: u64,
}

impl ProfileRecord {
    pub fn new(profile: Profile, revision: u64) -> Self {
        Self { profile, revision }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub id: GroupId,
    pub name: String,
    pub appearance: Option<Value>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyProtocol {
    Http,
    Https,
    Socks4,
    Socks5,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Proxy {
    pub id: ProxyId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub protocol: ProxyProtocol,
    pub host: String,
    pub port: u16,
    pub credential_ref: Option<CredentialId>,
    pub rotation_url: Option<String>,
    pub bypass_rules: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VpnKind {
    Wireguard,
    Openvpn,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Vpn {
    pub id: VpnId,
    pub name: String,
    pub kind: VpnKind,
    pub config_ref: CredentialId,
    pub last_used_at: Option<String>,
    pub sync_mode: Option<SyncMode>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtensionPackageType {
    Xpi,
    Crx,
    Zip,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserCompatibility {
    Firefox,
    Chromium,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Extension {
    pub id: ExtensionId,
    pub name: String,
    pub file_name: Option<String>,
    pub package_type: ExtensionPackageType,
    pub artifact_ref: ArtifactId,
    pub artifact_sha256: String,
    pub compatibility: Vec<BrowserCompatibility>,
    pub sync_mode: Option<SyncMode>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionGroup {
    pub id: ExtensionGroupId,
    pub name: String,
    pub extension_ids: Vec<ExtensionId>,
    pub sync_mode: Option<SyncMode>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimePlatform {
    Windows,
    Macos,
    Linux,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeArchitecture {
    X64,
    Arm64,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeInstallState {
    Queued,
    Downloading,
    Verifying,
    Installing,
    Ready,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInstall {
    pub id: RuntimeInstallId,
    pub runtime: Runtime,
    pub version: String,
    pub platform: RuntimePlatform,
    pub architecture: RuntimeArchitecture,
    pub state: RuntimeInstallState,
    pub checksum: String,
    pub source: Option<String>,
}
