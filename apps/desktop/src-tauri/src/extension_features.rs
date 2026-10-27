use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter};
use url::Url;

use crate::app_instance::AppInstance;
use buglogin_domain::{entities::GitExtensionSource, ids::ExtensionId};
use std::collections::BTreeMap;

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
            icon_data TEXT,
            package_data BLOB
        );
        "#,
    )?;
    let has_package_data = conn
        .prepare("PRAGMA table_info(extensions)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(Result::ok)
        .any(|name| name == "package_data");
    if !has_package_data {
        conn.execute("ALTER TABLE extensions ADD COLUMN package_data BLOB", [])?;
    }
    for column in ["git_source", "source_revision"] {
        let exists = conn
            .prepare("PRAGMA table_info(extensions)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .any(|name| name == column);
        if !exists {
            conn.execute(
                &format!("ALTER TABLE extensions ADD COLUMN {column} TEXT"),
                [],
            )?;
        }
    }
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
    pub git_source: Option<GitExtensionSource>,
    pub source_revision: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FetchedExtension {
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub author: Option<String>,
    pub file_name: String,
    pub file_data: Option<String>,
    pub path: Option<String>,
    pub source_url: Option<String>,
    pub git_source: Option<GitExtensionSource>,
    pub source_revision: Option<String>,
}

#[tauri::command]
pub async fn list_extensions() -> Result<Vec<ExtensionItem>, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, name, file_name, file_type, browser_compatibility, created_at, updated_at, sync_enabled, version, description, author, homepage_url, git_source, source_revision FROM extensions ORDER BY created_at DESC")
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
                git_source: r
                    .get::<_, Option<String>>(12)?
                    .map(|value| {
                        serde_json::from_str(&value).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                12,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })
                    })
                    .transpose()?,
                source_revision: r.get(13)?,
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
    file_data: Option<Vec<u8>>,
    git_source: Option<GitExtensionSource>,
    source_revision: Option<String>,
) -> Result<ExtensionItem, String> {
    let file_data = file_data.ok_or_else(|| "Chưa nhận được dữ liệu tiện ích.".to_string())?;
    if file_data.is_empty() || file_data.len() > 64 * 1024 * 1024 {
        return Err("Tệp tiện ích rỗng hoặc vượt quá 64 MiB.".to_string());
    }
    let file_type = file_name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .filter(|extension| matches!(extension.as_str(), "zip" | "crx"))
        .ok_or_else(|| "Tiện ích phải là tệp .zip hoặc .crx.".to_string())?;
    if file_type == "zip" && file_data.get(..2) != Some(&b"PK"[..]) {
        return Err("Tệp ZIP không hợp lệ.".to_string());
    }
    if file_type == "crx" && file_data.get(..4) != Some(&b"Cr24"[..]) {
        return Err("Tệp CRX không hợp lệ.".to_string());
    }
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let manifest = crate::extension_packages::read_manifest(&file_data)?;
    let source_revision = if let Some(source) = git_source.as_ref() {
        source.validate().map_err(str::to_owned)?;
        Some(
            source_revision
                .filter(|revision| {
                    revision.len() == 40 && revision.bytes().all(|b| b.is_ascii_hexdigit())
                })
                .ok_or("Hãy tải lại thông tin repository trước khi lưu tiện ích Git.")?,
        )
    } else {
        None
    };
    let id = git_source
        .as_ref()
        .map(crate::extension_packages::source_id)
        .unwrap_or_else(|| {
            format!(
                "ext_{}_{}",
                now_secs(),
                NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            )
        });
    let item = ExtensionItem {
        id: id.clone(),
        name,
        file_name,
        file_type,
        browser_compatibility: vec!["chromium".to_string()],
        created_at: now_secs(),
        updated_at: now_secs(),
        sync_enabled: false,
        version: manifest
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_owned),
        description: manifest
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned),
        author: None,
        homepage_url: None,
        git_source,
        source_revision,
    };

    let conn = init_db().map_err(|e| e.to_string())?;
    let compat =
        serde_json::to_string(&item.browser_compatibility).unwrap_or_else(|_| "[]".to_string());
    conn.execute(
        r#"
        INSERT INTO extensions (id, name, file_name, file_type, browser_compatibility, created_at, updated_at, sync_enabled, version, description, author, homepage_url, icon_data, package_data, git_source, source_revision)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?9, ?10, ?11, NULL, ?12, ?13, ?14)
        ON CONFLICT(id) DO UPDATE SET name=excluded.name, package_data=excluded.package_data, version=excluded.version, description=excluded.description, updated_at=excluded.updated_at, git_source=excluded.git_source, source_revision=excluded.source_revision
        "#,
        params![
            item.id, item.name, item.file_name, item.file_type, compat,
            item.created_at, item.updated_at, item.version, item.description,
            item.author, item.homepage_url, file_data,
            item.git_source.as_ref().map(serde_json::to_string).transpose().map_err(|e| e.to_string())?, item.source_revision
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
        git_source: None,
        source_revision: None,
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
    file_data: Option<Vec<u8>>,
) -> Result<(), String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    if let (Some(fn_val), Some(fd)) = (file_name, file_data) {
        conn.execute(
            "UPDATE extensions SET name = ?1, file_name = ?2, package_data = ?3, updated_at = ?4, git_source = NULL, source_revision = NULL WHERE id = ?5",
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
        "UPDATE extensions SET name = ?1, file_name = ?2, updated_at = ?3, file_type = 'unpacked', git_source = NULL, source_revision = NULL WHERE id = ?4",
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
    if Url::parse(url.trim()).ok().is_some_and(|url| {
        url.host_str() == Some("github.com") && url.path().trim_matches('/').split('/').count() == 2
    }) {
        let source = crate::extension_packages::source_from_url(&url).await?;
        let revision = crate::extension_packages::resolve_revision(&source).await?;
        let package = crate::extension_packages::fetch(source, revision).await?;
        return Ok(FetchedExtension {
            name: package
                .manifest
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned),
            version: package
                .manifest
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_owned),
            description: package
                .manifest
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned),
            author: None,
            file_name: "github-extension.zip".into(),
            file_data: Some(base64::engine::general_purpose::STANDARD.encode(package.bytes)),
            path: None,
            source_url: Some(url),
            git_source: Some(package.source),
            source_revision: Some(package.revision),
        });
    }
    const MAX_BYTES: usize = 64 * 1024 * 1024;
    const ZIP_MAGIC: &[u8] = b"PK\x03\x04";
    const CRX_MAGIC: &[u8] = b"Cr24";

    let input = url.trim();
    let source_url = if input.len() == 32
        && input
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() && byte <= b'p')
    {
        format!(
            "https://clients2.google.com/service/update2/crx?response=redirect&acceptformat=crx3&prodversion=120.0.0.0&nacl_arch=x86-64&x=id%3D{input}%26installsource%3Dondemand%26uc"
        )
    } else {
        let parsed =
            Url::parse(input).map_err(|_| "Liên kết tiện ích không hợp lệ.".to_string())?;
        if parsed.scheme() != "https" {
            return Err("Chỉ tải tiện ích qua HTTPS.".to_string());
        }
        let host = parsed.host_str().unwrap_or_default();
        let is_store = matches!(
            host,
            "chromewebstore.google.com" | "chrome.google.com" | "www.chrome.google.com"
        );
        if is_store {
            let id = parsed
                .path_segments()
                .into_iter()
                .flatten()
                .find(|segment| {
                    segment.len() == 32
                        && segment
                            .bytes()
                            .all(|byte| byte.is_ascii_lowercase() && byte <= b'p')
                })
                .ok_or_else(|| {
                    "Không tìm thấy ID tiện ích trong liên kết Chrome Web Store.".to_string()
                })?;
            format!(
                "https://clients2.google.com/service/update2/crx?response=redirect&acceptformat=crx3&prodversion=120.0.0.0&nacl_arch=x86-64&x=id%3D{id}%26installsource%3Dondemand%26uc"
            )
        } else if parsed.path().to_ascii_lowercase().ends_with(".zip")
            || parsed.path().to_ascii_lowercase().ends_with(".crx")
        {
            parsed.to_string()
        } else {
            return Err(
                "Liên kết phải là Chrome Web Store hoặc tệp .zip/.crx trực tiếp.".to_string(),
            );
        }
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .connect_timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.url().scheme() == "https" && attempt.previous().len() < 5 {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()
        .map_err(|_| "Không thể khởi tạo trình tải tiện ích.".to_string())?;
    let response = client
        .get(&source_url)
        .send()
        .await
        .map_err(|_| "Không tải được tiện ích từ liên kết này.".to_string())?;
    if !response.status().is_success() || response.url().scheme() != "https" {
        return Err("Máy chủ không trả về gói tiện ích hợp lệ.".to_string());
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BYTES as u64)
    {
        return Err("Tệp tiện ích vượt giới hạn 64 MiB.".to_string());
    }
    use futures_util::StreamExt;
    let mut archive_bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "Tải gói tiện ích bị gián đoạn.".to_string())?;
        if archive_bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
            return Err("Tệp tiện ích vượt giới hạn 64 MiB.".to_string());
        }
        archive_bytes.extend_from_slice(&chunk);
    }

    let zip_bytes = if archive_bytes.starts_with(ZIP_MAGIC) {
        archive_bytes
    } else if archive_bytes.starts_with(CRX_MAGIC) && archive_bytes.len() >= 12 {
        let version = u32::from_le_bytes(archive_bytes[4..8].try_into().unwrap_or_default());
        let header_len =
            u32::from_le_bytes(archive_bytes[8..12].try_into().unwrap_or_default()) as usize;
        let start = 12usize
            .checked_add(header_len)
            .ok_or_else(|| "Gói CRX không hợp lệ.".to_string())?;
        if version != 3 {
            return Err("Chỉ hỗ trợ gói CRX3.".to_string());
        }
        let payload = archive_bytes
            .get(start..)
            .filter(|payload| payload.starts_with(ZIP_MAGIC))
            .ok_or_else(|| "Gói CRX không hợp lệ.".to_string())?;
        payload.to_vec()
    } else {
        return Err("Dữ liệu tải về không phải gói ZIP/CRX.".to_string());
    };

    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&zip_bytes))
        .map_err(|_| "Tệp ZIP tiện ích bị lỗi.".to_string())?;
    let mut manifest_text = String::new();
    zip.by_name("manifest.json")
        .map_err(|_| "Gói không có manifest.json ở thư mục gốc.".to_string())?
        .read_to_string(&mut manifest_text)
        .map_err(|_| "Không đọc được manifest.json của tiện ích.".to_string())?;
    let manifest: Value = serde_json::from_str(&manifest_text)
        .map_err(|_| "manifest.json của tiện ích không hợp lệ.".to_string())?;
    let manifest_name = manifest
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty());
    let version = manifest
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let description = manifest
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let file_name = if input.len() == 32 {
        format!("{input}.zip")
    } else {
        Url::parse(input)
            .ok()
            .and_then(|parsed| {
                parsed
                    .path_segments()
                    .and_then(|mut parts| parts.rfind(|part| !part.is_empty()).map(str::to_owned))
            })
            .unwrap_or_else(|| "extension.zip".to_string())
            .trim_end_matches(".crx")
            .to_owned()
            + ".zip"
    };
    let name = manifest_name
        .map(str::to_owned)
        .unwrap_or_else(|| file_name.trim_end_matches(".zip").to_string());
    Ok(FetchedExtension {
        name: Some(name),
        version,
        description,
        author: None,
        file_name,
        file_data: Some(base64::engine::general_purpose::STANDARD.encode(zip_bytes)),
        path: None,
        source_url: Some(input.to_string()),
        git_source: None,
        source_revision: None,
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

/// Sources travel with the authenticated API group so another device can install them.
pub fn group_git_sources(ids: &Value, existing: &Value) -> Result<Value, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let mut result = serde_json::Map::new();
    for id in ids.as_array().ok_or("Danh sách tiện ích không hợp lệ.")? {
        let id = id.as_str().ok_or("ID tiện ích không hợp lệ.")?;
        let local: Option<Option<String>> = conn
            .query_row(
                "SELECT git_source FROM extensions WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(Some(source)) = local.as_ref() {
            let source: GitExtensionSource =
                serde_json::from_str(&source).map_err(|e| e.to_string())?;
            source.validate().map_err(str::to_owned)?;
            result.insert(
                id.into(),
                serde_json::to_value(source).map_err(|e| e.to_string())?,
            );
        } else if local.is_none()
            && let Some(source) = existing.get(id)
        {
            result.insert(id.into(), source.clone());
        }
    }
    Ok(Value::Object(result))
}

pub fn remember_group_sources(group: &Value) -> Result<(), String> {
    let Some(sources) = group.get("extensionSources").and_then(Value::as_object) else {
        return Ok(());
    };
    let conn = init_db().map_err(|e| e.to_string())?;
    for (id, value) in sources {
        let source: GitExtensionSource =
            serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        source.validate().map_err(str::to_owned)?;
        if id != &source.extension_id() {
            return Err("ID tiện ích Git không khớp nguồn.".into());
        }
        let name = source
            .repository_url
            .rsplit('/')
            .next()
            .unwrap_or("GitHub extension");
        conn.execute("INSERT INTO extensions (id, name, file_name, file_type, browser_compatibility, created_at, updated_at, sync_enabled, git_source) VALUES (?1, ?2, 'github-extension.zip', 'zip', '[\"chromium\"]', ?3, ?3, 0, ?4) ON CONFLICT(id) DO NOTHING",
            params![id, name, now_secs(), serde_json::to_string(&source).map_err(|e| e.to_string())?]).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[derive(Default)]
pub struct PreparedExtensions {
    pub paths: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

struct LocalPackage {
    id: String,
    kind: String,
    path: String,
    bytes: Option<Vec<u8>>,
    source: Option<GitExtensionSource>,
}

fn local_packages() -> Result<Vec<LocalPackage>, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let mut statement = conn
        .prepare("SELECT id, file_type, file_name, package_data, git_source FROM extensions")
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<Vec<u8>>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    rows.map(|row| {
        let (id, kind, path, bytes, source) = row.map_err(|e| e.to_string())?;
        Ok(LocalPackage {
            id,
            kind,
            path,
            bytes,
            source: source
                .map(|value| serde_json::from_str(&value))
                .transpose()
                .map_err(|e| e.to_string())?,
        })
    })
    .collect()
}

pub async fn prepare_for_profile(
    profile_id: &str,
    ids: &[ExtensionId],
    sources: &BTreeMap<ExtensionId, GitExtensionSource>,
) -> Result<PreparedExtensions, String> {
    use sha2::{Digest, Sha256};
    let packages = local_packages()?;
    let root = get_app_data_dir().join("profile-extensions");
    let mut prepared = PreparedExtensions::default();
    for id in ids {
        let local = packages.iter().find(|package| package.id == id.as_str());
        let source = sources
            .get(id)
            .cloned()
            .or_else(|| local.and_then(|package| package.source.clone()));
        let cached = crate::extension_packages::installed(&root, profile_id, id.as_str()).filter(
            |(_, revision)| {
                source.as_ref().is_none_or(|source| {
                    revision.starts_with(&format!(
                        "{}:",
                        crate::extension_packages::source_id(source)
                    ))
                })
            },
        );
        let result: Result<PathBuf, String> = async {
            if let Some(source) = source {
                // GitHub is an optional updater, not part of profile lifecycle.
                // Bound its network work so an unavailable GitHub cannot hold
                // the browser launch open; use the installed revision instead.
                let update = tokio::time::timeout(std::time::Duration::from_secs(3), async {
                    let revision = crate::extension_packages::resolve_revision(&source).await?;
                    // Include source identity: changing a group's source must not reuse another repo's cache.
                    let identity = format!(
                        "{}:{revision}",
                        crate::extension_packages::source_id(&source)
                    );
                    if cached
                        .as_ref()
                        .is_some_and(|(_, installed_revision)| installed_revision == &identity)
                    {
                        return Ok(None);
                    }
                    let package = crate::extension_packages::fetch(source, revision).await?;
                    Ok::<_, String>(Some((identity, package.bytes)))
                })
                .await
                .map_err(|_| "GitHub extension update timed out.".to_owned())??;
                let Some((identity, bytes)) = update else {
                    return cached
                        .as_ref()
                        .map(|(path, _)| path.clone())
                        .ok_or_else(|| "Extension cache is missing.".to_owned());
                };
                let root = root.clone();
                let profile_id = profile_id.to_owned();
                let id = id.as_str().to_owned();
                return tokio::task::spawn_blocking(move || {
                    crate::extension_packages::install(&root, &profile_id, &id, &identity, &bytes)
                })
                .await
                .map_err(|e| e.to_string())?;
            }
            let local = local.ok_or("Chưa tải gói tiện ích trên thiết bị này.")?;
            if local.kind == "unpacked" {
                let path = std::fs::canonicalize(&local.path)
                    .map_err(|_| "Không tìm thấy thư mục tiện ích.")?;
                let manifest: Value = serde_json::from_slice(
                    &std::fs::read(path.join("manifest.json"))
                        .map_err(|_| "Thư mục tiện ích thiếu manifest.json.")?,
                )
                .map_err(|_| "Manifest tiện ích không hợp lệ.")?;
                if manifest.get("manifest_version").is_none() {
                    return Err("Manifest tiện ích không hợp lệ.".into());
                }
                return Ok(path);
            }
            let bytes = local
                .bytes
                .as_ref()
                .ok_or("Tiện ích chưa có dữ liệu gói.")?;
            let revision = format!("{:x}", Sha256::digest(bytes));
            if let Some((path, installed_revision)) = cached.as_ref() {
                if installed_revision == &revision {
                    return Ok(path.clone());
                }
            }
            let root = root.clone();
            let profile_id = profile_id.to_owned();
            let id = id.as_str().to_owned();
            let bytes = bytes.clone();
            tokio::task::spawn_blocking(move || {
                crate::extension_packages::install(&root, &profile_id, &id, &revision, &bytes)
            })
            .await
            .map_err(|e| e.to_string())?
        }
        .await;
        match result {
            Ok(path) => prepared.paths.push(path),
            Err(error) => {
                if let Some((path, _)) = cached {
                    prepared.paths.push(path);
                    prepared.warnings.push(format!(
                        "{}: chưa cập nhật được, đang dùng bản đã cài. {error}",
                        id.as_str()
                    ));
                } else {
                    prepared
                        .warnings
                        .push(format!("{}: chưa cài được tiện ích. {error}", id.as_str()));
                }
            }
        }
    }
    Ok(prepared)
}
