use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use buglogin_domain::errors::Problem;
use buglogin_network::proxy::parse_lines;
use serde::Deserialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, State};
use tokio::time::sleep;

use crate::desktop_state::DesktopState;
use crate::proxy_store::ImportedProxy;

static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyParsedProxy {
    #[serde(default)]
    name: Option<String>,
    #[serde(rename = "type", alias = "proxy_type")]
    proxy_type: String,
    host: String,
    port: u16,
    username: Option<String>,
    password: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LegacyProxyExport {
    proxies: Vec<LegacyParsedProxy>,
}

#[tauri::command]
pub fn get_stored_proxies(state: State<'_, DesktopState>) -> Result<Vec<Value>, Problem> {
    state
        .list_proxies()
        .map(|proxies| proxies.iter().map(legacy_stored_proxy).collect())
}

#[tauri::command(rename_all = "camelCase")]
pub fn import_proxies_from_parsed(
    state: State<'_, DesktopState>,
    app: AppHandle,
    parsed_proxies: Vec<LegacyParsedProxy>,
    name_prefix: Option<String>,
) -> Result<Value, Problem> {
    let rows = parsed_proxies
        .into_iter()
        .map(|proxy| parsed_proxy_row(proxy))
        .collect::<Result<Vec<_>, _>>()?;
    let report = state.import_proxies(rows, name_prefix.as_deref())?;
    app.emit("proxies-changed", json!({})).map_err(|_| {
        Problem::new(
            "desktop_event_failed",
            "Desktop event failed",
            500,
            "proxy-import",
        )
    })?;
    Ok(json!({
        "imported_count": report.imported_count,
        "skipped_count": report.skipped_count,
        "errors": report.errors,
        "proxies": report.proxies.iter().map(legacy_stored_proxy).collect::<Vec<_>>(),
    }))
}

#[tauri::command]
pub fn import_proxies_json(
    state: State<'_, DesktopState>,
    app: AppHandle,
    content: String,
) -> Result<Value, Problem> {
    let export = serde_json::from_str::<LegacyProxyExport>(&content).map_err(|_| {
        Problem::new(
            "proxy_json_invalid",
            "Proxy JSON is invalid",
            400,
            "proxy-import",
        )
    })?;
    let rows = export
        .proxies
        .into_iter()
        .map(|proxy| {
            let name = proxy
                .name
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "Imported Proxy".to_owned());
            parsed_proxy_row(proxy).map(|row| (row, name))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let report = state.import_named_proxies(rows)?;
    app.emit("proxies-changed", json!({})).map_err(|_| {
        Problem::new(
            "desktop_event_failed",
            "Desktop event failed",
            500,
            "proxy-import",
        )
    })?;
    Ok(json!({
        "imported_count": report.imported_count,
        "skipped_count": report.skipped_count,
        "errors": report.errors,
        "proxies": report.proxies.iter().map(legacy_stored_proxy).collect::<Vec<_>>(),
    }))
}

fn parsed_proxy_row(
    proxy: LegacyParsedProxy,
) -> Result<buglogin_network::proxy::ProxyRow, Problem> {
    let protocol = match proxy.proxy_type.to_ascii_lowercase().as_str() {
        "http" => buglogin_network::proxy::ProxyProtocol::Http,
        "https" => buglogin_network::proxy::ProxyProtocol::Https,
        "socks4" => buglogin_network::proxy::ProxyProtocol::Socks4,
        "socks" | "socks5" => buglogin_network::proxy::ProxyProtocol::Socks5,
        _ => {
            return Err(Problem::new(
                "proxy_protocol_invalid",
                "Proxy protocol is invalid",
                422,
                "proxy-import",
            ));
        }
    };
    Ok(buglogin_network::proxy::ProxyRow {
        protocol,
        host: proxy.host,
        port: proxy.port,
        username: proxy.username,
        password: proxy.password,
    })
}

fn legacy_stored_proxy(proxy: &ImportedProxy) -> Value {
    json!({
        "id": proxy.id,
        "name": proxy.name,
        "proxy_settings": {
            "proxy_type": proxy.protocol,
            "host": proxy.host,
            "port": proxy.port,
            "username": proxy.username,
        },
    })
}

#[tauri::command]
pub fn parse_txt_proxies(content: String) -> Vec<Value> {
    content
        .lines()
        .filter_map(|raw| {
            let line = raw.trim();
            if line.is_empty() {
                return None;
            }
            let report = parse_lines(line);
            if let Some(row) = report.rows.into_iter().next() {
                return Some(json!({
                    "status": "parsed",
                    "proxy_type": row.protocol,
                    "host": row.host,
                    "port": row.port,
                    "username": row.username,
                    "password": row.password,
                    "original_line": line,
                }));
            }
            Some(json!({
                "status": "invalid",
                "line": line,
                "reason": report.errors.first().map(|error| error.code).unwrap_or("invalid_proxy"),
            }))
        })
        .collect()
}
#[tauri::command]
pub async fn list_browser_profiles(state: State<'_, DesktopState>) -> Result<Vec<Value>, Problem> {
    let correlation_id = request_id("list");
    let result = state
        .invoke("profile_list", &correlation_id, None, json!({}))
        .await?;
    result
        .data
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_response(&correlation_id))
        .map(|items| items.iter().map(legacy_profile).collect())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn create_browser_profile_new(
    state: State<'_, DesktopState>,
    app: AppHandle,
    name: String,
    browser_str: String,
    version: String,
) -> Result<Value, Problem> {
    let correlation_id = request_id("create");
    let runtime = normalize_runtime(&browser_str, &correlation_id)?;
    let result = state
        .invoke(
            "profile_create",
            &correlation_id,
            Some(&request_id("idem-create")),
            json!({
                "name": name,
                "runtime": runtime,
                "runtimeVersion": version,
            }),
        )
        .await?;
    let profile = legacy_profile(&result.data);
    app.emit("profiles-changed", json!({ "profileId": profile["id"] }))
        .map_err(|_| {
            Problem::new(
                "desktop_event_failed",
                "Desktop event failed",
                500,
                correlation_id,
            )
        })?;
    Ok(profile)
}

#[tauri::command]
pub async fn launch_browser_profile(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile: Value,
) -> Result<Value, Problem> {
    let profile_id = profile_id(&profile)?;
    run_operation(
        &state,
        "profile_launch",
        json!({ "profileId": profile_id, "headless": false }),
    )
    .await?;
    app.emit(
        "profile-running-changed",
        json!({ "id": profile_id, "is_running": true }),
    )
    .map_err(|_| {
        Problem::new(
            "desktop_event_failed",
            "Desktop event failed",
            500,
            request_id("event"),
        )
    })?;
    Ok(profile)
}

#[tauri::command]
pub async fn kill_browser_profile(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile: Value,
) -> Result<(), Problem> {
    let profile_id = profile_id(&profile)?;
    run_operation(&state, "profile_stop", json!({ "id": profile_id })).await?;
    app.emit(
        "profile-running-changed",
        json!({ "id": profile_id, "is_running": false }),
    )
    .map_err(|_| {
        Problem::new(
            "desktop_event_failed",
            "Desktop event failed",
            500,
            request_id("event"),
        )
    })?;
    Ok(())
}

#[tauri::command]
pub async fn check_browser_status(
    state: State<'_, DesktopState>,
    profile: Value,
) -> Result<bool, Problem> {
    let profile_id = profile_id(&profile)?;
    let profiles = list_browser_profiles(state).await?;
    Ok(profiles.iter().any(|candidate| {
        candidate.get("id").and_then(Value::as_str) == Some(profile_id)
            && matches!(
                candidate.get("state").and_then(Value::as_str),
                Some("launching" | "running" | "recovering")
            )
    }))
}

async fn run_operation(state: &DesktopState, command: &str, payload: Value) -> Result<(), Problem> {
    let correlation_id = request_id(command);
    let accepted = state
        .invoke(
            command,
            &correlation_id,
            Some(&request_id("idem-operation")),
            payload,
        )
        .await?;
    let operation_id = accepted
        .data
        .get("operationId")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_response(&correlation_id))?;

    for _ in 0..1_200 {
        let operation = state
            .invoke(
                "operation_get",
                &correlation_id,
                None,
                json!({ "operationId": operation_id }),
            )
            .await?;
        match operation.data.get("status").and_then(Value::as_str) {
            Some("completed") => return Ok(()),
            Some("failed" | "cancelled") => {
                if let Some(error) = operation.data.get("error") {
                    if let Ok(problem) = serde_json::from_value::<Problem>(error.clone()) {
                        return Err(problem);
                    }
                }
                return Err(Problem::new(
                    "desktop_operation_failed",
                    "Desktop operation failed",
                    409,
                    correlation_id,
                ));
            }
            _ => sleep(Duration::from_millis(100)).await,
        }
    }

    Err(Problem::new(
        "desktop_operation_timeout",
        "Desktop operation timed out",
        504,
        correlation_id,
    )
    .retryable(true))
}

fn legacy_profile(profile: &Value) -> Value {
    let state = profile.get("state").cloned().unwrap_or(Value::Null);
    let runtime = profile.get("runtime").cloned().unwrap_or(Value::Null);
    let version = profile
        .get("runtimeVersion")
        .cloned()
        .unwrap_or_else(|| Value::String("latest".to_owned()));
    let group_id = profile
        .get("groupIds")
        .and_then(Value::as_array)
        .and_then(|groups| groups.first())
        .cloned();

    // ponytail: canonical runtime contract does not yet persist release channel; remove fallback when CAP-RUN-001 adds it.
    json!({
        "id": profile.get("id").cloned().unwrap_or(Value::Null),
        "name": profile.get("name").cloned().unwrap_or(Value::Null),
        "browser": runtime,
        "version": version,
        "release_type": "stable",
        "state": state,
        "proxy_id": profile.get("proxyId").cloned().unwrap_or(Value::Null),
        "vpn_id": profile.get("vpnId").cloned().unwrap_or(Value::Null),
        "group_id": group_id,
        "tags": profile.get("tags").cloned().unwrap_or_else(|| json!([])),
        "extension_group_id": profile.get("extensionGroupId").cloned().unwrap_or(Value::Null),
        "sync_mode": legacy_sync_mode(profile.get("syncMode")),
    })
}

fn legacy_sync_mode(value: Option<&Value>) -> Value {
    match value.and_then(Value::as_str) {
        Some("managed") => Value::String("Regular".to_owned()),
        Some("disabled" | "local") | None => Value::String("Disabled".to_owned()),
        Some(other) => Value::String(other.to_owned()),
    }
}

fn normalize_runtime<'a>(runtime: &'a str, correlation_id: &str) -> Result<&'a str, Problem> {
    if runtime.eq_ignore_ascii_case("wayfern") {
        Ok("wayfern")
    } else {
        Err(Problem::new(
            "runtime_not_supported",
            "Runtime is not supported",
            422,
            correlation_id,
        ))
    }
}

