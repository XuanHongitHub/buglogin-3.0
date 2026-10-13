use serde::{Deserialize, Serialize};
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
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileState {
    Created,
    Ready,
    Launching,
    Running,
    Stopping,
    Failed,
    Recovering,
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
    pub runtime: Runtime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
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
