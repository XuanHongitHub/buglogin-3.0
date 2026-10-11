use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use buglogin_domain::errors::Problem;
use buglogin_domain::ids::migration_uuid;
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use url::Url;

use super::ProfileSourceRef;
use crate::encrypted_secrets::ProxyCredentialSecret;

pub struct LegacyProxyExport {
    pub records: Vec<NeutralProxyRecord>,
    pub credentials: Vec<ProxyCredentialSecret>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyProxyRowStatus {
    Valid,
    Invalid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyProxyRowResult {
    pub source_ref: String,
    pub source_id: Option<String>,
    pub status: LegacyProxyRowStatus,
    pub code: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyProxyBatchAnalysis {
    pub schema: String,
    pub policy: String,
    pub valid_records: u64,
    pub invalid_records: u64,
    pub rows: Vec<LegacyProxyRowResult>,
}

struct ParsedProxyRow {
    source_ref: String,
    source_id: Option<String>,
    result: Result<(NeutralProxyRecord, Option<ProxyCredentialSecret>), Problem>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct NeutralGroupData {
    pub name: String,
    pub appearance: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct NeutralGroupRecord {
    pub schema: String,
    pub source_ref: ProfileSourceRef,
    pub id: String,
    pub revision: u32,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub data: NeutralGroupData,
    pub extensions: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct NeutralProxyData {
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub credential_ref: Option<String>,
    pub rotation_url: Option<String>,
    pub bypass_rules: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct NeutralProxyRecord {
    pub schema: String,
    pub source_ref: ProfileSourceRef,
    pub id: String,
    pub revision: u32,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub data: NeutralProxyData,
    pub extensions: Map<String, Value>,
}

pub fn map_legacy_groups(
    source_root: &Path,
    source_instance_id: &str,
) -> Result<Vec<NeutralGroupRecord>, Problem> {
    let path = source_root.join("groups.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    reject_symlink(&path)?;
    let value: Value = serde_json::from_slice(&fs::read(&path).map_err(io_problem)?)
        .map_err(|_| invalid("groups.json"))?;
    let groups = value
        .as_array()
        .or_else(|| value.get("groups").and_then(Value::as_array))
        .ok_or_else(|| invalid("groups.json"))?;
    let mut records = groups
        .iter()
        .map(|value| map_group(value, source_instance_id))
        .collect::<Result<Vec<_>, _>>()?;
    records.sort_by(|left, right| left.source_ref.id.cmp(&right.source_ref.id));
    Ok(records)
}

pub fn map_legacy_proxies(
    source_root: &Path,
    source_instance_id: &str,
) -> Result<Vec<NeutralProxyRecord>, Problem> {
    Ok(map_proxy_export(source_root, source_instance_id, false)?.records)
}

pub fn map_legacy_proxies_for_encrypted_package(
    source_root: &Path,
    source_instance_id: &str,
) -> Result<LegacyProxyExport, Problem> {
    map_proxy_export(source_root, source_instance_id, true)
}

pub fn analyze_legacy_proxy_batch(
    source_root: &Path,
    source_instance_id: &str,
) -> Result<LegacyProxyBatchAnalysis, Problem> {
    let rows = parse_proxy_rows(source_root, source_instance_id, true)?;
    let rows = rows
        .into_iter()
        .map(|row| match row.result {
            Ok((record, _)) => LegacyProxyRowResult {
                source_ref: row.source_ref,
                source_id: Some(record.source_ref.id),
                status: LegacyProxyRowStatus::Valid,
                code: None,
            },
            Err(error) => LegacyProxyRowResult {
                source_ref: row.source_ref,
                source_id: row.source_id,
                status: LegacyProxyRowStatus::Invalid,
                code: Some(proxy_row_code(&error)),
            },
        })
        .collect::<Vec<_>>();
    let valid_records = rows
        .iter()
        .filter(|row| row.status == LegacyProxyRowStatus::Valid)
        .count() as u64;
    // ponytail: migration imports stay atomic; add exclusion-ledger reconciliation before valid-only commits.
    Ok(LegacyProxyBatchAnalysis {
        schema: "buglogin.proxy-batch-analysis/1".into(),
        policy: "atomic".into(),
        valid_records,
        invalid_records: rows.len() as u64 - valid_records,
        rows,
    })
}

fn map_proxy_export(
    source_root: &Path,
    source_instance_id: &str,
    allow_credentials: bool,
) -> Result<LegacyProxyExport, Problem> {
    let rows = parse_proxy_rows(source_root, source_instance_id, allow_credentials)?;
    let mut records = Vec::new();
    let mut credentials = Vec::new();
    for row in rows {
        let (record, credential) = row.result?;
        records.push(record);
        credentials.extend(credential);
    }
    records.sort_by(|left, right| left.source_ref.id.cmp(&right.source_ref.id));
    credentials.sort_by(|left, right| left.credential_id.cmp(&right.credential_id));
    Ok(LegacyProxyExport {
        records,
        credentials,
    })
}

fn parse_proxy_rows(
    source_root: &Path,
    source_instance_id: &str,
    allow_credentials: bool,
) -> Result<Vec<ParsedProxyRow>, Problem> {
    let root = source_root.join("proxies");
    if !root.exists() {
        return Ok(Vec::new());
    }
    reject_symlink(&root)?;
    let mut paths = fs::read_dir(&root)
        .map_err(io_problem)?
        .map(|entry| entry.map(|entry| entry.path()).map_err(io_problem))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    let mut rows = Vec::new();
    for path in paths {
        let metadata = fs::symlink_metadata(&path).map_err(io_problem)?;
        if metadata.file_type().is_symlink() {
            return Err(invalid("proxies symlink"));
        }
        if !metadata.is_file() || path.extension().and_then(|value| value.to_str()) != Some("json")
        {
            continue;
        }
        let source_ref = format!(
            "proxies/{}",
            path.file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| invalid("proxy filename"))?
        );
        let bytes = fs::read(&path).map_err(io_problem)?;
        let fallback_id = path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| invalid("proxy filename"))?;
        let value = serde_json::from_slice::<Value>(&bytes).map_err(|_| invalid("proxy JSON"));
        let source_id = value.as_ref().ok().and_then(|value| {
            value
                .as_object()
                .and_then(|object| object.get("id"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
        let result = value.and_then(|value| {
            map_proxy(&value, fallback_id, source_instance_id, allow_credentials)
        });
        rows.push(ParsedProxyRow {
            source_ref,
            source_id,
            result,
        });
    }
    let mut counts = BTreeMap::<String, usize>::new();
    for row in &rows {
        if let Ok((record, _)) = &row.result {
            *counts.entry(record.source_ref.id.clone()).or_default() += 1;
        }
    }
    for row in &mut rows {
        let duplicate_id = row.result.as_ref().ok().and_then(|(record, _)| {
            (counts.get(&record.source_ref.id).copied() > Some(1))
                .then(|| record.source_ref.id.clone())
        });
        if let Some(id) = duplicate_id {
            row.result = Err(Problem::new(
                "migration_proxy_duplicate_source_id",
                "Legacy proxy source ID is duplicated",
                422,
                "migration",
            )
            .with_field("sourceId", id));
        }
    }
    Ok(rows)
}

fn proxy_row_code(error: &Problem) -> String {
    if error.code != "migration_entity_invalid" {
        return error.code.clone();
    }
    match error.fields.get("field").and_then(Value::as_str) {
        Some("proxy JSON") => "migration_proxy_json_invalid",
        Some("proxy ID") => "migration_proxy_id_invalid",
        Some("proxy protocol") => "migration_proxy_protocol_invalid",
        Some("proxy host") => "migration_proxy_host_invalid",
        Some("proxy port") => "migration_proxy_port_invalid",
        Some("proxy URL") => "migration_proxy_url_invalid",
        Some("proxy credential") => "migration_proxy_credential_invalid",
        _ => "migration_proxy_record_invalid",
    }
    .to_owned()
}

fn map_group(value: &Value, source_instance_id: &str) -> Result<NeutralGroupRecord, Problem> {
    let object = value.as_object().ok_or_else(|| invalid("group"))?;
    let source_id = required_string(object, "id", "group")?;
    let name = required_string(object, "name", "group")?;
    let appearance = object.get("appearance").cloned().or_else(|| {
        object
            .get("color")
            .cloned()
            .map(|color| serde_json::json!({ "color": color }))
    });
    Ok(NeutralGroupRecord {
        schema: "buglogin.group/1".into(),
        source_ref: source_ref(source_instance_id, source_id),
        id: mapped_id(source_instance_id, "group", source_id),
        revision: 1,
        created_at: timestamp(object, "created_at", "createdAt"),
        updated_at: timestamp(object, "updated_at", "updatedAt"),
        data: NeutralGroupData {
            name: name.into(),
            appearance,
        },
        extensions: Map::new(),
    })
}

fn map_proxy(
    value: &Value,
    fallback_id: &str,
    source_instance_id: &str,
    allow_credentials: bool,
) -> Result<(NeutralProxyRecord, Option<ProxyCredentialSecret>), Problem> {
    let object = value.as_object().ok_or_else(|| invalid("proxy"))?;
    let source_id = object
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(fallback_id);
    if source_id.trim().is_empty() {
        return Err(invalid("proxy ID"));
    }
    let (protocol, host, port, bypass_rules, username, password) = if let Some(settings) = object
        .get("proxy_settings")
        .or_else(|| object.get("proxySettings"))
        .and_then(Value::as_object)
    {
        (
            protocol(required_string(settings, "proxy_type", "proxy protocol")?)?,
            required_string(settings, "host", "proxy host")?.to_owned(),
            required_port(settings.get("port"))?,
            string_array(
                object
                    .get("bypass_rules")
                    .or_else(|| object.get("bypassRules")),
            ),
            optional_credential(settings, "username")?,
            optional_credential(settings, "password")?,
        )
    } else {
        let upstream = required_string(object, "upstream_url", "proxy URL")?;
        if upstream.eq_ignore_ascii_case("DIRECT") {
            return Err(problem(
                "migration_direct_proxy_unsupported",
                "Direct proxy records are not migrated as proxy entities",
            ));
        }
        let parsed = Url::parse(upstream).map_err(|_| invalid("proxy URL"))?;
        (
            protocol(parsed.scheme())?,
            parsed
                .host_str()
                .ok_or_else(|| invalid("proxy host"))?
                .to_owned(),
            parsed.port().ok_or_else(|| invalid("proxy port"))?,
            string_array(
                object
                    .get("bypass_rules")
                    .or_else(|| object.get("bypassRules")),
            ),
            if parsed.username().is_empty() {
                None
            } else {
                Some(decode_url_credential(parsed.username())?)
            },
            parsed.password().map(decode_url_credential).transpose()?,
        )
    };
    if host.trim().is_empty() {
        return Err(invalid("proxy host"));
    }
    let has_credentials = username.is_some() || password.is_some();
    if has_credentials && !allow_credentials {
        return Err(credentials_problem());
    }
    let credential_id =
        has_credentials.then(|| mapped_id(source_instance_id, "proxy-credential", source_id));
    let record = NeutralProxyRecord {
        schema: "buglogin.proxy/1".into(),
        source_ref: source_ref(source_instance_id, source_id),
        id: mapped_id(source_instance_id, "proxy", source_id),
        revision: 1,
        created_at: timestamp(object, "created_at", "createdAt"),
        updated_at: timestamp(object, "updated_at", "updatedAt"),
        data: NeutralProxyData {
            protocol,
            host,
            port,
            credential_ref: credential_id.clone(),
            rotation_url: None,
            bypass_rules,
        },
        extensions: Map::new(),
    };
    let credential = credential_id.map(|credential_id| ProxyCredentialSecret {
        credential_id,
        username,
        password,
    });
    Ok((record, credential))
}

fn optional_credential(object: &Map<String, Value>, key: &str) -> Result<Option<String>, Problem> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.is_empty() => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(invalid("proxy credential")),
    }
}

fn decode_url_credential(value: &str) -> Result<String, Problem> {
    percent_decode_str(value)
        .decode_utf8()
        .map(|value| value.into_owned())
        .map_err(|_| invalid("proxy credential"))
}

fn protocol(value: &str) -> Result<String, Problem> {
    let normalized = value.trim().to_ascii_lowercase();
    if matches!(normalized.as_str(), "http" | "https" | "socks4" | "socks5") {
        Ok(normalized)
    } else {
        Err(invalid("proxy protocol"))
    }
}

fn required_port(value: Option<&Value>) -> Result<u16, Problem> {
    let port = value
        .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
        .and_then(|value| u16::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid("proxy port"))?;
    Ok(port)
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    field: &str,
) -> Result<&'a str, Problem> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid(field))
}

fn timestamp(object: &Map<String, Value>, snake: &str, camel: &str) -> Option<String> {
    object
        .get(snake)
        .or_else(|| object.get(camel))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
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

fn credentials_problem() -> Problem {
    problem(
        "migration_proxy_credentials_require_secret_import",
        "Proxy credentials require encrypted secret import",
    )
}

fn invalid(field: &str) -> Problem {
    Problem::new(
        "migration_entity_invalid",
        "Legacy migration entity is invalid",
        422,
        "migration",
    )
    .with_field("field", field)
}

fn io_problem(error: std::io::Error) -> Problem {
    Problem::new(
        "migration_entity_read_failed",
        "Legacy migration entity could not be read",
        500,
        "migration",
    )
    .with_detail(error.to_string())
}

fn problem(code: &str, title: &str) -> Problem {
    Problem::new(code, title, 422, "migration")
}
