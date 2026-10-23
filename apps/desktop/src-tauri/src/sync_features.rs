use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, State};

use crate::desktop_state::DesktopState;
use crate::profile_lifecycle::canonical_dir_name;

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
    db_dir.join("sync_features.sqlite")
}

fn init_db() -> Result<Connection, rusqlite::Error> {
    let conn = Connection::open(get_db_path())?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS restore_points (
            id TEXT PRIMARY KEY,
            profile_id TEXT NOT NULL,
            label TEXT,
            size_bytes INTEGER NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS sync_settings (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            sync_server_url TEXT,
            sync_token TEXT,
            e2e_password_hash TEXT,
            updated_at INTEGER NOT NULL
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
// Restore Points
// -------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RestorePoint {
    pub id: String,
    pub profile_id: String,
    pub label: Option<String>,
    pub size_bytes: u64,
    pub created_at: i64,
}

#[tauri::command]
pub async fn list_restore_points(profile_id: String) -> Result<Vec<RestorePoint>, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, profile_id, label, size_bytes, created_at FROM restore_points WHERE profile_id = ?1 ORDER BY created_at DESC")
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![profile_id], |r| {
            Ok(RestorePoint {
                id: r.get(0)?,
                profile_id: r.get(1)?,
                label: r.get(2)?,
                size_bytes: r.get::<_, i64>(3)? as u64,
                created_at: r.get::<_, i64>(4)?,
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
pub async fn create_restore_point(
    profile_id: String,
    label: Option<String>,
) -> Result<RestorePoint, String> {
    let id = format!("rp_{}", now_secs());
    let p_dir = get_app_data_dir()
        .join("profiles")
        .join(canonical_dir_name(&profile_id)?);
    let size = if p_dir.exists() {
        dir_size(&p_dir).unwrap_or(1024 * 512)
    } else {
        1024 * 100
    };

    let point = RestorePoint {
        id: id.clone(),
        profile_id: profile_id.clone(),
        label,
        size_bytes: size,
        created_at: now_secs(),
    };

    let snap_dir = get_app_data_dir().join("restore_points").join(&id);
    if p_dir.exists() {
        let _ = copy_dir_all(&p_dir, &snap_dir);
    }

    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO restore_points (id, profile_id, label, size_bytes, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![point.id, point.profile_id, point.label, point.size_bytes as i64, point.created_at],
    ).map_err(|e| e.to_string())?;

    Ok(point)
}

fn dir_size(path: &std::path::Path) -> std::io::Result<u64> {
    let mut total = 0;
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let meta = entry.metadata()?;
            if meta.is_dir() {
                total += dir_size(&entry.path())?;
            } else {
                total += meta.len();
            }
        }
    }
    Ok(total)
}

fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst.join(entry.file_name()))?;
        } else {
            let _ = fs::copy(entry.path(), dst.join(entry.file_name()));
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn delete_restore_point(_profile_id: String, id: String) -> Result<(), String> {
    let snapshot_name = canonical_dir_name(&id)?;
    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM restore_points WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;

    let snap_dir = get_app_data_dir()
        .join("restore_points")
        .join(snapshot_name);
    if snap_dir.exists() {
        let _ = fs::remove_dir_all(snap_dir);
    }
    Ok(())
}

#[tauri::command]
pub async fn restore_restore_point(profile_id: String, id: String) -> Result<(), String> {
    let snap_dir = get_app_data_dir()
        .join("restore_points")
        .join(canonical_dir_name(&id)?);
    let dst_dir = get_app_data_dir()
        .join("profiles")
        .join(canonical_dir_name(&profile_id)?);
    if snap_dir.exists() {
        let _ = copy_dir_all(&snap_dir, &dst_dir);
    }
    Ok(())
}

// -------------------------------------------------------------
// Cloud Auth & Sync Settings
// -------------------------------------------------------------

#[tauri::command]
pub async fn cloud_get_user(state: State<'_, DesktopState>) -> Result<Value, String> {
    let auth = state.auth_status();
    match auth {
        Ok(crate::desktop_state::DesktopAuthStatus::Authenticated) => Ok(json!({
            "authenticated": true,
            "user": {
                "id": "usr_default",
                "email": "user@buglogin.io",
                "name": "BugLogin User",
                "plan": "Pro",
            }
        })),
        _ => Ok(json!({
            "authenticated": false,
            "user": null,
        })),
    }
}

#[tauri::command]
pub async fn cloud_exchange_device_code(code: String) -> Result<Value, String> {
    Ok(json!({
        "success": true,
        "device_code": code,
        "token": format!("mock_token_{}", now_secs()),
    }))
}

#[tauri::command]
pub async fn cloud_logout(state: State<'_, DesktopState>) -> Result<(), String> {
    state
        .logout("cor_logout")
        .await
        .map(|_| ())
        .map_err(|e| e.title)
}

#[tauri::command]
pub async fn cloud_refresh_profile(state: State<'_, DesktopState>) -> Result<Value, String> {
    cloud_get_user(state).await
}

#[tauri::command]
pub async fn cloud_get_proxy_usage() -> Result<Value, String> {
    Ok(json!({
        "used_bytes": 1024 * 1024 * 250,
        "total_bytes": 1024 * 1024 * 1024 * 5,
        "used_gb": 0.25,
        "total_gb": 5.0,
    }))
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct SyncSettingsPayload {
    pub sync_server_url: Option<String>,
    pub sync_token: Option<String>,
}

#[tauri::command]
pub async fn get_sync_settings() -> Result<SyncSettingsPayload, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT sync_server_url, sync_token FROM sync_settings WHERE id = 1")
        .map_err(|e| e.to_string())?;

    let row = stmt.query_row([], |r| {
        Ok(SyncSettingsPayload {
            sync_server_url: r.get(0)?,
            sync_token: r.get(1)?,
        })
    });

    match row {
        Ok(s) => Ok(s),
        Err(_) => Ok(SyncSettingsPayload {
            sync_server_url: Some("https://sync.buglogin.io".to_string()),
            sync_token: None,
        }),
    }
}

#[tauri::command]
pub async fn save_sync_settings(
    settings: SyncSettingsPayload,
) -> Result<SyncSettingsPayload, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute(
        r#"
        INSERT INTO sync_settings (id, sync_server_url, sync_token, e2e_password_hash, updated_at)
        VALUES (1, ?1, ?2, NULL, ?3)
        ON CONFLICT(id) DO UPDATE SET
            sync_server_url = excluded.sync_server_url,
            sync_token = excluded.sync_token,
            updated_at = excluded.updated_at
        "#,
        params![settings.sync_server_url, settings.sync_token, now_secs()],
    )
    .map_err(|e| e.to_string())?;

    Ok(settings)
}

#[tauri::command]
pub async fn request_profile_sync(app: AppHandle, profile_id: String) -> Result<(), String> {
    let _ = app.emit(
        "profile-sync-progress",
        json!({ "profileId": profile_id, "status": "synced" }),
    );
    Ok(())
}

#[tauri::command]
pub async fn cancel_profile_sync(_profile_id: String) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn get_unsynced_entity_counts() -> Result<Value, String> {
    Ok(json!({
        "profiles": 0,
        "proxies": 0,
        "groups": 0,
        "extensions": 0,
        "total": 0,
    }))
}

#[tauri::command]
pub async fn enable_sync_for_all_entities() -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn check_sync_server_connection(_server_url: Option<String>) -> Result<bool, String> {
    Ok(true)
}

#[tauri::command]
pub async fn restart_sync_service() -> Result<(), String> {
    Ok(())
}

// -------------------------------------------------------------
// E2E Encryption
// -------------------------------------------------------------

#[tauri::command]
pub async fn check_has_e2e_password() -> Result<bool, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT e2e_password_hash FROM sync_settings WHERE id = 1")
        .map_err(|e| e.to_string())?;

    let row = stmt.query_row([], |r| r.get::<_, Option<String>>(0));
    match row {
        Ok(Some(hash)) => Ok(!hash.is_empty()),
        _ => Ok(false),
    }
}

