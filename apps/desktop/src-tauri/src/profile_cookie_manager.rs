#![forbid(unsafe_code)]

use aes::Aes128;
use buglogin_domain::errors::Problem;
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use ring::pbkdf2;
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, State};

use crate::desktop_state::DesktopState;
use crate::legacy_compat::has_profile_password;
use crate::profile_lifecycle::canonical_dir_name;
use crate::profile_password;

type Aes128CbcDec = cbc::Decryptor<Aes128>;

const KEY_LEN: usize = 16;
const SALT: &[u8] = b"saltysalt";
const IV: [u8; 16] = [b' '; 16];
const HOST_HASH_LEN: usize = 32;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UnifiedCookieItem {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires: i64,
    pub is_secure: bool,
    pub is_http_only: bool,
    pub same_site: i32,
}

fn derive_key(password: &[u8]) -> [u8; KEY_LEN] {
    let mut key = [0u8; KEY_LEN];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA1,
        NonZeroU32::new(1).expect("non-zero"),
        SALT,
        password,
        &mut key,
    );
    key
}

fn decrypt_cookie_value(encrypted: &[u8], host_key: &str, key: &[u8; KEY_LEN]) -> Option<String> {
    if encrypted.len() < 3 {
        return None;
    }
    let prefix = &encrypted[..3];
    if prefix != b"v10" && prefix != b"v11" {
        return None;
    }
    let ciphertext = &encrypted[3..];
    if ciphertext.is_empty() {
        return Some(String::new());
    }

    let mut buf = ciphertext.to_vec();
    let decrypted = Aes128CbcDec::new(key.into(), &IV.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .ok()?;

    if decrypted.len() >= HOST_HASH_LEN {
        let expected: [u8; HOST_HASH_LEN] = Sha256::digest(host_key.as_bytes()).into();
        if decrypted[..HOST_HASH_LEN] == expected {
            return String::from_utf8(decrypted[HOST_HASH_LEN..].to_vec()).ok();
        }
    }

    String::from_utf8(decrypted.to_vec()).ok()
}

pub fn resolve_profile_dir(state: &DesktopState, profile_id: &str) -> Option<PathBuf> {
    let profile_id = canonical_dir_name(profile_id).ok()?;
    let profiles_root = state.profiles_root()?;
    let direct = profiles_root.join(profile_id);
    if direct.exists() {
        return Some(direct);
    }
    // Also check standard AppData profiles path
    if let Some(data_dir) = dirs::data_local_dir() {
        let candidate = data_dir.join("BugLogin").join("profiles").join(profile_id);
        if candidate.exists() {
            return Some(candidate);
        }
        let candidate_local = data_dir
            .join("BugLogin-LocalApi")
            .join("profiles")
            .join(profile_id);
        if candidate_local.exists() {
            return Some(candidate_local);
        }
    }
    Some(direct)
}

pub fn find_cookie_db(profile_dir: &Path) -> Option<PathBuf> {
    let candidates = [
        profile_dir
            .join("profile")
            .join("Default")
            .join("Network")
            .join("Cookies"),
        profile_dir.join("Default").join("Network").join("Cookies"),
        profile_dir.join("profile").join("Default").join("Cookies"),
        profile_dir.join("Default").join("Cookies"),
    ];
    for path in &candidates {
        if path.exists() {
            return Some(path.clone());
        }
    }
    None
}

fn read_os_crypt_key(profile_dir: &Path) -> Option<[u8; KEY_LEN]> {
    let candidates = [
        profile_dir.join("os_crypt_key"),
        profile_dir.join("profile").join("os_crypt_key"),
    ];
    for path in &candidates {
        if let Ok(bytes) = fs::read(path) {
            if !bytes.is_empty() {
                return Some(derive_key(&bytes));
            }
        }
    }
    None
}

#[tauri::command(rename_all = "camelCase")]
pub async fn get_profile_cookie_stats(
    state: State<'_, DesktopState>,
    profile_id: String,
) -> Result<Value, Problem> {
    let Some(profile_dir) = resolve_profile_dir(&state, &profile_id) else {
        return Ok(json!({
            "profile_id": profile_id,
            "browser_type": "wayfern",
            "total_count": 0,
            "domains": []
        }));
    };

    let Some(db_path) = find_cookie_db(&profile_dir) else {
        return Ok(json!({
            "profile_id": profile_id,
            "browser_type": "wayfern",
            "total_count": 0,
            "domains": []
        }));
    };

    let uri = format!("file:{}?mode=ro&immutable=1", db_path.to_string_lossy());
    let conn = Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| {
        Problem::new(
            "cookie_db_error",
            format!("Failed to open cookies database: {e}"),
            500,
            "cookie-stats",
        )
    })?;

    let total_count: usize = conn
        .query_row("SELECT COUNT(*) FROM cookies", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap_or(0) as usize;

    let mut domains = Vec::new();
    if let Ok(mut stmt) = conn.prepare("SELECT host_key, COUNT(*) FROM cookies GROUP BY host_key ORDER BY COUNT(*) DESC, host_key ASC") {
        if let Ok(rows) = stmt.query_map([], |row| {
            Ok(json!({
                "domain": row.get::<_, String>(0)?,
                "count": row.get::<_, i64>(1)?,
            }))
        }) {
            for r in rows.flatten() {
                domains.push(r);
            }
        }
    }

    Ok(json!({
        "profile_id": profile_id,
        "browser_type": "wayfern",
        "total_count": total_count,
        "domains": domains
    }))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn export_profile_cookies(
    state: State<'_, DesktopState>,
    profile_id: String,
    format: String,
) -> Result<String, Problem> {
    if has_profile_password(&profile_id) && !profile_password::take_unlock(&profile_id) {
        return Err(profile_password::locked_problem());
    }
    let profile_dir = resolve_profile_dir(&state, &profile_id).ok_or_else(|| {
        Problem::new(
            "profile_not_found",
            "Profile directory not found",
            404,
            "cookie-export",
        )
    })?;

    let db_path = find_cookie_db(&profile_dir).ok_or_else(|| {
        Problem::new(
            "cookie_db_missing",
            "No cookie database found for this profile",
            404,
            "cookie-export",
        )
    })?;

    let encryption_key = read_os_crypt_key(&profile_dir);

    let uri = format!("file:{}?mode=ro&immutable=1", db_path.to_string_lossy());
    let conn = Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| {
        Problem::new(
            "cookie_db_error",
            format!("Failed to open cookies database: {e}"),
            500,
            "cookie-export",
        )
    })?;

    let mut stmt = conn
        .prepare("SELECT name, value, host_key, path, expires_utc, is_secure, is_httponly, samesite, encrypted_value FROM cookies")
        .map_err(|e| Problem::new("cookie_query_error", format!("Failed to query cookies: {e}"), 500, "cookie-export"))?;

    let rows = stmt
        .query_map([], |row| {
            let name: String = row.get(0)?;
            let plaintext_value: String = row.get(1)?;
            let domain: String = row.get(2)?;
            let path: String = row.get(3)?;
            let expires_utc: i64 = row.get(4)?;
            let is_secure: i32 = row.get(5)?;
            let is_httponly: i32 = row.get(6)?;
            let samesite: i32 = row.get(7)?;
            let encrypted_value: Vec<u8> = row.get(8)?;

            let value = if !plaintext_value.is_empty() {
                plaintext_value
            } else if !encrypted_value.is_empty() {
                encryption_key
                    .and_then(|key| decrypt_cookie_value(&encrypted_value, &domain, &key))
                    .unwrap_or_default()
            } else {
                String::new()
            };

            let expires_unix = if expires_utc == 0 {
                0
            } else {
                let windows_epoch_diff = 11_644_473_600_i64;
                (expires_utc / 1_000_000) - windows_epoch_diff
            };

            Ok(UnifiedCookieItem {
                name,
                value,
                domain,
                path,
                expires: expires_unix,
                is_secure: is_secure != 0,
                is_http_only: is_httponly != 0,
                same_site: samesite,
            })
        })
        .map_err(|e| {
            Problem::new(
                "cookie_query_error",
                format!("Failed to read cookie rows: {e}"),
                500,
                "cookie-export",
            )
        })?;

    let mut cookies = Vec::new();
    for r in rows.flatten() {
        cookies.push(r);
    }

    match format.as_str() {
        "json" => {
            let json_cookies: Vec<Value> = cookies
                .iter()
                .map(|c| {
                    let same_site_str = match c.same_site {
                        1 => "lax",
                        2 => "strict",
                        _ => "no_restriction",
                    };
                    json!({
                        "name": c.name,
                        "value": c.value,
                        "domain": c.domain,
                        "path": c.path,
                        "secure": c.is_secure,
                        "httpOnly": c.is_http_only,
                        "sameSite": same_site_str,
                        "expirationDate": c.expires,
                        "session": c.expires == 0,
                        "hostOnly": !c.domain.starts_with('.'),
                    })
                })
                .collect();
            serde_json::to_string_pretty(&json_cookies).map_err(|e| {
                Problem::new("serialization_failed", e.to_string(), 500, "cookie-export")
            })
        }
        "netscape" => {
            let mut lines = vec![
                "# Netscape HTTP Cookie File".to_string(),
                "# https://curl.haxx.se/rfc/cookie_spec.html".to_string(),
                "# This file was generated by BugLogin".to_string(),
                "".to_string(),
            ];
            for c in cookies {
                let flag = if c.domain.starts_with('.') {
                    "TRUE"
                } else {
                    "FALSE"
                };
                let secure = if c.is_secure { "TRUE" } else { "FALSE" };
                lines.push(format!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    c.domain, flag, c.path, secure, c.expires, c.name, c.value
                ));
            }
            Ok(lines.join("\n"))
        }
        other => Err(Problem::new(
            "unsupported_format",
            format!("Format {other} not supported"),
            400,
            "cookie-export",
        )),
    }
}