fn profile_id(profile: &Value) -> Result<&str, Problem> {
    profile
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| {
            Problem::new(
                "profile_id_required",
                "Profile id is required",
                400,
                request_id("profile"),
            )
        })
}

fn invalid_response(correlation_id: &str) -> Problem {
    Problem::new(
        "desktop_adapter_response_invalid",
        "Desktop adapter response is invalid",
        500,
        correlation_id,
    )
}

fn request_id(prefix: &str) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("legacy-{prefix}-{timestamp:x}-{sequence:x}")
}

#[cfg(test)]
mod tests {
    use super::{LegacyProxyExport, legacy_profile, normalize_runtime, parse_txt_proxies};
    use serde_json::json;

    #[test]
    fn canonical_profile_maps_to_donut_shape_without_fake_process_id() {
        let profile = legacy_profile(&json!({
            "id": "profile:1",
            "name": "Primary",
            "runtime": "wayfern",
            "runtimeVersion": "126.0.1",
            "state": "running",
            "groupIds": ["group:1"],
            "tags": ["prod"],
            "syncMode": "managed"
        }));
        assert_eq!(profile["browser"], "wayfern");
        assert_eq!(profile["version"], "126.0.1");
        assert_eq!(profile["state"], "running");
        assert_eq!(profile["group_id"], "group:1");
        assert_eq!(profile["sync_mode"], "Regular");
        assert!(profile.get("process_id").is_none());
    }

