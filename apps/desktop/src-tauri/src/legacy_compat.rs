use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use buglogin_domain::entities::ProxyProtocol;
use buglogin_domain::errors::Problem;
use buglogin_network::proxy::parse_lines;
use buglogin_storage::secrets::SecretValue;
use futures_util::{StreamExt, stream};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, State};
use tokio::time::sleep;

use crate::app_instance::AppInstance;
use crate::desktop_state::{DesktopAuthStatus, DesktopState};
use crate::profile_lifecycle::canonical_dir_name;
use crate::profile_password::{self, PasswordCheck};

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
pub async fn get_stored_proxies(state: State<'_, DesktopState>) -> Result<Vec<Value>, Problem> {
    let workspace_id = default_workspace_id(&state).await?;
    let result = state
        .invoke(
            "proxy_list",
            &request_id("proxy-list"),
            None,
            json!({ "workspaceId": workspace_id }),
        )
        .await?;
    let items = result
        .data
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_response("proxy-list"))?;
    Ok(items.iter().map(legacy_remote_proxy).collect())
}

#[tauri::command]
pub async fn create_stored_proxy(
    state: State<'_, DesktopState>,
    app: AppHandle,
    name: String,
    proxy_settings: Value,
    idempotency_key: Option<String>,
    proxy_id: Option<String>,
    created_at: Option<String>,
) -> Result<Value, Problem> {
    let workspace_id = default_workspace_id(&state).await?;
    let id = proxy_id.unwrap_or_else(|| format!("proxy_{}", request_id("create")));
    let (mut proxy, credential) = legacy_proxy_input(&id, &name, &proxy_settings, None)?;
    if let Some(created_at) = created_at {
        time::OffsetDateTime::parse(&created_at, &time::format_description::well_known::Rfc3339)
            .map_err(|_| {
                Problem::new(
                    "proxy_timestamp_invalid",
                    "Proxy timestamp is invalid",
                    400,
                    request_id("proxy-create"),
                )
            })?;
        proxy["createdAt"] = json!(created_at);
        proxy["updatedAt"] = json!(created_at);
    }
    let correlation_id = request_id("proxy-create");
    let idempotency_key = idempotency_key.unwrap_or_else(|| request_id("proxy-create-idem"));
    let result = state
        .invoke(
            "proxy_create",
            &correlation_id,
            Some(&idempotency_key),
            json!({ "workspaceId": workspace_id, "proxy": proxy, "credential": credential }),
        )
        .await?;
    app.emit("proxies-changed", json!({})).map_err(|_| {
        Problem::new(
            "desktop_event_failed",
            "Desktop event failed",
            500,
            request_id("proxy-event"),
        )
    })?;
    Ok(legacy_remote_proxy(&result.data))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn update_stored_proxy(
    state: State<'_, DesktopState>,
    app: AppHandle,
    proxy_id: String,
    name: String,
    proxy_settings: Value,
) -> Result<Value, Problem> {
    let workspace_id = default_workspace_id(&state).await?;
    let existing = remote_proxy_record(&state, &workspace_id, &proxy_id).await?;
    let revision = existing
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_response("proxy-update"))?;
    let old_proxy = existing.get("proxy").cloned().unwrap_or(Value::Null);
    let (mut proxy, credential) =
        legacy_proxy_input(&proxy_id, &name, &proxy_settings, Some(&old_proxy))?;
    proxy["hasCredential"] =
        Value::Bool(credential.is_some() || old_proxy["hasCredential"].as_bool().unwrap_or(false));
    let result = state
        .invoke(
            "proxy_update",
            &request_id("proxy-update"),
            Some(&request_id("proxy-update-idem")),
            json!({
                "workspaceId": workspace_id,
                "proxyId": proxy_id,
                "expectedRevision": revision,
                "proxy": proxy,
                "credential": credential,
            }),
        )
        .await?;
    app.emit("proxies-changed", json!({})).map_err(|_| {
        Problem::new(
            "desktop_event_failed",
            "Desktop event failed",
            500,
            request_id("proxy-event"),
        )
    })?;
    Ok(legacy_remote_proxy(&result.data))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn delete_stored_proxy(
    state: State<'_, DesktopState>,
    app: AppHandle,
    proxy_id: String,
) -> Result<(), Problem> {
    let workspace_id = default_workspace_id(&state).await?;
    let existing = remote_proxy_record(&state, &workspace_id, &proxy_id).await?;
    let revision = existing
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_response("proxy-delete"))?;
    state
        .invoke(
            "proxy_delete",
            &request_id("proxy-delete"),
            Some(&request_id("proxy-delete-idem")),
            json!({ "workspaceId": workspace_id, "proxyId": proxy_id, "expectedRevision": revision }),
        )
        .await?;
    app.emit("proxies-changed", json!({})).map_err(|_| {
        Problem::new(
            "desktop_event_failed",
            "Desktop event failed",
            500,
            request_id("proxy-event"),
        )
    })?;
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn check_stored_proxy(
    state: State<'_, DesktopState>,
    proxy_id: String,
) -> Result<Value, Problem> {
    let workspace_id = default_workspace_id(&state).await?;
    let result = state
        .invoke(
            "proxy_check",
            &request_id("proxy-check"),
            Some(&request_id("proxy-check-idem")),
            json!({ "workspaceId": workspace_id, "proxyId": proxy_id }),
        )
        .await?;
    Ok(result.data)
}

/// Checks an unsaved proxy draft locally. Credentials are never sent to the API.
#[tauri::command(rename_all = "camelCase")]
pub async fn check_proxy_validity(proxy_settings: Value) -> Result<Value, Problem> {
    let proxy_type = proxy_settings
        .get("proxy_type")
        .or_else(|| proxy_settings.get("proxyType"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if proxy_type == "none" {
        let (ip, latency_ms) = direct_network_exit_ip().await?;
        let location = proxy_ip_location(&ip).await.unwrap_or_default();
        return Ok(json!({
            "is_valid": true,
            "ip": ip,
            "city": location.get("city").and_then(Value::as_str),
            "region": location.get("regionName").and_then(Value::as_str),
            "country": location.get("country").and_then(Value::as_str),
            "country_code": location.get("countryCode").and_then(Value::as_str),
            "latitude": location.get("lat").and_then(Value::as_f64),
            "longitude": location.get("lon").and_then(Value::as_f64),
            "zip": location.get("zip").and_then(Value::as_str),
            "latency_ms": latency_ms,
            "timestamp": now_unix_millis()
        }));
    }
    let protocol = match proxy_settings
        .get("proxy_type")
        .or_else(|| proxy_settings.get("proxyType"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "http" => ProxyProtocol::Http,
        "https" => ProxyProtocol::Https,
        "socks4" => ProxyProtocol::Socks4,
        "socks5" => ProxyProtocol::Socks5,
        _ => return Err(proxy_draft_invalid()),
    };
    let host = proxy_settings
        .get("host")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    let port = proxy_settings
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(proxy_draft_invalid)?;
    if host.is_empty() || host.len() > 253 || host.chars().any(char::is_whitespace) {
        return Err(proxy_draft_invalid());
    }
    let username = proxy_settings
        .get("username")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let password = proxy_settings
        .get("password")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let credential = if username.is_empty() && password.is_empty() {
        None
    } else {
        let bytes = serde_json::to_vec(&json!({ "username": username, "password": password }))
            .map_err(|_| proxy_draft_invalid())?;
        Some(SecretValue::new(bytes).map_err(|_| proxy_draft_invalid())?)
    };
    let (ip, latency_ms) =
        crate::proxy_bridge::check_proxy_exit_ip(crate::proxy_bridge::ProxyBridgeSpec {
            protocol,
            host,
            port,
            credential,
        })
        .await?;
    let location = proxy_ip_location(&ip).await.unwrap_or_default();
    Ok(json!({
        "is_valid": true,
        "ip": ip,
        "city": location.get("city").and_then(Value::as_str),
        "region": location.get("regionName").and_then(Value::as_str),
        "country": location.get("country").and_then(Value::as_str),
        "country_code": location.get("countryCode").and_then(Value::as_str),
        "latitude": location.get("lat").and_then(Value::as_f64),
        "longitude": location.get("lon").and_then(Value::as_f64),
        "zip": location.get("zip").and_then(Value::as_str),
        "latency_ms": latency_ms,
        "timestamp": now_unix_millis()
    }))
}

async fn direct_network_exit_ip() -> Result<(String, u64), Problem> {
    let started = Instant::now();
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|_| network_check_failed())?
        .get("https://api.ipify.org")
        .send()
        .await
        .map_err(|_| network_check_failed())?;
    if !response.status().is_success() {
        return Err(network_check_failed());
    }
    let ip = response
        .text()
        .await
        .map_err(|_| network_check_failed())?
        .trim()
        .parse::<std::net::IpAddr>()
        .map_err(|_| network_check_failed())?
        .to_string();
    Ok((
        ip,
        started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
    ))
}

fn network_check_failed() -> Problem {
    Problem::new(
        "network_check_failed",
        "Direct network connection could not be checked",
        502,
        request_id("network-check"),
    )
}

async fn proxy_ip_location(ip: &str) -> Result<Value, ()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| ())?;
    let response = client
        .get(format!("http://ip-api.com/json/{ip}?fields=status,country,countryCode,regionName,city,zip,lat,lon"))
        .send()
        .await
        .map_err(|_| ())?;
    if !response.status().is_success() {
        return Err(());
    }
    let value = response.json::<Value>().await.map_err(|_| ())?;
    (value.get("status").and_then(Value::as_str) == Some("success"))
        .then_some(value)
        .ok_or(())
}

fn proxy_draft_invalid() -> Problem {
    Problem::new(
        "proxy_settings_invalid",
        "Proxy settings are invalid",
        422,
        request_id("proxy-check"),
    )
}

fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[tauri::command]
pub async fn get_groups_with_profile_counts(
    state: State<'_, DesktopState>,
    profiles: Option<Vec<Value>>,
) -> Result<Vec<Value>, Problem> {
    let workspace_id = default_workspace_id(&state).await?;
    let group_result = state
        .invoke(
            "group_list",
            &request_id("group-list"),
            None,
            json!({ "workspaceId": workspace_id }),
        )
        .await?;
    let profiles = match profiles {
        Some(profiles) => profiles,
        None => list_all_profiles(&state, None).await?,
    };
    let groups = group_result
        .data
        .as_array()
        .or_else(|| group_result.data.get("items").and_then(Value::as_array))
        .ok_or_else(|| invalid_response("group-list"))?;
    Ok(groups.iter().map(|record| {
        let group = record.get("group").unwrap_or(record);
        let id = group.get("id").and_then(Value::as_str).unwrap_or_default();
        let count = profiles
            .iter()
            .filter(|profile| {
                profile
                    .get("groupIds")
                    .or_else(|| profile.get("group_ids"))
                    .and_then(Value::as_array)
                    .is_some_and(|ids| ids.iter().any(|value| value.as_str() == Some(id)))
                    || profile.get("group_id").and_then(Value::as_str) == Some(id)
            })
            .count();
        json!({ "id": id, "name": group.get("name").cloned().unwrap_or(Value::Null), "count": count })
    }).collect())
}

#[tauri::command]
pub async fn get_profile_groups(state: State<'_, DesktopState>) -> Result<Vec<Value>, Problem> {
    get_groups_with_profile_counts(state, None).await
}

/// Compatibility name used by the profile information dialog.
#[tauri::command]
pub async fn get_groups(state: State<'_, DesktopState>) -> Result<Vec<Value>, Problem> {
    get_groups_with_profile_counts(state, None).await
}

#[tauri::command]
pub async fn create_profile_group(
    state: State<'_, DesktopState>,
    app: AppHandle,
    name: String,
) -> Result<Value, Problem> {
    let workspace_id = default_workspace_id(&state).await?;
    let group_id = format!("group_{}", request_id("create"));
    let result = state
        .invoke(
            "group_create",
            &request_id("group-create"),
            Some(&request_id("group-create-idem")),
            json!({ "workspaceId": workspace_id, "groupId": group_id, "name": name.trim() }),
        )
        .await?;
    app.emit("groups-changed", json!({})).map_err(|_| {
        Problem::new(
            "desktop_event_failed",
            "Desktop event failed",
            500,
            request_id("group-event"),
        )
    })?;
    Ok(result.data.get("group").cloned().unwrap_or(result.data))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn update_profile_group(
    state: State<'_, DesktopState>,
    app: AppHandle,
    group_id: String,
    name: String,
) -> Result<Value, Problem> {
    let workspace_id = default_workspace_id(&state).await?;
    let existing = state
        .invoke(
            "group_get",
            &request_id("group-get"),
            None,
            json!({ "workspaceId": workspace_id, "groupId": group_id }),
        )
        .await?;
    let group = existing.data.get("group").unwrap_or(&existing.data);
    let revision = existing
        .data
        .get("revision")
        .and_then(Value::as_u64)
        .or_else(|| group.get("revision").and_then(Value::as_u64))
        .ok_or_else(|| invalid_response("group-update"))?;
    let result = state.invoke(
        "group_update", &request_id("group-update"), Some(&request_id("group-update-idem")),
        json!({ "workspaceId": workspace_id, "groupId": group_id, "expectedRevision": revision, "name": name.trim() }),
    ).await?;
    app.emit("groups-changed", json!({})).map_err(|_| {
        Problem::new(
            "desktop_event_failed",
            "Desktop event failed",
            500,
            request_id("group-event"),
        )
    })?;
    Ok(result.data.get("group").cloned().unwrap_or(result.data))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn delete_profile_group(
    state: State<'_, DesktopState>,
    app: AppHandle,
    group_id: String,
) -> Result<(), Problem> {
    let workspace_id = default_workspace_id(&state).await?;
    let existing = state
        .invoke(
            "group_get",
            &request_id("group-get"),
            None,
            json!({ "workspaceId": workspace_id, "groupId": group_id }),
        )
        .await?;
    let revision = existing
        .data
        .get("revision")
        .and_then(Value::as_u64)
        .or_else(|| {
            existing
                .data
                .pointer("/group/revision")
                .and_then(Value::as_u64)
        })
        .ok_or_else(|| invalid_response("group-delete"))?;
    state.invoke(
        "group_delete", &request_id("group-delete"), Some(&request_id("group-delete-idem")),
        json!({ "workspaceId": workspace_id, "groupId": group_id, "expectedRevision": revision }),
    ).await?;
    app.emit("groups-changed", json!({})).map_err(|_| {
        Problem::new(
            "desktop_event_failed",
            "Desktop event failed",
            500,
            request_id("group-event"),
        )
    })?;
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn assign_profiles_to_group(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_ids: Vec<String>,
    group_id: Option<String>,
) -> Result<(), Problem> {
    for profile_id in profile_ids {
        let profile = remote_profile_record(&state, &profile_id).await?;
        let workspace_id = profile
            .get("workspaceId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or(default_workspace_id(&state).await?);
        let revision = profile
            .get("revision")
            .and_then(Value::as_u64)
            .ok_or_else(|| invalid_response("profile-groups"))?;
        let group_ids = replacement_group_ids(group_id.as_deref());
        state.invoke(
            "profile_groups_replace", &request_id("profile-groups"), Some(&request_id("profile-groups-idem")),
            json!({ "workspaceId": workspace_id, "profileId": profile_id, "expectedRevision": revision, "groupIds": group_ids }),
        ).await?;
        app.emit("profiles-changed", json!({ "profileId": profile_id }))
            .map_err(|_| {
                Problem::new(
                    "desktop_event_failed",
                    "Desktop event failed",
                    500,
                    request_id("group-event"),
                )
            })?;
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn update_profile_proxy(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
    proxy_id: Option<String>,
) -> Result<Value, Problem> {
    let profile = remote_profile_record(&state, &profile_id).await?;
    let workspace_id = profile
        .get("workspaceId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or(default_workspace_id(&state).await?);
    let revision = profile
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_response("profile-proxy"))?;
    let result = state.invoke(
        "profile_proxy_assignment_set", &request_id("profile-proxy"), Some(&request_id("profile-proxy-idem")),
        json!({ "workspaceId": workspace_id, "profileId": profile_id, "expectedProfileRevision": revision, "proxyId": proxy_id }),
    ).await?;
    app.emit("profiles-changed", json!({ "profileId": profile_id }))
        .map_err(|_| {
            Problem::new(
                "desktop_event_failed",
                "Desktop event failed",
                500,
                request_id("proxy-event"),
            )
        })?;
    Ok(result.data)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn rename_profile(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
    new_name: String,
) -> Result<Value, Problem> {
    let profile = remote_profile_record(&state, &profile_id).await?;
    let workspace_id = profile
        .get("workspaceId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or(default_workspace_id(&state).await?);
    let revision = profile
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_response("profile-update"))?;
    let result = state
        .invoke(
            "profile_update",
            &request_id("profile-update"),
            Some(&request_id("profile-update-idem")),
            json!({
                "workspaceId": workspace_id,
                "profileId": profile_id,
                "expectedRevision": revision,
                "name": new_name.trim(),
                "note": profile.get("note").cloned().unwrap_or(Value::Null),
                "tags": profile.get("tags").cloned().unwrap_or_else(|| json!([])),
            }),
        )
        .await?;
    app.emit("profiles-changed", json!({ "profileId": profile_id }))
        .map_err(|_| {
            Problem::new(
                "desktop_event_failed",
                "Desktop event failed",
                500,
                request_id("profile-event"),
            )
        })?;
    Ok(legacy_profile(&result.data))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn update_profile_tags(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
    tags: Vec<String>,
) -> Result<Value, Problem> {
    let profile = remote_profile_record(&state, &profile_id).await?;
    let note = profile.get("note").cloned().unwrap_or(Value::Null);
    update_profile_metadata(&state, &app, &profile_id, &profile, note, json!(tags)).await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn update_profile_note(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
    note: Option<String>,
) -> Result<Value, Problem> {
    let profile = remote_profile_record(&state, &profile_id).await?;
    let tags = profile.get("tags").cloned().unwrap_or_else(|| json!([]));
    update_profile_metadata(&state, &app, &profile_id, &profile, json!(note), tags).await
}

async fn update_profile_metadata(
    state: &DesktopState,
    app: &AppHandle,
    profile_id: &str,
    profile: &Value,
    note: Value,
    tags: Value,
) -> Result<Value, Problem> {
    let workspace_id = match profile.get("workspaceId").and_then(Value::as_str) {
        Some(workspace_id) => workspace_id.to_owned(),
        None => default_workspace_id(state).await?,
    };
    let revision = profile
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_response("profile-update"))?;
    let name = profile
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_response("profile-update"))?;
    let result = state
        .invoke(
            "profile_update",
            &request_id("profile-update"),
            Some(&request_id("profile-update-idem")),
            json!({
                "workspaceId": workspace_id,
                "profileId": profile_id,
                "expectedRevision": revision,
                "name": name,
                "note": note,
                "tags": tags,
            }),
        )
        .await?;
    app.emit("profiles-changed", json!({ "profileId": profile_id }))
        .map_err(|_| {
            Problem::new(
                "desktop_event_failed",
                "Desktop event failed",
                500,
                request_id("profile-event"),
            )
        })?;
    Ok(legacy_profile(&result.data))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn delete_profile(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
) -> Result<(), Problem> {
    archive_profile(&state, &profile_id).await?;
    app.emit("profiles-changed", json!({ "profileId": profile_id }))
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

#[tauri::command(rename_all = "camelCase")]
pub async fn delete_selected_profiles(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_ids: Vec<String>,
) -> Result<(), Problem> {
    for profile_id in &profile_ids {
        archive_profile(&state, profile_id).await?;
    }
    app.emit("profiles-changed", json!({ "profileIds": profile_ids }))
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

async fn archive_profile(state: &DesktopState, profile_id: &str) -> Result<(), Problem> {
    let profile = remote_profile_record(state, profile_id).await?;
    let revision = profile
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_response("profile-archive"))?;
    state
        .invoke(
            "profile_archive",
            &request_id("profile-archive"),
            Some(&request_id("profile-archive-idem")),
            json!({ "profileId": profile_id, "revision": revision }),
        )
        .await?;
    Ok(())
}

async fn remote_profile_record(state: &DesktopState, profile_id: &str) -> Result<Value, Problem> {
    let result = state
        .invoke(
            "profile_get",
            &request_id("profile-get"),
            None,
            json!({ "profileId": profile_id }),
        )
        .await?;
    let mut record = result.data;
    if record
        .get("workspaceId")
        .and_then(Value::as_str)
        .filter(|workspace_id| !workspace_id.trim().is_empty())
        .is_none()
    {
        let binding = state
            .invoke(
                "profile_binding_get",
                &request_id("profile-binding"),
                None,
                json!({ "profileId": profile_id }),
            )
            .await?;
        let workspace_id = binding
            .data
            .get("workspaceId")
            .and_then(Value::as_str)
            .filter(|workspace_id| !workspace_id.trim().is_empty())
            .ok_or_else(|| invalid_response("profile-binding"))?;
        record["workspaceId"] = json!(workspace_id);
    }
    Ok(record)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn import_proxies_from_parsed(
    state: State<'_, DesktopState>,
    app: AppHandle,
    parsed_proxies: Vec<LegacyParsedProxy>,
    name_prefix: Option<String>,
) -> Result<Value, Problem> {
    let workspace_id = default_workspace_id(&state).await?;
    let mut imported = Vec::new();
    let mut errors = Vec::new();
    for (index, proxy) in parsed_proxies.into_iter().enumerate() {
        let name = proxy
            .name
            .clone()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                name_prefix
                    .as_ref()
                    .map(|prefix| format!("{} {}", prefix.trim(), index + 1))
            })
            .unwrap_or_else(|| format!("Proxy {}", index + 1));
        match create_remote_proxy(&state, &workspace_id, &name, proxy).await {
            Ok(record) => imported.push(legacy_remote_proxy(&record)),
            Err(problem) => errors
                .push(json!({ "index": index, "code": problem.code, "message": problem.title })),
        }
    }
    app.emit("proxies-changed", json!({})).map_err(|_| {
        Problem::new(
            "desktop_event_failed",
            "Desktop event failed",
            500,
            "proxy-import",
        )
    })?;
    Ok(json!({
        "imported_count": imported.len(),
        "skipped_count": 0,
        "errors": errors,
        "proxies": imported,
    }))
}

#[tauri::command]
pub async fn import_proxies_json(
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
    let workspace_id = default_workspace_id(&state).await?;
    let mut imported = Vec::new();
    let mut errors = Vec::new();
    for (index, proxy) in export.proxies.into_iter().enumerate() {
        let name = proxy
            .name
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| format!("Imported Proxy {}", index + 1));
        match create_remote_proxy(&state, &workspace_id, &name, proxy).await {
            Ok(record) => imported.push(legacy_remote_proxy(&record)),
            Err(problem) => errors
                .push(json!({ "index": index, "code": problem.code, "message": problem.title })),
        }
    }
    app.emit("proxies-changed", json!({})).map_err(|_| {
        Problem::new(
            "desktop_event_failed",
            "Desktop event failed",
            500,
            "proxy-import",
        )
    })?;
    Ok(json!({
        "imported_count": imported.len(),
        "skipped_count": 0,
        "errors": errors,
        "proxies": imported,
    }))
}

async fn default_workspace_id(state: &DesktopState) -> Result<String, Problem> {
    let (account_id, workspaces) = get_available_workspace_records(state).await?;
    resolve_and_persist_workspace_selection(state, account_id.as_deref(), &workspaces)?.ok_or_else(
        || {
            Problem::new(
                "workspace_unavailable",
                "No hosted workspace is available for this account",
                409,
                request_id("workspace"),
            )
        },
    )
}

#[tauri::command]
pub async fn get_available_workspaces(
    state: State<'_, DesktopState>,
) -> Result<Vec<Value>, Problem> {
    let (account_id, mut workspaces) = get_available_workspace_records(&state).await?;
    let selected =
        resolve_and_persist_workspace_selection(&state, account_id.as_deref(), &workspaces)?;
    for workspace in &mut workspaces {
        workspace["selected"] =
            json!(workspace.get("id").and_then(Value::as_str) == selected.as_deref());
    }
    Ok(workspaces)
}

fn resolve_and_persist_workspace_selection(
    state: &DesktopState,
    account_id: Option<&str>,
    workspaces: &[Value],
) -> Result<Option<String>, Problem> {
    let current = state.selected_workspace_id(account_id);
    let selected = choose_workspace_selection(current.as_deref(), workspaces);
    if let (Some(account_id), Some(workspace_id)) = (account_id, selected.as_ref()) {
        state.set_selected_workspace_id(account_id, workspace_id.clone())?;
    }
    Ok(selected)
}

fn choose_workspace_selection(current: Option<&str>, workspaces: &[Value]) -> Option<String> {
    let available = |workspace_id: &str| {
        workspaces
            .iter()
            .any(|workspace| workspace.get("id").and_then(Value::as_str) == Some(workspace_id))
    };
    current
        .filter(|workspace_id| available(workspace_id))
        .map(str::to_owned)
        .or_else(|| {
            workspaces
                .iter()
                .filter_map(|workspace| workspace.get("id").and_then(Value::as_str))
                .min()
                .map(str::to_owned)
        })
}

#[tauri::command(rename_all = "camelCase")]
pub async fn select_active_workspace(
    state: State<'_, DesktopState>,
    app: AppHandle,
    workspace_id: String,
) -> Result<Value, Problem> {
    let (account_id, workspaces) = get_available_workspace_records(&state).await?;
    let selected = workspaces
        .iter()
        .find(|workspace| {
            workspace.get("id").and_then(Value::as_str) == Some(workspace_id.as_str())
        })
        .ok_or_else(|| {
            Problem::new(
                "workspace_access_denied",
                "Workspace is not available to this account",
                403,
                request_id("workspace-select"),
            )
        })?;
    let organization_id = selected
        .get("organizationId")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_response("workspace-list"))?;
    state
        .invoke(
            "workspace_get",
            &request_id("workspace-get"),
            None,
            json!({ "organizationId": organization_id, "workspaceId": workspace_id }),
        )
        .await?;
    let account_id = account_id.ok_or_else(|| {
        Problem::new(
            "hosted_workspace_required",
            "Sign in to select a hosted workspace",
            409,
            request_id("workspace-select"),
        )
    })?;
    state.set_selected_workspace_id(&account_id, workspace_id.clone())?;
    app.emit("profiles-changed", json!({ "workspaceId": workspace_id }))
        .map_err(|_| {
            Problem::new(
                "desktop_event_failed",
                "Desktop event failed",
                500,
                request_id("workspace-select"),
            )
        })?;
    app.emit("proxies-changed", json!({ "workspaceId": workspace_id }))
        .map_err(|_| {
            Problem::new(
                "desktop_event_failed",
                "Desktop event failed",
                500,
                request_id("workspace-select"),
            )
        })?;
    Ok(selected.clone())
}

async fn count_profiles_for_workspace(state: &DesktopState, workspace_id: &str) -> Option<usize> {
    let mut total: usize = 0;
    let mut cursor: Option<String> = None;
    let mut seen_cursors = std::collections::BTreeSet::new();
    loop {
        let correlation_id = request_id("ws-profile-count");
        let mut payload = json!({
            "includeArchived": false,
            "limit": 100,
            "workspaceId": workspace_id,
        });
        if let Some(ref c) = cursor {
            payload["cursor"] = json!(c);
        }
        match state
            .invoke("profile_list", &correlation_id, None, payload)
            .await
        {
            Ok(p_resp) => {
                let items = p_resp.data.get("items")?.as_array()?;
                total = total.checked_add(items.len())?;
                match p_resp.data.get("nextCursor") {
                    None | Some(Value::Null) => return Some(total),
                    Some(Value::String(next))
                        if !next.is_empty()
                            && !seen_cursors.contains(next)
                            && !items.is_empty() =>
                    {
                        seen_cursors.insert(next.clone());
                        cursor = Some(next.clone());
                    }
                    _ => return None,
                }
            }
            Err(_) => return None,
        }
    }
}

async fn get_available_workspace_records(
    state: &DesktopState,
) -> Result<(Option<String>, Vec<Value>), Problem> {
    let account_id = match state.auth_status()? {
        DesktopAuthStatus::Authenticated => {
            let result = state
                .invoke("account_get", &request_id("account-get"), None, json!({}))
                .await?;
            let account_id = result
                .data
                .get("accountId")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| invalid_response("account-get"))?;
            Some(account_id.to_owned())
        }
        DesktopAuthStatus::AuthNotApplicable => None,
        DesktopAuthStatus::Unauthenticated => {
            return Err(Problem::new(
                "authentication_required",
                "Sign in to BugLogin",
                401,
                request_id("workspace-list"),
            ));
        }
    };
    let organizations = state
        .invoke(
            "organization_list",
            &request_id("organization-list"),
            None,
            json!({}),
        )
        .await?;
    let orgs = organizations
        .data
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_response("organization-list"))?;
    let organization_rows: Vec<(String, String, Option<String>)> = orgs
        .iter()
        .filter_map(|org| {
            Some((
                org.get("id")?.as_str()?.to_owned(),
                org.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                org.get("accountId")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            ))
        })
        .collect();

    // Workspace discovery used to perform every organization, membership, and
    // profile-count request serially. On refresh this made latency grow with
    // the number of organizations/workspaces. Keep the same server-authorized
    // reads, but overlap independent requests with a small concurrency bound.
    let workspace_pages: Vec<Result<(String, String, Option<String>, Vec<Value>), Problem>> =
        stream::iter(organization_rows.into_iter().map(
            |(org_id, org_name, owner_id)| async move {
                let response = state
                    .invoke(
                        "workspace_list",
                        &request_id("workspace-list"),
                        None,
                        json!({ "organizationId": org_id }),
                    )
                    .await?;
                let items = response
                    .data
                    .get("items")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                Ok((org_id, org_name, owner_id, items))
            },
        ))
        .buffer_unordered(4)
        .collect()
        .await;

    let mut workspace_rows = Vec::new();
    for page in workspace_pages {
        let (org_id, org_name, owner_id, items) = page?;
        let is_org_owner =
            account_id.as_deref().is_some() && owner_id.as_deref() == account_id.as_deref();
        for item in items {
            let mut normalized = item
                .get("workspace")
                .cloned()
                .unwrap_or_else(|| item.clone());
            normalized["organizationId"] = json!(org_id);
            normalized["organizationName"] = json!(org_name);
            workspace_rows.push((normalized, is_org_owner));
        }
    }

    let mut workspaces: Vec<Value> = stream::iter(workspace_rows.into_iter().map(
        |(mut workspace, is_org_owner)| {
            let account_id = account_id.clone();
            async move {
                let workspace_id = workspace
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let member_future = async {
                    if is_org_owner || workspace_id.is_empty() {
                        return None;
                    }
                    state
                        .invoke(
                            "member_list",
                            &request_id("member-list"),
                            None,
                            json!({ "workspaceId": workspace_id }),
                        )
                        .await
                        .ok()
                        .and_then(|response| response.data.get("items")?.as_array().cloned())
                        .and_then(|members| {
                            members.into_iter().find_map(|member| {
                                (member.get("accountId").and_then(Value::as_str)
                                    == account_id.as_deref())
                                .then(|| {
                                    member
                                        .get("role")
                                        .and_then(Value::as_str)
                                        .map(str::to_owned)
                                })
                                .flatten()
                            })
                        })
                };
                let count_future = async {
                    if workspace_id.is_empty() {
                        None
                    } else {
                        count_profiles_for_workspace(state, &workspace_id).await
                    }
                };
                let (member_role, profile_count) = tokio::join!(member_future, count_future);
                let role = if is_org_owner {
                    "owner".to_owned()
                } else {
                    member_role.unwrap_or_else(|| "member".to_owned())
                };
                workspace["role"] = json!(role);
                if let Some(max) = workspace
                    .get("policy")
                    .and_then(|policy| policy.get("maxProfiles"))
                    .and_then(Value::as_u64)
                {
                    workspace["maxProfiles"] = json!(max);
                }
                if !workspace_id.is_empty() {
                    workspace["profileCount"] = json!(profile_count);
                }
                workspace
            }
        },
    ))
    .buffer_unordered(4)
    .collect()
    .await;
    workspaces.sort_by(|left, right| {
        left.get("name")
            .and_then(Value::as_str)
            .cmp(&right.get("name").and_then(Value::as_str))
    });
    workspaces.dedup_by(|left, right| left.get("id") == right.get("id"));
    Ok((account_id, workspaces))
}

async fn remote_proxy_record(
    state: &DesktopState,
    workspace_id: &str,
    proxy_id: &str,
) -> Result<Value, Problem> {
    let result = state
        .invoke(
            "proxy_get",
            &request_id("proxy-get"),
            None,
            json!({ "workspaceId": workspace_id, "proxyId": proxy_id }),
        )
        .await?;
    Ok(result.data)
}

async fn create_remote_proxy(
    state: &DesktopState,
    workspace_id: &str,
    name: &str,
    proxy: LegacyParsedProxy,
) -> Result<Value, Problem> {
    let settings = json!({
        "proxy_type": proxy.proxy_type,
        "host": proxy.host,
        "port": proxy.port,
        "username": proxy.username,
        "password": proxy.password,
    });
    let id = format!("proxy_{}", request_id("import"));
    let (record, credential) = legacy_proxy_input(&id, name, &settings, None)?;
    let result = state
        .invoke(
            "proxy_create",
            &request_id("proxy-import"),
            Some(&request_id("proxy-import-idem")),
            json!({ "workspaceId": workspace_id, "proxy": record, "credential": credential }),
        )
        .await?;
    Ok(result.data)
}

fn legacy_remote_proxy(record: &Value) -> Value {
    let proxy = record.get("proxy").unwrap_or(record);
    let host = proxy.get("host").cloned().unwrap_or(Value::Null);
    let port = proxy.get("port").cloned().unwrap_or(Value::Null);
    let name = proxy
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{}:{}", host.as_str().unwrap_or_default(), port));
    json!({
        "id": proxy.get("id").cloned().unwrap_or(Value::Null),
        "name": name,
        "proxy_settings": {
            "proxy_type": proxy.get("protocol").cloned().unwrap_or(Value::Null),
            "host": host,
            "port": port,
            "username": null,
            "has_credential": proxy.get("hasCredential").and_then(Value::as_bool).unwrap_or(false),
        },
        "is_cloud_managed": false,
        "is_cloud_derived": false,
    })
}

fn legacy_proxy_input(
    proxy_id: &str,
    name: &str,
    settings: &Value,
    existing: Option<&Value>,
) -> Result<(Value, Option<Value>), Problem> {
    let protocol = settings
        .get("proxy_type")
        .or_else(|| settings.get("proxyType"))
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_response("proxy-input"))?
        .to_ascii_lowercase();
    if !matches!(protocol.as_str(), "http" | "https" | "socks4" | "socks5") {
        return Err(Problem::new(
            "proxy_protocol_invalid",
            "This proxy protocol is not supported by BugLogin 3.0",
            422,
            request_id("proxy-input"),
        ));
    }
    let host = settings
        .get("host")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let port = settings
        .get("port")
        .and_then(Value::as_u64)
        .filter(|port| (1..=65535).contains(port))
        .ok_or_else(|| {
            Problem::new(
                "proxy_port_invalid",
                "Proxy port must be between 1 and 65535",
                422,
                request_id("proxy-input"),
            )
        })?;
    if host.is_empty() {
        return Err(Problem::new(
            "proxy_host_required",
            "Proxy host is required",
            422,
            request_id("proxy-input"),
        ));
    }
    let username = settings
        .get("username")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let password = settings
        .get("password")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let credential = if !username.is_empty() || !password.is_empty() {
        Some(json!({ "username": username, "password": password }))
    } else {
        None
    };
    let has_credential = credential.is_some()
        || existing
            .and_then(|proxy| proxy.get("hasCredential"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let timestamp = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| {
            Problem::new(
                "clock_format_failed",
                "Could not format proxy timestamp",
                500,
                request_id("proxy-input"),
            )
        })?;
    Ok((
        json!({
            "id": proxy_id,
            "name": if name.trim().is_empty() { Value::Null } else { json!(name.trim()) },
            "protocol": protocol,
            "host": host,
            "port": port,
            "hasCredential": has_credential,
            "bypassRules": existing.and_then(|proxy| proxy.get("bypassRules")).cloned().unwrap_or_else(|| json!([])),
            "createdAt": existing.and_then(|proxy| proxy.get("createdAt")).cloned().unwrap_or_else(|| json!(timestamp)),
            "updatedAt": timestamp,
        }),
        credential,
    ))
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
pub async fn list_browser_profiles(
    state: State<'_, DesktopState>,
    workspace_id: Option<String>,
) -> Result<Vec<Value>, Problem> {
    Ok(list_all_profiles(&state, workspace_id.as_deref())
        .await?
        .iter()
        .map(legacy_profile)
        .collect())
}

/// Compatibility name used by the profile-group bulk actions.
#[tauri::command]
pub async fn get_browser_profiles(state: State<'_, DesktopState>) -> Result<Vec<Value>, Problem> {
    list_browser_profiles(state, None).await
}

async fn list_all_profiles(
    state: &DesktopState,
    requested_workspace_id: Option<&str>,
) -> Result<Vec<Value>, Problem> {
    let mut items = Vec::new();
    let mut cursor: Option<String> = None;
    let workspace_id = match requested_workspace_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        Some(workspace_id) => workspace_id.to_owned(),
        None => default_workspace_id(state).await?,
    };
    loop {
        let correlation_id = request_id("profile-list");
        let mut payload =
            json!({ "includeArchived": false, "limit": 100, "workspaceId": workspace_id });
        if let Some(value) = cursor.as_ref() {
            payload["cursor"] = json!(value);
        }
        let page = state
            .invoke("profile_list", &correlation_id, None, payload)
            .await?;
        let page_items = page
            .data
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid_response(&correlation_id))?;
        items.extend(page_items.iter().cloned());
        let next = page
            .data
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        match next {
            Some(next) if cursor.as_deref() != Some(next.as_str()) => cursor = Some(next),
            _ => break,
        }
    }
    Ok(items)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn create_browser_profile_new(
    state: State<'_, DesktopState>,
    app: AppHandle,
    name: String,
    idempotency_key: String,
    browser_str: String,
    version: Option<String>,
    proxy_id: Option<String>,
    proxy_settings: Option<Value>,
    vpn_id: Option<String>,
    wayfern_config: Option<Value>,
    group_id: Option<String>,
) -> Result<Value, Problem> {
    let correlation_id = request_id("create");
    if vpn_id.is_some() {
        return Err(Problem::new(
            "vpn_assignment_unsupported",
            "VPN assignments are not available in BugLogin 3.0 yet",
            422,
            correlation_id,
        ));
    }
    let runtime = normalize_runtime(&browser_str, &correlation_id)?;
    let fingerprint_target = legacy_fingerprint_target(wayfern_config.as_ref(), &correlation_id)?;
    if let Some(target) = fingerprint_target.as_ref() {
        ensure_local_fingerprint_target(target, &correlation_id)?;
    }
    let workspace_id = default_workspace_id(&state).await?;
    if let Some(group_id) = group_id.as_deref() {
        // Validate the requested group before creating an inline proxy so a
        // bad group cannot leave an unassigned proxy in the workspace.
        state
            .invoke(
                "group_get",
                &request_id("create-group-get"),
                None,
                json!({ "workspaceId": workspace_id.clone(), "groupId": group_id }),
            )
            .await?;
    }
    let proxy_id = match (proxy_id, proxy_settings) {
        (Some(_), Some(_)) => {
            return Err(Problem::new(
                "proxy_assignment_ambiguous",
                "Choose a saved proxy or enter a custom proxy, not both",
                422,
                correlation_id,
            ));
        }
        (Some(proxy_id), None) => Some(proxy_id),
        (None, Some(settings)) => {
            let digest = format!("{:x}", Sha256::digest(idempotency_key.as_bytes()));
            let proxy_id = format!("prx_{}", &digest[..32]);
            let (proxy, credential) = legacy_proxy_input(&proxy_id, "", &settings, None)?;
            let mut proxy_payload = json!({ "workspaceId": workspace_id.clone(), "proxy": proxy });
            if let Some(credential) = credential {
                proxy_payload["credential"] = credential;
            }
            let proxy_idempotency = format!("proxy-create-{}", &digest[..32]);
            state
                .invoke(
                    "proxy_create",
                    &request_id("create-profile-proxy"),
                    Some(&proxy_idempotency),
                    proxy_payload,
                )
                .await?;
            Some(proxy_id)
        }
        (None, None) => None,
    };
    let mut create_payload = json!({
        "name": name,
        "runtime": runtime,
        "workspaceId": workspace_id.clone(),
        "proxyId": proxy_id,
        "fingerprintTarget": fingerprint_target,
    });
    if let Some(version) = version {
        create_payload["runtimeVersion"] = json!(version);
    }
    let result = state
        .invoke(
            "profile_create",
            &correlation_id,
            Some(&idempotency_key),
            create_payload,
        )
        .await?;
    let mut created = result.data;
    if let Some(group_id) = group_id {
        let profile_id = profile_id(&created)?.to_owned();
        // Read the latest revision because this create request may be a
        // replay after the profile was created but group assignment or event
        // delivery failed previously.
        let current = remote_profile_record(&state, &profile_id).await?;
        let revision = current
            .get("revision")
            .and_then(Value::as_u64)
            .ok_or_else(|| invalid_response("create-profile-groups"))?;
        let mut group_ids = current
            .get("groupIds")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect::<Vec<_>>();
        if !group_ids.iter().any(|existing| existing == &group_id) {
            group_ids.push(group_id);
            match state
                .invoke(
                    "profile_groups_replace",
                    &request_id("create-profile-groups"),
                    Some(&request_id("create-profile-groups-idem")),
                    json!({
                        "workspaceId": workspace_id.clone(),
                        "profileId": profile_id,
                        "expectedRevision": revision,
                        "groupIds": group_ids,
                    }),
                )
                .await
            {
                Ok(grouped) => created = grouped.data,
                Err(group_error) => {
                    let _ = app.emit("profiles-changed", json!({ "profileId": profile_id }));
                    return Err(Problem::new(
                        "profile_created_group_assignment_failed",
                        "Profile was created but group assignment failed; retry to resume",
                        group_error.status,
                        request_id("create-profile-groups"),
                    )
                    .with_detail(format!("{} ({})", group_error.title, group_error.code))
                    .with_field("profileId", profile_id));
                }
            }
        } else {
            created = current;
        }
    }
    let profile = legacy_profile(&created);
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

#[tauri::command(rename_all = "camelCase")]
pub async fn update_wayfern_config(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
    config: Value,
) -> Result<Value, Problem> {
    let correlation_id = request_id("profile-fingerprint-update");
    let target = legacy_fingerprint_target(Some(&config), &correlation_id)?.ok_or_else(|| {
        Problem::new(
            "fingerprint_target_required",
            "Select a fingerprint operating system",
            400,
            &correlation_id,
        )
    })?;
    ensure_local_fingerprint_target(&target, &correlation_id)?;

    let profile = remote_profile_record(&state, &profile_id).await?;
    let workspace_id = profile
        .get("workspaceId")
        .and_then(Value::as_str)
        .filter(|workspace_id| !workspace_id.trim().is_empty())
        .ok_or_else(|| invalid_response(&correlation_id))?;
    let expected_revision = profile
        .get("revision")
        .and_then(Value::as_u64)
        .filter(|revision| *revision > 0)
        .ok_or_else(|| invalid_response(&correlation_id))?;
    let name = profile
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_response(&correlation_id))?;
    let tags = profile
        .get("tags")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_response(&correlation_id))?;
    let runtime_config = json!({
        "schemaVersion": buglogin_domain::entities::PROFILE_RUNTIME_CONFIG_SCHEMA_VERSION,
        "wayfern": { "fingerprintTarget": target },
    });
    let updated = state
        .invoke(
            "profile_update",
            &correlation_id,
            Some(&request_id("profile-fingerprint-idempotency")),
            json!({
                "workspaceId": workspace_id,
                "profileId": profile_id,
                "expectedRevision": expected_revision,
                "name": name,
                "note": profile.get("note").cloned().unwrap_or(Value::Null),
                "tags": tags,
                "runtimeConfig": runtime_config,
            }),
        )
        .await?;
    let _ = app.emit("profiles-changed", json!({ "profileId": profile_id }));
    Ok(legacy_profile(&updated.data))
}

fn legacy_fingerprint_target(
    config: Option<&Value>,
    correlation_id: &str,
) -> Result<Option<Value>, Problem> {
    let Some(config) = config else {
        return Ok(None);
    };
    if config
        .get("randomize_fingerprint_on_launch")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || config
            .get("fingerprint")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    {
        return Err(Problem::new(
            "fingerprint_configuration_unsupported",
            "Custom and per-launch fingerprint settings are not supported by this BugLogin runtime",
            422,
            correlation_id,
        ));
    }
    let Some(os) = config.get("os").and_then(Value::as_str) else {
        return Ok(None);
    };
    let normalized = match os.to_ascii_lowercase().as_str() {
        "windows" => "windows",
        "macos" | "mac" => "macos",
        "linux" => "linux",
        "android" => "android",
        "ios" => "ios",
        _ => {
            return Err(Problem::new(
                "fingerprint_os_invalid",
                "The selected fingerprint operating system is invalid",
                400,
                correlation_id,
            ));
        }
    };
    Ok(Some(json!({ "operatingSystem": normalized })))
}

fn ensure_local_fingerprint_target(target: &Value, correlation_id: &str) -> Result<(), Problem> {
    let requested = target
        .get("operatingSystem")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_response(correlation_id))?;
    match requested {
        "windows" | "macos" | "linux" => Ok(()),
        _ => Err(Problem::new(
            "fingerprint_provider_unavailable",
            "This BugLogin build can generate fingerprints only for supported desktop operating systems (Windows, macOS, Linux)",
            409,
            correlation_id,
        )),
    }
}

#[tauri::command]
pub async fn launch_browser_profile(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile: Option<Value>,
    profile_id: Option<String>,
) -> Result<Value, Problem> {
    let (profile, profile_id) = profile_argument(profile, profile_id)?;
    if has_profile_password(&profile_id) && !profile_password::take_unlock(&profile_id) {
        return Err(profile_password::locked_problem());
    }
    let correlation_id = request_id("profile_launch");
    let launch = async {
        crate::ensure_profile_runtime_installed(
            app.clone(),
            state.inner(),
            &profile_id,
            &correlation_id,
        )
        .await?;
        run_operation_result(
            &state,
            "profile_launch",
            json!({ "profileId": profile_id, "headless": false }),
        )
        .await
    }
    .await;
    let receipt = match launch {
        Ok(receipt) => receipt,
        Err(error) => {
            crate::diagnostics::report_problem_failure(
                state.inner().clone(),
                "profile.launch",
                error.clone(),
            );
            return Err(error);
        }
    };
    if let Some(warnings) = receipt
        .get("extensionWarnings")
        .and_then(Value::as_array)
        .filter(|warnings| !warnings.is_empty())
    {
        let _ = app.emit(
            "profile-extension-warning",
            json!({ "profileId": profile_id, "warnings": warnings }),
        );
    }
    if let Some(warning) = receipt
        .get("fingerprintWarning")
        .filter(|warning| !warning.is_null())
    {
        let _ = app.emit(
            "profile-fingerprint-warning",
            json!({"profileId":profile_id,"warning":warning}),
        );
    }
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
    profile: Option<Value>,
    profile_id: Option<String>,
) -> Result<(), Problem> {
    let (_, profile_id) = profile_argument(profile, profile_id)?;
    let correlation_id = request_id("profile-stop-local");
    let process_stopped = state
        .stop_browser_process_locally(&profile_id, &correlation_id)
        .await?;
    // Production stops the device-owned process before managed sync starts.
    // Local API mode stops inside `profile_stop`, so emitting early there would
    // temporarily claim the still-running process had exited.
    if process_stopped {
        emit_profile_stopped(&app, &profile_id)?;
    }
    let _ = app.emit(
        "profile-sync-status",
        json!({ "profile_id": profile_id, "status": "syncing" }),
    );
    if let Err(problem) =
        run_operation(&state, "profile_stop", json!({ "profileId": profile_id })).await
    {
        // The process may already be closed while cloud sync is being retried.
        // Keep the UI's process state truthful even when the sync operation fails.
        crate::diagnostics::report_problem_failure(
            state.inner().clone(),
            "profile.stop",
            problem.clone(),
        );
        let _ = app.emit(
            "profile-sync-status",
            json!({ "profile_id": profile_id, "status": "error", "error": problem.code }),
        );
        return Err(problem);
    }
    if !process_stopped {
        emit_profile_stopped(&app, &profile_id)?;
    }
    let _ = app.emit(
        "profile-sync-status",
        json!({ "profile_id": profile_id, "status": "synced" }),
    );
    Ok(())
}

fn emit_profile_stopped(app: &AppHandle, profile_id: &str) -> Result<(), Problem> {
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
    })
}

#[tauri::command]
pub async fn get_all_traffic_snapshots() -> Result<Vec<Value>, Problem> {
    Ok(Vec::new())
}

#[tauri::command]
pub async fn check_browser_status(
    state: State<'_, DesktopState>,
    profile: Value,
) -> Result<bool, Problem> {
    let profile_id = profile_id(&profile)?;
    Ok(state
        .active_browser_profiles()
        .await?
        .iter()
        .any(|active_id| active_id == profile_id))
}

#[tauri::command]
pub async fn list_running_browser_profiles(
    state: State<'_, DesktopState>,
) -> Result<Vec<String>, Problem> {
    state.active_browser_profiles().await
}

async fn run_operation(state: &DesktopState, command: &str, payload: Value) -> Result<(), Problem> {
    run_operation_result(state, command, payload)
        .await
        .map(|_| ())
}

async fn run_operation_result(
    state: &DesktopState,
    command: &str,
    payload: Value,
) -> Result<Value, Problem> {
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

    // Large profiles can take minutes to hydrate or upload; launch/stop are
    // serialized per profile, so waiting longer never starts a parallel run.
    for _ in 0..6_000 {
        let operation = state
            .invoke(
                "operation_get",
                &correlation_id,
                None,
                json!({ "operationId": operation_id }),
            )
            .await?;
        match operation.data.get("status").and_then(Value::as_str) {
            Some("completed") => {
                return Ok(operation.data.get("result").cloned().unwrap_or(Value::Null));
            }
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

pub(crate) fn load_local_profile_settings(profile_id: &str) -> Option<Value> {
    let profile_id = canonical_dir_name(profile_id).ok()?;
    let data_dir = local_profile_settings_base()?;
    for root in [
        data_dir.join("BugLogin").join("profiles").join(profile_id),
        data_dir
            .join("BugLogin-LocalApi")
            .join("profiles")
            .join(profile_id),
    ] {
        let settings_path = root.join("local_settings.json");
        if let Ok(bytes) = std::fs::read(&settings_path) {
            if let Ok(val) = serde_json::from_slice::<Value>(&bytes) {
                return Some(val);
            }
        }
    }
    None
}

pub(crate) fn profile_window_color(profile_id: &str) -> String {
    load_local_profile_settings(profile_id)
        .and_then(|settings| {
            settings
                .get("window_color")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .filter(|color| {
            let hex = color.strip_prefix('#').unwrap_or(color);
            hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .map(|color| format!("#{}", color.trim_start_matches('#').to_ascii_lowercase()))
        .unwrap_or_else(|| crate::adapter::profile_color_for_id(profile_id))
}

pub(crate) fn has_profile_password(profile_id: &str) -> bool {
    let local = load_local_profile_settings(profile_id);
    let hash = local
        .as_ref()
        .and_then(|l| l.get("password_hash"))
        .and_then(Value::as_str)
        .unwrap_or("");
    !hash.is_empty()
}

fn save_local_profile_settings(
    profile_id: &str,
    mutate: impl FnOnce(&mut Value),
) -> Result<Value, Problem> {
    let profile_id = canonical_dir_name(profile_id).map_err(|_| {
        Problem::new(
            "profile_id_invalid",
            "Profile identifier is invalid",
            400,
            "local-settings",
        )
    })?;
    let data_dir = local_profile_settings_base().ok_or_else(|| {
        Problem::new(
            "data_dir_unavailable",
            "Data directory unavailable",
            500,
            "local-settings",
        )
    })?;
    let dir = data_dir.join("BugLogin").join("profiles").join(profile_id);
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("local_settings.json");
    let mut current = if let Ok(bytes) = std::fs::read(&path) {
        serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };
    mutate(&mut current);
    let bytes = serde_json::to_vec_pretty(&current)
        .map_err(|e| Problem::new("serialization_failed", e.to_string(), 500, "local-settings"))?;
    std::fs::write(&path, bytes)
        .map_err(|e| Problem::new("io_failed", e.to_string(), 500, "local-settings"))?;
    Ok(current)
}

fn local_profile_settings_base() -> Option<PathBuf> {
    let base = if let Ok(dir) = std::env::var("BUGLOGIN_DATA_DIR") {
        PathBuf::from(dir)
    } else {
        dirs::data_local_dir()?
    };
    Some(AppInstance::app_data_dir(base))
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

    let id_str = profile.get("id").and_then(Value::as_str).unwrap_or("");
    let local = if !id_str.is_empty() {
        load_local_profile_settings(id_str)
    } else {
        None
    };

    let clear_on_close = local
        .as_ref()
        .and_then(|l| l.get("clear_on_close"))
        .cloned()
        .unwrap_or_else(|| json!(false));

    let window_color = json!(profile_window_color(id_str));

    let launch_hook = local
        .as_ref()
        .and_then(|l| l.get("launch_hook"))
        .cloned()
        .unwrap_or(Value::Null);

    let start_urls = local
        .as_ref()
        .and_then(|l| l.get("start_urls"))
        .cloned()
        .unwrap_or_else(|| json!([]));

    let dns_server = local
        .as_ref()
        .and_then(|l| l.get("dns_server"))
        .cloned()
        .unwrap_or(Value::Null);

    let dns_blocklist = local
        .as_ref()
        .and_then(|l| l.get("dns_blocklist"))
        .cloned()
        .unwrap_or(Value::Null);

    let proxy_bypass_rules = local
        .as_ref()
        .and_then(|l| l.get("proxy_bypass_rules"))
        .cloned()
        .unwrap_or_else(|| json!([]));

    let is_password_protected = local
        .as_ref()
        .and_then(|l| l.get("password_hash"))
        .is_some();

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
        "note": profile.get("note").cloned().unwrap_or(Value::Null),
        "extension_group_id": profile.get("extensionGroupId").cloned().unwrap_or(Value::Null),
        "revision": profile.get("revision").cloned().unwrap_or(Value::Null),
        "wayfern_config": legacy_wayfern_config(profile),
        "workspaceId": profile.get("workspaceId").cloned().unwrap_or(Value::Null),
        "sync_mode": legacy_sync_mode(profile.get("syncMode")),
        "clear_on_close": clear_on_close,
        "window_color": window_color,
        "launch_hook": launch_hook,
        "start_urls": start_urls,
        "dns_server": dns_server,
        "dns_blocklist": dns_blocklist,
        "proxy_bypass_rules": proxy_bypass_rules,
        "password_protected": is_password_protected,
    })
}

fn legacy_wayfern_config(profile: &Value) -> Value {
    profile
        .pointer("/runtimeConfig/wayfern/fingerprintTarget/operatingSystem")
        .and_then(Value::as_str)
        .map(|os| json!({ "os": os }))
        .unwrap_or(Value::Null)
}

fn legacy_sync_mode(value: Option<&Value>) -> Value {
    match value.and_then(Value::as_str) {
        Some("managed") => json!("Regular"),
        Some("local") => json!("Disabled"),
        Some("disabled") => json!("Disabled"),
        _ => Value::Null,
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

fn profile_argument(
    profile: Option<Value>,
    requested_profile_id: Option<String>,
) -> Result<(Value, String), Problem> {
    let provided_profile_id = requested_profile_id.filter(|id| !id.trim().is_empty());
    match (profile, provided_profile_id) {
        (Some(profile), Some(id)) => {
            let embedded_id = profile_id(&profile)?;
            if embedded_id != id {
                return Err(Problem::new(
                    "profile_id_mismatch",
                    "Profile id does not match the supplied profile",
                    400,
                    request_id("profile"),
                ));
            }
            Ok((profile, id))
        }
        (Some(profile), None) => {
            let id = profile_id(&profile)?.to_owned();
            Ok((profile, id))
        }
        (None, Some(id)) => Ok((json!({ "id": id }), id)),
        (None, None) => Err(Problem::new(
            "profile_id_required",
            "Profile id is required",
            400,
            request_id("profile"),
        )),
    }
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

fn replacement_group_ids(group_id: Option<&str>) -> Vec<String> {
    group_id.map(str::to_owned).into_iter().collect()
}

#[tauri::command(rename_all = "camelCase")]
pub async fn get_profile_traffic_snapshot(
    _state: State<'_, DesktopState>,
    _profile_id: String,
) -> Result<Option<Value>, Problem> {
    Ok(None)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn update_profile_clear_on_close(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
    clear_on_close: bool,
) -> Result<Value, Problem> {
    let _ = save_local_profile_settings(&profile_id, |s| {
        s["clear_on_close"] = json!(clear_on_close);
    });
    let _ = app.emit("profiles-changed", json!({ "profileId": profile_id }));
    let profile = remote_profile_record(&state, &profile_id).await?;
    Ok(legacy_profile(&profile))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn update_profile_window_color(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
    window_color: Option<String>,
) -> Result<Value, Problem> {
    let _ = save_local_profile_settings(&profile_id, |s| {
        s["window_color"] = json!(window_color);
    });
    let _ = app.emit("profiles-changed", json!({ "profileId": profile_id }));
    let profile = remote_profile_record(&state, &profile_id).await?;
    Ok(legacy_profile(&profile))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn update_profile_launch_hook(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
    launch_hook: Option<String>,
) -> Result<Value, Problem> {
    let _ = save_local_profile_settings(&profile_id, |s| {
        s["launch_hook"] = json!(launch_hook);
    });
    let _ = app.emit("profiles-changed", json!({ "profileId": profile_id }));
    let profile = remote_profile_record(&state, &profile_id).await?;
    Ok(legacy_profile(&profile))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn update_profile_start_urls(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
    start_urls: Vec<String>,
) -> Result<Value, Problem> {
    let _ = save_local_profile_settings(&profile_id, |s| {
        s["start_urls"] = json!(start_urls);
    });
    let _ = app.emit("profiles-changed", json!({ "profileId": profile_id }));
    let profile = remote_profile_record(&state, &profile_id).await?;
    Ok(legacy_profile(&profile))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn set_profile_sync_mode(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
    sync_mode: String,
) -> Result<Value, Problem> {
    let _ = save_local_profile_settings(&profile_id, |s| {
        s["sync_mode"] = json!(sync_mode);
    });
    let _ = app.emit("profiles-changed", json!({ "profileId": profile_id }));
    let profile = remote_profile_record(&state, &profile_id).await?;
    Ok(legacy_profile(&profile))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn update_profile_dns_server(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
    dns_server: Option<String>,
) -> Result<Value, Problem> {
    let _ = save_local_profile_settings(&profile_id, |s| {
        s["dns_server"] = json!(dns_server);
    });
    let _ = app.emit("profiles-changed", json!({ "profileId": profile_id }));
    let profile = remote_profile_record(&state, &profile_id).await?;
    Ok(legacy_profile(&profile))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn update_profile_dns_blocklist(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
    dns_blocklist: Option<String>,
) -> Result<Value, Problem> {
    let _ = save_local_profile_settings(&profile_id, |s| {
        s["dns_blocklist"] = json!(dns_blocklist);
    });
    let _ = app.emit("profiles-changed", json!({ "profileId": profile_id }));
    let profile = remote_profile_record(&state, &profile_id).await?;
    Ok(legacy_profile(&profile))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn update_profile_proxy_bypass_rules(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
    rules: Vec<String>,
) -> Result<Value, Problem> {
    let _ = save_local_profile_settings(&profile_id, |s| {
        s["proxy_bypass_rules"] = json!(rules);
    });
    let _ = app.emit("profiles-changed", json!({ "profileId": profile_id }));
    let profile = remote_profile_record(&state, &profile_id).await?;
    Ok(legacy_profile(&profile))
}

fn invalid_profile_password(title: &str) -> Problem {
    Problem::new("invalid_password", title, 401, "profile-password")
}

fn store_profile_password_hash(profile_id: &str, phc: &str) -> Result<Value, Problem> {
    save_local_profile_settings(profile_id, |s| {
        if let Some(obj) = s.as_object_mut() {
            obj.remove("password_salt");
        }
        s["password_hash"] = json!(phc);
    })
}

#[tauri::command(rename_all = "camelCase")]
pub async fn verify_profile_password(
    _state: State<'_, DesktopState>,
    profile_id: String,
    password: String,
) -> Result<(), Problem> {
    let local = load_local_profile_settings(&profile_id);
    match profile_password::check_password(local.as_ref(), &password) {
        PasswordCheck::NotSet => Ok(()),
        PasswordCheck::Valid { upgrade } => {
            if let Some(phc) = upgrade {
                let _ = store_profile_password_hash(&profile_id, &phc);
            }
            Ok(())
        }
        PasswordCheck::Invalid => Err(invalid_profile_password("Mật khẩu profile không chính xác")),
    }
}

#[tauri::command(rename_all = "camelCase")]
pub async fn set_profile_password(
    app: AppHandle,
    profile_id: String,
    password: String,
) -> Result<(), Problem> {
    let local = load_local_profile_settings(&profile_id);
    profile_password::ensure_password_can_be_set(local.as_ref())?;
    let phc = profile_password::hash_password(&password)?;
    store_profile_password_hash(&profile_id, &phc)?;
    let _ = app.emit("profiles-changed", json!({ "profileId": profile_id }));
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn change_profile_password(
    app: AppHandle,
    profile_id: String,
    old_password: String,
    new_password: String,
) -> Result<(), Problem> {
    let local = load_local_profile_settings(&profile_id);
    if profile_password::check_password(local.as_ref(), &old_password) == PasswordCheck::Invalid {
        return Err(invalid_profile_password(
            "Mật khẩu hiện tại không chính xác",
        ));
    }
    let phc = profile_password::hash_password(&new_password)?;
    store_profile_password_hash(&profile_id, &phc)?;
    let _ = app.emit("profiles-changed", json!({ "profileId": profile_id }));
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn remove_profile_password(
    app: AppHandle,
    profile_id: String,
    password: String,
) -> Result<(), Problem> {
    let local = load_local_profile_settings(&profile_id);
    if profile_password::check_password(local.as_ref(), &password) == PasswordCheck::Invalid {
        return Err(invalid_profile_password(
            "Mật khẩu hiện tại không chính xác",
        ));
    }

    save_local_profile_settings(&profile_id, |s| {
        if let Some(obj) = s.as_object_mut() {
            obj.remove("password_salt");
            obj.remove("password_hash");
        }
    })?;
    let _ = app.emit("profiles-changed", json!({ "profileId": profile_id }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        LegacyProxyExport, choose_workspace_selection, count_profiles_for_workspace,
        legacy_profile, normalize_runtime, parse_txt_proxies, profile_argument,
        replacement_group_ids,
    };
    use crate::desktop_state::DesktopState;
    use buglogin_domain::errors::Problem;
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
    fn bulk_profile_argument_accepts_id_and_rejects_mismatched_identity() {
        let (profile, id) = profile_argument(None, Some("profile:1".to_owned())).unwrap();
        assert_eq!(profile["id"], "profile:1");
        assert_eq!(id, "profile:1");

        let error = profile_argument(
            Some(json!({"id":"profile:1"})),
            Some("profile:2".to_owned()),
        )
        .unwrap_err();
        assert_eq!(error.code, "profile_id_mismatch");
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

    #[test]
    fn workspace_restore_keeps_accessible_choice_and_defaults_by_id() {
        let workspaces = [
            json!({"id":"ws_zeta","name":"Alpha"}),
            json!({"id":"ws_beta","name":"Zulu"}),
        ];
        assert_eq!(
            choose_workspace_selection(Some("ws_zeta"), &workspaces).as_deref(),
            Some("ws_zeta")
        );
        assert_eq!(
            choose_workspace_selection(Some("ws_revoked"), &workspaces).as_deref(),
            Some("ws_beta")
        );
        assert_eq!(choose_workspace_selection(None, &[]), None);
    }

    #[tokio::test]
    async fn inaccessible_profile_count_stays_unavailable_instead_of_becoming_zero() {
        let state = DesktopState::Unavailable(Problem::new(
            "profile_forbidden",
            "Profile access denied",
            403,
            "count-test",
        ));
        assert_eq!(count_profiles_for_workspace(&state, "ws_count").await, None);
    }

    #[test]
    fn group_move_replaces_membership_and_no_group_clears_it() {
        assert_eq!(
            replacement_group_ids(Some("grp_destination")),
            ["grp_destination"]
        );
        assert!(replacement_group_ids(None).is_empty());
    }

    #[test]
    fn fingerprint_target_supports_cross_os_windows_macos_linux() {
        for os in ["windows", "macos", "linux"] {
            let target = json!({ "operatingSystem": os });
            assert!(super::ensure_local_fingerprint_target(&target, "cor-test").is_ok());
        }
        let invalid = json!({ "operatingSystem": "solaris" });
        assert_eq!(
            super::ensure_local_fingerprint_target(&invalid, "cor-test")
                .unwrap_err()
                .code,
            "fingerprint_provider_unavailable"
        );
    }
}