pub fn parse_raw_cookies(content: &str) -> Vec<UnifiedCookieItem> {
    let trimmed = content.trim();
    if trimmed.starts_with('[') {
        if let Ok(values) = serde_json::from_str::<Vec<Value>>(trimmed) {
            let mut result = Vec::new();
            for item in values {
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let value = item
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let domain = item
                    .get("domain")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("host").and_then(Value::as_str))
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let path = item
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or("/")
                    .trim()
                    .to_string();
                let is_secure = item.get("secure").and_then(Value::as_bool).unwrap_or(false);
                let is_http_only = item
                    .get("httpOnly")
                    .or_else(|| item.get("http_only"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let expires = item
                    .get("expirationDate")
                    .or_else(|| item.get("expires"))
                    .or_else(|| item.get("expires_utc"))
                    .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
                    .unwrap_or(0);
                let same_site = match item
                    .get("sameSite")
                    .or_else(|| item.get("same_site"))
                    .and_then(Value::as_str)
                {
                    Some("lax") => 1,
                    Some("strict") => 2,
                    _ => 0,
                };
                if !name.is_empty() && !domain.is_empty() {
                    result.push(UnifiedCookieItem {
                        name,
                        value,
                        domain,
                        path,
                        expires,
                        is_secure,
                        is_http_only,
                        same_site,
                    });
                }
            }
            if !result.is_empty() {
                return result;
            }
        }
    }

    // Netscape format fallback
    let mut result = Vec::new();
    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 7 {
            let domain = parts[0].trim().to_string();
            let path = parts[2].trim().to_string();
            let is_secure = parts[3].eq_ignore_ascii_case("true");
            let expires = parts[4].trim().parse::<i64>().unwrap_or(0);
            let name = parts[5].trim().to_string();
            let value = parts[6].to_string();
            if !name.is_empty() && !domain.is_empty() {
                result.push(UnifiedCookieItem {
                    name,
                    value,
                    domain,
                    path,
                    expires,
                    is_secure,
                    is_http_only: false,
                    same_site: 0,
                });
            }
        }
    }
    result
}

