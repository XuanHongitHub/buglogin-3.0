use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::future::Future;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

use crate::{
    app_instance::AppInstance,
    desktop_state::DesktopState,
    remote_api::{FeedbackReceipt, FeedbackSubmission},
};

fn get_app_data_dir() -> PathBuf {
    let base = if let Ok(dir) = std::env::var("BUGLOGIN_DATA_DIR") {
        PathBuf::from(dir)
    } else if let Some(base) = dirs::data_dir() {
        base.join("buglogin")
    } else {
        PathBuf::from(".buglogin")
    };
    AppInstance::app_data_dir(base)
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
        CREATE TABLE IF NOT EXISTS dismissed_browser_updates (
            id TEXT PRIMARY KEY,
            dismissed_at INTEGER NOT NULL
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

fn compute_totp(secret: &str) -> Result<(String, u32), String> {
    compute_totp_at(secret, now_secs())
}

/// RFC 6238 TOTP (HMAC-SHA1, 30-second step, 6 digits) over the base32 secret
/// that authenticator apps share; spaces, dashes, case and padding are ignored.
fn compute_totp_at(secret: &str, now: i64) -> Result<(String, u32), String> {
    use hmac::{Hmac, KeyInit, Mac};

    let normalized: String = secret
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '=')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let key = data_encoding::BASE32_NOPAD
        .decode(normalized.as_bytes())
        .ok()
        .filter(|key| !key.is_empty())
        .ok_or_else(|| "TOTP secret must be a base32 key".to_string())?;
    let step = 30;
    let counter = (now / step) as u64;
    let remaining = (step - (now % step)) as u32;

    let mut mac = Hmac::<sha1::Sha1>::new_from_slice(&key)
        .map_err(|_| "TOTP secret must be a base32 key".to_string())?;
    mac.update(&counter.to_be_bytes());
    let hash = mac.finalize().into_bytes();
    let offset = (hash[19] & 0xf) as usize;
    let code_num = u32::from_be_bytes([
        hash[offset] & 0x7f,
        hash[offset + 1],
        hash[offset + 2],
        hash[offset + 3],
    ]) % 1_000_000;
    Ok((format!("{:06}", code_num), remaining))
}

#[tauri::command]
pub async fn generate_totp_code(key: String) -> Result<TotpCode, String> {
    let (code, remaining_seconds) = compute_totp(&key)?;
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
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;

    let mut list = Vec::new();
    for row in rows {
        if let Ok((id, key)) = row {
            let (code, remaining) = compute_totp(&key)?;
            list.push(TotpEntryCode {
                id,
                code,
                remaining_seconds: remaining,
            });
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

struct PreparedAppUpdateStore<U = tauri_plugin_updater::Update> {
    prepared: Option<PreparedAppUpdate<U>>,
}

impl<U> Default for PreparedAppUpdateStore<U> {
    fn default() -> Self {
        Self { prepared: None }
    }
}

struct PreparedAppUpdate<U = tauri_plugin_updater::Update> {
    version: String,
    bytes: Vec<u8>,
    update: U,
}

impl<U> PreparedAppUpdateStore<U> {
    fn replace(&mut self, version: String, bytes: Vec<u8>, update: U) {
        self.prepared = Some(PreparedAppUpdate {
            version,
            bytes,
            update,
        });
    }

    fn take(&mut self, version: &str) -> Result<PreparedAppUpdate<U>, String> {
        match self.prepared.take() {
            Some(prepared) if prepared.version == version => Ok(prepared),
            Some(prepared) => {
                self.prepared = Some(prepared);
                Err("app_update_prepared_version_mismatch".to_owned())
            }
            None => Err("app_update_not_prepared".to_owned()),
        }
    }

    fn restore(&mut self, prepared: PreparedAppUpdate<U>) {
        self.prepared = Some(prepared);
    }

    fn clear(&mut self) {
        self.prepared = None;
    }
}

fn prepared_app_update_store() -> &'static Mutex<PreparedAppUpdateStore> {
    static STORE: OnceLock<Mutex<PreparedAppUpdateStore<tauri_plugin_updater::Update>>> =
        OnceLock::new();
    STORE.get_or_init(|| Mutex::new(PreparedAppUpdateStore::default()))
}

async fn shutdown_before_update_install<T>(
    shutdown: impl Future<Output = Result<(), String>>,
    install: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    shutdown.await?;
    install()
}

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

pub async fn check_for_app_updates_manual(app: AppHandle) -> Result<Option<Value>, String> {
    check_for_app_updates(app).await
}

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
    let bytes = update
        .download(
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
    prepared_app_update_store()
        .lock()
        .map_err(|_| "app_update_state_unavailable".to_owned())?
        .replace(requested_version.to_owned(), bytes, update);
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

pub async fn restart_application(
    app: AppHandle,
    state: tauri::State<'_, DesktopState>,
) -> Result<(), String> {
    let _ = &app;
    let prepared = {
        let version = {
            // Only one prepared package is retained. Read its version without moving the bytes
            // so an unrelated restart request cannot discard it.
            let store = prepared_app_update_store()
                .lock()
                .map_err(|_| "app_update_state_unavailable".to_owned())?;
            store
                .prepared
                .as_ref()
                .map(|prepared| prepared.version.clone())
                .ok_or_else(|| "app_update_not_prepared".to_owned())?
        };
        prepared_app_update_store()
            .lock()
            .map_err(|_| "app_update_state_unavailable".to_owned())?
            .take(&version)?
    };

    if let Err(error) = prepared.update.verify(&prepared.bytes) {
        prepared_app_update_store()
            .lock()
            .map_err(|_| "app_update_state_unavailable".to_owned())?
            .restore(prepared);
        return Err(error.to_string());
    }

    let install_result = shutdown_before_update_install(
        async {
            state
                .shutdown()
                .await
                .map_err(|error| format!("{}: {}", error.code, error.title))
        },
        || {
            prepared
                .update
                .install(&prepared.bytes)
                .map_err(|error| error.to_string())
        },
    )
    .await;
    if let Err(error) = install_result {
        prepared_app_update_store()
            .lock()
            .map_err(|_| "app_update_state_unavailable".to_owned())?
            .restore(prepared);
        return Err(error);
    }

    // The Windows updater installer exits the process and restarts it after replacement. On
    // macOS the signed bundle is replaced in place, so explicitly relaunch only after install.
    #[cfg(not(target_os = "windows"))]
    app.restart();

    Ok(())
}

pub fn discard_prepared_app_update() -> Result<(), String> {
    prepared_app_update_store()
        .lock()
        .map_err(|_| "app_update_state_unavailable".to_owned())?
        .clear();
    Ok(())
}

#[tauri::command]
pub async fn check_browser_exists(
    state: tauri::State<'_, DesktopState>,
    browser_str: String,
    version: Option<String>,
) -> Result<bool, String> {
    let runtime = browser_runtime(&browser_str)?;
    let installed = state
        .runtime_installed_versions_get(&runtime, "browser-installed-check")
        .map_err(|error| format!("{}: {}", error.code, error.title))?;
    Ok(match version {
        Some(version) => installed.iter().any(|installed| installed == &version),
        None => !installed.is_empty(),
    })
}

pub async fn download_browser(
    app: AppHandle,
    state: tauri::State<'_, DesktopState>,
    browser_str: String,
    version: Option<String>,
) -> Result<(), String> {
    let runtime = browser_runtime(&browser_str)?;
    let version = version
        .filter(|version| !version.trim().is_empty())
        .ok_or_else(|| "runtime_version_required: A browser version is required".to_owned())?;
    let installed = state
        .runtime_installed_versions_get(&runtime, "browser-download")
        .map_err(|error| format!("{}: {}", error.code, error.title))?;
    if installed.iter().any(|installed| installed == &version) {
        return Ok(());
    }
    crate::install_verified_runtime(app, state.inner(), runtime, &version, "browser-download")
        .await
        .map(|_| ())
        .map_err(|error| format!("{}: {}", error.code, error.title))
}

#[tauri::command]
pub async fn check_for_browser_updates(
    state: tauri::State<'_, DesktopState>,
) -> Result<Vec<Value>, String> {
    let runtime = browser_runtime("wayfern")?;
    let installed = state
        .runtime_installed_versions_get(&runtime, "browser-update-check")
        .map_err(|error| format!("{}: {}", error.code, error.title))?;
    let Some(current_version) = newest_version(&installed).cloned() else {
        return Ok(Vec::new());
    };
    let versions = available_browser_versions(state.inner(), &runtime).await?;
    let Some(new_version) = newest_update_version(&versions, &installed).cloned() else {
        return Ok(Vec::new());
    };
    let notification = json!({
        "id": format!("wayfern:{current_version}:{new_version}"),
        "browser": "wayfern",
        "current_version": current_version,
        "new_version": new_version,
        "affected_profiles": [],
        "timestamp": now_secs(),
        "is_rolling_release": false,
    });
    let notification_id = notification["id"]
        .as_str()
        .ok_or_else(|| "notification_id_invalid: Update notification identifier is invalid")?;
    if is_browser_update_dismissed(notification_id)? {
        return Ok(Vec::new());
    }
    Ok(vec![notification])
}

#[tauri::command]
pub async fn get_version_update_status() -> Result<(Option<i64>, u64), String> {
    // The old periodic updater is not a background job in the active runtime
    // architecture. `None, 0` means no completed check is recorded and a
    // check can run now; do not report fake progress or a fabricated timestamp.
    Ok((None, 0))
}

#[tauri::command]
pub async fn trigger_manual_version_update(
    app: AppHandle,
    state: tauri::State<'_, DesktopState>,
) -> Result<Vec<Value>, String> {
    let runtime = browser_runtime("wayfern")?;
    let installed = state
        .runtime_installed_versions_get(&runtime, "browser-manual-update")
        .map_err(|error| format!("{}: {}", error.code, error.title))?;
    let versions = available_browser_versions(state.inner(), &runtime).await?;
    let Some(latest) = newest_update_version(&versions, &installed).cloned() else {
        return Ok(vec![json!({
            "browser": "wayfern",
            "new_versions_count": 0,
            "total_versions_count": versions.len(),
            "updated_successfully": true,
        })]);
    };

    let result = crate::install_verified_runtime(
        app,
        state.inner(),
        runtime,
        &latest,
        "browser-manual-update",
    )
    .await;
    match result {
        Ok(_) => Ok(vec![json!({
            "browser": "wayfern",
            "new_versions_count": 1,
            "total_versions_count": versions.len(),
            "updated_successfully": true,
        })]),
        Err(error) => Ok(vec![json!({
            "browser": "wayfern",
            "new_versions_count": 1,
            "total_versions_count": versions.len(),
            "updated_successfully": false,
            "error": format!("{}: {}", error.code, error.title),
        })]),
    }
}

#[tauri::command]
pub async fn fetch_browser_versions_with_count(
    state: tauri::State<'_, DesktopState>,
    browser_str: String,
) -> Result<Value, String> {
    let runtime = browser_runtime(&browser_str)?;
    let versions = available_browser_versions(state.inner(), &runtime).await?;
    let installed = state
        .runtime_installed_versions_get(&runtime, "browser-version-list")
        .map_err(|error| format!("{}: {}", error.code, error.title))?;
    let new_versions_count = versions
        .iter()
        .filter(|version| {
            !installed.iter().any(|installed| installed == *version)
                && newest_version(&installed)
                    .is_none_or(|current| compare_versions(version, current).is_gt())
        })
        .count();
    Ok(json!({
        "versions": versions,
        "new_versions_count": new_versions_count,
        "total_versions_count": versions.len(),
    }))
}

#[tauri::command]
pub async fn complete_browser_update_with_auto_update(
    state: tauri::State<'_, DesktopState>,
    browser: String,
    new_version: String,
) -> Result<Vec<String>, String> {
    let runtime = browser_runtime(&browser)?;
    let installed = state
        .runtime_installed_versions_get(&runtime, "browser-update-complete")
        .map_err(|error| format!("{}: {}", error.code, error.title))?;
    if !installed.iter().any(|installed| installed == &new_version) {
        return Err(format!(
            "runtime_not_installed: Browser version {new_version} is not installed"
        ));
    }
    // Profiles pin their runtime version and are switched by an explicit user
    // action. Installing a catalog release does not silently mutate profiles.
    Ok(Vec::new())
}

#[tauri::command]
pub async fn dismiss_update_notification(notification_id: String) -> Result<(), String> {
    let notification_id = notification_id.trim();
    if notification_id.is_empty() || notification_id.len() > 256 {
        return Err("notification_id_invalid: Update notification identifier is invalid".into());
    }
    let conn = init_db().map_err(|error| error.to_string())?;
    conn.execute(
        "INSERT INTO dismissed_browser_updates (id, dismissed_at) VALUES (?1, ?2) ON CONFLICT(id) DO NOTHING",
        params![notification_id, now_secs()],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn clear_all_version_cache_and_refetch(
    state: tauri::State<'_, DesktopState>,
) -> Result<(), String> {
    let conn = init_db().map_err(|error| error.to_string())?;
    conn.execute("DELETE FROM dismissed_browser_updates", [])
        .map_err(|error| error.to_string())?;
    drop(conn);
    state
        .runtime_catalog_get("browser-version-cache-refetch")
        .await
        .map(|_| ())
        .map_err(|error| format!("{}: {}", error.code, error.title))
}

fn is_browser_update_dismissed(notification_id: &str) -> Result<bool, String> {
    let conn = init_db().map_err(|error| error.to_string())?;
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM dismissed_browser_updates WHERE id = ?1)",
        [notification_id],
        |row| row.get(0),
    )
    .map_err(|error| error.to_string())
}

fn browser_runtime(browser: &str) -> Result<buglogin_domain::entities::Runtime, String> {
    match browser.trim().to_ascii_lowercase().as_str() {
        "wayfern" => Ok(buglogin_domain::entities::Runtime::Wayfern),
        _ => Err(format!(
            "runtime_not_supported: Browser runtime `{browser}` is not supported"
        )),
    }
}

async fn available_browser_versions(
    state: &DesktopState,
    runtime: &buglogin_domain::entities::Runtime,
) -> Result<Vec<String>, String> {
    let correlation_id = "browser-version-catalog";
    let (platform, architecture) =
        crate::runtime_install::current_platform_architecture(correlation_id)
            .map_err(|error| format!("{}: {}", error.code, error.title))?;
    let catalog = state
        .runtime_catalog_get(correlation_id)
        .await
        .map_err(|error| format!("{}: {}", error.code, error.title))?;
    let verifier = crate::runtime_config::pinned_runtime_verifier(correlation_id)
        .map_err(|error| format!("{}: {}", error.code, error.title))?;
    let releases = catalog
        .verify(&verifier)
        .map_err(|error| format!("{}: {}", error.code, error.title))?;
    let mut versions = releases
        .into_iter()
        .filter_map(|release| {
            let release = release.release();
            (release.runtime == *runtime
                && release.platform == platform
                && release.architecture == architecture)
                .then(|| release.version.clone())
        })
        .collect::<Vec<_>>();
    versions.sort_by(|left, right| compare_versions(right, left));
    versions.dedup();
    Ok(versions)
}

fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    let left_parts = left
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect::<Vec<_>>();
    let right_parts = right
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect::<Vec<_>>();
    let width = left_parts.len().max(right_parts.len());
    (0..width)
        .map(|index| {
            left_parts
                .get(index)
                .copied()
                .unwrap_or(0)
                .cmp(&right_parts.get(index).copied().unwrap_or(0))
        })
        .find(|ordering| !ordering.is_eq())
        .unwrap_or(std::cmp::Ordering::Equal)
}

fn newest_version(versions: &[String]) -> Option<&String> {
    versions
        .iter()
        .max_by(|left, right| compare_versions(left, right))
}

fn newest_update_version<'a>(versions: &'a [String], installed: &[String]) -> Option<&'a String> {
    let latest = newest_version(versions)?;
    if installed.iter().any(|version| version == latest)
        || newest_version(installed)
            .is_some_and(|current| !compare_versions(latest, current).is_gt())
    {
        return None;
    }
    Some(latest)
}

// -------------------------------------------------------------
// Logs, Diagnostics & Tips
// -------------------------------------------------------------

#[tauri::command]
pub async fn read_log_files(app: AppHandle) -> Result<String, String> {
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|error| error.to_string())?;
    if !log_dir.exists() {
        return Ok("No log files have been written yet.".to_string());
    }
    let mut logs = String::new();
    let mut entries = fs::read_dir(&log_dir)
        .map_err(|error| format!("Could not list app log directory: {error}"))?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_file())
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if let Ok(content) = fs::read_to_string(entry.path()) {
            logs.push_str(&content);
            logs.push('\n');
        }
    }
    Ok(if logs.is_empty() {
        format!("No readable log records in {}.", log_dir.display())
    } else {
        logs
    })
}

