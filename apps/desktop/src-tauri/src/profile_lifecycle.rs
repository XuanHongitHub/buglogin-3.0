use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use buglogin_domain::errors::Problem;
use buglogin_domain::ids::ProfileId;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, State};

use crate::desktop_state::DesktopState;
use crate::legacy_compat::{self, create_browser_profile_new};
use crate::profile_password;

static REQ_ID_SEQ: AtomicU64 = AtomicU64::new(1000);

fn req_id(prefix: &str) -> String {
    let seq = REQ_ID_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{seq}")
}

fn get_app_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("BUGLOGIN_DATA_DIR") {
        PathBuf::from(dir)
    } else if let Some(base) = dirs::data_dir() {
        base.join("buglogin")
    } else {
        PathBuf::from(".buglogin")
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TrashedProfileSummary {
    pub id: String,
    pub name: String,
    pub browser: String,
    #[serde(default)]
    pub version: Option<String>,
    pub deleted_at: i64,
    pub expires_at: i64,
    pub size_bytes: u64,
    #[serde(default)]
    pub sync_enabled: bool,
    #[serde(default)]
    pub password_protected: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConsistencyResult {
    pub consistent: bool,
    pub checked: bool,
    pub exit_ip: Option<String>,
    pub exit_country_code: Option<String>,
    pub exit_timezone: Option<String>,
    pub fingerprint_timezone: Option<String>,
    pub fingerprint_language: Option<String>,
    pub mismatches: Vec<String>,
}

#[tauri::command]
pub async fn clone_profile(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
    name: String,
) -> Result<Value, Problem> {
    let source = state
        .invoke(
            "profile_get",
            &req_id("clone_get"),
            None,
            json!({ "profileId": profile_id }),
        )
        .await
        .map_err(|e| Problem::new("profile_get_failed", e.title, 404, req_id("clone")))?;

    let source_data = source.data;
    let browser = source_data
        .get("browser")
        .and_then(Value::as_str)
        .unwrap_or("chromium")
        .to_owned();
    let version = source_data
        .get("version")
        .and_then(Value::as_str)
        .map(String::from);
    let proxy_id = source_data
        .get("proxyId")
        .and_then(Value::as_str)
        .map(String::from);
    let group_id = source_data
        .get("groupId")
        .and_then(Value::as_str)
        .map(String::from);
    let wayfern = source_data.get("wayfernConfig").cloned();

    let new_profile = create_browser_profile_new(
        state.clone(),
        app.clone(),
        name,
        req_id("clone_idem"),
        browser,
        version,
        proxy_id,
        None,
        None,
        wayfern,
        group_id,
    )
    .await?;

    let new_id = new_profile
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();

    if let (Ok(source), Ok(target)) = (canonical_dir_name(&profile_id), canonical_dir_name(&new_id))
    {
        let src_dir = get_app_data_dir().join("profiles").join(source);
        let dst_dir = get_app_data_dir().join("profiles").join(target);
        if src_dir.exists() && !dst_dir.exists() {
            let _ = copy_dir_all(&src_dir, &dst_dir);
        }
    }

    let _ = app.emit("profiles-changed", json!({ "profileId": new_id }));
    Ok(new_profile)
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
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
pub async fn open_url_with_profile(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
    url: String,
    _consent_token: Option<String>,
) -> Result<Value, Problem> {
    let _ = legacy_compat::update_profile_start_urls(
        state.clone(),
        app.clone(),
        profile_id.clone(),
        vec![url],
    )
    .await;
    legacy_compat::launch_browser_profile(state, app, None, Some(profile_id)).await
}

#[tauri::command]
pub async fn is_profile_locked(profile_id: String) -> Result<bool, String> {
    Ok(legacy_compat::has_profile_password(&profile_id)
        && !profile_password::is_unlocked(&profile_id))
}

#[tauri::command]
pub async fn unlock_profile(
    state: State<'_, DesktopState>,
    profile_id: String,
    password: String,
) -> Result<(), String> {
    legacy_compat::verify_profile_password(state, profile_id.clone(), password)
        .await
        .map_err(|e| e.title)?;
    profile_password::grant_unlock(&profile_id);
    Ok(())
}

#[tauri::command]
pub async fn check_profile_fingerprint_consistency(
    state: State<'_, DesktopState>,
    profile_id: String,
) -> Result<ConsistencyResult, String> {
    let profile = state
        .invoke(
            "profile_get",
            &req_id("consist"),
            None,
            json!({ "profileId": profile_id }),
        )
        .await
        .map_err(|e| e.title)?;

    let wayfern = profile.data.get("wayfernConfig");
    let timezone = wayfern
        .and_then(|w| w.get("timezone"))
        .and_then(Value::as_str)
        .map(String::from);
    let language = wayfern
        .and_then(|w| w.get("language"))
        .and_then(Value::as_str)
        .map(String::from);

    Ok(ConsistencyResult {
        consistent: true,
        checked: false,
        exit_ip: None,
        exit_country_code: None,
        exit_timezone: None,
        fingerprint_timezone: timezone,
        fingerprint_language: language,
        mismatches: Vec::new(),
    })
}

#[tauri::command]
pub async fn match_profile_fingerprint_to_exit(
    _state: State<'_, DesktopState>,
    _profile_id: String,
    _exit_ip: String,
) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn list_trashed_profiles(
    state: State<'_, DesktopState>,
) -> Result<Vec<TrashedProfileSummary>, Problem> {
    let list_res = state
        .invoke(
            "profile_list",
            &req_id("trash_list"),
            None,
            json!({ "limit": 100 }),
        )
        .await?;

    let items = list_res
        .data
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut trashed = Vec::new();
    let now = now_secs();

    for item in items {
        let is_archived = item
            .get("archived")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if is_archived {
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let browser = item
                .get("browser")
                .and_then(Value::as_str)
                .unwrap_or("chromium")
                .to_owned();
            let version = item
                .get("version")
                .and_then(Value::as_str)
                .map(String::from);
            let updated = item.get("updatedAt").and_then(Value::as_i64).unwrap_or(now);
            let has_pwd = legacy_compat::has_profile_password(&id);

            let size = canonical_dir_name(&id)
                .ok()
                .and_then(|name| dir_size(&get_app_data_dir().join("profiles").join(name)).ok())
                .unwrap_or(1024 * 1024);

            trashed.push(TrashedProfileSummary {
                id,
                name,
                browser,
                version,
                deleted_at: updated,
                expires_at: updated + (30 * 24 * 3600),
                size_bytes: size,
                sync_enabled: false,
                password_protected: has_pwd,
            });
        }
    }

    Ok(trashed)
}

fn dir_size(path: &Path) -> std::io::Result<u64> {
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

#[tauri::command]
pub async fn restore_trashed_profile(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
) -> Result<Value, Problem> {
    let res = state
        .invoke(
            "profile_restore",
            &req_id("trash_restore"),
            Some(&req_id("idem_res")),
            json!({ "profileId": profile_id }),
        )
        .await?;

    let _ = app.emit("trash-changed", ());
    let _ = app.emit("profiles-changed", json!({ "profileId": profile_id }));
    Ok(res.data)
}

#[tauri::command]
pub async fn purge_trashed_profile(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
) -> Result<(), Problem> {
    state
        .invoke(
            "profile_delete",
            &req_id("trash_purge"),
            Some(&req_id("idem_del")),
            json!({ "profileId": profile_id }),
        )
        .await?;

    if let Ok(name) = canonical_dir_name(&profile_id) {
        let p_dir = get_app_data_dir().join("profiles").join(name);
        if p_dir.exists() {
            let _ = fs::remove_dir_all(p_dir);
        }
    }

    let _ = app.emit("trash-changed", ());
    let _ = app.emit("profiles-changed", json!({ "profileId": profile_id }));
    Ok(())
}

#[tauri::command]
pub async fn empty_trash(state: State<'_, DesktopState>, app: AppHandle) -> Result<(), Problem> {
    let trashed = list_trashed_profiles(state.clone()).await?;
    for item in trashed {
        let _ = state
            .invoke(
                "profile_delete",
                &req_id("trash_purge"),
                Some(&req_id("idem_empty")),
                json!({ "profileId": item.id }),
            )
            .await;
        if let Ok(name) = canonical_dir_name(&item.id) {
            let p_dir = get_app_data_dir().join("profiles").join(name);
            if p_dir.exists() {
                let _ = fs::remove_dir_all(p_dir);
            }
        }
    }

    let _ = app.emit("trash-changed", ());
    let _ = app.emit("profiles-changed", ());
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProfileArchivePreview {
    pub name: String,
    pub browser: String,
    pub version: Option<String>,
    pub created_at: Option<i64>,
    pub size_bytes: u64,
}

/// Profile and restore-point IDs reach the filesystem from the webview; only
/// canonical IDs (no separators, leading dot or absolute path) may name a directory.
pub(crate) fn canonical_dir_name(id: &str) -> Result<&str, String> {
    ProfileId::new(id)
        .map(|_| id)
        .map_err(|_| "Invalid identifier".to_string())
}

#[tauri::command]
pub async fn export_profile(profile_id: String, destination_path: String) -> Result<(), String> {
    if legacy_compat::has_profile_password(&profile_id)
        && !profile_password::take_unlock(&profile_id)
    {
        return Err(profile_password::locked_problem().title);
    }
    export_profile_from(
        &get_app_data_dir(),
        &profile_id,
        Path::new(&destination_path),
    )
}

fn export_profile_from(data_dir: &Path, profile_id: &str, dst: &Path) -> Result<(), String> {
    let src = data_dir
        .join("profiles")
        .join(canonical_dir_name(profile_id)?);
    if !src.exists() {
        return Err("Profile data directory not found".to_string());
    }
    if fs::symlink_metadata(dst).is_ok() {
        return Err("Export destination already exists".to_string());
    }
    if let Some(parent) = dst.parent() {
        let _ = fs::create_dir_all(parent);
    }
    copy_dir_all(&src, dst).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn preview_profile_archive(
    archive_path: String,
) -> Result<ProfileArchivePreview, String> {
    let p = PathBuf::from(&archive_path);
    if !p.exists() {
        return Err("Archive not found".to_string());
    }
    let meta = fs::metadata(&p).map_err(|e| e.to_string())?;
    Ok(ProfileArchivePreview {
        name: p
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "imported-profile".to_string()),
        browser: "chromium".to_string(),
        version: None,
        created_at: Some(now_secs()),
        size_bytes: meta.len(),
    })
}

#[tauri::command]
pub async fn import_profile_archive(
    state: State<'_, DesktopState>,
    app: AppHandle,
    archive_path: String,
    target_name: Option<String>,
) -> Result<Value, Problem> {
    let p = PathBuf::from(&archive_path);
    let name = target_name.unwrap_or_else(|| {
        p.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Imported Profile".to_string())
    });

    let new_profile = create_browser_profile_new(
        state,
        app.clone(),
        name,
        req_id("import_idem"),
        "chromium".to_string(),
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await?;

    let new_id = new_profile
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if let (Ok(target), true) = (canonical_dir_name(&new_id), p.is_dir()) {
        let dst_dir = get_app_data_dir().join("profiles").join(target);
        let _ = copy_dir_all(&p, &dst_dir);
    }

    let _ = app.emit("profiles-changed", json!({ "profileId": new_id }));
    Ok(new_profile)
}

#[cfg(test)]
mod tests {
    use super::{canonical_dir_name, export_profile_from};
    use std::fs;
    use std::path::PathBuf;

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "buglogin-profile-lifecycle-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("profiles/prf_source/Default")).unwrap();
        fs::write(root.join("profiles/prf_source/Default/Preferences"), b"{}").unwrap();
        root
    }

    #[test]
    fn canonical_dir_name_accepts_only_canonical_ids() {
        assert_eq!(canonical_dir_name("prf_source").unwrap(), "prf_source");
        for bad in [
            "",
            "..",
            "../outside",
            "/Users/someone",
            "a/b",
            r"a\b",
            ".hidden",
        ] {
            assert!(canonical_dir_name(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn export_rejects_profile_ids_that_leave_the_profiles_root() {
        let root = temp_root("export-traversal");
        fs::create_dir_all(root.join("outside")).unwrap();
        let destination = root.join("exported");

        assert!(export_profile_from(&root, "../outside", &destination).is_err());
        assert!(export_profile_from(&root, root.to_str().unwrap(), &destination).is_err());
        assert!(!destination.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn export_never_merges_into_an_existing_directory() {
        let root = temp_root("export-existing");
        let existing = root.join("existing");
        fs::create_dir_all(&existing).unwrap();

        assert!(export_profile_from(&root, "prf_source", &existing).is_err());
        assert!(!existing.join("Default").exists());

        let fresh = root.join("fresh");
        export_profile_from(&root, "prf_source", &fresh).unwrap();
        assert!(fresh.join("Default/Preferences").is_file());
        fs::remove_dir_all(root).unwrap();
    }
}
