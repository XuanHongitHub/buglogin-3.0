use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use buglogin_domain::errors::Problem;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::legacy::{
    LegacyProxyRowStatus, analyze_legacy_proxy_batch, inspect_legacy_profile_reference_warnings,
    map_legacy_extensions,
};
use crate::snapshot::{SourceSnapshot, create_snapshot};

const SOURCE_METADATA_FILE: &str = "version.json";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceIdentity {
    pub system: String,
    pub version: String,
    pub instance_id: String,
    pub os: String,
    pub architecture: String,
    pub root: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationClass {
    Direct,
    Partial,
    Unsupported,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeFinding {
    pub runtime: String,
    pub class: MigrationClass,
    pub reason_code: String,
    pub records: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordIssue {
    pub source_ref: String,
    pub code: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretScan {
    pub files_with_secret_fields: u64,
    pub secret_fields_present: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CutoverEligibility {
    Eligible,
    Blocked,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectReport {
    pub schema: String,
    pub source: SourceIdentity,
    pub entity_counts: BTreeMap<String, u64>,
    pub total_bytes: u64,
    pub runtime_findings: Vec<RuntimeFinding>,
    pub secret_scan: SecretScan,
    pub invalid_records: Vec<RecordIssue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<RecordIssue>,
    pub dangling_references: Vec<RecordIssue>,
    pub source_tree_sha256: String,
    pub cutover_eligibility: CutoverEligibility,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceMetadata {
    system: String,
    version: String,
    instance_id: String,
    os: Option<String>,
    architecture: Option<String>,
}

pub fn inspect_source(source_root: &Path) -> Result<(InspectReport, SourceSnapshot), Problem> {
    let source = read_source_identity(source_root)?;
    inspect_source_with_identity(source_root, source)
}

pub fn inspect_source_with_identity(
    source_root: &Path,
    source: SourceIdentity,
) -> Result<(InspectReport, SourceSnapshot), Problem> {
    let snapshot = create_snapshot(source_root)?;
    let mut entity_counts = BTreeMap::new();
    let mut runtime_counts = BTreeMap::new();
    let mut secret_scan = SecretScan::default();
    let mut invalid_records = Vec::new();
    let mut warnings = Vec::new();
    let mut dangling_references = Vec::new();

    inspect_profile_directory_metadata(source_root, &mut invalid_records)?;

    for file in &snapshot.files {
        if is_runtime_residue(&file.path) {
            continue;
        }
        count_entity(&file.path, &mut entity_counts);
        if !file.path.ends_with(".json") || file.path == SOURCE_METADATA_FILE {
            continue;
        }
        let bytes =
            fs::read(source_root.join(file.path.replace('/', std::path::MAIN_SEPARATOR_STR)))
                .map_err(|_| source_read_failed())?;
        let value: Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => {
                if !file.path.starts_with("proxies/") {
                    invalid_records.push(RecordIssue {
                        source_ref: file.path.clone(),
                        code: "invalid_json".to_owned(),
                    });
                }
                continue;
            }
        };
        if file.path == "vpn/vpn_configs.json" {
            inspect_vpn_inventory(&file.path, &value, &mut entity_counts, &mut invalid_records);
        }
        let before = secret_scan.secret_fields_present;
        scan_value(
            &value,
            is_profile_metadata(&file.path, "profiles"),
            &mut runtime_counts,
            &mut secret_scan,
        );
        if secret_scan.secret_fields_present > before {
            secret_scan.files_with_secret_fields += 1;
        }
    }
    let proxy_analysis = analyze_legacy_proxy_batch(source_root, &source.instance_id)?;
    invalid_records.extend(
        proxy_analysis
            .rows
            .into_iter()
            .filter(|row| row.status == LegacyProxyRowStatus::Invalid)
            .map(|row| RecordIssue {
                source_ref: row.source_ref,
                code: row
                    .code
                    .unwrap_or_else(|| "migration_entity_invalid".to_owned()),
            }),
    );
    invalid_records.sort_by(|left, right| {
        left.source_ref
            .cmp(&right.source_ref)
            .then_with(|| left.code.cmp(&right.code))
    });
    warnings.extend(
        inspect_legacy_profile_reference_warnings(source_root)?
            .into_iter()
            .map(|warning| RecordIssue {
                source_ref: format!("{}#{}", warning.source_ref, warning.field),
                code: "migration_optional_reference_dropped".to_owned(),
            }),
    );
    if let Err(error) = map_legacy_extensions(source_root, &source.instance_id)
        && error
            .fields
            .get("field")
            .and_then(Value::as_str)
            .is_some_and(|field| field == "dangling extension group assignment")
    {
        dangling_references.push(RecordIssue {
            source_ref: "extension_groups.json".to_owned(),
            code: "migration_reference_missing".to_owned(),
        });
    }

    let verified_snapshot = create_snapshot(source_root)?;
    ensure_snapshot_integrity(&snapshot, &verified_snapshot)?;

    let runtime_findings = runtime_counts
        .into_iter()
        .map(|(runtime, records)| classify_runtime(runtime, records))
        .collect::<Vec<_>>();
    let supported_source = matches!(
        (source.system.as_str(), source.version.as_str()),
        ("buglogin-legacy", "1.1.84")
            | ("donutbrowser", "0.28.2")
            | ("neutral-export", "buglogin.export/1")
    );
    let blocked_runtime = runtime_findings
        .iter()
        .any(|finding| finding.class != MigrationClass::Direct);
    let cutover_eligibility = if supported_source
        && invalid_records.is_empty()
        && dangling_references.is_empty()
        && !blocked_runtime
    {
        CutoverEligibility::Eligible
    } else {
        CutoverEligibility::Blocked
    };

    Ok((
        InspectReport {
            schema: "buglogin.inspect/1".to_owned(),
            source,
            entity_counts,
            total_bytes: snapshot.total_bytes,
            runtime_findings,
            secret_scan,
            invalid_records,
            warnings,
            dangling_references,
            source_tree_sha256: snapshot.source_tree_sha256.clone(),
            cutover_eligibility,
        },
        snapshot,
    ))
}

fn ensure_snapshot_integrity(
    expected: &SourceSnapshot,
    actual: &SourceSnapshot,
) -> Result<(), Problem> {
    if expected.has_same_integrity(actual) {
        return Ok(());
    }
    Err(Problem::new(
        "migration_source_changed",
        "Migration source changed during inspection",
        409,
        "migration",
    )
    .retryable(true))
}

#[doc(hidden)]
pub fn ensure_snapshot_integrity_for_test(
    expected: &SourceSnapshot,
    actual: &SourceSnapshot,
) -> Result<(), Problem> {
    ensure_snapshot_integrity(expected, actual)
}

fn inspect_profile_directory_metadata(
    source_root: &Path,
    invalid_records: &mut Vec<RecordIssue>,
) -> Result<(), Problem> {
    let profiles_root = source_root.join("profiles");
    if !profiles_root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&profiles_root).map_err(|_| source_read_failed())? {
        let entry = entry.map_err(|_| source_read_failed())?;
        let file_type = entry.file_type().map_err(|_| source_read_failed())?;
        if file_type.is_symlink() || !file_type.is_dir() {
            continue;
        }
        let metadata = entry.path().join("metadata.json");
        if !metadata.is_file() {
            let source_id = entry.file_name().to_string_lossy().into_owned();
            invalid_records.push(RecordIssue {
                source_ref: format!("profiles/{source_id}/metadata.json"),
                code: "profile_metadata_missing".to_owned(),
            });
        }
    }
    Ok(())
}

fn read_source_identity(source_root: &Path) -> Result<SourceIdentity, Problem> {
    let bytes = fs::read(source_root.join(SOURCE_METADATA_FILE)).map_err(|_| {
        Problem::new(
            "migration_source_version_missing",
            "Migration source version metadata is missing",
            422,
            "migration",
        )
    })?;
    let metadata: SourceMetadata = serde_json::from_slice(&bytes).map_err(|_| {
        Problem::new(
            "migration_source_version_invalid",
            "Migration source version metadata is invalid",
            422,
            "migration",
        )
    })?;
    if metadata.system.is_empty() || metadata.version.is_empty() || metadata.instance_id.is_empty()
    {
        return Err(Problem::new(
            "migration_source_identity_invalid",
            "Migration source identity is invalid",
            422,
            "migration",
        ));
    }
    Ok(SourceIdentity {
        system: metadata.system,
        version: metadata.version,
        instance_id: format!(
            "sha256:{}",
            buglogin_storage::blob::checksum(metadata.instance_id.as_bytes())
        ),
        os: metadata.os.unwrap_or_else(|| "unknown".to_owned()),
        architecture: metadata
            .architecture
            .unwrap_or_else(|| "unknown".to_owned()),
        root: ".".to_owned(),
    })
}

fn count_entity(path: &str, counts: &mut BTreeMap<String, u64>) {
    let Some(root) = path.split('/').next() else {
        return;
    };
    let entity = match root {
        "profiles" if is_profile_metadata(path, "profiles") => "profiles",
        "profiles-deleted" if is_profile_metadata(path, "profiles-deleted") => "profilesDeleted",
        "groups.json" => "groups",
        "proxies" if path.ends_with(".json") => "proxies",
        "extensions" if path.ends_with(".json") => "extensions",
        _ => return,
    };
    *counts.entry(entity.to_owned()).or_default() += 1;
}

fn is_profile_metadata(path: &str, root: &str) -> bool {
    let mut parts = path.split('/');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(actual_root), Some(_), Some("metadata.json"), None) if actual_root == root
    )
}

fn is_runtime_residue(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .is_some_and(|name| name.starts_with("vpn_worker_") && name.ends_with(".json"))
}

fn inspect_vpn_inventory(
    path: &str,
    value: &Value,
    counts: &mut BTreeMap<String, u64>,
    invalid_records: &mut Vec<RecordIssue>,
) {
    let Some(object) = value.as_object() else {
        invalid_records.push(record_issue(path, "vpn_storage_invalid"));
        return;
    };
    if object.get("version").and_then(Value::as_u64) != Some(1) {
        invalid_records.push(record_issue(path, "vpn_version_unsupported"));
    }
    let Some(configs) = object.get("configs").and_then(Value::as_array) else {
        invalid_records.push(record_issue(path, "vpn_storage_invalid"));
        return;
    };

    counts.insert("vpns".to_owned(), configs.len() as u64);
    for (index, config) in configs.iter().enumerate() {
        if !valid_vpn_record(config) {
            invalid_records.push(record_issue(
                &format!("{path}#configs/{index}"),
                "vpn_record_invalid",
            ));
        }
    }
}

fn valid_vpn_record(value: &Value) -> bool {
    let Some(record) = value.as_object() else {
        return false;
    };
    non_empty_string(record.get("id"))
        && non_empty_string(record.get("name"))
        && matches!(
            record.get("vpn_type").and_then(Value::as_str),
            Some("WireGuard" | "OpenVPN")
        )
        && non_empty_string(record.get("encrypted_data"))
        && non_empty_string(record.get("nonce"))
        && record.get("created_at").and_then(Value::as_i64).is_some()
        && optional_i64(record.get("last_used"))
        && optional_bool(record.get("sync_enabled"))
        && optional_u64(record.get("last_sync"))
}

fn non_empty_string(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

fn optional_i64(value: Option<&Value>) -> bool {
    value.is_none_or(|value| value.is_null() || value.as_i64().is_some())
}

fn optional_u64(value: Option<&Value>) -> bool {
    value.is_none_or(|value| value.is_null() || value.as_u64().is_some())
}

fn optional_bool(value: Option<&Value>) -> bool {
    value.is_none_or(|value| value.as_bool().is_some())
}

fn record_issue(source_ref: &str, code: &str) -> RecordIssue {
    RecordIssue {
        source_ref: source_ref.to_owned(),
        code: code.to_owned(),
    }
}

fn scan_value(
    value: &Value,
    count_runtimes: bool,
    runtimes: &mut BTreeMap<String, u64>,
    secrets: &mut SecretScan,
) {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let normalized = key.to_ascii_lowercase().replace(['-', '_'], "");
                if is_secret_key(&normalized) && !child.is_null() {
                    secrets.secret_fields_present += 1;
                }
                if count_runtimes && matches!(normalized.as_str(), "runtime" | "browser") {
                    if let Some(runtime) = child.as_str() {
                        *runtimes.entry(runtime_report_key(runtime)).or_default() += 1;
                    }
                }
                scan_value(child, count_runtimes, runtimes, secrets);
            }
        }
        Value::Array(values) => {
            for child in values {
                scan_value(child, count_runtimes, runtimes, secrets);
            }
        }
        _ => {}
    }
}

fn is_secret_key(key: &str) -> bool {
    const SECRET_KEYS: [&str; 10] = [
        "password",
        "proxypassword",
        "token",
        "accesstoken",
        "refreshtoken",
        "apikey",
        "secret",
        "privatekey",
        "cookie",
        "cookies",
    ];
    SECRET_KEYS.into_iter().any(|candidate| key == candidate)
}

fn runtime_report_key(runtime: &str) -> String {
    let normalized = runtime.to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "wayfern"
            | "camoufox"
            | "bugox"
            | "bugium"
            | "bugmini"
            | "bugthor"
            | "bughelium"
            | "cloak"
            | "cloakbrowser"
            | "thorium"
            | "helium"
    ) {
        normalized
    } else {
        format!(
            "unknown:sha256:{}",
            &buglogin_storage::blob::checksum(runtime.as_bytes())[..12]
        )
    }
}

fn classify_runtime(runtime: String, records: u64) -> RuntimeFinding {
    let (class, reason_code) = match runtime.as_str() {
        "wayfern" => (MigrationClass::Direct, "runtime_supported"),
        "camoufox" => (MigrationClass::Unsupported, "runtime_unsupported"),
        "bugmini" => (
            MigrationClass::Partial,
            "runtime_adapter_available_artifact_pending",
        ),
        "bugox" | "bugium" | "bugthor" | "bughelium" | "cloak" | "cloakbrowser" | "thorium"
        | "helium" => (MigrationClass::Unsupported, "runtime_adapter_not_approved"),
        _ => (MigrationClass::Unknown, "runtime_unknown"),
    };
    RuntimeFinding {
        runtime,
        class,
        reason_code: reason_code.to_owned(),
        records,
    }
}

fn source_read_failed() -> Problem {
    Problem::new(
        "migration_source_read_failed",
        "Migration source could not be read",
        503,
        "migration",
    )
    .retryable(true)
}