#[tauri::command]
pub async fn open_log_directory(app: AppHandle) -> Result<(), String> {
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|error| error.to_string())?;
    fs::create_dir_all(&log_dir).map_err(|error| error.to_string())?;
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(&log_dir).spawn();
    Ok(())
}

#[tauri::command]
pub async fn preview_feedback_logs(app: AppHandle) -> Result<String, String> {
    let logs = read_log_files(app).await?;
    const MAX_FEEDBACK_LOG_BYTES: usize = 128 * 1024;
    if logs.len() <= MAX_FEEDBACK_LOG_BYTES {
        return Ok(logs);
    }
    let mut start = logs.len() - MAX_FEEDBACK_LOG_BYTES;
    while !logs.is_char_boundary(start) {
        start += 1;
    }
    Ok(format!("[Earlier log lines omitted]\n{}", &logs[start..]))
}

#[tauri::command]
pub async fn send_feedback(
    app: AppHandle,
    state: tauri::State<'_, DesktopState>,
    kind: String,
    message: String,
    include_logs: bool,
    email: Option<String>,
    locale: String,
    idempotency_key: String,
    correlation_id: String,
) -> Result<FeedbackReceipt, buglogin_domain::errors::Problem> {
    if !(16..=128).contains(&idempotency_key.len()) || !idempotency_key.is_ascii() {
        return Err(buglogin_domain::errors::Problem::new(
            "idempotency_key_required",
            "Valid feedback idempotency key required",
            400,
            &correlation_id,
        ));
    }
    if correlation_id.trim().is_empty() || correlation_id.len() > 128 {
        return Err(buglogin_domain::errors::Problem::new(
            "correlation_id_invalid",
            "Feedback request correlation is invalid",
            400,
            "send-feedback",
        ));
    }
    let logs = if include_logs {
        Some(preview_feedback_logs(app).await.map_err(|_| {
            buglogin_domain::errors::Problem::new(
                "feedback_logs_unreadable",
                "Recent logs could not be read",
                500,
                &correlation_id,
            )
        })?)
    } else {
        None
    };
    state
        .submit_feedback(
            FeedbackSubmission {
                kind,
                message,
                include_logs,
                logs,
                email,
                locale,
            },
            &idempotency_key,
            &correlation_id,
        )
        .await
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

#[cfg(test)]
mod tests {
    use super::{
        PreparedAppUpdateStore, browser_runtime, compare_versions, compute_totp_at,
        newest_update_version, shutdown_before_update_install,
    };
    use std::sync::{Arc, Mutex};

    // RFC 6238 appendix B (SHA-1, secret "12345678901234567890"), 6-digit truncation.
    const RFC_SECRET: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

    #[test]
    fn totp_matches_rfc_6238_sha1_vectors() {
        assert_eq!(
            compute_totp_at(RFC_SECRET, 59).unwrap(),
            ("287082".to_owned(), 1)
        );
        assert_eq!(
            compute_totp_at(RFC_SECRET, 1_111_111_109).unwrap().0,
            "081804"
        );
        assert_eq!(
            compute_totp_at(RFC_SECRET, 1_234_567_890).unwrap().0,
            "005924"
        );
        assert_eq!(
            compute_totp_at(RFC_SECRET, 2_000_000_000).unwrap().0,
            "279037"
        );
    }

    #[test]
    fn totp_accepts_authenticator_formatted_secrets() {
        let formatted = "gezd gnbv gy3t qojq gezd gnbv gy3t qojq";
        assert_eq!(compute_totp_at(formatted, 59).unwrap().0, "287082");
    }

    #[test]
    fn totp_rejects_secrets_that_are_not_base32() {
        assert!(compute_totp_at("not-base32!", 59).is_err());
    }

    #[test]
    fn browser_runtime_rejects_unapproved_engines() {
        assert_eq!(
            browser_runtime("wayfern").unwrap(),
            buglogin_domain::entities::Runtime::Wayfern
        );
        assert!(
            browser_runtime("chrome")
                .unwrap_err()
                .contains("runtime_not_supported")
        );
    }

    #[test]
    fn browser_versions_are_compared_numerically_and_skip_installed_versions() {
        let available = vec![
            "154.0.8037.57".to_owned(),
            "155.0.8059.79".to_owned(),
            "155.0.8059.8".to_owned(),
        ];
        let installed = vec!["154.0.8037.57".to_owned(), "155.0.8059.8".to_owned()];

        assert!(compare_versions("155.0.8059.79", "155.0.8059.8").is_gt());
        assert_eq!(
            newest_update_version(&available, &installed).map(String::as_str),
            Some("155.0.8059.79")
        );
        let latest_installed = vec!["155.0.8059.79".to_owned()];
        assert!(newest_update_version(&available, &latest_installed).is_none());
    }

    #[test]
    fn prepared_update_store_preserves_only_the_matching_release() {
        let mut store = PreparedAppUpdateStore::<()>::default();
        store.replace("3.0.8".to_owned(), vec![1, 2, 3], ());

        assert_eq!(
            store.take("3.0.7").err().as_deref(),
            Some("app_update_prepared_version_mismatch")
        );
        let prepared = store.take("3.0.8").expect("matching package retained");
        assert_eq!(prepared.bytes, vec![1, 2, 3]);
        store.restore(prepared);
        store.clear();
        assert_eq!(
            store.take("3.0.8").err().as_deref(),
            Some("app_update_not_prepared")
        );
    }

    #[tokio::test]
    async fn update_install_waits_for_managed_runtime_shutdown() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let shutdown_order = order.clone();
        let install_order = order.clone();

        shutdown_before_update_install(
            async move {
                shutdown_order.lock().unwrap().push("shutdown");
                Ok(())
            },
            move || {
                install_order.lock().unwrap().push("install");
                Ok::<_, String>(())
            },
        )
        .await
        .unwrap();

        assert_eq!(*order.lock().unwrap(), vec!["shutdown", "install"]);
    }
}
