use std::{collections::BTreeMap, fs, path::PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::app_instance::AppInstance;

const SCHEMA_VERSION: u8 = 1;
const FILE_NAME: &str = "desktop-preferences.json";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TableSortingSettings {
    pub column: String,
    pub direction: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct AppSettings {
    pub set_as_default_browser: bool,
    pub theme: String,
    pub custom_theme: Option<std::collections::BTreeMap<String, String>>,
    pub api_enabled: bool,
    pub api_port: u16,
    pub language: Option<String>,
    pub disable_auto_updates: bool,
    pub keep_decrypted_profiles_in_ram: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            set_as_default_browser: false,
            theme: "system".to_owned(),
            custom_theme: None,
            api_enabled: false,
            api_port: 10108,
            language: None,
            disable_auto_updates: false,
            keep_decrypted_profiles_in_ram: false,
        }
    }
}

impl Default for TableSortingSettings {
    fn default() -> Self {
        Self {
            column: "name".to_owned(),
            direction: "asc".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreferencesDocument {
    schema_version: u8,
    onboarding_completed: bool,
    #[serde(default)]
    window_resize_warning_dismissed: bool,
    table_sorting: TableSortingSettings,
    #[serde(default)]
    table_preferences: BTreeMap<String, TablePreferences>,
    #[serde(default)]
    app_settings: AppSettings,
}

impl Default for PreferencesDocument {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            onboarding_completed: false,
            window_resize_warning_dismissed: false,
            table_sorting: TableSortingSettings::default(),
            table_preferences: BTreeMap::new(),
            app_settings: AppSettings::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TablePreferences {
    pub visibility: BTreeMap<String, bool>,
    pub sizing: BTreeMap<String, f64>,
    pub sorting: Vec<TableSortRule>,
    pub density: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TableSortRule {
    pub id: String,
    pub desc: bool,
}

impl Default for TablePreferences {
    fn default() -> Self {
        Self {
            visibility: BTreeMap::new(),
            sizing: BTreeMap::new(),
            sorting: vec![TableSortRule {
                id: "name".to_owned(),
                desc: false,
            }],
            density: "compact".to_owned(),
        }
    }
}

fn validate_table_id(table_id: &str) -> Result<(), String> {
    if matches!(
        table_id,
        "profiles" | "proxies" | "vpns" | "extensions" | "extensionGroups" | "groups"
    ) {
        Ok(())
    } else {
        Err("table_preferences_id_invalid".to_owned())
    }
}

fn validate_table_preferences(preferences: &TablePreferences) -> Result<(), String> {
    if preferences.visibility.len() > 128
        || preferences.sizing.len() > 128
        || preferences.sorting.len() > 8
        || !matches!(preferences.density.as_str(), "compact" | "comfortable")
        || preferences
            .visibility
            .keys()
            .chain(preferences.sizing.keys())
            .any(|key| key.is_empty() || key.len() > 128)
        || preferences
            .sizing
            .values()
            .any(|width| !width.is_finite() || !(24.0..=2000.0).contains(width))
        || preferences
            .sorting
            .iter()
            .any(|sort| sort.id.is_empty() || sort.id.len() > 128)
    {
        return Err("table_preferences_invalid".to_owned());
    }
    Ok(())
}

fn preferences_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| AppInstance::app_data_dir(directory).join(FILE_NAME))
        .map_err(|error| format!("preferences_path_unavailable: {error}"))
}

fn read_document(path: &PathBuf) -> Result<PreferencesDocument, String> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PreferencesDocument::default());
        }
        Err(error) => return Err(format!("preferences_read_failed: {error}")),
    };

    let document: PreferencesDocument =
        serde_json::from_str(&contents).map_err(|error| format!("preferences_invalid: {error}"))?;
    if document.schema_version != SCHEMA_VERSION {
        return Err("preferences_schema_unsupported".to_owned());
    }
    Ok(document)
}

fn write_document(path: &PathBuf, document: &PreferencesDocument) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "preferences_parent_missing".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| format!("preferences_directory_failed: {error}"))?;
    let contents = serde_json::to_vec(document)
        .map_err(|error| format!("preferences_serialize_failed: {error}"))?;
    fs::write(path, contents).map_err(|error| format!("preferences_write_failed: {error}"))
}

#[tauri::command]
pub fn get_table_sorting_settings(app: AppHandle) -> Result<TableSortingSettings, String> {
    Ok(read_document(&preferences_path(&app)?)?.table_sorting)
}

#[tauri::command]
pub fn save_table_sorting_settings(
    app: AppHandle,
    sorting: TableSortingSettings,
) -> Result<(), String> {
    if sorting.column.trim().is_empty() || sorting.column.len() > 128 {
        return Err("invalid_sort_column".to_owned());
    }
    if sorting.direction != "asc" && sorting.direction != "desc" {
        return Err("invalid_sort_direction".to_owned());
    }

    let path = preferences_path(&app)?;
    let mut document = read_document(&path)?;
    document.table_sorting = sorting;
    write_document(&path, &document)
}

