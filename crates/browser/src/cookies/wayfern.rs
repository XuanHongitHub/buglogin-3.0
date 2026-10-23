use super::{CookieInput, CookieSameSite};
use aes::Aes128;
use buglogin_storage::secrets::SecretValue;
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use rusqlite::{Connection, OpenFlags, params};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    io::Read,
    num::NonZeroU32,
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

const WINDOWS_EPOCH_DIFF: i64 = 11_644_473_600;
const MIN_SUPPORTED_COOKIE_SCHEMA_VERSION: i64 = 23;
const COOKIE_KEY_BYTES: usize = 16;
const COOKIE_KEY_SALT: &[u8] = b"saltysalt";
const COOKIE_KEY_IV: [u8; 16] = [b' '; 16];
const COOKIE_HOST_HASH_BYTES: usize = 32;
const MAX_OS_CRYPT_KEY_BYTES: usize = 4096;
const MAX_COOKIE_ROWS: usize = 100_000;
const MAX_COOKIE_VALUE_BYTES: usize = 64 * 1024 * 1024;

#[cfg(target_os = "macos")]
const COOKIE_KEY_ITERATIONS: u32 = 1003;
#[cfg(not(target_os = "macos"))]
const COOKIE_KEY_ITERATIONS: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileCookieState {
    Stopped,
    Running,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WayfernCookieReport {
    pub inserted: usize,
    pub replaced: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncryptedCookieInput {
    pub name: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub expires_at: Option<i64>,
    pub same_site: CookieSameSite,
    ciphertext: Vec<u8>,
}

impl EncryptedCookieInput {
    pub fn new(
        name: String,
        domain: String,
        path: String,
        secure: bool,
        http_only: bool,
        expires_at: Option<i64>,
        same_site: CookieSameSite,
        ciphertext: impl Into<Vec<u8>>,
    ) -> Result<Self, WayfernCookieError> {
        let ciphertext = ciphertext.into();
        if ciphertext.len() <= 3 || !matches!(&ciphertext[..3], b"v10" | b"v11") {
            return Err(WayfernCookieError::InvalidCiphertext);
        }
        Ok(Self {
            name,
            domain,
            path,
            secure,
            http_only,
            expires_at,
            same_site,
            ciphertext,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WayfernCookieError {
    RunningProfile,
    PlaintextPersistenceForbidden,
    InvalidCiphertext,
    InvalidValue,
    InvalidExpiry,
    ResourceLimit,
    PartitionedCookiesUnsupported,
    EncryptionKeyUnavailable,
    UnsupportedSchema,
    Database,
}

impl fmt::Display for WayfernCookieError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::RunningProfile => "cookie_profile_running",
            Self::PlaintextPersistenceForbidden => "cookie_plaintext_persistence_forbidden",
            Self::InvalidCiphertext => "invalid_cookie_ciphertext",
            Self::InvalidValue => "invalid_cookie_value",
            Self::InvalidExpiry => "invalid_cookie_expiry",
            Self::ResourceLimit => "cookie_resource_limit_exceeded",
            Self::PartitionedCookiesUnsupported => "cookie_partitioned_unsupported",
            Self::EncryptionKeyUnavailable => "cookie_encryption_key_unavailable",
            Self::UnsupportedSchema => "unsupported_cookie_database_schema",
            Self::Database => "cookie_database_error",
        })
    }
}

impl std::error::Error for WayfernCookieError {}

pub fn database_path(profile_data_root: &Path) -> PathBuf {
    let default = profile_data_root.join("Default");
    if cfg!(target_os = "windows") {
        default.join("Network").join("Cookies")
    } else {
        default.join("Cookies")
    }
}

pub fn materialize(
    profile_data_root: &Path,
    state: ProfileCookieState,
    cookies: &[CookieInput],
    now_unix: i64,
) -> Result<WayfernCookieReport, WayfernCookieError> {
    if state == ProfileCookieState::Running {
        return Err(WayfernCookieError::RunningProfile);
    }
    let _ = (profile_data_root, cookies, now_unix);
    Err(WayfernCookieError::PlaintextPersistenceForbidden)
}

pub fn materialize_from_inputs(
    profile_data_root: &Path,
    state: ProfileCookieState,
    cookies: &[CookieInput],
    now_unix: i64,
) -> Result<WayfernCookieReport, WayfernCookieError> {
    if state == ProfileCookieState::Running {
        return Err(WayfernCookieError::RunningProfile);
    }
    if now_unix < 0 {
        return Err(WayfernCookieError::InvalidExpiry);
    }
    let os_crypt_key = read_os_crypt_key(profile_data_root)?;
    if os_crypt_key.is_empty() || os_crypt_key.len() > MAX_OS_CRYPT_KEY_BYTES {
        return Err(WayfernCookieError::EncryptionKeyUnavailable);
    }
    let encrypted = cookies
        .iter()
        .map(|cookie| {
            let ciphertext =
                encrypt_cookie_value(&os_crypt_key, &cookie.domain, cookie.value.as_bytes())?;
            EncryptedCookieInput::new(
                cookie.name.clone(),
                cookie.domain.clone(),
                cookie.path.clone(),
                cookie.secure,
                cookie.http_only,
                cookie.expires_at,
                cookie.same_site,
                ciphertext,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    materialize_encrypted(profile_data_root, state, &encrypted, now_unix)
}

fn read_os_crypt_key(profile_data_root: &Path) -> Result<Zeroizing<Vec<u8>>, WayfernCookieError> {
    let file = std::fs::File::open(profile_data_root.join("os_crypt_key"))
        .map_err(|_| WayfernCookieError::EncryptionKeyUnavailable)?;
    let mut key = Zeroizing::new(Vec::with_capacity(MAX_OS_CRYPT_KEY_BYTES + 1));
    file.take((MAX_OS_CRYPT_KEY_BYTES + 1) as u64)
        .read_to_end(&mut key)
        .map_err(|_| WayfernCookieError::EncryptionKeyUnavailable)?;
    if key.is_empty() || key.len() > MAX_OS_CRYPT_KEY_BYTES {
        return Err(WayfernCookieError::EncryptionKeyUnavailable);
    }
    Ok(key)
}

fn encrypt_cookie_value(
    os_crypt_key: &[u8],
    host_key: &str,
    value: &[u8],
) -> Result<Vec<u8>, WayfernCookieError> {
    if os_crypt_key.is_empty()
        || os_crypt_key.len() > MAX_OS_CRYPT_KEY_BYTES
        || host_key.is_empty()
        || value.is_empty()
    {
        return Err(WayfernCookieError::InvalidValue);
    }
    let mut aes_key = Zeroizing::new([0_u8; COOKIE_KEY_BYTES]);
    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA1,
        NonZeroU32::new(COOKIE_KEY_ITERATIONS)
            .ok_or(WayfernCookieError::EncryptionKeyUnavailable)?,
        COOKIE_KEY_SALT,
        os_crypt_key,
        &mut *aes_key,
    );
    let mut plaintext = Zeroizing::new(Vec::with_capacity(
        COOKIE_HOST_HASH_BYTES + value.len() + 16,
    ));
    plaintext.extend_from_slice(&Sha256::digest(host_key.as_bytes()));
    plaintext.extend_from_slice(value);
    let original_len = plaintext.len();
    plaintext.resize(original_len + 16, 0);
    let encryption_result =
        cbc::Encryptor::<Aes128>::new(&(*aes_key).into(), &COOKIE_KEY_IV.into())
            .encrypt_padded_mut::<Pkcs7>(&mut *plaintext, original_len);
    let ciphertext = encryption_result.map_err(|_| WayfernCookieError::InvalidValue)?;
    let mut output = Vec::with_capacity(3 + ciphertext.len());
    output.extend_from_slice(b"v10");
    output.extend_from_slice(ciphertext);
    Ok(output)
}

pub fn materialize_encrypted(
    profile_data_root: &Path,
    state: ProfileCookieState,
    cookies: &[EncryptedCookieInput],
    now_unix: i64,
) -> Result<WayfernCookieReport, WayfernCookieError> {
    if state == ProfileCookieState::Running {
        return Err(WayfernCookieError::RunningProfile);
    }
    if now_unix < 0 {
        return Err(WayfernCookieError::InvalidExpiry);
    }
    let path = database_path(profile_data_root);
    if !path.is_file() {
        return Err(WayfernCookieError::Database);
    }
    let mut connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| WayfernCookieError::Database)?;
    validate_supported_schema(&connection)?;
    let transaction = connection
        .transaction()
        .map_err(|_| WayfernCookieError::Database)?;
    let mut report = WayfernCookieReport::default();
    for cookie in cookies {
        let (expires_utc, has_expires, is_persistent) = match cookie.expires_at {
            Some(expires) if expires >= 0 => (unix_to_chrome_time(expires)?, 1, 1),
            Some(_) => return Err(WayfernCookieError::InvalidExpiry),
            None => (0, 0, 0),
        };
        let source_scheme = if cookie.secure { 2 } else { 1 };
        let source_port = if cookie.secure { 443 } else { 80 };
        let existing: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM cookies WHERE host_key = ?1 AND top_frame_site_key = '' AND has_cross_site_ancestor = 0 AND name = ?2 AND path = ?3 AND source_scheme = ?4 AND source_port = ?5)",
                params![cookie.domain, cookie.name, cookie.path, source_scheme, source_port],
                |row| row.get(0),
            )
            .map_err(|_| WayfernCookieError::Database)?;
        let now_utc = unix_to_chrome_time(now_unix)?;
        let same_site = same_site_to_db(cookie.same_site);
        let secure = i32::from(cookie.secure);
        let http_only = i32::from(cookie.http_only);
        if existing {
            transaction.execute(
                "UPDATE cookies SET value = '', encrypted_value = ?1, expires_utc = ?2, is_secure = ?3, is_httponly = ?4, last_access_utc = ?5, has_expires = ?6, is_persistent = ?7, samesite = ?8, last_update_utc = ?9 WHERE host_key = ?10 AND top_frame_site_key = '' AND has_cross_site_ancestor = 0 AND name = ?11 AND path = ?12 AND source_scheme = ?13 AND source_port = ?14",
                params![cookie.ciphertext, expires_utc, secure, http_only, now_utc, has_expires, is_persistent, same_site, now_utc, cookie.domain, cookie.name, cookie.path, source_scheme, source_port],
            ).map_err(|_| WayfernCookieError::Database)?;
            report.replaced += 1;
        } else {
            transaction.execute(
                "INSERT INTO cookies (creation_utc, host_key, top_frame_site_key, name, value, encrypted_value, path, expires_utc, is_secure, is_httponly, last_access_utc, has_expires, is_persistent, priority, samesite, source_scheme, source_port, source_type, has_cross_site_ancestor, last_update_utc) VALUES (?1, ?2, '', ?3, '', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1, ?12, ?13, ?14, 0, 0, ?15)",
                params![now_utc, cookie.domain, cookie.name, cookie.ciphertext, cookie.path, expires_utc, secure, http_only, now_utc, has_expires, is_persistent, same_site, source_scheme, source_port, now_utc],
            ).map_err(|_| WayfernCookieError::Database)?;
            report.inserted += 1;
        }
    }
    transaction
        .commit()
        .map_err(|_| WayfernCookieError::Database)?;
    Ok(report)
}

pub fn read_metadata(
    profile_data_root: &Path,
) -> Result<Vec<CookieInputMetadata>, WayfernCookieError> {
    let path = database_path(profile_data_root);
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| WayfernCookieError::Database)?;
    let mut statement = connection.prepare("SELECT name, host_key, path, expires_utc, is_secure, is_httponly, samesite FROM cookies").map_err(|_| WayfernCookieError::Database)?;
    let rows = statement
        .query_map([], |row| {
            Ok(CookieInputMetadata {
                name: row.get(0)?,
                domain: row.get(1)?,
                path: row.get(2)?,
                expires_at: chrome_to_unix(row.get(3)?),
                secure: row.get::<_, i32>(4)? != 0,
                http_only: row.get::<_, i32>(5)? != 0,
                same_site: same_site_from_db(row.get(6)?),
            })
        })
        .map_err(|_| WayfernCookieError::Database)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| WayfernCookieError::Database)
}

