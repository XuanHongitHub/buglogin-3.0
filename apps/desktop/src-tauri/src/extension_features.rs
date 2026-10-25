use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::app_instance::AppInstance;

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
    db_dir.join("extensions.sqlite")
}

fn init_db() -> Result<Connection, rusqlite::Error> {
    let conn = Connection::open(get_db_path())?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS extensions (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            file_name TEXT NOT NULL,
            file_type TEXT NOT NULL,
            browser_compatibility TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            sync_enabled INTEGER DEFAULT 0,
            version TEXT,
            description TEXT,
            author TEXT,
            homepage_url TEXT,
            icon_data TEXT
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ExtensionItem {
    pub id: String,
    pub name: String,
    pub file_name: String,
    pub file_type: String,
    pub browser_compatibility: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub sync_enabled: bool,
    pub version: Option<String>,
    pub description: Option<String>,
    pub author: Option<String>,
    pub homepage_url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FetchedExtension {
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub author: Option<String>,
    pub file_name: String,
    pub data: Option<String>,
    pub path: Option<String>,
}

#[tauri::command]
pub async fn list_extensions() -> Result<Vec<ExtensionItem>, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, name, file_name, file_type, browser_compatibility, created_at, updated_at, sync_enabled, version, description, author, homepage_url FROM extensions ORDER BY created_at DESC")
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |r| {
            let compat_str: String = r.get(4)?;
            Ok(ExtensionItem {
                id: r.get(0)?,
                name: r.get(1)?,
                file_name: r.get(2)?,
                file_type: r.get(3)?,
                browser_compatibility: serde_json::from_str(&compat_str)
                    .unwrap_or_else(|_| vec!["chromium".to_string()]),
                created_at: r.get(5)?,
                updated_at: r.get(6)?,
                sync_enabled: r.get::<_, i64>(7)? != 0,
                version: r.get(8)?,
                description: r.get(9)?,
                author: r.get(10)?,
                homepage_url: r.get(11)?,
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
pub async fn add_extension(
    app: AppHandle,
    name: String,
    file_name: String,
    file_data: Option<String>,
) -> Result<ExtensionItem, String> {
    let id = format!("ext_{}", now_secs());
    let item = ExtensionItem {
        id: id.clone(),
        name,
        file_name,
        file_type: "crx".to_string(),
        browser_compatibility: vec!["chromium".to_string()],
        created_at: now_secs(),
        updated_at: now_secs(),
        sync_enabled: false,
        version: Some("1.0.0".to_string()),
        description: None,
        author: None,
        homepage_url: None,
    };

    let conn = init_db().map_err(|e| e.to_string())?;
    let compat =
        serde_json::to_string(&item.browser_compatibility).unwrap_or_else(|_| "[]".to_string());
    conn.execute(
        r#"
        INSERT INTO extensions (id, name, file_name, file_type, browser_compatibility, created_at, updated_at, sync_enabled, version, description, author, homepage_url, icon_data)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?9, ?10, ?11, ?12)
        "#,
        params![
            item.id, item.name, item.file_name, item.file_type, compat,
            item.created_at, item.updated_at, item.version, item.description,
            item.author, item.homepage_url, file_data
        ],
    ).map_err(|e| e.to_string())?;

    let _ = app.emit("extensions-changed", ());
    Ok(item)
}

#[tauri::command]
pub async fn add_unpacked_extension(
    app: AppHandle,
    name: String,
    path: String,
    _link: Option<bool>,
) -> Result<ExtensionItem, String> {
    let id = format!("ext_{}", now_secs());
    let item = ExtensionItem {
        id: id.clone(),
        name,
        file_name: path,
        file_type: "unpacked".to_string(),
        browser_compatibility: vec!["chromium".to_string()],
        created_at: now_secs(),
        updated_at: now_secs(),
        sync_enabled: false,
        version: Some("1.0.0".to_string()),
        description: None,
        author: None,
        homepage_url: None,
    };

    let conn = init_db().map_err(|e| e.to_string())?;
    let compat =
        serde_json::to_string(&item.browser_compatibility).unwrap_or_else(|_| "[]".to_string());
    conn.execute(
        r#"
        INSERT INTO extensions (id, name, file_name, file_type, browser_compatibility, created_at, updated_at, sync_enabled, version, description, author, homepage_url, icon_data)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?9, ?10, ?11, NULL)
        "#,
        params![
            item.id, item.name, item.file_name, item.file_type, compat,
            item.created_at, item.updated_at, item.version, item.description,
            item.author, item.homepage_url
        ],
    ).map_err(|e| e.to_string())?;

    let _ = app.emit("extensions-changed", ());
    Ok(item)
}

#[tauri::command]
pub async fn update_extension(
    app: AppHandle,
    extension_id: String,
    name: String,
    file_name: Option<String>,
    file_data: Option<String>,
) -> Result<(), String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    if let (Some(fn_val), Some(fd)) = (file_name, file_data) {
        conn.execute(
            "UPDATE extensions SET name = ?1, file_name = ?2, icon_data = ?3, updated_at = ?4 WHERE id = ?5",
            params![name, fn_val, fd, now_secs(), extension_id],
        ).map_err(|e| e.to_string())?;
    } else {
        conn.execute(
            "UPDATE extensions SET name = ?1, updated_at = ?2 WHERE id = ?3",
            params![name, now_secs(), extension_id],
        )
        .map_err(|e| e.to_string())?;
    }
    let _ = app.emit("extensions-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn update_extension_from_path(
    app: AppHandle,
    extension_id: String,
    name: String,
    path: String,
    _link: Option<bool>,
) -> Result<(), String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE extensions SET name = ?1, file_name = ?2, updated_at = ?3 WHERE id = ?4",
        params![name, path, now_secs(), extension_id],
    )
    .map_err(|e| e.to_string())?;
    let _ = app.emit("extensions-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn delete_extension(app: AppHandle, extension_id: String) -> Result<(), String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM extensions WHERE id = ?1",
        params![extension_id],
    )
    .map_err(|e| e.to_string())?;
    let _ = app.emit("extensions-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn fetch_extension_from_url(url: String) -> Result<FetchedExtension, String> {
    let name = url
        .split('/')
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or("Extension")
        .to_string();
    Ok(FetchedExtension {
        name: Some(name.clone()),
        version: Some("1.0.0".to_string()),
        description: Some(format!("Fetched from {url}")),
        author: None,
        file_name: format!("{name}.crx"),
        data: None,
        path: None,
    })
}

#[tauri::command]
pub async fn get_extension_icon(extension_id: String) -> Result<Option<String>, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT icon_data FROM extensions WHERE id = ?1")
        .map_err(|e| e.to_string())?;
    let row = stmt.query_row(params![extension_id], |r| r.get::<_, Option<String>>(0));
    match row {
        Ok(data) => Ok(data),
        Err(_) => Ok(None),
    }
}

#[tauri::command]
pub async fn set_extension_sync_enabled(extension_id: String, enabled: bool) -> Result<(), String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE extensions SET sync_enabled = ?1 WHERE id = ?2",
        params![if enabled { 1 } else { 0 }, extension_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
