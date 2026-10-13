use super::{CookieInput, CookieSameSite};
use rusqlite::{Connection, OpenFlags, params};
use std::{
    fmt,
    path::{Path, PathBuf},
};

const WINDOWS_EPOCH_DIFF: i64 = 11_644_473_600;
const MIN_SUPPORTED_COOKIE_SCHEMA_VERSION: i64 = 23;

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
        let expires = cookie
            .expires_at
            .unwrap_or_else(|| now_unix.saturating_add(30 * 86_400));
        if expires < 0 {
            return Err(WayfernCookieError::InvalidExpiry);
        }
        let source_scheme = if cookie.secure { 2 } else { 1 };
        let source_port = if cookie.secure { 443 } else { 80 };
        let existing: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM cookies WHERE host_key = ?1 AND top_frame_site_key = '' AND has_cross_site_ancestor = 0 AND name = ?2 AND path = ?3 AND source_scheme = ?4 AND source_port = ?5)",
                params![cookie.domain, cookie.name, cookie.path, source_scheme, source_port],
                |row| row.get(0),
            )
            .map_err(|_| WayfernCookieError::Database)?;
        let expires_utc = unix_to_chrome_time(expires)?;
        let now_utc = unix_to_chrome_time(now_unix)?;
        let same_site = same_site_to_db(cookie.same_site);
        let secure = i32::from(cookie.secure);
        let http_only = i32::from(cookie.http_only);
        if existing {
            transaction.execute(
                "UPDATE cookies SET value = '', encrypted_value = ?1, expires_utc = ?2, is_secure = ?3, is_httponly = ?4, last_access_utc = ?5, has_expires = 1, is_persistent = 1, samesite = ?6, last_update_utc = ?7 WHERE host_key = ?8 AND top_frame_site_key = '' AND has_cross_site_ancestor = 0 AND name = ?9 AND path = ?10 AND source_scheme = ?11 AND source_port = ?12",
                params![cookie.ciphertext, expires_utc, secure, http_only, now_utc, same_site, now_utc, cookie.domain, cookie.name, cookie.path, source_scheme, source_port],
            ).map_err(|_| WayfernCookieError::Database)?;
            report.replaced += 1;
        } else {
            transaction.execute(
                "INSERT INTO cookies (creation_utc, host_key, top_frame_site_key, name, value, encrypted_value, path, expires_utc, is_secure, is_httponly, last_access_utc, has_expires, is_persistent, priority, samesite, source_scheme, source_port, source_type, has_cross_site_ancestor, last_update_utc) VALUES (?1, ?2, '', ?3, '', ?4, ?5, ?6, ?7, ?8, ?9, 1, 1, 1, ?10, ?11, ?12, 0, 0, ?13)",
                params![now_utc, cookie.domain, cookie.name, cookie.ciphertext, cookie.path, expires_utc, secure, http_only, now_utc, same_site, source_scheme, source_port, now_utc],
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
            let (value, encrypted): (String, Vec<u8>) = connection
                .query_row(
                    "SELECT value, encrypted_value FROM cookies WHERE name = 'sid'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert!(value.is_empty());
            assert_eq!(encrypted, b"v10\x01opaque-ciphertext");
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