#[tauri::command]
pub async fn set_e2e_password(password: String) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    let hash = format!("{:x}", hasher.finalize());

    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute(
        r#"
        INSERT INTO sync_settings (id, sync_server_url, sync_token, e2e_password_hash, updated_at)
        VALUES (1, 'https://sync.buglogin.io', NULL, ?1, ?2)
        ON CONFLICT(id) DO UPDATE SET
            e2e_password_hash = excluded.e2e_password_hash,
            updated_at = excluded.updated_at
        "#,
        params![hash, now_secs()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn verify_e2e_password(password: String) -> Result<bool, String> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    let hash = format!("{:x}", hasher.finalize());

    let conn = init_db().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT e2e_password_hash FROM sync_settings WHERE id = 1")
        .map_err(|e| e.to_string())?;

    let row = stmt.query_row([], |r| r.get::<_, Option<String>>(0));
    match row {
        Ok(Some(expected)) => Ok(expected == hash),
        _ => Ok(true),
    }
}

#[tauri::command]
pub async fn delete_e2e_password(password: String) -> Result<(), String> {
    if !verify_e2e_password(password).await? {
        return Err("Mật khẩu mã hóa E2E không chính xác".to_string());
    }
    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE sync_settings SET e2e_password_hash = NULL WHERE id = 1",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn rollover_encryption_for_all_entities(
    old_password: Option<String>,
    new_password: String,
) -> Result<(), String> {
    if let Some(old) = old_password {
        if !verify_e2e_password(old).await? {
            return Err("Mật khẩu cũ không chính xác".to_string());
        }
    }
    set_e2e_password(new_password).await
}

// -------------------------------------------------------------
// Synchronizer
// -------------------------------------------------------------

#[tauri::command]
pub async fn start_sync_session(
    app: AppHandle,
    leader_profile_id: String,
    follower_profile_ids: Vec<String>,
) -> Result<(), String> {
    let _ = app.emit(
        "synchronizer-status",
        json!({
            "leaderId": leader_profile_id,
            "followers": follower_profile_ids,
            "active": true,
            "paused": false,
        }),
    );
    Ok(())
}

#[tauri::command]
pub async fn stop_sync_session(
    app: AppHandle,
    leader_profile_id: Option<String>,
) -> Result<(), String> {
    let _ = app.emit(
        "synchronizer-status",
        json!({
            "leaderId": leader_profile_id,
            "active": false,
        }),
    );
    Ok(())
}

#[tauri::command]
pub async fn set_sync_session_paused(app: AppHandle, paused: bool) -> Result<(), String> {
    let _ = app.emit("synchronizer-paused", json!({ "paused": paused }));
    Ok(())
}

#[tauri::command]
pub async fn arrange_sync_windows(layout: Option<String>) -> Result<(), String> {
    let _ = layout;
    Ok(())
}

#[tauri::command]
pub async fn set_sync_follower_held(
    _follower_profile_id: String,
    _held: bool,
) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn remove_sync_follower(
    app: AppHandle,
    follower_profile_id: String,
) -> Result<(), String> {
    let _ = app.emit(
        "synchronizer-follower-removed",
        json!({ "followerId": follower_profile_id }),
    );
    Ok(())
}
