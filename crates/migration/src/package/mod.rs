use std::collections::BTreeMap;

use buglogin_domain::errors::Problem;
use buglogin_domain::ids::migration_uuid;
use serde::{Deserialize, Serialize};

use crate::encrypted_data::encrypt_legacy_profile_data;
use crate::inspect::{CutoverEligibility, InspectReport, MigrationClass, RecordIssue};
use crate::snapshot::{SourceSnapshot, create_snapshot};
use buglogin_sync::crypto::ClientKey;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PackageMetadata {
    pub created_at: String,
    pub tool_version: String,
    pub tool_commit: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PackageContent {
    pub path: String,
    pub media_type: String,
    pub records: u64,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct NeutralPackageManifest {
    pub schema: String,
    pub package_id: String,
    pub created_at: String,
    pub created_by_tool: CreatedByTool,
    pub source: PackageSource,
    pub compatibility: PackageCompatibility,
    pub security: PackageSecurity,
    pub contents: Vec<PackageContent>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CreatedByTool {
    pub name: String,
    pub version: String,
    pub commit: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PackageSource {
    pub system: String,
    pub version: String,
    pub instance_id: String,
    pub os: String,
    pub architecture: String,
    pub snapshot_id: String,
    pub snapshot_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PackageCompatibility {
    pub minimum_importer: String,
    pub required_capabilities: Vec<String>,
    pub optional_capabilities: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PackageSecurity {
    pub contains_secrets: bool,
    pub contains_cookie_values: bool,
    pub encrypted: bool,
    pub encryption: Option<String>,
    pub redaction_policy: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NeutralPackage {
    files: BTreeMap<String, Vec<u8>>,
}

impl NeutralPackage {
    pub fn from_files(files: BTreeMap<String, Vec<u8>>) -> Self {
        Self { files }
    }

    pub fn files(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.files
    }

    pub fn file(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }
}

pub fn build_neutral_package(
    report: &InspectReport,
    snapshot: &SourceSnapshot,
    metadata: &PackageMetadata,
) -> Result<NeutralPackage, Problem> {
    if report.source_tree_sha256 != snapshot.source_tree_sha256 {
        return Err(Problem::new(
            "migration_snapshot_mismatch",
            "Migration snapshot does not match inspection",
            409,
            "migration",
        ));
    }
    if !matches!(report.source.os.as_str(), "windows" | "macos" | "linux")
        || !matches!(report.source.architecture.as_str(), "x86_64" | "aarch64")
    {
        return Err(Problem::new(
            "migration_source_platform_unsupported",
            "Migration source platform is unsupported",
            422,
            "migration",
        ));
    }

    let mut files = BTreeMap::new();
    insert_json(&mut files, "reports/source-inventory.json", report)?;
    let unsupported = report
        .runtime_findings
        .iter()
        .filter(|finding| finding.class != MigrationClass::Direct)
        .map(|finding| RecordIssue {
            source_ref: format!("runtime:{}", finding.runtime),
            code: finding.reason_code.clone(),
        })
        .collect::<Vec<_>>();
    insert_json(&mut files, "reports/unsupported.json", &unsupported)?;
    insert_json(&mut files, "snapshot.json", snapshot)?;

    let contents = files
        .iter()
        .map(|(path, bytes)| PackageContent {
            path: path.clone(),
            media_type: "application/json".to_owned(),
            records: 1,
            bytes: bytes.len() as u64,
            sha256: buglogin_storage::blob::checksum(bytes),
        })
        .collect::<Vec<_>>();
    let package_id = migration_uuid(
        &report.source.system,
        &report.source.instance_id,
        "migration-package",
        &snapshot.source_tree_sha256,
    )
    .to_string();
    let manifest = NeutralPackageManifest {
        schema: "buglogin.export/1".to_owned(),
        package_id,
        created_at: metadata.created_at.clone(),
        created_by_tool: CreatedByTool {
            name: "buglogin-migrate".to_owned(),
            version: metadata.tool_version.clone(),
            commit: metadata.tool_commit.clone(),
        },
        source: PackageSource {
            system: report.source.system.clone(),
            version: report.source.version.clone(),
            instance_id: report.source.instance_id.clone(),
            os: report.source.os.clone(),
            architecture: report.source.architecture.clone(),
            snapshot_id: snapshot.snapshot_id.clone(),
            snapshot_sha256: snapshot.source_tree_sha256.clone(),
        },
        compatibility: PackageCompatibility {
            minimum_importer: "0.1.0".to_owned(),
            required_capabilities: vec!["migration.read".to_owned()],
            optional_capabilities: Vec::new(),
        },
        security: PackageSecurity {
            contains_secrets: false,
            contains_cookie_values: false,
            encrypted: false,
            encryption: None,
            redaction_policy: "buglogin.redaction/1".to_owned(),
        },
        contents,
    };
    insert_json(&mut files, "manifest.json", &manifest)?;

    let checksums = files
        .iter()
        .map(|(path, bytes)| format!("{}  {path}\n", buglogin_storage::blob::checksum(bytes)))
        .collect::<String>();
    files.insert("checksums.sha256".to_owned(), checksums.into_bytes());
    Ok(NeutralPackage { files })
}

fn insert_json_lines<T: Serialize>(
    files: &mut BTreeMap<String, Vec<u8>>,
    path: &str,
    values: &[T],
) -> Result<(), Problem> {
    let mut bytes = Vec::new();
    for value in values {
        serde_json::to_writer(&mut bytes, value).map_err(|_| {
            Problem::new(
                "migration_package_serialization_failed",
                "Migration package serialization failed",
                500,
                "migration",
            )
        })?;
        bytes.push(b'\n');
    }
    files.insert(path.to_owned(), bytes);
    Ok(())
}

fn insert_json<T: Serialize>(
    files: &mut BTreeMap<String, Vec<u8>>,
    path: &str,
    value: &T,
) -> Result<(), Problem> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| {
        Problem::new(
            "migration_package_serialization_failed",
            "Migration package serialization failed",
            500,
            "migration",
        )
    })?;
    bytes.push(b'\n');
    files.insert(path.to_owned(), bytes);
    Ok(())
}

pub fn build_encrypted_profile_package(
    source_root: &std::path::Path,
    report: &InspectReport,
    snapshot: &SourceSnapshot,
    metadata: &PackageMetadata,
    key: &ClientKey,
) -> Result<NeutralPackage, Problem> {
    if report.cutover_eligibility != CutoverEligibility::Eligible
        && !preservation_export_allowed(report)
    {
        return Err(Problem::new(
            "migration_cutover_blocked",
            "Migration cutover is blocked by source compatibility findings",
            409,
            "migration",
        ));
    }
    let source_before = create_snapshot(source_root)?;
    if source_before.source_tree_sha256 != snapshot.source_tree_sha256 {
        return Err(Problem::new(
            "migration_source_changed",
            "Migration source changed before packaging",
            409,
            "migration",
        ));
    }
    let encrypted = encrypt_legacy_profile_data(source_root, key)?;
    let source_after = create_snapshot(source_root)?;
    if !source_before.has_same_integrity(&source_after) {
        return Err(Problem::new(
            "migration_source_changed",
            "Migration source changed while packaging",
            409,
            "migration",
        ));
    }
    let package = build_neutral_package(report, snapshot, metadata)?;
    let mut manifest: NeutralPackageManifest =
        serde_json::from_slice(package.file("manifest.json").ok_or_else(|| {
            Problem::new(
                "migration_manifest_missing",
                "Migration manifest is missing",
                500,
                "migration",
            )
        })?)
        .map_err(|_| {
            Problem::new(
                "migration_manifest_invalid",
                "Migration manifest is invalid",
                500,
                "migration",
            )
        })?;
    let mut files = package.files;
    files.remove("manifest.json");
    files.remove("checksums.sha256");
    let profiles = crate::legacy::map_legacy_profiles_for_encrypted_package(
        source_root,
        &report.source.instance_id,
    )?;
    if profiles
        .records
        .iter()
        .any(|profile| !matches!(profile.data.migration_class.as_str(), "direct" | "adapter"))
    {
        manifest
            .compatibility
            .required_capabilities
            .push("migration.preserve-only".into());
    }
    insert_json_lines(&mut files, "entities/profiles.jsonl", &profiles.records)?;
    let groups = crate::legacy::map_legacy_groups(source_root, &report.source.instance_id)?;
    if !groups.is_empty() {
        insert_json_lines(&mut files, "entities/groups.jsonl", &groups)?;
    }
    let proxies = crate::legacy::map_legacy_proxies_for_encrypted_package(
        source_root,
        &report.source.instance_id,
    )?;
    if !proxies.records.is_empty() {
        insert_json_lines(&mut files, "entities/proxies.jsonl", &proxies.records)?;
    }
    if !proxies.credentials.is_empty() {
        let encrypted_credentials =
            crate::encrypted_secrets::encrypt_proxy_credentials(&proxies.credentials, key)?;
        insert_json(
            &mut files,
            crate::encrypted_secrets::index_path(),
            &encrypted_credentials.index,
        )?;
        files.extend(encrypted_credentials.objects);
    }
    let extensions = crate::legacy::map_legacy_extensions(source_root, &report.source.instance_id)?;
    if !extensions.records.is_empty() {
        insert_json_lines(&mut files, "entities/extensions.jsonl", &extensions.records)?;
    }
    if !extensions.groups.is_empty() {
        insert_json_lines(
            &mut files,
            "entities/extension-groups.jsonl",
            &extensions.groups,
        )?;
    }
    for artifact in extensions.artifacts {
        files.insert(artifact.package_path, artifact.bytes);
    }
    let vpns = crate::legacy::map_legacy_vpns(source_root, &report.source.instance_id)?;
    if !vpns.records.is_empty() {
        insert_json_lines(&mut files, "entities/vpns.jsonl", &vpns.records)?;
        let configs = vpns
            .configs
            .iter()
            .map(|config| crate::encrypted_vpn_configs::VpnConfigSecret {
                schema: "buglogin.vpn-config-secret/1".into(),
                config_ref: config.config_ref.clone(),
                vpn_id: config.vpn_id.clone(),
                kind: match config.kind.as_str() {
                    "wireguard" => buglogin_domain::entities::VpnKind::Wireguard,
                    "openvpn" => buglogin_domain::entities::VpnKind::Openvpn,
                    _ => unreachable!("legacy VPN mapper validates kind"),
                },
                config_sha256: config.config_sha256.clone(),
                config_data: config.config_data().to_owned(),
            })
            .collect::<Vec<_>>();
        let encrypted_configs = crate::encrypted_vpn_configs::encrypt_vpn_configs(&configs, key)?;
        insert_json(
            &mut files,
            crate::encrypted_vpn_configs::index_path(),
            &encrypted_configs.index,
        )?;
        files.extend(encrypted_configs.objects);
    }
    let sync_export_path = source_root.join("sync-export.json");
    if snapshot
        .files
        .iter()
        .any(|file| file.path == "sync-export.json")
    {
        let bytes = std::fs::read(&sync_export_path).map_err(|_| {
            Problem::new(
                "migration_sync_export_read_failed",
                "Legacy sync export could not be read",
                422,
                "migration",
            )
        })?;
        let sync_export: buglogin_sync::export::SyncExport = serde_json::from_slice(&bytes)
            .map_err(|_| {
                Problem::new(
                    "migration_sync_export_invalid",
                    "Legacy sync export is invalid",
                    422,
                    "migration",
                )
            })?;
        sync_export.validate().map_err(|error| {
            Problem::new(
                "migration_sync_export_invalid",
                "Legacy sync export is invalid",
                422,
                "migration",
            )
            .with_detail(error.code.clone())
        })?;
        insert_json(&mut files, "sync/export.json", &sync_export)?;
    }
    let control_plane_path = source_root.join("control-plane-export.json");
    if snapshot
        .files
        .iter()
        .any(|file| file.path == "control-plane-export.json")
    {
        let bytes = std::fs::read(&control_plane_path).map_err(|_| {
            Problem::new(
                "migration_control_plane_read_failed",
                "Legacy control-plane export could not be read",
                422,
                "migration",
            )
        })?;
        let control_plane =
            crate::legacy::map_legacy_control_plane_json(&bytes, &report.source.instance_id)?;
        insert_json_lines(
            &mut files,
            "entities/accounts.jsonl",
            &control_plane.accounts,
        )?;
        insert_json_lines(
            &mut files,
            "entities/organizations.jsonl",
            &control_plane.organizations,
        )?;
        insert_json_lines(
            &mut files,
            "entities/workspaces.jsonl",
            &control_plane.workspaces,
        )?;
        insert_json_lines(&mut files, "entities/teams.jsonl", &control_plane.teams)?;
        insert_json_lines(&mut files, "entities/members.jsonl", &control_plane.members)?;
        insert_json_lines(
            &mut files,
            "entities/resources.jsonl",
            &control_plane.resources,
        )?;
        validate_profile_resource_bindings(&profiles.records, &control_plane)?;
        insert_json_lines(
            &mut files,
            "entities/profile-resources.jsonl",
            &control_plane.profile_resources,
        )?;
        insert_json_lines(&mut files, "entities/grants.jsonl", &control_plane.grants)?;
    }
    insert_json(
        &mut files,
        crate::encrypted_data::index_path(),
        &encrypted.index,
    )?;
    files.extend(encrypted.objects);

    manifest.security.encrypted = true;
    manifest.security.contains_secrets =
        !proxies.credentials.is_empty() || !vpns.configs.is_empty();
    manifest.security.encryption = Some("aes-256-gcm-chunks/1".to_owned());
    manifest.contents = files
        .iter()
        .map(|(path, bytes)| PackageContent {
            path: path.clone(),
            media_type: if (path.starts_with("encrypted/profile-data/")
                && path != crate::encrypted_data::index_path())
                || (path.starts_with("encrypted/proxy-credentials/")
                    && path != crate::encrypted_secrets::index_path())
                || (path.starts_with("encrypted/vpn-configs/")
                    && path != crate::encrypted_vpn_configs::index_path())
                || path.starts_with("artifacts/extensions/")
            {
                "application/octet-stream".to_owned()
            } else {
                "application/json".to_owned()
            },
            records: 1,
            bytes: bytes.len() as u64,
            sha256: buglogin_storage::blob::checksum(bytes),
        })
        .collect();
    insert_json(&mut files, "manifest.json", &manifest)?;
    let checksums = files
        .iter()
        .map(|(path, bytes)| format!("{}  {path}\n", buglogin_storage::blob::checksum(bytes)))
        .collect::<String>();
    files.insert("checksums.sha256".to_owned(), checksums.into_bytes());
    Ok(NeutralPackage { files })
}

fn preservation_export_allowed(report: &InspectReport) -> bool {
    matches!(
        (
            report.source.system.as_str(),
            report.source.version.as_str()
        ),
        ("buglogin-legacy", "1.1.84")
            | ("donutbrowser", "0.28.2")
            | ("neutral-export", "buglogin.export/1")
    ) && report.invalid_records.is_empty()
        && report.dangling_references.is_empty()
        && report.runtime_findings.iter().all(|finding| {
            matches!(
                finding.class,
                MigrationClass::Direct
                    | MigrationClass::Partial
                    | MigrationClass::Unsupported
                    | MigrationClass::Unknown
            )
        })
}

fn validate_profile_resource_bindings(
    profiles: &[crate::legacy::NeutralProfileRecord],
    control_plane: &crate::legacy::LegacyControlPlaneMapping,
) -> Result<(), Problem> {
    let profile_ids = unique_ids(
        profiles.iter().map(|record| record.id.as_str()),
        "duplicate packaged profile ID",
    )?;
    let workspace_ids = unique_ids(
        control_plane
            .workspaces
            .iter()
            .map(|record| record.id.as_str()),
        "duplicate packaged workspace ID",
    )?;
    let resource_ids = unique_ids(
        control_plane
            .resources
            .iter()
            .map(|record| record.id.as_str()),
        "duplicate packaged resource ID",
    )?;
    let resource_workspaces = control_plane
        .resources
        .iter()
        .map(|record| {
            if record.schema != "buglogin.resource/1" || record.data.kind != "profile" {
                return Err(binding_invalid("malformed packaged profile resource"));
            }
            Ok((record.id.as_str(), record.data.workspace_id.as_str()))
        })
        .collect::<Result<BTreeMap<_, _>, Problem>>()?;
    let mut binding_ids = BTreeMap::new();
    let mut bound_profiles = BTreeMap::new();
    let mut bound_resources = BTreeMap::new();
    for binding in &control_plane.profile_resources {
        if binding.schema != "buglogin.profile-resource/1" || binding.data.archive_state != "active"
        {
            return Err(binding_invalid(
                "malformed packaged profile resource binding",
            ));
        }
        if binding_ids.insert(&binding.id, ()).is_some() {
            return Err(binding_invalid("duplicate profile resource binding ID"));
        }
        if bound_profiles
            .insert(&binding.data.profile_id, &binding.data.resource_id)
            .is_some()
            || bound_resources
                .insert(&binding.data.resource_id, &binding.data.profile_id)
                .is_some()
        {
            return Err(binding_invalid("ambiguous profile resource binding"));
        }
        if !profile_ids.contains_key(binding.data.profile_id.as_str()) {
            return Err(binding_invalid(
                "dangling profile resource profile reference",
            ));
        }
        if !resource_ids.contains_key(binding.data.resource_id.as_str()) {
            return Err(binding_invalid(
                "dangling profile resource resource reference",
            ));
        }
        if !workspace_ids.contains_key(binding.data.workspace_id.as_str()) {
            return Err(binding_invalid(
                "dangling profile resource workspace reference",
            ));
        }
        if resource_workspaces
            .get(binding.data.resource_id.as_str())
            .copied()
            != Some(binding.data.workspace_id.as_str())
        {
            return Err(binding_invalid("profile resource workspace mismatch"));
        }
    }
    if control_plane.profile_resources.len() != control_plane.resources.len() {
        return Err(binding_invalid("profile resource binding set mismatch"));
    }
    Ok(())
}

fn unique_ids<'a>(
    ids: impl Iterator<Item = &'a str>,
    field: &'static str,
) -> Result<BTreeMap<&'a str, ()>, Problem> {
    let mut unique = BTreeMap::new();
    for id in ids {
        if id.is_empty() || unique.insert(id, ()).is_some() {
            return Err(binding_invalid(field));
        }
    }
    Ok(unique)
}

fn binding_invalid(field: &'static str) -> Problem {
    Problem::new(
        "migration_profile_resource_invalid",
        "Migration profile resource binding is invalid",
        422,
        "migration",
    )
    .with_field("field", field)
}