    #[test]
    fn proxy_text_parser_returns_donut_rows() {
        let rows = parse_txt_proxies(
            "user:secret:proxy.example:8080\nsocks://proxy.example:1080\ninvalid".to_owned(),
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["status"], "parsed");
        assert_eq!(rows[0]["proxy_type"], "http");
        assert_eq!(rows[0]["host"], "proxy.example");
        assert_eq!(rows[1]["proxy_type"], "socks5");
        assert_eq!(rows[2]["status"], "invalid");
    }
    #[test]
    fn compatibility_create_rejects_non_wayfern_runtime() {
        assert!(normalize_runtime("wayfern", "cor-test").is_ok());
        assert_eq!(
            normalize_runtime("bugox", "cor-test").unwrap_err().code,
            "runtime_not_supported"
        );
    }

    #[test]
    fn donut_proxy_export_accepts_type_and_preserves_name() {
        let export: LegacyProxyExport = serde_json::from_str(
            r#"{"version":"1","proxies":[{"name":"Office","type":"https","host":"proxy.example","port":8443}],"exported_at":"now","source":"donut"}"#,
        )
        .unwrap();
        assert_eq!(export.proxies[0].name.as_deref(), Some("Office"));
        assert_eq!(export.proxies[0].proxy_type, "https");
    }
}