/// Reads cookie values from a stopped local Wayfern profile into zeroizing secret containers.
/// Values are decrypted in memory only; this function never writes plaintext to disk or logs.
pub fn read_inputs(profile_data_root: &Path) -> Result<Vec<CookieInput>, WayfernCookieError> {
    let os_crypt_key = read_os_crypt_key(profile_data_root)?;
    let path = database_path(profile_data_root);
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| WayfernCookieError::Database)?;
    validate_supported_schema(&connection)?;
    let has_partitioned_cookies: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM cookies WHERE top_frame_site_key <> '' OR has_cross_site_ancestor <> 0)",
            [],
            |row| row.get(0),
        )
        .map_err(|_| WayfernCookieError::Database)?;
    if has_partitioned_cookies {
        return Err(WayfernCookieError::PartitionedCookiesUnsupported);
    }
    let mut statement = connection
        .prepare("SELECT name, host_key, path, value, encrypted_value, expires_utc, is_secure, is_httponly, samesite FROM cookies WHERE top_frame_site_key = '' AND has_cross_site_ancestor = 0")
        .map_err(|_| WayfernCookieError::Database)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Vec<u8>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i32>(6)? != 0,
                row.get::<_, i32>(7)? != 0,
                row.get::<_, i32>(8)?,
            ))
        })
        .map_err(|_| WayfernCookieError::Database)?;
    let mut cookies = Vec::new();
    let mut total_value_bytes = 0_usize;
    for row in rows {
        if cookies.len() >= MAX_COOKIE_ROWS {
            return Err(WayfernCookieError::ResourceLimit);
        }
        let (
            name,
            domain,
            path,
            plaintext_value,
            ciphertext,
            expires_utc,
            secure,
            http_only,
            same_site,
        ) = row.map_err(|_| WayfernCookieError::Database)?;
        let value = if !ciphertext.is_empty() {
            decrypt_cookie_value(&os_crypt_key, &domain, &ciphertext)?
        } else if !plaintext_value.is_empty() {
            Zeroizing::new(plaintext_value.into_bytes())
        } else {
            return Err(WayfernCookieError::InvalidValue);
        };
        total_value_bytes = total_value_bytes.saturating_add(value.len());
        if total_value_bytes > MAX_COOKIE_VALUE_BYTES {
            return Err(WayfernCookieError::ResourceLimit);
        }
        let value =
            SecretValue::new(value.to_vec()).map_err(|_| WayfernCookieError::InvalidValue)?;
        cookies.push(CookieInput {
            name,
            domain,
            path,
            secure,
            http_only,
            expires_at: chrome_to_unix(expires_utc),
            same_site: same_site_from_db(same_site),
            value,
        });
    }
    Ok(cookies)
}

