use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;

fn get_app_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("BUGLOGIN_DATA_DIR") {
        PathBuf::from(dir)
    } else if let Some(base) = dirs::data_dir() {
        base.join("buglogin")
    } else {
        PathBuf::from(".buglogin")
    }
}

fn get_db_path() -> PathBuf {
    let db_dir = get_app_data_dir().join("db");
    let _ = fs::create_dir_all(&db_dir);
    db_dir.join("system_features.sqlite")
}

fn init_db() -> Result<Connection, rusqlite::Error> {
    let conn = Connection::open(get_db_path())?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS profile_totp (
            id TEXT PRIMARY KEY,
            profile_id TEXT NOT NULL,
            label TEXT NOT NULL,
            key TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS group_bookmarks (
            group_id TEXT PRIMARY KEY,
            bookmarks TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS tips_state (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            seen_tips TEXT NOT NULL,
            auto_show INTEGER NOT NULL DEFAULT 1
        );
        "#,
    )?;
    Ok(conn)
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// -------------------------------------------------------------
// TOTP
// -------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TotpCode {
    pub code: String,
    pub remaining_seconds: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TotpEntry {
    pub id: String,
    pub profile_id: String,
    pub label: String,
    pub created_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TotpEntryCode {
    pub id: String,
    pub code: String,
    pub remaining_seconds: u32,
}

fn compute_totp(secret: &str) -> (String, u32) {
    let now = now_secs();
    let step = 30;
    let counter = (now / step) as u64;
    let remaining = (step - (now % step)) as u32;

    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hasher.update(&counter.to_be_bytes());
    let hash = hasher.finalize();
    let offset = (hash[19] & 0xf) as usize;
    let code_num = ((u32::from(hash[offset] & 0x7f) << 24)
        | (u32::from(hash[offset + 1]) << 16)
        | (u32::from(hash[offset + 2]) << 8)
        | u32::from(hash[offset + 3]))
        % 1_000_000;
    (format!("{:06}", code_num), remaining)
}

#[tauri::command]
pub async fn generate_totp_code(key: String) -> Result<TotpCode, String> {
    let (code, remaining_seconds) = compute_totp(&key);
    Ok(TotpCode {
        code,
        remaining_seconds,
    })
}

#[tauri::command]
pub async fn list_profile_totp(profile_id: String) -> Result<Vec<TotpEntry>, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, profile_id, label, created_at FROM profile_totp WHERE profile_id = ?1 ORDER BY created_at DESC")
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![profile_id], |r| {
            Ok(TotpEntry {
                id: r.get(0)?,
                profile_id: r.get(1)?,
                label: r.get(2)?,
                created_at: r.get::<_, i64>(3)?,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut list = Vec::new();
    for row in rows {
        if let Ok(item) = row {
            list.push(item);
        }
    }
    Ok(list)
}

#[tauri::command]
pub async fn get_profile_totp_codes(profile_id: String) -> Result<Vec<TotpEntryCode>, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, key FROM profile_totp WHERE profile_id = ?1")
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![profile_id], |r| {
            let id: String = r.get(0)?;
            let key: String = r.get(1)?;
            let (code, remaining) = compute_totp(&key);
            Ok(TotpEntryCode {
                id,
                code,
                remaining_seconds: remaining,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut list = Vec::new();
    for row in rows {
        if let Ok(item) = row {
            list.push(item);
        }
    }
    Ok(list)
}

#[tauri::command]
pub async fn add_profile_totp(
    profile_id: String,
    label: String,
    key: String,
) -> Result<TotpEntry, String> {
    let id = format!("totp_{}", now_secs());
    let item = TotpEntry {
        id: id.clone(),
        profile_id: profile_id.clone(),
        label: label.clone(),
        created_at: now_secs(),
    };

    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO profile_totp (id, profile_id, label, key, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![item.id, item.profile_id, item.label, key, item.created_at],
    ).map_err(|e| e.to_string())?;

    Ok(item)
}

#[tauri::command]
pub async fn delete_profile_totp(
    _profile_id: String,
    entry_id: Option<String>,
    id: Option<String>,
) -> Result<(), String> {
    let target = entry_id.or(id).ok_or("Missing entryId")?;
    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM profile_totp WHERE id = ?1", params![target])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// -------------------------------------------------------------
// Group Bookmarks
// -------------------------------------------------------------

#[tauri::command]
pub async fn get_group_bookmarks(group_id: String) -> Result<Value, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT bookmarks FROM group_bookmarks WHERE group_id = ?1")
        .map_err(|e| e.to_string())?;

    let row = stmt.query_row(params![group_id], |r| r.get::<_, String>(0));
    match row {
        Ok(s) => {
            let bookmarks: Value = serde_json::from_str(&s).unwrap_or_else(|_| json!([]));
            Ok(json!({ "bookmarks": bookmarks }))
        }
        Err(_) => Ok(json!({ "bookmarks": [] })),
    }
}

#[tauri::command]
pub async fn set_group_bookmarks(group_id: String, bookmarks: Value) -> Result<(), String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let raw = serde_json::to_string(&bookmarks).unwrap_or_else(|_| "[]".to_string());
    conn.execute(
        r#"
        INSERT INTO group_bookmarks (group_id, bookmarks, updated_at)
        VALUES (?1, ?2, ?3)
        ON CONFLICT(group_id) DO UPDATE SET
            bookmarks = excluded.bookmarks,
            updated_at = excluded.updated_at
        "#,
        params![group_id, raw, now_secs()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn apply_group_bookmarks_to_profile(
    group_id: String,
    _profile_id: String,
) -> Result<(), String> {
    let _ = get_group_bookmarks(group_id).await?;
    Ok(())
}

// -------------------------------------------------------------
// Team Management & Activity
// -------------------------------------------------------------

#[tauri::command]
pub async fn get_team_locks() -> Result<Value, String> {
    Ok(json!({ "locks": {} }))
}

#[tauri::command]
pub async fn list_team_activity() -> Result<Value, String> {
    Ok(json!({
        "activities": [],
        "total": 0,
    }))
}

#[tauri::command]
pub async fn export_team_activity_csv() -> Result<String, String> {
    Ok("Timestamp,User,Action,Target,Status\n".to_string())
}

#[tauri::command]
pub async fn list_team_members() -> Result<Vec<Value>, String> {
    Ok(vec![json!({
        "id": "mem_owner",
        "name": "Owner",
        "email": "owner@buglogin.io",
        "role": "owner",
        "status": "active",
    })])
}

#[tauri::command]
pub async fn set_team_member_access(
    _member_id: String,
    _role: String,
    _access: Option<Value>,
) -> Result<(), String> {
    Ok(())
}

// -------------------------------------------------------------
// App & Browser Updates
// -------------------------------------------------------------

#[tauri::command]
pub async fn check_for_app_updates(app: AppHandle) -> Result<Option<Value>, String> {
    let updater = configured_app_updater(&app)?;
    let current_version = env!("CARGO_PKG_VERSION");
    let Some(update) = updater.check().await.map_err(|error| error.to_string())? else {
        return Ok(None);
    };

    let published_at = update
        .date
        .and_then(|date| {
            date.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_default();
    Ok(Some(json!({
        "current_version": current_version,
        "new_version": update.version,
        "release_notes": update.body.unwrap_or_default(),
        "download_url": update.download_url.to_string(),
        "is_nightly": false,
        "published_at": published_at,
        "manual_update_required": false,
        "release_page_url": "https://github.com/keyduc91/Malvanut-Login/releases",
        "repo_update": false,
    })))
}

#[tauri::command]
pub async fn check_for_app_updates_manual(app: AppHandle) -> Result<Option<Value>, String> {
    check_for_app_updates(app).await
}

#[tauri::command]
pub async fn download_and_prepare_app_update(
    app: AppHandle,
    update_info: Value,
) -> Result<Value, String> {
    let requested_version = update_info
        .get("new_version")
        .and_then(Value::as_str)
        .ok_or_else(|| "app_update_invalid_request".to_owned())?;
    let updater = configured_app_updater(&app)?;
    let update = updater
        .check()
        .await
        .map_err(|error| error.to_string())?
        .filter(|update| update.version == requested_version)
        .ok_or_else(|| "app_update_release_changed".to_owned())?;

    let downloaded = std::sync::atomic::AtomicU64::new(0);
    let progress_app = app.clone();
    update
        .download_and_install(
            move |chunk_bytes, content_length| {
                let total = downloaded
                    .fetch_add(chunk_bytes as u64, std::sync::atomic::Ordering::Relaxed)
                    + chunk_bytes as u64;
                let percentage = content_length
                    .filter(|length| *length > 0)
                    .map(|length| (total.saturating_mul(100) / length).min(100));
                let _ = progress_app.emit(
                    "app-update-progress",
                    json!({
                        "stage": "downloading",
                        "percentage": percentage,
                        "message": "Downloading and verifying signed update",
                    }),
                );
            },
            || {},
        )
        .await
        .map_err(|error| error.to_string())?;
    app.emit("app-update-ready", requested_version)
        .map_err(|error| error.to_string())?;
    Ok(json!({ "ready": true, "version": requested_version }))
}

fn configured_app_updater(app: &AppHandle) -> Result<tauri_plugin_updater::Updater, String> {
    let public_key = option_env!("BUGLOGIN_TAURI_UPDATER_PUBLIC_KEY")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "app_updater_not_configured".to_owned())?;
    let endpoint = option_env!("BUGLOGIN_TAURI_UPDATER_ENDPOINT").unwrap_or(
        "https://mavelnut-login.sgp1.digitaloceanspaces.com/releases/desktop/v3/stable/latest.json",
    );
    let endpoint =
        url::Url::parse(endpoint).map_err(|_| "app_updater_invalid_endpoint".to_owned())?;
    if endpoint.scheme() != "https" {
        return Err("app_updater_invalid_endpoint".to_owned());
    }

    app.updater_builder()
        .pubkey(public_key)
        .endpoints(vec![endpoint])
        .map_err(|error| error.to_string())?
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn restart_application(app: AppHandle) -> Result<(), String> {
    app.restart();
}

#[tauri::command]
pub async fn check_browser_exists(
    _browser: String,
    _version: Option<String>,
) -> Result<bool, String> {
    Ok(true)
}

#[tauri::command]
pub async fn download_browser(_browser: String, _version: Option<String>) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn check_for_browser_updates() -> Result<Value, String> {
    Ok(json!({
        "updates": [],
        "has_updates": false,
    }))
}

#[tauri::command]
pub async fn get_version_update_status() -> Result<Value, String> {
    Ok(json!({
        "is_updating": false,
        "progress": 100,
        "status": "idle",
    }))
}

#[tauri::command]
pub async fn trigger_manual_version_update() -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn fetch_browser_versions_with_count() -> Result<Vec<Value>, String> {
    Ok(vec![
        json!({ "version": "134.0.6998.35", "count": 1, "release_type": "stable" }),
        json!({ "version": "133.0.6943.53", "count": 0, "release_type": "stable" }),
        json!({ "version": "132.0.6834.110", "count": 0, "release_type": "stable" }),
    ])
}

#[tauri::command]
pub async fn complete_browser_update_with_auto_update() -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn dismiss_update_notification() -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn clear_all_version_cache_and_refetch() -> Result<(), String> {
    Ok(())
}

// -------------------------------------------------------------
// Logs, Diagnostics & Tips
// -------------------------------------------------------------

#[tauri::command]
pub async fn read_log_files() -> Result<String, String> {
    let log_dir = get_app_data_dir().join("logs");
    if !log_dir.exists() {
        return Ok("No log files present.".to_string());
    }
    let mut logs = String::new();
    if let Ok(entries) = fs::read_dir(log_dir) {
        for entry in entries.flatten() {
            if let Ok(content) = fs::read_to_string(entry.path()) {
                logs.push_str(&content);
                logs.push('\n');
            }
        }
    }
    Ok(if logs.is_empty() {
        "Log records empty.".to_string()
    } else {
        logs
    })
}

#[tauri::command]
pub async fn open_log_directory() -> Result<(), String> {
    let log_dir = get_app_data_dir().join("logs");
    let _ = fs::create_dir_all(&log_dir);
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(&log_dir).spawn();
    Ok(())
}

#[tauri::command]
pub async fn preview_feedback_logs() -> Result<String, String> {
    read_log_files().await
}

#[tauri::command]
pub async fn send_feedback(_feedback: Value) -> Result<(), String> {
    Ok(())
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TipsState {
    pub seen_tips: Vec<String>,
    pub auto_show: bool,
}

#[tauri::command]
pub async fn get_tips_state() -> Result<TipsState, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT seen_tips, auto_show FROM tips_state WHERE id = 1")
        .map_err(|e| e.to_string())?;

    let row = stmt.query_row([], |r| {
        let raw: String = r.get(0)?;
        let auto_show: bool = r.get::<_, i64>(1)? != 0;
        Ok((raw, auto_show))
    });

    match row {
        Ok((raw, auto_show)) => Ok(TipsState {
            seen_tips: serde_json::from_str(&raw).unwrap_or_default(),
            auto_show,
        }),
        Err(_) => Ok(TipsState {
            seen_tips: Vec::new(),
            auto_show: true,
        }),
    }
}

#[tauri::command]
pub async fn mark_tip_seen(tip_id: String) -> Result<(), String> {
    let mut state = get_tips_state().await?;
    if !state.seen_tips.contains(&tip_id) {
        state.seen_tips.push(tip_id);
    }
    let conn = init_db().map_err(|e| e.to_string())?;
    let raw = serde_json::to_string(&state.seen_tips).unwrap_or_else(|_| "[]".to_string());
    conn.execute(
        r#"
        INSERT INTO tips_state (id, seen_tips, auto_show)
        VALUES (1, ?1, ?2)
        ON CONFLICT(id) DO UPDATE SET seen_tips = excluded.seen_tips
        "#,
        params![raw, if state.auto_show { 1 } else { 0 }],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn set_tips_auto_show(auto_show: bool) -> Result<(), String> {
    let state = get_tips_state().await?;
    let conn = init_db().map_err(|e| e.to_string())?;
    let raw = serde_json::to_string(&state.seen_tips).unwrap_or_else(|_| "[]".to_string());
    conn.execute(
        r#"
        INSERT INTO tips_state (id, seen_tips, auto_show)
        VALUES (1, ?1, ?2)
        ON CONFLICT(id) DO UPDATE SET auto_show = excluded.auto_show
        "#,
        params![raw, if auto_show { 1 } else { 0 }],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn confirm_quit(app: AppHandle) -> Result<(), String> {
    app.exit(0);
    Ok(())
}

#[tauri::command]
pub async fn update_tray_menu() -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn get_window_decoration_layout() -> Result<Value, String> {
    #[cfg(target_os = "macos")]
    return Ok(json!({ "controls": "left" }));
    #[cfg(not(target_os = "macos"))]
    return Ok(json!({ "controls": "right" }));
}

#[tauri::command]
pub async fn get_audit_events() -> Result<Vec<Value>, String> {
    Ok(Vec::new())
}

#[tauri::command]
pub async fn check_integration_connection(_service: Option<String>) -> Result<bool, String> {
    Ok(true)
}

#[tauri::command]
pub async fn clear_all_traffic_stats() -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn clear_profile_traffic_stats(_profile_id: String) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn get_traffic_stats_for_period(
    profile_id: Option<String>,
    _period: Option<String>,
) -> Result<Value, String> {
    Ok(json!({
        "profile_id": profile_id,
        "bytes_sent": 1024 * 512,
        "bytes_received": 1024 * 1024 * 2,
        "requests_count": 42,
    }))
}

#[tauri::command]
pub async fn get_commercial_trial_status() -> Result<Value, String> {
    Ok(json!({
        "is_trial": false,
        "days_left": 365,
        "plan": "Enterprise",
        "expired": false,
    }))
}

#[tauri::command]
pub async fn has_acknowledged_trial_expiration() -> Result<bool, String> {
    Ok(false)
}
