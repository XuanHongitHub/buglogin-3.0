use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use buglogin_domain::{entities::Runtime, errors::Problem, ids::ProfileId};
use buglogin_storage::paths::confined_path;
use rusqlite::{Connection, OpenFlags, params};

use super::{
    LaunchSpecification, RuntimeAdapter, RuntimeBinary, RuntimeCapability, ValidatedRuntimeBinary,
    invalid_runtime,
};

const CAPABILITIES: &[RuntimeCapability] = &[
    RuntimeCapability::ProfileIsolation,
    RuntimeCapability::Proxy,
    RuntimeCapability::Cookies,
    RuntimeCapability::Extensions,
    RuntimeCapability::Fingerprint,
    RuntimeCapability::Cdp,
];
const DONUT_NEW_TAB_SHORTCUT: &str = "https://donutbrowser.com/";

/// Hide generated New Tab shortcuts for BugLogin-managed profiles.
/// Keep the profile history intact, but blacklist and clear the cached site
/// suggestions so the runtime cannot immediately repopulate vendor shortcuts.
pub fn hide_new_tab_shortcuts(profile_path: &Path) -> Result<(), Problem> {
    let mut suggested_urls = read_top_site_urls(profile_path)?;
    if !suggested_urls
        .iter()
        .any(|url| url == DONUT_NEW_TAB_SHORTCUT)
    {
        suggested_urls.push(DONUT_NEW_TAB_SHORTCUT.to_owned());
    }
    let default_profile = profile_path.join("Default");
    fs::create_dir_all(&default_profile).map_err(|_| {
        Problem::new(
            "wayfern_profile_preferences_unavailable",
            "Wayfern profile preferences are unavailable",
            503,
            "wayfern-new-tab",
        )
    })?;
    let preferences_path = default_profile.join("Preferences");
    let mut preferences = match fs::read(&preferences_path) {
        Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|_| {
            Problem::new(
                "wayfern_profile_preferences_invalid",
                "Wayfern profile preferences could not be read",
                409,
                "wayfern-new-tab",
            )
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(_) => {
            return Err(Problem::new(
                "wayfern_profile_preferences_unavailable",
                "Wayfern profile preferences are unavailable",
                503,
                "wayfern-new-tab",
            ));
        }
    };
    let Some(root) = preferences.as_object_mut() else {
        return Err(Problem::new(
            "wayfern_profile_preferences_invalid",
            "Wayfern profile preferences could not be read",
            409,
            "wayfern-new-tab",
        ));
    };
    let ntp = root.entry("ntp").or_insert_with(|| serde_json::json!({}));
    if !ntp.is_object() {
        *ntp = serde_json::json!({});
    }
    let Some(ntp) = ntp.as_object_mut() else {
        return Err(Problem::new(
            "wayfern_profile_preferences_invalid",
            "Wayfern profile preferences could not be saved",
            500,
            "wayfern-new-tab",
        ));
    };
    // Chromium's registered preference uses this historical spelling.
    ntp.insert(
        "shortcust_visible".to_owned(),
        serde_json::Value::Bool(false),
    );
    ntp.insert(
        "custom_links_visible".to_owned(),
        serde_json::Value::Bool(false),
    );
    ntp.insert(
        "enterprise_shortcuts_visible".to_owned(),
        serde_json::Value::Bool(false),
    );
    ntp.insert(
        "personal_shortcuts_visible".to_owned(),
        serde_json::Value::Bool(false),
    );
    let blacklist = ntp
        .entry("most_visited_blacklist")
        .or_insert_with(|| serde_json::json!({}));
    if !blacklist.is_object() {
        *blacklist = serde_json::json!({});
    }
    let Some(blacklist) = blacklist.as_object_mut() else {
        return Err(Problem::new(
            "wayfern_profile_preferences_invalid",
            "Wayfern New Tab suggestions could not be disabled",
            500,
            "wayfern-new-tab",
        ));
    };
    for url in suggested_urls {
        blacklist.insert(
            format!("{:x}", md5::compute(url.as_bytes())),
            serde_json::Value::Null,
        );
    }
    let encoded = serde_json::to_vec(&preferences).map_err(|_| {
        Problem::new(
            "wayfern_profile_preferences_invalid",
            "Wayfern profile preferences could not be saved",
            500,
            "wayfern-new-tab",
        )
    })?;
    persist_preferences_atomically(&preferences_path, &encoded).map_err(|_| {
        Problem::new(
            "wayfern_profile_preferences_unavailable",
            "Wayfern profile preferences could not be saved",
            503,
            "wayfern-new-tab",
        )
    })?;

    clear_top_site_suggestions(profile_path)
}

fn read_top_site_urls(profile_path: &Path) -> Result<Vec<String>, Problem> {
    let database_path = profile_path.join("Default").join("Top Sites");
    let metadata = match fs::symlink_metadata(&database_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(top_sites_unavailable()),
    };
    if !metadata.file_type().is_file() {
        return Err(top_sites_unavailable());
    }

    let connection = Connection::open_with_flags(
        &database_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| top_sites_unavailable())?;
    connection
        .busy_timeout(Duration::from_secs(2))
        .map_err(|_| top_sites_unavailable())?;

    let has_top_sites_table: bool = connection
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'top_sites')",
            [],
            |row| row.get(0),
        )
        .map_err(|_| top_sites_unavailable())?;
    if !has_top_sites_table {
        return Ok(Vec::new());
    }

    let mut statement = connection
        .prepare("SELECT url FROM top_sites")
        .map_err(|_| top_sites_unavailable())?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|_| top_sites_unavailable())?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| top_sites_unavailable())
}