fn decrypt_cookie_value(
    os_crypt_key: &[u8],
    host_key: &str,
    ciphertext: &[u8],
) -> Result<Zeroizing<Vec<u8>>, WayfernCookieError> {
    if os_crypt_key.is_empty()
        || os_crypt_key.len() > MAX_OS_CRYPT_KEY_BYTES
        || host_key.is_empty()
        || ciphertext.len() <= 3
        || !matches!(&ciphertext[..3], b"v10" | b"v11")
        || (ciphertext.len() - 3) % 16 != 0
    {
        return Err(WayfernCookieError::InvalidCiphertext);
    }
    let mut aes_key = Zeroizing::new([0_u8; COOKIE_KEY_BYTES]);
    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA1,
        NonZeroU32::new(COOKIE_KEY_ITERATIONS)
            .ok_or(WayfernCookieError::EncryptionKeyUnavailable)?,
        COOKIE_KEY_SALT,
        os_crypt_key,
        &mut *aes_key,
    );
    let mut plaintext = Zeroizing::new(ciphertext[3..].to_vec());
    let decrypted = cbc::Decryptor::<Aes128>::new(&(*aes_key).into(), &COOKIE_KEY_IV.into())
        .decrypt_padded_mut::<Pkcs7>(&mut *plaintext)
        .map_err(|_| WayfernCookieError::InvalidCiphertext)?;
    if decrypted.len() <= COOKIE_HOST_HASH_BYTES
        || decrypted[..COOKIE_HOST_HASH_BYTES] != Sha256::digest(host_key.as_bytes())[..]
    {
        return Err(WayfernCookieError::InvalidCiphertext);
    }
    let value = decrypted[COOKIE_HOST_HASH_BYTES..].to_vec();
    plaintext.clear();
    Ok(Zeroizing::new(value))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CookieInputMetadata {
    pub name: String,
    pub domain: String,
    pub path: String,
    pub expires_at: Option<i64>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: CookieSameSite,
}

