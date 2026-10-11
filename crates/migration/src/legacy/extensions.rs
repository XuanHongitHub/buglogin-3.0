use std::{collections::BTreeSet, fs, path::Path};

use buglogin_domain::{errors::Problem, ids::migration_uuid};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::ProfileSourceRef;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct NeutralExtensionData {
    pub name: String,
    pub file_name: String,
    pub package_type: String,
    pub artifact_ref: String,
    pub artifact_sha256: String,
    pub compatibility: Vec<String>,
    pub sync_mode: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct NeutralExtensionRecord {
    pub schema: String,
    pub source_ref: ProfileSourceRef,
    pub id: String,
    pub revision: u32,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub data: NeutralExtensionData,
    pub extensions: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct NeutralExtensionGroupData {
    pub name: String,
    pub extension_ids: Vec<String>,
    pub sync_mode: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct NeutralExtensionGroupRecord {
    pub schema: String,
    pub source_ref: ProfileSourceRef,
    pub id: String,
    pub revision: u32,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub data: NeutralExtensionGroupData,
    pub extensions: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionArtifact {
    pub artifact_ref: String,
    pub package_path: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyExtensionExport {
    pub records: Vec<NeutralExtensionRecord>,
    pub groups: Vec<NeutralExtensionGroupRecord>,
    pub artifacts: Vec<ExtensionArtifact>,
}

pub fn map_legacy_extensions(
    source_root: &Path,
    source_instance_id: &str,
) -> Result<LegacyExtensionExport, Problem> {
    let root = source_root.join("extensions");
    if !root.exists() {
        return Ok(LegacyExtensionExport {
            records: Vec::new(),
            groups: map_groups(source_root, source_instance_id, &BTreeSet::new())?,
            artifacts: Vec::new(),
        });
    }
    reject_symlink(&root)?;
    let mut directories = fs::read_dir(&root)
        .map_err(io_problem)?
        .map(|entry| entry.map(|entry| entry.path()).map_err(io_problem))
        .collect::<Result<Vec<_>, _>>()?;
    directories.sort();
    let mut records = Vec::new();
    let mut artifacts = Vec::new();
    let mut source_ids = BTreeSet::new();
    for directory in directories {
        let metadata = fs::symlink_metadata(&directory).map_err(io_problem)?;
        if metadata.file_type().is_symlink() {
            return Err(invalid("extension symlink"));
        }
        if !metadata.is_dir() {
            continue;
        }
        let source_id = directory
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| invalid("extension directory"))?;
        if !source_ids.insert(source_id.to_owned()) {
            return Err(invalid("duplicate extension ID"));
        }
        let metadata_path = directory.join("metadata.json");
        reject_symlink(&metadata_path)?;
        let value: Value = serde_json::from_slice(&fs::read(&metadata_path).map_err(io_problem)?)
            .map_err(|_| invalid("extension metadata"))?;
        let object = value
            .as_object()
            .ok_or_else(|| invalid("extension metadata"))?;
        if required_string(object, "id")? != source_id {
            return Err(invalid("extension ID mismatch"));
        }
        let file_name = required_string(object, "file_name")?;
        if Path::new(file_name)
            .file_name()
            .and_then(|value| value.to_str())
            != Some(file_name)
        {
            return Err(invalid("extension filename"));
        }
        let package_type = required_string(object, "file_type")?.to_ascii_lowercase();
        if !matches!(package_type.as_str(), "xpi" | "crx" | "zip") {
            return Err(invalid("extension package type"));
        }
        let artifact_path = directory.join("file").join(file_name);
        reject_symlink(&artifact_path)?;
        let bytes = fs::read(&artifact_path).map_err(io_problem)?;
        if bytes.is_empty() {
            return Err(invalid("extension artifact"));
        }
        let artifact_sha256 = buglogin_storage::blob::checksum(&bytes);
        let artifact_ref = mapped_id(source_instance_id, "extension-artifact", source_id);
        let extension_id = mapped_id(source_instance_id, "extension", source_id);
        records.push(NeutralExtensionRecord {
            schema: "buglogin.extension/1".into(),
            source_ref: source_ref(source_instance_id, source_id),
            id: extension_id,
            revision: 1,
            created_at: unix_timestamp(object.get("created_at"))?,
            updated_at: unix_timestamp(object.get("updated_at"))?,
            data: NeutralExtensionData {
                name: required_string(object, "name")?.into(),
                file_name: file_name.into(),
                package_type,
                artifact_ref: artifact_ref.clone(),
                artifact_sha256: artifact_sha256.clone(),
                compatibility: compatibility(object.get("browser_compatibility"))?,
                sync_mode: sync_mode(object),
            },
            extensions: Map::new(),
        });
        artifacts.push(ExtensionArtifact {
            artifact_ref: artifact_ref.clone(),
            package_path: format!("artifacts/extensions/{artifact_ref}/{artifact_sha256}"),
            bytes,
        });
    }
    records.sort_by(|left, right| left.source_ref.id.cmp(&right.source_ref.id));
    artifacts.sort_by(|left, right| left.artifact_ref.cmp(&right.artifact_ref));
    let groups = map_groups(source_root, source_instance_id, &source_ids)?;
    Ok(LegacyExtensionExport {
        records,
        groups,
        artifacts,
    })
}

fn map_groups(
    source_root: &Path,
    source_instance_id: &str,
    extension_source_ids: &BTreeSet<String>,
) -> Result<Vec<NeutralExtensionGroupRecord>, Problem> {
    let path = [
        source_root.join("data").join("extension_groups.json"),
        source_root.join("extension_groups.json"),
    ]
    .into_iter()
    .find(|path| path.exists());
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    reject_symlink(&path)?;
    let value: Value = serde_json::from_slice(&fs::read(path).map_err(io_problem)?)
        .map_err(|_| invalid("extension groups"))?;
    let groups = value
        .get("groups")
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
        .ok_or_else(|| invalid("extension groups"))?;
    let mut records = Vec::new();
    let mut ids = BTreeSet::new();
    for value in groups {
        let object = value
            .as_object()
            .ok_or_else(|| invalid("extension group"))?;
        let source_id = required_string(object, "id")?;
        if !ids.insert(source_id) {
            return Err(invalid("duplicate extension group ID"));
        }
        let mut extension_ids = object
            .get("extension_ids")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("extension group assignments"))?
            .iter()
            .map(|value| {
                let source_extension_id = value
                    .as_str()
                    .ok_or_else(|| invalid("extension group assignment"))?;
                if !extension_source_ids.contains(source_extension_id) {
                    return Err(invalid("dangling extension group assignment"));
                }
                Ok(mapped_id(
                    source_instance_id,
                    "extension",
                    source_extension_id,
                ))
            })
            .collect::<Result<Vec<_>, Problem>>()?;
        extension_ids.sort();
        extension_ids.dedup();
        records.push(NeutralExtensionGroupRecord {
            schema: "buglogin.extension-group/1".into(),
            source_ref: source_ref(source_instance_id, source_id),
            id: mapped_id(source_instance_id, "extension-group", source_id),
            revision: 1,
            created_at: unix_timestamp(object.get("created_at"))?,
            updated_at: unix_timestamp(object.get("updated_at"))?,
            data: NeutralExtensionGroupData {
                name: required_string(object, "name")?.into(),
                extension_ids,
                sync_mode: sync_mode(object),
            },
            extensions: Map::new(),
        });
    }
    records.sort_by(|left, right| left.source_ref.id.cmp(&right.source_ref.id));
    Ok(records)
}

fn compatibility(value: Option<&Value>) -> Result<Vec<String>, Problem> {
    let mut values = value
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("extension compatibility"))?
        .iter()
        .map(|value| {
            let value = value
                .as_str()
                .ok_or_else(|| invalid("extension compatibility"))?
                .to_ascii_lowercase();
            if matches!(value.as_str(), "firefox" | "chromium") {
                Ok(value)
            } else {
                Err(invalid("extension compatibility"))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    values.sort();
    values.dedup();
    if values.is_empty() {
        return Err(invalid("extension compatibility"));
    }
    Ok(values)
}

fn sync_mode(object: &Map<String, Value>) -> String {
    if object
        .get("sync_enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "managed".into()
    } else {
        "local".into()
    }
}

fn unix_timestamp(value: Option<&Value>) -> Result<Option<String>, Problem> {
    let Some(seconds) = value.and_then(Value::as_i64) else {
        return Ok(None);
    };
    OffsetDateTime::from_unix_timestamp(seconds)
        .map_err(|_| invalid("extension timestamp"))?
        .format(&Rfc3339)
        .map(Some)
        .map_err(|_| invalid("extension timestamp"))
}

fn required_string<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str, Problem> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid(key))
}

fn source_ref(instance_id: &str, source_id: &str) -> ProfileSourceRef {
    ProfileSourceRef {
        system: "buglogin-legacy".into(),
        instance_id: instance_id.into(),
        id: source_id.into(),
    }
}

fn mapped_id(instance_id: &str, entity: &str, source_id: &str) -> String {
    migration_uuid("buglogin-legacy", instance_id, entity, source_id).to_string()
}

fn reject_symlink(path: &Path) -> Result<(), Problem> {
    if fs::symlink_metadata(path)
        .map_err(io_problem)?
        .file_type()
        .is_symlink()
    {
        Err(invalid("symlink"))
    } else {
        Ok(())
    }
}

fn invalid(field: &str) -> Problem {
    Problem::new(
        "migration_extension_invalid",
        "Legacy extension migration data is invalid",
        422,
        "migration",
    )
    .with_field("field", field)
}

fn io_problem(error: std::io::Error) -> Problem {
    Problem::new(
        "migration_extension_read_failed",
        "Legacy extension migration data could not be read",
        500,
        "migration",
    )
    .with_detail(error.to_string())
}