fn clear_top_site_suggestions(profile_path: &Path) -> Result<(), Problem> {
    let database_path = profile_path.join("Default").join("Top Sites");
    if !database_path.exists() {
        return Ok(());
    }
    let connection = Connection::open_with_flags(
        &database_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| top_sites_unavailable())?;
    connection
        .busy_timeout(Duration::from_secs(2))
        .map_err(|_| top_sites_unavailable())?;
    let has_top_sites_table: bool = connection
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'top_sites')",
            [],
            |row| row.get(0),
        )
        .map_err(|_| top_sites_unavailable())?;
    if has_top_sites_table {
        connection
            .execute("DELETE FROM top_sites", params![])
            .map_err(|_| top_sites_unavailable())?;
    }
    Ok(())
}

fn top_sites_unavailable() -> Problem {
    Problem::new(
        "wayfern_top_sites_unavailable",
        "Wayfern New Tab shortcuts could not be updated",
        503,
        "wayfern-new-tab",
    )
}

fn persist_preferences_atomically(path: &Path, encoded: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let temporary = path.with_file_name("Preferences.buglogin-tmp");
    let backup = path.with_file_name("Preferences.buglogin-backup");
    if !path.exists() && backup.exists() {
        fs::rename(&backup, path)?;
    } else if backup.exists() {
        fs::remove_file(&backup)?;
    }
    if temporary.exists() {
        fs::remove_file(&temporary)?;
    }
    let mut file = fs::File::create(&temporary)?;
    file.write_all(encoded)?;
    file.sync_all()?;
    if path.exists() {
        fs::rename(path, &backup)?;
    }
    if let Err(error) = fs::rename(&temporary, path) {
        if backup.exists() {
            let _ = fs::rename(&backup, path);
        }
        return Err(error);
    }
    if backup.exists() {
        let _ = fs::remove_file(backup);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WayfernAdapter;

impl RuntimeAdapter for WayfernAdapter {
    fn runtime(&self) -> Runtime {
        Runtime::Wayfern
    }

    fn capabilities(&self) -> &'static [RuntimeCapability] {
        CAPABILITIES
    }

    fn validate_binary(&self, binary: &RuntimeBinary) -> Result<ValidatedRuntimeBinary, Problem> {
        if binary.version.trim().is_empty() {
            return Err(invalid_runtime(
                "runtime_version_required",
                "Runtime version required",
            ));
        }
        if !is_sha256(&binary.expected_checksum) {
            return Err(invalid_runtime(
                "runtime_checksum_invalid",
                "Runtime checksum invalid",
            ));
        }
        let metadata = fs::symlink_metadata(&binary.path)
            .map_err(|_| invalid_runtime("runtime_binary_not_found", "Runtime binary not found"))?;
        if !metadata.file_type().is_file() {
            return Err(invalid_runtime(
                "runtime_binary_invalid",
                "Runtime binary is not a file",
            ));
        }
        let file = fs::File::open(&binary.path).map_err(|_| {
            invalid_runtime("runtime_binary_unreadable", "Runtime binary is unreadable")
        })?;
        let (actual_checksum, actual_size) = buglogin_storage::blob::checksum_reader(file)
            .map_err(|_| {
                invalid_runtime("runtime_binary_unreadable", "Runtime binary is unreadable")
            })?;
        if actual_size != metadata.len() {
            return Err(invalid_runtime(
                "runtime_size_mismatch",
                "Runtime size changed during validation",
            ));
        }
        if actual_checksum != binary.expected_checksum.to_ascii_lowercase() {
            return Err(invalid_runtime(
                "runtime_checksum_mismatch",
                "Runtime checksum mismatch",
            ));
        }
        Ok(ValidatedRuntimeBinary {
            path: binary.path.clone(),
            version: binary.version.clone(),
            checksum: actual_checksum,
            size_bytes: metadata.len(),
        })
    }

    fn profile_path(
        &self,
        profiles_root: &Path,
        profile_id: &ProfileId,
    ) -> Result<PathBuf, Problem> {
        confined_path(profiles_root, Path::new(profile_id.as_str()))
    }

    fn launch_specification(
        &self,
        binary: &ValidatedRuntimeBinary,
        profile_path: &Path,
        extra_args: &[String],
    ) -> Result<LaunchSpecification, Problem> {
        if extra_args
            .iter()
            .any(|argument| is_user_data_dir_override(argument))
        {
            return Err(invalid_runtime(
                "runtime_profile_path_override_forbidden",
                "Runtime arguments cannot override the isolated profile data directory",
            ));
        }
        if !profile_path.is_absolute() {
            return Err(invalid_runtime(
                "runtime_profile_path_invalid",
                "Runtime profile path must be absolute",
            ));
        }
        let mut args = vec![format!(
            "--user-data-dir={}",
            profile_path.to_string_lossy()
        )];
        args.extend(extra_args.iter().cloned());
        Ok(LaunchSpecification {
            runtime: Runtime::Wayfern,
            version: binary.version.clone(),
            executable: binary.path.clone(),
            profile_path: profile_path.to_path_buf(),
            args,
        })
    }
}

fn is_user_data_dir_override(argument: &str) -> bool {
    argument
        .split_once('=')
        .map_or(argument, |(name, _)| name)
        .eq_ignore_ascii_case("--user-data-dir")
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binary() -> ValidatedRuntimeBinary {
        ValidatedRuntimeBinary {
            path: PathBuf::from(r"C:\BugLogin\runtimes\wayfern\chrome.exe"),
            version: "154.0.8037.57".into(),
            checksum: "a".repeat(64),
            size_bytes: 1,
        }
    }

    #[test]
    fn launch_specification_uses_the_profile_specific_data_directory() {
        let profile_path = Path::new(r"C:\BugLogin\profiles\profile-one");
        let specification = WayfernAdapter
            .launch_specification(&binary(), profile_path, &["--disable-sync".into()])
            .unwrap();

        assert_eq!(
            specification.args,
            vec![
                format!("--user-data-dir={}", profile_path.to_string_lossy()),
                "--disable-sync".to_owned(),
            ]
        );
    }

    #[test]
    fn launch_specification_rejects_runtime_data_directory_overrides() {
        let error = WayfernAdapter
            .launch_specification(
                &binary(),
                Path::new(r"C:\BugLogin\profiles\profile-one"),
                &["--user-data-dir=C:\\Users\\Acer\\AppData\\Local\\Wayfern".into()],
            )
            .unwrap_err();

        assert_eq!(error.code, "runtime_profile_path_override_forbidden");
    }

    #[test]
    fn hide_new_tab_shortcuts_preserves_profile_preferences_and_disables_each_shortcut_source() {
        let profile_path = std::env::temp_dir().join(format!(
            "buglogin-wayfern-ntp-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let default_profile = profile_path.join("Default");
        fs::create_dir_all(&default_profile).unwrap();
        let preferences_path = default_profile.join("Preferences");
        fs::write(
            &preferences_path,
            br#"{"bookmark_bar":{"show_on_all_tabs":true},"ntp":{"shortcust_visible":true,"most_visited_blacklist":{"preexisting":null}}}"#,
        )
        .unwrap();
        let top_sites_path = default_profile.join("Top Sites");
        let connection = Connection::open(&top_sites_path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE top_sites(url TEXT PRIMARY KEY, url_rank INTEGER NOT NULL, title TEXT NOT NULL);",
            )
            .unwrap();
        let suggested_urls = [
            "https://donutbrowser.com/",
            "https://chromewebstore.google.com/",
            "https://example.test/",
        ];
        for (rank, url) in suggested_urls.iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO top_sites(url, url_rank, title) VALUES (?1, ?2, ?3)",
                    params![url, rank as i64, url],
                )
                .unwrap();
        }
        drop(connection);

        hide_new_tab_shortcuts(&profile_path).unwrap();

        let preferences: serde_json::Value =
            serde_json::from_slice(&fs::read(&preferences_path).unwrap()).unwrap();
        assert_eq!(preferences["bookmark_bar"]["show_on_all_tabs"], true);
        for key in [
            "shortcust_visible",
            "custom_links_visible",
            "enterprise_shortcuts_visible",
            "personal_shortcuts_visible",
        ] {
            assert_eq!(preferences["ntp"][key], false, "{key}");
        }
        for url in suggested_urls {
            let hash = format!("{:x}", md5::compute(url.as_bytes()));
            assert_eq!(
                preferences["ntp"]["most_visited_blacklist"][hash],
                serde_json::Value::Null
            );
        }
        assert_eq!(
            preferences["ntp"]["most_visited_blacklist"]["preexisting"],
            serde_json::Value::Null
        );

        let connection = Connection::open(&top_sites_path).unwrap();
        let remaining_suggestions: i64 = connection
            .query_row("SELECT COUNT(*) FROM top_sites", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining_suggestions, 0);
        drop(connection);

        fs::remove_dir_all(profile_path).unwrap();
    }

    #[test]
    fn first_launch_blacklists_donut_shortcut_before_top_sites_database_exists() {
        let profile_path = std::env::temp_dir().join(format!(
            "buglogin-wayfern-ntp-first-launch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let default_profile = profile_path.join("Default");
        fs::create_dir_all(&default_profile).unwrap();
        let preferences_path = default_profile.join("Preferences");

        hide_new_tab_shortcuts(&profile_path).unwrap();

        let preferences: serde_json::Value =
            serde_json::from_slice(&fs::read(&preferences_path).unwrap()).unwrap();
        let donut_hash = format!("{:x}", md5::compute(DONUT_NEW_TAB_SHORTCUT.as_bytes()));
        assert_eq!(
            preferences["ntp"]["most_visited_blacklist"][donut_hash],
            serde_json::Value::Null
        );
        fs::remove_dir_all(profile_path).unwrap();
    }
}