fn validate_supported_schema(connection: &Connection) -> Result<(), WayfernCookieError> {
    let version = connection
        .query_row("SELECT value FROM meta WHERE key = 'version'", [], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|_| WayfernCookieError::UnsupportedSchema)?
        .parse::<i64>()
        .map_err(|_| WayfernCookieError::UnsupportedSchema)?;
    if version < MIN_SUPPORTED_COOKIE_SCHEMA_VERSION {
        return Err(WayfernCookieError::UnsupportedSchema);
    }

    let mut statement = connection
        .prepare("PRAGMA table_info(cookies)")
        .map_err(|_| WayfernCookieError::Database)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|_| WayfernCookieError::Database)?
        .collect::<Result<std::collections::BTreeSet<_>, _>>()
        .map_err(|_| WayfernCookieError::Database)?;
    let required_columns = [
        "creation_utc",
        "host_key",
        "top_frame_site_key",
        "name",
        "value",
        "encrypted_value",
        "path",
        "expires_utc",
        "is_secure",
        "is_httponly",
        "last_access_utc",
        "has_expires",
        "is_persistent",
        "priority",
        "samesite",
        "source_scheme",
        "source_port",
        "last_update_utc",
        "source_type",
        "has_cross_site_ancestor",
    ];
    if required_columns
        .iter()
        .any(|required| !columns.contains(*required))
    {
        return Err(WayfernCookieError::UnsupportedSchema);
    }

    let mut indexes = connection
        .prepare("PRAGMA index_list(cookies)")
        .map_err(|_| WayfernCookieError::Database)?;
    let has_expected_index = indexes
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(2)? != 0))
        })
        .map_err(|_| WayfernCookieError::Database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| WayfernCookieError::Database)?
        .iter()
        .any(|(name, unique)| name == "cookies_unique_index" && *unique);
    if !has_expected_index {
        return Err(WayfernCookieError::UnsupportedSchema);
    }
    let mut index_columns = connection
        .prepare("PRAGMA index_info(cookies_unique_index)")
        .map_err(|_| WayfernCookieError::Database)?;
    let index_columns = index_columns
        .query_map([], |row| row.get::<_, String>(2))
        .map_err(|_| WayfernCookieError::Database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| WayfernCookieError::Database)?;
    let expected_index_columns = [
        "host_key",
        "top_frame_site_key",
        "has_cross_site_ancestor",
        "name",
        "path",
        "source_scheme",
        "source_port",
    ];
    if index_columns
        .iter()
        .map(String::as_str)
        .ne(expected_index_columns)
    {
        return Err(WayfernCookieError::UnsupportedSchema);
    }
    Ok(())
}

