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
// The bundled runtime has shipped its vendor shortcut with and without the
// trailing slash. Keep both forms blacklisted locally and seed BugLogin as a
// custom NTP shortcut so first-launch profiles never expose the vendor tile.
const UPSTREAM_SHORTCUT_HOST: &str = "donutbrowser.com";
const BUGLOGIN_NEW_TAB_SHORTCUT: &str = "https://buglogin.com/";
const UPSTREAM_NEW_TAB_SHORTCUTS: &[&str] =
    &["https://donutbrowser.com", "https://donutbrowser.com/"];

/// Replace the runtime vendor tile with BugLogin without discarding unrelated
/// profile shortcuts or browsing history.
pub fn configure_new_tab_shortcuts(profile_path: &Path) -> Result<(), Problem> {
    let mut vendor_urls = read_top_site_urls(profile_path)?
        .into_iter()
        .filter(|url| is_upstream_shortcut(url))
        .collect::<Vec<_>>();
    for url in UPSTREAM_NEW_TAB_SHORTCUTS {
        if !vendor_urls.iter().any(|suggested| suggested == url) {
            vendor_urls.push((*url).to_owned());
        }
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
        serde_json::Value::Bool(true),
    );
    ntp.insert(
        "custom_links_visible".to_owned(),
        serde_json::Value::Bool(true),
    );
    ntp.insert(
        "enterprise_shortcuts_visible".to_owned(),
        serde_json::Value::Bool(false),
    );
    ntp.insert(
        "personal_shortcuts_visible".to_owned(),
        serde_json::Value::Bool(true),
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
    for url in vendor_urls {
        blacklist.insert(
            format!("{:x}", md5::compute(url.as_bytes())),
            serde_json::Value::Null,
        );
    }

    seed_buglogin_custom_shortcut(root)?;
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

    replace_vendor_top_site(profile_path)
}

fn seed_buglogin_custom_shortcut(
    root: &mut serde_json::Map<String, serde_json::Value>,
) -> Result<(), Problem> {
    let custom_links = root
        .entry("custom_links")
        .or_insert_with(|| serde_json::json!({}));
    if !custom_links.is_object() {
        *custom_links = serde_json::json!({});
    }
    let Some(custom_links) = custom_links.as_object_mut() else {
        return Err(Problem::new(
            "wayfern_profile_preferences_invalid",
            "Wayfern custom shortcuts could not be saved",
            500,
            "wayfern-new-tab",
        ));
    };
    let existing = custom_links
        .get("list")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut links = Vec::with_capacity(existing.len() + 1);
    let mut has_buglogin = false;
    for value in existing {
        let Some(url) = value.as_str() else {
            continue;
        };
        if is_upstream_shortcut(url) {
            if !has_buglogin {
                links.push(serde_json::Value::String(
                    BUGLOGIN_NEW_TAB_SHORTCUT.to_owned(),
                ));
                has_buglogin = true;
            }
        } else {
            if url == BUGLOGIN_NEW_TAB_SHORTCUT {
                has_buglogin = true;
            }
            links.push(value);
        }
    }
    if !has_buglogin {
        links.push(serde_json::Value::String(
            BUGLOGIN_NEW_TAB_SHORTCUT.to_owned(),
        ));
    }
    custom_links.insert("list".to_owned(), serde_json::Value::Array(links));
    custom_links.insert("initialized".to_owned(), serde_json::Value::Bool(true));
    Ok(())
}

fn is_upstream_shortcut(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|host| host == UPSTREAM_SHORTCUT_HOST || host.ends_with(".donutbrowser.com"))
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

fn replace_vendor_top_site(profile_path: &Path) -> Result<(), Problem> {
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
        let urls = {
            let mut statement = connection
                .prepare("SELECT url FROM top_sites")
                .map_err(|_| top_sites_unavailable())?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|_| top_sites_unavailable())?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|_| top_sites_unavailable())?
        };
        for url in urls.iter().filter(|url| is_upstream_shortcut(url)) {
            connection
                .execute("DELETE FROM top_sites WHERE url = ?1", params![url])
                .map_err(|_| top_sites_unavailable())?;
        }
        connection
            .execute(
                "UPDATE top_sites SET url_rank = url_rank + 1 WHERE url != ?1",
                params![BUGLOGIN_NEW_TAB_SHORTCUT],
            )
            .map_err(|_| top_sites_unavailable())?;
        connection
            .execute(
                "INSERT OR REPLACE INTO top_sites(url, url_rank, title) VALUES (?1, 0, 'BugLogin')",
                params![BUGLOGIN_NEW_TAB_SHORTCUT],
            )
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
        let profile_path = &std::env::temp_dir().join("BugLogin/profiles/profile-one");
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
    fn new_tab_replaces_vendor_shortcut_with_buglogin_and_preserves_other_profile_data() {
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
            br#"{"bookmark_bar":{"show_on_all_tabs":true},"ntp":{"shortcust_visible":false,"most_visited_blacklist":{"preexisting":null}},"custom_links":{"initialized":true,"list":["https://donutbrowser.com","https://example.org/"]}}"#,
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

        configure_new_tab_shortcuts(&profile_path).unwrap();

        let preferences: serde_json::Value =
            serde_json::from_slice(&fs::read(&preferences_path).unwrap()).unwrap();
        assert_eq!(preferences["bookmark_bar"]["show_on_all_tabs"], true);
        for key in [
            "shortcust_visible",
            "custom_links_visible",
            "personal_shortcuts_visible",
        ] {
            assert_eq!(preferences["ntp"][key], true, "{key}");
        }
        assert_eq!(preferences["ntp"]["enterprise_shortcuts_visible"], false);
        for url in ["https://donutbrowser.com/", "https://donutbrowser.com"] {
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
        assert_eq!(preferences["custom_links"]["initialized"], true);
        assert_eq!(
            preferences["custom_links"]["list"],
            serde_json::json!(["https://buglogin.com/", "https://example.org/"])
        );

        let connection = Connection::open(&top_sites_path).unwrap();
        let top_site_urls: Vec<String> = connection
            .prepare("SELECT url FROM top_sites ORDER BY url_rank")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            top_site_urls,
            vec![
                "https://buglogin.com/",
                "https://chromewebstore.google.com/",
                "https://example.test/"
            ]
        );
        drop(connection);

        fs::remove_dir_all(profile_path).unwrap();
    }

    #[test]
    fn first_launch_seeds_buglogin_and_blacklists_upstream_url_variants_before_top_sites_exists() {
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

        configure_new_tab_shortcuts(&profile_path).unwrap();

        let preferences: serde_json::Value =
            serde_json::from_slice(&fs::read(&preferences_path).unwrap()).unwrap();
        for url in UPSTREAM_NEW_TAB_SHORTCUTS {
            let hash = format!("{:x}", md5::compute(url.as_bytes()));
            assert_eq!(
                preferences["ntp"]["most_visited_blacklist"][hash],
                serde_json::Value::Null,
                "fresh profiles must blacklist {url}"
            );
        }
        assert_eq!(
            preferences["ntp"]["custom_links_visible"], true,
            "new tabs must show custom BugLogin shortcut"
        );
        assert_eq!(
            preferences["custom_links"]["list"],
            serde_json::json!([BUGLOGIN_NEW_TAB_SHORTCUT])
        );
        fs::remove_dir_all(profile_path).unwrap();
    }
}