#[tauri::command(rename_all = "camelCase")]
pub async fn import_profile_cookies(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
    content: String,
) -> Result<Value, Problem> {
    import_profile_cookies_inner(state, app, profile_id, content, None).await
}

pub async fn import_profile_cookies_replacing_domains(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
    content: String,
    domains: Vec<String>,
) -> Result<Value, Problem> {
    import_profile_cookies_inner(state, app, profile_id, content, Some(domains)).await
}

async fn import_profile_cookies_inner(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
    content: String,
    replace_domains: Option<Vec<String>>,
) -> Result<Value, Problem> {
    let cookies = parse_raw_cookies(&content);
    if cookies.is_empty() {
        return Err(Problem::new(
            "invalid_cookies",
            "Không tìm thấy cookie hợp lệ nào trong tệp",
            400,
            "cookie-import",
        ));
    }

    let profile_dir = resolve_profile_dir(&state, &profile_id).ok_or_else(|| {
        Problem::new(
            "profile_not_found",
            "Profile directory not found",
            404,
            "cookie-import",
        )
    })?;

    let default_dir = if profile_dir.join("profile").exists() {
        profile_dir.join("profile").join("Default")
    } else {
        profile_dir.join("Default")
    };
    let network_dir = default_dir.join("Network");
    let _ = fs::create_dir_all(&network_dir);
    let db_path = network_dir.join("Cookies");

    let mut conn = Connection::open(&db_path).map_err(|e| {
        Problem::new(
            "cookie_db_error",
            format!("Không thể mở cơ sở dữ liệu cookie: {e}"),
            500,
            "cookie-import",
        )
    })?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS cookies (
            creation_utc INTEGER NOT NULL,
            host_key TEXT NOT NULL,
            top_frame_site_key TEXT NOT NULL DEFAULT '',
            name TEXT NOT NULL,
            value TEXT NOT NULL DEFAULT '',
            encrypted_value BLOB NOT NULL DEFAULT '',
            path TEXT NOT NULL,
            expires_utc INTEGER NOT NULL,
            is_secure INTEGER NOT NULL,
            is_httponly INTEGER NOT NULL,
            last_access_utc INTEGER NOT NULL,
            has_expires INTEGER NOT NULL DEFAULT 1,
            is_persistent INTEGER NOT NULL DEFAULT 1,
            priority INTEGER NOT NULL DEFAULT 1,
            samesite INTEGER NOT NULL DEFAULT -1,
            source_scheme INTEGER NOT NULL DEFAULT 0,
            source_port INTEGER NOT NULL DEFAULT -1,
            last_update_utc INTEGER NOT NULL DEFAULT 0,
            source_type INTEGER NOT NULL DEFAULT 0,
            has_cross_site_ancestor INTEGER NOT NULL DEFAULT 0
        );
        CREATE UNIQUE INDEX IF NOT EXISTS cookies_unique_index
            ON cookies(host_key, top_frame_site_key, name, path);
        CREATE TABLE IF NOT EXISTS meta (
            key LONGVARCHAR NOT NULL UNIQUE PRIMARY KEY,
            value LONGVARCHAR
        );",
    )
    .map_err(|e| {
        Problem::new(
            "cookie_schema_error",
            format!("Failed to create cookies table: {e}"),
            500,
            "cookie-import",
        )
    })?;

    let tx = conn
        .transaction()
        .map_err(|e| Problem::new("tx_error", e.to_string(), 500, "cookie-import"))?;

    let now_epoch_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0);
    let windows_epoch_diff = 11_644_473_600_i64 * 1_000_000;
    let now_chrome = now_epoch_us + windows_epoch_diff;

    let mut imported = 0;
    let mut added = 0;
    let mut replaced = 0;
    let mut deleted = 0;
    {
        if let Some(domains) = replace_domains.as_ref() {
            for domain in domains {
                deleted += tx
                    .execute(
                        "DELETE FROM cookies WHERE host_key = ?1 AND top_frame_site_key = ''",
                        params![domain],
                    )
                    .map_err(|e| {
                        Problem::new("cookie_delete_error", e.to_string(), 500, "cookie-import")
                    })?;
            }
        }
        let mut insert_stmt = tx
            .prepare_cached(
                "INSERT OR REPLACE INTO cookies (
                creation_utc, host_key, top_frame_site_key, name, value, encrypted_value,
                path, expires_utc, is_secure, is_httponly, last_access_utc, has_expires,
                is_persistent, priority, samesite, source_scheme, source_port,
                last_update_utc, source_type, has_cross_site_ancestor
            ) VALUES (
                ?1, ?2, '', ?3, ?4, X'', ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1, ?12, ?13, ?14, ?15, 0, 0
            )",
            )
            .map_err(|e| Problem::new("prepare_error", e.to_string(), 500, "cookie-import"))?;

        for c in &cookies {
            let expires_chrome = if c.expires == 0 {
                0
            } else {
                (c.expires + 11_644_473_600_i64) * 1_000_000
            };
            let has_expires = if c.expires > 0 { 1 } else { 0 };
            let is_persistent = has_expires;
            let source_scheme = if c.is_secure { 2 } else { 1 };
            let source_port = if c.is_secure { 443 } else { 80 };

            let existed: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM cookies WHERE host_key = ?1 AND top_frame_site_key = '' AND name = ?2 AND path = ?3)",
                    params![c.domain, c.name, c.path],
                    |row| row.get(0),
                )
                .unwrap_or(false);

            insert_stmt
                .execute(params![
                    now_chrome,
                    c.domain,
                    c.name,
                    c.value,
                    c.path,
                    expires_chrome,
                    if c.is_secure { 1 } else { 0 },
                    if c.is_http_only { 1 } else { 0 },
                    now_chrome,
                    has_expires,
                    is_persistent,
                    c.same_site,
                    source_scheme,
                    source_port,
                    now_chrome,
                ])
                .map_err(|e| {
                    Problem::new("cookie_insert_error", e.to_string(), 500, "cookie-import")
                })?;
            imported += 1;
            if existed {
                replaced += 1;
            } else {
                added += 1;
            }
        }
    }

    tx.commit()
        .map_err(|e| Problem::new("commit_error", e.to_string(), 500, "cookie-import"))?;

    let _ = app.emit("profiles-changed", json!({ "profileId": profile_id }));
    let _ = app.emit(
        "profile-cookies-changed",
        json!({ "profileId": profile_id }),
    );

    Ok(json!({
        "cookies_imported": imported,
        "cookies_added": added,
        "cookies_replaced": replaced,
        "cookies_deleted": deleted,
        "errors": []
    }))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn get_profile_local_storage_stats(
    state: State<'_, DesktopState>,
    profile_id: String,
) -> Result<Value, Problem> {
    let Some(profile_dir) = resolve_profile_dir(&state, &profile_id) else {
        return Ok(json!({ "origins": 0, "items": 0 }));
    };

    let leveldb_candidates = [
        profile_dir
            .join("profile")
            .join("Default")
            .join("Local Storage")
            .join("leveldb"),
        profile_dir
            .join("Default")
            .join("Local Storage")
            .join("leveldb"),
    ];

    let mut origins = 0;
    let mut items = 0;

    for dir in &leveldb_candidates {
        if dir.exists() {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    if ext == "ldb" || ext == "log" {
                        origins += 1;
                        items += (entry.metadata().map(|m| m.len()).unwrap_or(0) / 1024) as usize;
                    }
                }
            }
            break;
        }
    }

    Ok(json!({
        "origins": origins,
        "items": items.max(origins)
    }))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn export_profile_storage_state(
    state: State<'_, DesktopState>,
    profile_id: String,
    include_cookies: Option<bool>,
    include_indexed_db: Option<bool>,
) -> Result<String, Problem> {
    let cookies_json: Value = if include_cookies.unwrap_or(true) {
        if let Ok(raw_cookies) =
            export_profile_cookies(state.clone(), profile_id.clone(), "json".to_string()).await
        {
            serde_json::from_str(&raw_cookies).unwrap_or_else(|_| json!([]))
        } else {
            json!([])
        }
    } else {
        json!([])
    };

    let output = json!({
        "profileId": profile_id,
        "schemaVersion": 1,
        "cookies": cookies_json,
        "origins": [],
        "includeIndexedDb": include_indexed_db.unwrap_or(false)
    });

    serde_json::to_string_pretty(&output)
        .map_err(|e| Problem::new("export_storage_error", e.to_string(), 500, "storage-state"))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn import_profile_storage_state(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
    content: String,
) -> Result<Value, Problem> {
    let parsed: Value = serde_json::from_str(&content).map_err(|e| {
        Problem::new(
            "invalid_json",
            format!("Invalid storage state JSON: {e}"),
            400,
            "storage-state",
        )
    })?;

    let mut cookies_imported = 0;
    if let Some(cookies_val) = parsed.get("cookies") {
        let cookies_str = cookies_val.to_string();
        if let Ok(res) = import_profile_cookies(state, app, profile_id, cookies_str).await {
            cookies_imported = res
                .get("cookies_imported")
                .and_then(Value::as_u64)
                .unwrap_or(0);
        }
    }

    Ok(json!({
        "origins": 0,
        "items": 0,
        "cookies_imported": cookies_imported,
        "indexed_db_records": 0
    }))
}
