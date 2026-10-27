use std::{
    fs,
    path::{Path, PathBuf},
};

use super::{
    LaunchSpecification, RuntimeAdapter, RuntimeBinary, RuntimeCapability, ValidatedRuntimeBinary,
    invalid_runtime,
};
use buglogin_domain::{entities::Runtime, errors::Problem, ids::ProfileId};
use buglogin_storage::paths::confined_path;

const CAPABILITIES: &[RuntimeCapability] = &[
    RuntimeCapability::ProfileIsolation,
    RuntimeCapability::Proxy,
    RuntimeCapability::Cookies,
    RuntimeCapability::Extensions,
    RuntimeCapability::Fingerprint,
    RuntimeCapability::Cdp,
];
const BUGLOGIN_NEW_TAB_SHORTCUT: &str = "https://buglogin.com/";
const LEGACY_DONUT_TOP_SITES: &[&str] =
    &["https://donutbrowser.com/", "https://www.donutbrowser.com/"];

fn top_site_blacklist_key(url: &str) -> String {
    format!("{:x}", md5::compute(url.as_bytes()))
}

/// Repair the BugLogin NTP state. Wayfern 155 can crash on a generated string
/// shortcut, so remove that legacy entry and blacklist the former Donut home.
pub fn configure_new_tab_shortcuts(profile_path: &Path) -> Result<(), Problem> {
    let preferences_path = profile_path.join("Default").join("Preferences");
    let bytes = match fs::read(&preferences_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            return Err(Problem::new(
                "wayfern_profile_preferences_unavailable",
                "Wayfern profile preferences are unavailable",
                503,
                "wayfern-new-tab",
            ));
        }
    };
    let mut preferences = serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|_| {
        Problem::new(
            "wayfern_profile_preferences_invalid",
            "Wayfern profile preferences could not be read",
            409,
            "wayfern-new-tab",
        )
    })?;
    let Some(root) = preferences.as_object_mut() else {
        return Err(Problem::new(
            "wayfern_profile_preferences_invalid",
            "Wayfern profile preferences could not be read",
            409,
            "wayfern-new-tab",
        ));
    };
    let mut changed = false;
    let ntp = root
        .entry("ntp")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if let Some(ntp) = ntp.as_object_mut() {
        let blacklist = ntp
            .entry("most_visited_blacklist")
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let Some(blacklist) = blacklist.as_object_mut() {
            for url in LEGACY_DONUT_TOP_SITES {
                let key = top_site_blacklist_key(url);
                if blacklist.remove(*url).is_some() {
                    changed = true;
                }
                if !blacklist.contains_key(&key) {
                    blacklist.insert(key, serde_json::Value::Null);
                    changed = true;
                }
            }
        }
    }
    if let Some(custom_links) = root
        .get_mut("custom_links")
        .and_then(serde_json::Value::as_object_mut)
    {
        let has_buglogin_shortcut = custom_links
            .get("list")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|links| {
                links
                    .iter()
                    .any(|link| link.as_str() == Some(BUGLOGIN_NEW_TAB_SHORTCUT))
            });
        if has_buglogin_shortcut {
            if let Some(links) = custom_links
                .get_mut("list")
                .and_then(serde_json::Value::as_array_mut)
            {
                links.retain(|link| link.as_str() != Some(BUGLOGIN_NEW_TAB_SHORTCUT));
            }
            if custom_links
                .get("list")
                .and_then(serde_json::Value::as_array)
                .is_some_and(Vec::is_empty)
            {
                custom_links.remove("list");
            }
            if custom_links.get("initialized") == Some(&serde_json::Value::Bool(true)) {
                custom_links.remove("initialized");
            }
            let remove_custom_links = custom_links.is_empty();
            if remove_custom_links {
                root.remove("custom_links");
            }
            changed = true;
        }
    }
    if !changed {
        return Ok(());
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
    Ok(())
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
            chromium_compatible_user_data_dir(profile_path)
        )];
        args.extend(extra_args.iter().cloned());
        args.push("--disable-background-mode".to_owned());
        Ok(LaunchSpecification {
            runtime: Runtime::Wayfern,
            version: binary.version.clone(),
            executable: binary.path.clone(),
            profile_path: profile_path.to_path_buf(),
            args,
        })
    }
}

