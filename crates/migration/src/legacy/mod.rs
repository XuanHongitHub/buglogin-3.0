mod control_plane;
mod entities;
mod extensions;
mod postgres_export;
mod s3_profiles;
mod vpns;
pub use crate::legacy_account_credentials::{
    EncryptedLegacyCredentials, LegacyScryptCredential, decrypt_package_credentials,
    decrypt_source_credentials, encrypt_package_credentials, encrypt_source_credentials,
};
pub use control_plane::{
    LegacyCapabilityOverride, LegacyControlGroup, LegacyControlMembership,
    LegacyControlOrganization, LegacyControlPlaneExport, LegacyControlPlaneMapping,
    LegacyControlProfile, LegacyControlProfileGroupAssignment, LegacyControlShareGrant,
    LegacyControlWorkspace, LegacyPendingInvite, LegacyPendingShareGrant, NeutralAccountData,
    NeutralAccountRecord, NeutralGrantData, NeutralGrantRecord, NeutralGroupShareGrantData,
    NeutralGroupShareGrantRecord, NeutralMemberData, NeutralMemberRecord, NeutralOrganizationData,
    NeutralOrganizationRecord, NeutralProfileGroupAssignment, NeutralProfileResourceData,
    NeutralProfileResourceRecord, NeutralResourceData, NeutralResourceRecord, NeutralTeamData,
    NeutralTeamRecord, NeutralWorkspaceData, NeutralWorkspacePolicy, NeutralWorkspaceRecord,
    canonicalize_legacy_control_plane_json, legacy_local_profile_id, map_legacy_control_plane,
    map_legacy_control_plane_json,
};
pub use entities::{
    LegacyProxyBatchAnalysis, LegacyProxyRowResult, LegacyProxyRowStatus, NeutralGroupData,
    NeutralGroupRecord, NeutralProxyData, NeutralProxyRecord, analyze_legacy_proxy_batch,
    map_legacy_groups, map_legacy_proxies, map_legacy_proxies_for_encrypted_package,
    map_legacy_proxies_for_ids,
};
pub use extensions::{
    ExtensionArtifact, LegacyExtensionExport, NeutralExtensionData, NeutralExtensionGroupData,
    NeutralExtensionGroupRecord, NeutralExtensionRecord, map_legacy_extensions,
};
pub use postgres_export::{
    LegacyStoragePolicy, PostgresControlExport, export_read_only_postgres_control_plane,
};
pub use s3_profiles::{LegacyS3HydrationReport, hydrate_legacy_s3_profile_payloads};
pub use vpns::{
    LegacyVpnExport, NeutralVpnData, NeutralVpnRecord, VpnConfigSecret, map_legacy_vpns,
};

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use buglogin_domain::errors::Problem;
use buglogin_domain::ids::migration_uuid;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileDataClassification {
    Encrypted,
    Excluded,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ProfileDataDisposition {
    pub classification: ProfileDataClassification,
    pub reason_code: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ProfileDataObject {
    pub path: String,
    pub logical_type: String,
    pub disposition: ProfileDataDisposition,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ProfileSourceRef {
    pub system: String,
    pub instance_id: String,
    pub id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct NeutralProfileData {
    pub name: String,
    pub runtime: String,
    pub runtime_version: Option<String>,
    pub release_type: String,
    pub ephemeral: bool,
    pub source_data_object: ProfileDataObject,
    pub migration_class: String,
    pub group_id: Option<String>,
    pub proxy_id: Option<String>,
    pub vpn_id: Option<String>,
    pub extension_group_id: Option<String>,
    pub tags: Vec<String>,
    pub note: Option<String>,
    pub legacy_runtime_config: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct NeutralProfileRecord {
    pub schema: String,
    pub source_ref: ProfileSourceRef,
    pub id: String,
    pub revision: u32,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub data: NeutralProfileData,
    pub extensions: Map<String, Value>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyProfileMapping {
    pub records: Vec<NeutralProfileRecord>,
    pub warnings: Vec<LegacyReferenceWarning>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyReferenceWarning {
    pub source_ref: String,
    pub field: String,
    pub target_ref: String,
}

#[derive(Default)]
struct ReferenceInventory {
    groups: BTreeSet<String>,
    proxies: BTreeSet<String>,
    vpns: BTreeSet<String>,
    extension_groups: BTreeSet<String>,
}

pub fn map_legacy_profiles(
    source_root: &Path,
    source_instance_id: &str,
) -> Result<LegacyProfileMapping, Problem> {
    map_legacy_profiles_with_disposition(source_root, source_instance_id, None)
}

pub fn map_legacy_profiles_for_encrypted_package(
    source_root: &Path,
    source_instance_id: &str,
) -> Result<LegacyProfileMapping, Problem> {
    map_legacy_profiles_with_disposition(
        source_root,
        source_instance_id,
        Some(ProfileDataDisposition {
            classification: ProfileDataClassification::Encrypted,
            reason_code: "source_profile_data_authenticated_encrypted".to_owned(),
        }),
    )
}

fn map_legacy_profiles_with_disposition(
    source_root: &Path,
    source_instance_id: &str,
    packaged_disposition: Option<ProfileDataDisposition>,
) -> Result<LegacyProfileMapping, Problem> {
    if source_instance_id.trim().is_empty() {
        return Err(invalid_metadata("source instance ID"));
    }
    let inventory = reference_inventory(source_root)?;
    let mut directories = read_profile_directories(&source_root.join("profiles"))?;
    directories.sort_by(|left, right| left.0.cmp(&right.0));
    let mut records = Vec::with_capacity(directories.len());
    let mut warnings = Vec::new();
    for (source_id, directory) in directories {
        let relative_metadata = format!("profiles/{source_id}/metadata.json");
        let metadata_path = directory.join("metadata.json");
        let bytes =
            fs::read(&metadata_path).map_err(|error| io_problem(&relative_metadata, &error))?;
        let metadata =
            serde_json::from_slice(&bytes).map_err(|_| invalid_metadata(&relative_metadata))?;
        let mut record = map_profile(
            &metadata,
            &source_id,
            source_instance_id,
            packaged_disposition.clone(),
        )?;
        let profile_warnings = profile_reference_warnings(&metadata, &source_id, &inventory);
        for warning in &profile_warnings {
            match warning.field.as_str() {
                "groupId" => record.data.group_id = None,
                "proxyId" => record.data.proxy_id = None,
                "vpnId" => record.data.vpn_id = None,
                "extensionGroupId" => record.data.extension_group_id = None,
                _ => {}
            }
        }
        if !profile_warnings.is_empty() {
            record.extensions.insert(
                "migration_reference_warnings".into(),
                serde_json::to_value(&profile_warnings)
                    .map_err(|_| invalid_metadata(&relative_metadata))?,
            );
        }
        warnings.extend(profile_warnings);
        records.push(record);
    }
    Ok(LegacyProfileMapping { records, warnings })
}

pub fn inspect_legacy_profile_reference_warnings(
    source_root: &Path,
) -> Result<Vec<LegacyReferenceWarning>, Problem> {
    let inventory = reference_inventory(source_root)?;
    let profiles_root = source_root.join("profiles");
    if !profiles_root.exists() {
        return Ok(Vec::new());
    }
    let mut directories = fs::read_dir(&profiles_root)
        .map_err(|error| io_problem("profiles", &error))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let source_id = path.file_name()?.to_str()?.to_owned();
            let metadata = path.join("metadata.json");
            metadata.is_file().then_some((source_id, metadata))
        })
        .collect::<Vec<_>>();
    directories.sort_by(|left, right| left.0.cmp(&right.0));
    let mut warnings = Vec::new();
    for (source_id, metadata_path) in directories {
        let Ok(metadata) = serde_json::from_slice::<Value>(
            &fs::read(&metadata_path).map_err(|error| io_problem("profile metadata", &error))?,
        ) else {
            continue;
        };
        warnings.extend(profile_reference_warnings(
            &metadata, &source_id, &inventory,
        ));
    }
    Ok(warnings)
}

fn read_profile_directories(profiles_root: &Path) -> Result<Vec<(String, PathBuf)>, Problem> {
    if !profiles_root.exists() {
        return Ok(Vec::new());
    }
    let mut directories = Vec::new();
    for entry in fs::read_dir(profiles_root).map_err(|error| io_problem("profiles", &error))? {
        let entry = entry.map_err(|error| io_problem("profiles", &error))?;
        let file_type = entry
            .file_type()
            .map_err(|error| io_problem("profiles", &error))?;
        if file_type.is_symlink() {
            return Err(invalid_metadata("profiles symlink"));
        }
        if !file_type.is_dir() {
            continue;
        }
        let source_id = entry.file_name().to_string_lossy().into_owned();
        if uuid::Uuid::parse_str(&source_id).is_err() {
            return Err(invalid_metadata(&format!(
                "profiles/{source_id}/metadata.json"
            )));
        }
        let metadata = entry.path().join("metadata.json");
        if metadata
            .symlink_metadata()
            .is_ok_and(|value| value.file_type().is_symlink())
        {
            return Err(invalid_metadata(&format!(
                "profiles/{source_id}/metadata.json"
            )));
        }
        if metadata.is_file() {
            directories.push((source_id, entry.path()));
        }
    }
    Ok(directories)
}

fn map_profile(
    metadata: &Value,
    source_id: &str,
    source_instance_id: &str,
    packaged_disposition: Option<ProfileDataDisposition>,
) -> Result<NeutralProfileRecord, Problem> {
    let object = metadata
        .as_object()
        .ok_or_else(|| invalid_metadata("profiles/<uuid>/metadata.json"))?;
    let metadata_id = required_string(object, &["id"])?;
    if metadata_id != source_id || uuid::Uuid::parse_str(&metadata_id).is_err() {
        return Err(invalid_metadata("profiles/<uuid>/metadata.json"));
    }
    let source_runtime = required_string(object, &["runtime", "browser"])?;
    let runtime = map_runtime(&source_runtime);
    let disposition = packaged_disposition.unwrap_or_else(|| profile_data_disposition(metadata));
    let runtime_config = object
        .get("browser_adapter_config")
        .or_else(|| object.get("browserAdapterConfig"))
        .map(|value| {
            if buglogin_domain::security::contains_secret(value) {
                serde_json::json!({"present": true, "classification": "excluded"})
            } else {
                value.clone()
            }
        });
    let mut extensions = Map::new();
    extensions.insert("legacy_runtime".to_owned(), Value::String(source_runtime));
    extensions.insert(
        "profile_data_disposition".to_owned(),
        serde_json::to_value(&disposition).map_err(|_| invalid_metadata("profile data"))?,
    );

    Ok(NeutralProfileRecord {
        schema: "buglogin.profile/1".to_owned(),
        source_ref: ProfileSourceRef {
            system: "buglogin-legacy".to_owned(),
            instance_id: source_instance_id.to_owned(),
            id: source_id.to_owned(),
        },
        id: migration_uuid("buglogin-legacy", source_instance_id, "profile", source_id).to_string(),
        revision: 1,
        created_at: string_value(object, &["created_at", "createdAt"]),
        updated_at: string_value(object, &["updated_at", "updatedAt"]),
        data: NeutralProfileData {
            name: profile_name(object, source_id),
            runtime: runtime.canonical,
            runtime_version: string_value(
                object,
                &["version", "runtime_version", "runtimeVersion"],
            ),
            release_type: string_value(object, &["release_type", "releaseType"])
                .unwrap_or_else(|| "stable".to_owned()),
            ephemeral: object
                .get("ephemeral")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            source_data_object: ProfileDataObject {
                path: format!("profiles/{source_id}/profile"),
                logical_type: "profile-directory".to_owned(),
                disposition,
            },
            migration_class: runtime.class,
            group_id: mapped_reference(
                object,
                &["group_id", "groupId"],
                source_instance_id,
                "group",
            ),
            proxy_id: mapped_reference(
                object,
                &["proxy_id", "proxyId"],
                source_instance_id,
                "proxy",
            ),
            vpn_id: mapped_reference(object, &["vpn_id", "vpnId"], source_instance_id, "vpn"),
            extension_group_id: mapped_reference(
                object,
                &["extension_group_id", "extensionGroupId"],
                source_instance_id,
                "extension-group",
            ),
            tags: object
                .get("tags")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            note: string_value(object, &["note"]),
            legacy_runtime_config: runtime_config,
        },
        extensions,
    })
}

pub fn legacy_runtime_fingerprint(config: Option<&Value>) -> Result<Option<Value>, Problem> {
    let Some(config) = config else {
        return Ok(None);
    };
    if config.get("classification").and_then(Value::as_str) == Some("excluded") {
        return Err(invalid_metadata("browser_adapter_config"));
    }
    let Some(payload) = config.get("payload").and_then(Value::as_object) else {
        return Ok(None);
    };
    let Some(value) = payload.get("fingerprint") else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let parsed = match value {
        Value::String(serialized) => serde_json::from_str(serialized)
            .map_err(|_| invalid_metadata("browser_adapter_config.payload.fingerprint"))?,
        value => value.clone(),
    };
    if !parsed.is_object() {
        return Err(invalid_metadata(
            "browser_adapter_config.payload.fingerprint",
        ));
    }
    Ok(Some(parsed))
}

struct RuntimeMapping {
    canonical: String,
    class: String,
}

fn map_runtime(runtime: &str) -> RuntimeMapping {
    let normalized = runtime.trim().to_ascii_lowercase();
    let (canonical, class) = match normalized.as_str() {
        "wayfern" => ("wayfern", "direct"),
        "camoufox" => ("camoufox", "unsupported"),
        "bugox" | "bugium" | "bugmini" | "bugthor" | "bughelium" | "cloak" | "cloakbrowser"
        | "thorium" | "helium" => (normalized.as_str(), "unsupported"),
        _ => ("unsupported", "unsupported"),
    };
    if canonical == "unsupported" {
        RuntimeMapping {
            canonical: format!(
                "unknown:sha256:{}",
                buglogin_storage::blob::checksum(runtime.as_bytes())
            ),
            class: class.to_owned(),
        }
    } else {
        RuntimeMapping {
            canonical: canonical.to_owned(),
            class: class.to_owned(),
        }
    }
}

fn reference_inventory(source_root: &Path) -> Result<ReferenceInventory, Problem> {
    let mut inventory = ReferenceInventory::default();
    collect_ids_from_json(
        &source_root.join("groups.json"),
        "groups",
        &mut inventory.groups,
    )?;
    let proxy_root = source_root.join("proxies");
    if proxy_root.is_dir() {
        for entry in fs::read_dir(&proxy_root).map_err(|error| io_problem("proxies", &error))? {
            let entry = entry.map_err(|error| io_problem("proxies", &error))?;
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|value| value.to_str()) != Some("json")
            {
                continue;
            }
            let fallback = path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            let Some(value) = read_json_if_valid(&path)? else {
                continue;
            };
            let id = value
                .as_object()
                .and_then(|object| object.get("id"))
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(fallback);
            if !id.is_empty() {
                inventory.proxies.insert(id.to_owned());
            }
        }
    }
    collect_nested_ids_from_json(
        &source_root.join("vpn").join("vpn_configs.json"),
        "configs",
        &mut inventory.vpns,
    )?;
    let extension_groups = [
        source_root.join("data").join("extension_groups.json"),
        source_root.join("extension_groups.json"),
    ]
    .into_iter()
    .find(|path| path.exists());
    if let Some(path) = extension_groups {
        collect_ids_from_json(&path, "groups", &mut inventory.extension_groups)?;
    }
    Ok(inventory)
}

fn collect_ids_from_json(
    path: &Path,
    key: &str,
    output: &mut BTreeSet<String>,
) -> Result<(), Problem> {
    let Some(value) = read_json_if_valid(path)? else {
        return Ok(());
    };
    let values = value
        .as_array()
        .or_else(|| value.get(key).and_then(Value::as_array));
    if let Some(values) = values {
        for value in values {
            if let Some(id) = value
                .as_object()
                .and_then(|object| object.get("id"))
                .and_then(Value::as_str)
                .filter(|id| !id.trim().is_empty())
            {
                output.insert(id.to_owned());
            }
        }
    }
    Ok(())
}

fn collect_nested_ids_from_json(
    path: &Path,
    key: &str,
    output: &mut BTreeSet<String>,
) -> Result<(), Problem> {
    collect_ids_from_json(path, key, output)
}

fn read_json_if_valid(path: &Path) -> Result<Option<Value>, Problem> {
    if !path.exists() {
        return Ok(None);
    }
    if path
        .symlink_metadata()
        .map_err(|error| io_problem("reference source", &error))?
        .file_type()
        .is_symlink()
    {
        return Err(invalid_metadata("reference source symlink"));
    }
    let bytes = fs::read(path).map_err(|error| io_problem("reference source", &error))?;
    Ok(serde_json::from_slice(&bytes).ok())
}

fn profile_reference_warnings(
    metadata: &Value,
    source_id: &str,
    inventory: &ReferenceInventory,
) -> Vec<LegacyReferenceWarning> {
    let Some(object) = metadata.as_object() else {
        return Vec::new();
    };
    [
        ("groupId", &["group_id", "groupId"][..], &inventory.groups),
        ("proxyId", &["proxy_id", "proxyId"][..], &inventory.proxies),
        ("vpnId", &["vpn_id", "vpnId"][..], &inventory.vpns),
        (
            "extensionGroupId",
            &["extension_group_id", "extensionGroupId"][..],
            &inventory.extension_groups,
        ),
    ]
    .into_iter()
    .filter_map(|(field, keys, known)| {
        let target = string_value(object, keys)?;
        if known.contains(&target) {
            None
        } else {
            Some(LegacyReferenceWarning {
                source_ref: format!("profiles/{source_id}/metadata.json"),
                field: field.to_owned(),
                target_ref: target,
            })
        }
    })
    .collect()
}

fn profile_data_disposition(metadata: &Value) -> ProfileDataDisposition {
    if buglogin_domain::security::contains_secret(metadata) {
        return ProfileDataDisposition {
            classification: ProfileDataClassification::Excluded,
            reason_code: "source_profile_data_contains_unencrypted_secrets".to_owned(),
        };
    }

    ProfileDataDisposition {
        classification: ProfileDataClassification::Excluded,
        reason_code: "source_profile_data_not_authenticated_encrypted".to_owned(),
    }
}

fn profile_name(object: &Map<String, Value>, source_id: &str) -> String {
    string_value(object, &["name"]).unwrap_or_else(|| {
        let compact = source_id.replace('-', "");
        format!("profile-{}", &compact[compact.len().saturating_sub(7)..])
    })
}

fn required_string(object: &Map<String, Value>, keys: &[&str]) -> Result<String, Problem> {
    string_value(object, keys)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_metadata("profiles/<uuid>/metadata.json"))
}

fn mapped_reference(
    object: &Map<String, Value>,
    keys: &[&str],
    source_instance_id: &str,
    entity: &str,
) -> Option<String> {
    string_value(object, keys).map(|source_id| {
        migration_uuid("buglogin-legacy", source_instance_id, entity, &source_id).to_string()
    })
}
fn string_value(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        object
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

fn invalid_metadata(path: &str) -> Problem {
    Problem::new(
        "migration_legacy_metadata_invalid",
        "Legacy profile metadata is invalid",
        422,
        "migration",
    )
    .with_detail(path.to_owned())
}

fn io_problem(path: &str, error: &std::io::Error) -> Problem {
    Problem::new(
        "migration_legacy_read_failed",
        "Legacy profile metadata could not be read",
        503,
        "migration",
    )
    .with_detail(format!("{path}: {error}"))
    .retryable(true)
}