#[tauri::command]
pub fn get_table_preferences(app: AppHandle, table_id: String) -> Result<TablePreferences, String> {
    validate_table_id(&table_id)?;
    let mut document = read_document(&preferences_path(&app)?)?;
    Ok(document
        .table_preferences
        .remove(&table_id)
        .unwrap_or_default())
}

#[tauri::command]
pub fn save_table_preferences(
    app: AppHandle,
    table_id: String,
    preferences: TablePreferences,
) -> Result<(), String> {
    validate_table_id(&table_id)?;
    validate_table_preferences(&preferences)?;
    let path = preferences_path(&app)?;
    let mut document = read_document(&path)?;
    document.table_preferences.insert(table_id, preferences);
    write_document(&path, &document)
}

#[tauri::command]
pub fn get_onboarding_completed(app: AppHandle) -> Result<bool, String> {
    Ok(read_document(&preferences_path(&app)?)?.onboarding_completed)
}

#[tauri::command]
pub fn complete_onboarding(app: AppHandle) -> Result<(), String> {
    let path = preferences_path(&app)?;
    let mut document = read_document(&path)?;
    document.onboarding_completed = true;
    write_document(&path, &document)
}

#[tauri::command]
pub fn get_window_resize_warning_dismissed(app: AppHandle) -> Result<bool, String> {
    Ok(read_document(&preferences_path(&app)?)?.window_resize_warning_dismissed)
}

#[tauri::command]
pub fn dismiss_window_resize_warning(app: AppHandle) -> Result<(), String> {
    let path = preferences_path(&app)?;
    let mut document = read_document(&path)?;
    document.window_resize_warning_dismissed = true;
    write_document(&path, &document)
}

#[tauri::command]
pub fn get_app_settings(app: AppHandle) -> Result<AppSettings, String> {
    Ok(read_document(&preferences_path(&app)?)?.app_settings)
}

#[tauri::command]
pub fn save_app_settings(app: AppHandle, settings: Value) -> Result<AppSettings, String> {
    if !settings.is_object() || settings.to_string().len() > 64 * 1024 {
        return Err("app_settings_invalid".to_owned());
    }
    let settings: AppSettings =
        serde_json::from_value(settings).map_err(|_| "app_settings_invalid".to_owned())?;
    if !matches!(
        settings.theme.as_str(),
        "system" | "light" | "dark" | "custom"
    ) {
        return Err("app_settings_theme_invalid".to_owned());
    }
    if !(1..=65535).contains(&settings.api_port) {
        return Err("app_settings_port_invalid".to_owned());
    }
    if settings.language.as_ref().is_some_and(|language| {
        language.is_empty()
            || language.len() > 16
            || !language
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    }) {
        return Err("app_settings_language_invalid".to_owned());
    }
    if settings.custom_theme.as_ref().is_some_and(|colors| {
        colors.len() > 64
            || colors.iter().any(|(key, value)| {
                key.len() > 64
                    || value.len() > 128
                    || value.contains(';')
                    || value.contains('{')
                    || value.contains('}')
                    || value.to_ascii_lowercase().contains("url(")
            })
    }) {
        return Err("app_settings_theme_invalid".to_owned());
    }

    let path = preferences_path(&app)?;
    let mut document = read_document(&path)?;
    document.app_settings = settings.clone();
    write_document(&path, &document)?;
    Ok(settings)
}

#[tauri::command]
pub fn get_system_language() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok())
        .map(|value| {
            value
                .split(['.', '_', ':'])
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .find(|value| !value.is_empty())
        .unwrap_or_else(|| "en".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_preferences_are_bounded_and_whitelisted() {
        let preferences = TablePreferences::default();
        assert!(validate_table_id("profiles").is_ok());
        assert!(validate_table_id("unknown").is_err());
        assert!(validate_table_preferences(&preferences).is_ok());

        let invalid_width = TablePreferences {
            sizing: [("name".to_owned(), 20.0)].into(),
            ..TablePreferences::default()
        };
        assert_eq!(
            validate_table_preferences(&invalid_width).unwrap_err(),
            "table_preferences_invalid"
        );
    }

    #[test]
    fn existing_preferences_documents_default_new_table_settings() {
        let document: PreferencesDocument = serde_json::from_value(serde_json::json!({
            "schemaVersion": SCHEMA_VERSION,
            "onboardingCompleted": true,
            "tableSorting": {"column": "name", "direction": "asc"}
        }))
        .unwrap();
        assert!(document.table_preferences.is_empty());
        assert!(document.onboarding_completed);
        assert!(!document.window_resize_warning_dismissed);
    }

    #[test]
    fn resize_warning_dismissal_round_trips_in_preferences_document() {
        let directory = std::env::temp_dir().join(format!(
            "buglogin-preferences-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(FILE_NAME);
        let mut document = read_document(&path).unwrap();
        assert!(!document.window_resize_warning_dismissed);
        document.window_resize_warning_dismissed = true;
        write_document(&path, &document).unwrap();
        assert!(
            read_document(&path)
                .unwrap()
                .window_resize_warning_dismissed
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