fn same_site_to_db(value: CookieSameSite) -> i32 {
    match value {
        CookieSameSite::Unspecified => -1,
        CookieSameSite::None => 0,
        CookieSameSite::Lax => 1,
        CookieSameSite::Strict => 2,
    }
}

fn same_site_from_db(value: i32) -> CookieSameSite {
    match value {
        0 => CookieSameSite::None,
        1 => CookieSameSite::Lax,
        2 => CookieSameSite::Strict,
        _ => CookieSameSite::Unspecified,
    }
}

fn unix_to_chrome_time(value: i64) -> Result<i64, WayfernCookieError> {
    value
        .checked_add(WINDOWS_EPOCH_DIFF)
        .and_then(|value| value.checked_mul(1_000_000))
        .ok_or(WayfernCookieError::InvalidExpiry)
}

fn chrome_to_unix(value: i64) -> Option<i64> {
    if value == 0 {
        None
    } else {
        Some((value / 1_000_000).saturating_sub(WINDOWS_EPOCH_DIFF))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buglogin_storage::secrets::SecretValue;
    use rusqlite::Connection;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn input(value: &[u8]) -> CookieInput {
        CookieInput {
            name: "sid".into(),
            domain: ".example.com".into(),
            path: "/".into(),
            secure: true,
            http_only: true,
            expires_at: None,
            same_site: CookieSameSite::Lax,
            value: SecretValue::new(value.to_vec()).unwrap(),
        }
    }

    fn encrypted_input(ciphertext: &[u8]) -> EncryptedCookieInput {
        EncryptedCookieInput::new(
            "sid".into(),
            ".example.com".into(),
            "/".into(),
            true,
            true,
            None,
            CookieSameSite::Lax,
            ciphertext,
        )
        .unwrap()
    }

    #[test]
    fn decrypts_cookie_ciphertext_and_checks_host_binding() {
        let key = b"profile-local-os-crypt-key";
        let ciphertext = encrypt_cookie_value(key, ".example.com", b"session-value").unwrap();
        assert_eq!(
            decrypt_cookie_value(key, ".example.com", &ciphertext)
                .unwrap()
                .as_slice(),
            b"session-value"
        );
        assert_eq!(
            decrypt_cookie_value(key, ".other.example", &ciphertext),
            Err(WayfernCookieError::InvalidCiphertext)
        );
    }

    fn create_supported_database(root: &Path) {
        let path = database_path(root);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE meta(key LONGVARCHAR NOT NULL UNIQUE PRIMARY KEY, value LONGVARCHAR);
                 INSERT INTO meta(key, value) VALUES('version', '24');
                 INSERT INTO meta(key, value) VALUES('last_compatible_version', '24');
                 CREATE TABLE cookies(
                     creation_utc INTEGER NOT NULL,
                     host_key TEXT NOT NULL,
                     top_frame_site_key TEXT NOT NULL,
                     name TEXT NOT NULL,
                     value TEXT NOT NULL,
                     encrypted_value BLOB NOT NULL,
                     path TEXT NOT NULL,
                     expires_utc INTEGER NOT NULL,
                     is_secure INTEGER NOT NULL,
                     is_httponly INTEGER NOT NULL,
                     last_access_utc INTEGER NOT NULL,
                     has_expires INTEGER NOT NULL,
                     is_persistent INTEGER NOT NULL,
                     priority INTEGER NOT NULL,
                     samesite INTEGER NOT NULL,
                     source_scheme INTEGER NOT NULL,
                     source_port INTEGER NOT NULL,
                     last_update_utc INTEGER NOT NULL,
                     source_type INTEGER NOT NULL,
                     has_cross_site_ancestor INTEGER NOT NULL
                 );
                 CREATE UNIQUE INDEX cookies_unique_index ON cookies(
                     host_key, top_frame_site_key, has_cross_site_ancestor,
                     name, path, source_scheme, source_port
                 );",
            )
            .unwrap();
    }

    #[test]
    fn reads_decrypted_cookie_inputs_from_supported_profile_database() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-wayfern-cookie-read-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        create_supported_database(&root);
        let key = b"profile-local-os-crypt-key";
        fs::write(root.join("os_crypt_key"), key).unwrap();
        materialize_from_inputs(
            &root,
            ProfileCookieState::Stopped,
            &[input(b"session-value")],
            1_700_000_000,
        )
        .unwrap();

        let cookies = read_inputs(&root).unwrap();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].value.as_bytes(), b"session-value");
        assert_eq!(cookies[0].domain, ".example.com");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn writes_to_existing_supported_profile_database_and_redacts_reads() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-wayfern-cookie-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        create_supported_database(&root);
        let report = materialize_encrypted(
            &root,
            ProfileCookieState::Stopped,
            &[encrypted_input(b"v10\x01opaque-ciphertext")],
            1_700_000_000,
        )
        .unwrap();
        assert_eq!(report.inserted, 1);
        let database = fs::read(database_path(&root)).unwrap();
        assert!(
            !database
                .windows(b"secret".len())
                .any(|window| window == b"secret")
        );
        {
            let connection = Connection::open(database_path(&root)).unwrap();
            let (value, encrypted, expires_utc, has_expires, is_persistent): (
                String,
                Vec<u8>,
                i64,
                i64,
                i64,
            ) = connection
                .query_row(
                    "SELECT value, encrypted_value, expires_utc, has_expires, is_persistent FROM cookies WHERE name = 'sid'",
                    [],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .unwrap();
            assert!(value.is_empty());
            assert_eq!(encrypted, b"v10\x01opaque-ciphertext");
            assert_eq!((expires_utc, has_expires, is_persistent), (0, 0, 0));
        }
        let rows = read_metadata(&root).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].domain, ".example.com");
        assert!(!format!("{rows:?}").contains("ciphertext"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn plaintext_materialization_fails_before_database_creation() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-wayfern-cookie-plaintext-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert_eq!(
            materialize(
                &root,
                ProfileCookieState::Stopped,
                &[input(b"secret")],
                1_700_000_000
            ),
            Err(WayfernCookieError::PlaintextPersistenceForbidden)
        );
        assert!(!root.exists());
    }

    #[test]
    fn encrypted_boundary_rejects_unprefixed_payloads() {
        assert_eq!(
            EncryptedCookieInput::new(
                "sid".into(),
                ".example.com".into(),
                "/".into(),
                true,
                true,
                None,
                CookieSameSite::Lax,
                b"secret",
            ),
            Err(WayfernCookieError::InvalidCiphertext)
        );
    }

    #[test]
    fn running_profile_is_rejected_before_database_creation() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-wayfern-running-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert_eq!(
            materialize(
                &root,
                ProfileCookieState::Running,
                &[input(b"secret")],
                1_700_000_000
            ),
            Err(WayfernCookieError::RunningProfile)
        );
        assert!(!root.exists());
    }

    #[test]
    fn batch_failure_rolls_back_every_cookie_row() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-wayfern-rollback-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let invalid = EncryptedCookieInput {
            expires_at: Some(-1),
            ..encrypted_input(b"v10-invalid-expiry")
        };
        create_supported_database(&root);
        assert_eq!(
            materialize_encrypted(
                &root,
                ProfileCookieState::Stopped,
                &[encrypted_input(b"v10-valid"), invalid],
                1_700_000_000
            ),
            Err(WayfernCookieError::InvalidExpiry)
        );
        assert!(read_metadata(&root).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_or_unsupported_profile_database_is_never_created_or_rewritten() {
        let missing = std::env::temp_dir().join(format!(
            "buglogin-wayfern-cookie-missing-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert_eq!(
            materialize_encrypted(
                &missing,
                ProfileCookieState::Stopped,
                &[encrypted_input(b"v10-opaque-ciphertext")],
                1_700_000_000,
            ),
            Err(WayfernCookieError::Database)
        );
        assert!(!missing.exists());

        let unsupported = std::env::temp_dir().join(format!(
            "buglogin-wayfern-cookie-unsupported-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        create_supported_database(&unsupported);
        let db_path = database_path(&unsupported);
        Connection::open(&db_path)
            .unwrap()
            .execute("UPDATE meta SET value = '22' WHERE key = 'version'", [])
            .unwrap();
        let before = fs::read(&db_path).unwrap();
        assert_eq!(
            materialize_encrypted(
                &unsupported,
                ProfileCookieState::Stopped,
                &[encrypted_input(b"v10-opaque-ciphertext")],
                1_700_000_000,
            ),
            Err(WayfernCookieError::UnsupportedSchema)
        );
        assert_eq!(fs::read(&db_path).unwrap(), before);
        fs::remove_dir_all(unsupported).unwrap();
    }

    #[test]
    fn metadata_read_never_creates_a_missing_database() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-wayfern-readonly-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert_eq!(read_metadata(&root), Err(WayfernCookieError::Database));
        assert!(!root.exists());
    }
}