fn chromium_compatible_user_data_dir(profile_path: &Path) -> String {
    let path = profile_path.to_string_lossy();
    if let Some(unc_path) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{unc_path}");
    }
    if let Some(drive_path) = path.strip_prefix(r"\\?\") {
        let bytes = drive_path.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/')
        {
            return drive_path.to_owned();
        }
    }
    path.into_owned()
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
                "--disable-background-mode".to_owned(),
            ]
        );
    }

    #[test]
    fn chromium_data_directory_removes_verbatim_drive_prefix() {
        assert_eq!(
            chromium_compatible_user_data_dir(Path::new(
                r"\\?\D:\buglogin-temp\profiles\profile-one"
            )),
            r"D:\buglogin-temp\profiles\profile-one"
        );
    }

    #[test]
    fn chromium_data_directory_converts_verbatim_unc_prefix() {
        assert_eq!(
            chromium_compatible_user_data_dir(Path::new(
                r"\\?\UNC\server\share\profiles\profile-one"
            )),
            r"\\server\share\profiles\profile-one"
        );
    }

    #[test]
    fn chromium_data_directory_preserves_nonverbatim_paths() {
        assert_eq!(
            chromium_compatible_user_data_dir(Path::new(r"D:\buglogin-temp\profiles\profile-one")),
            r"D:\buglogin-temp\profiles\profile-one"
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
    fn new_tab_repair_removes_generated_shortcut_and_marker_preserving_profile_data() {
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
            br#"{"bookmark_bar":{"show_on_all_tabs":true},"ntp":{"shortcust_visible":false,"most_visited_blacklist":{"preexisting":null}},"custom_links":{"initialized":true,"list":["https://buglogin.com/","https://example.org/"]}}"#,
        )
        .unwrap();

        configure_new_tab_shortcuts(&profile_path).unwrap();

        let preferences: serde_json::Value =
            serde_json::from_slice(&fs::read(&preferences_path).unwrap()).unwrap();
        assert_eq!(preferences["bookmark_bar"]["show_on_all_tabs"], true);
        assert_eq!(
            preferences["ntp"]["shortcust_visible"], false,
            "existing NTP settings remain untouched"
        );
        assert_eq!(
            preferences["ntp"]["most_visited_blacklist"]["preexisting"],
            serde_json::Value::Null
        );
        for url in LEGACY_DONUT_TOP_SITES {
            assert_eq!(
                preferences["ntp"]["most_visited_blacklist"][top_site_blacklist_key(url)],
                serde_json::Value::Null
            );
            assert!(
                preferences["ntp"]["most_visited_blacklist"]
                    .get(*url)
                    .is_none()
            );
        }
        assert_eq!(
            preferences["custom_links"]["list"],
            serde_json::json!(["https://example.org/"])
        );
        assert!(preferences["custom_links"].get("initialized").is_none());

        fs::remove_dir_all(profile_path).unwrap();
    }

    #[test]
    fn new_tab_repair_removes_empty_generated_custom_links_object() {
        let profile_path = std::env::temp_dir().join(format!(
            "buglogin-wayfern-ntp-empty-links-{}-{}",
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
            br#"{"custom_links":{"list":["https://buglogin.com/"]},"profile":{"name":"QA"}}"#,
        )
        .unwrap();

        configure_new_tab_shortcuts(&profile_path).unwrap();

        let preferences: serde_json::Value =
            serde_json::from_slice(&fs::read(&preferences_path).unwrap()).unwrap();
        assert!(preferences.get("custom_links").is_none());
        assert_eq!(preferences["profile"]["name"], "QA");
        fs::remove_dir_all(profile_path).unwrap();
    }

    #[test]
    fn fresh_profile_new_tab_repair_does_not_create_preferences() {
        let profile_path = std::env::temp_dir().join(format!(
            "buglogin-wayfern-ntp-first-launch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&profile_path).unwrap();
        configure_new_tab_shortcuts(&profile_path).unwrap();

        assert!(!profile_path.join("Default").join("Preferences").exists());
        fs::remove_dir_all(profile_path).unwrap();
    }

    #[test]
    fn new_tab_repair_preserves_user_shortcuts_and_blacklists_legacy_top_sites() {
        let profile_path = std::env::temp_dir().join(format!(
            "buglogin-wayfern-ntp-user-links-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let default_profile = profile_path.join("Default");
        fs::create_dir_all(&default_profile).unwrap();
        let preferences_path = default_profile.join("Preferences");
        let bytes = br#"{"custom_links":{"list":["https://example.org/"]}}"#;
        fs::write(&preferences_path, bytes).unwrap();

        configure_new_tab_shortcuts(&profile_path).unwrap();

        let preferences: serde_json::Value =
            serde_json::from_slice(&fs::read(&preferences_path).unwrap()).unwrap();
        assert_eq!(
            preferences["custom_links"]["list"],
            serde_json::json!(["https://example.org/"])
        );
        for url in LEGACY_DONUT_TOP_SITES {
            assert_eq!(
                preferences["ntp"]["most_visited_blacklist"][top_site_blacklist_key(url)],
                serde_json::Value::Null
            );
        }
        fs::remove_dir_all(profile_path).unwrap();
    }

    #[test]
    fn top_site_blacklist_uses_chromium_md5_url_key() {
        assert_eq!(
            top_site_blacklist_key("https://example.com/"),
            "182ccedb33a9e03fbf1079b209da1a31"
        );
    }
}
