use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};
use serde_json::{Value, json};

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
    db_dir.join("agent_features.sqlite")
}

fn init_db() -> Result<Connection, rusqlite::Error> {
    let conn = Connection::open(get_db_path())?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS recipes (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            steps TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS schedules (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            cron TEXT NOT NULL,
            profile_ids TEXT NOT NULL,
            recipe_id TEXT,
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS cookie_bot_schedules (
            profile_id TEXT PRIMARY KEY,
            schedule_json TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS cookie_bot_templates (
            id TEXT PRIMARY KEY,
            template_json TEXT NOT NULL,
            updated_at INTEGER NOT NULL
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

fn agent_service_unavailable() -> String {
    r#"{"code":"AGENT_SERVICE_UNAVAILABLE"}"#.to_owned()
}

fn cookie_bot_worker_unavailable() -> String {
    r#"{"code":"COOKIE_BOT_WORKER_UNAVAILABLE"}"#.to_owned()
}

fn local_store_corrupt() -> String {
    r#"{"code":"LOCAL_FEATURE_STORE_CORRUPT"}"#.to_owned()
}

fn new_cookie_bot_template_id() -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| r#"{"code":"LOCAL_ID_GENERATION_FAILED"}"#.to_owned())?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "user:{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    ))
}

fn is_cookie_bot_user_template_id(id: &str) -> bool {
    id.starts_with("user:") || id.starts_with("tmpl_")
}

fn cookie_bot_template_record(template: Value, id: &str) -> Result<Value, String> {
    let mut record = template;
    let object = record
        .as_object_mut()
        .ok_or_else(|| r#"{"code":"COOKIE_BOT_TEMPLATE_INVALID"}"#.to_owned())?;
    object
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| r#"{"code":"COOKIE_BOT_TEMPLATE_INVALID"}"#.to_owned())?;
    let sites = object
        .get("sites")
        .and_then(Value::as_array)
        .filter(|sites| sites.iter().all(Value::is_string))
        .ok_or_else(|| r#"{"code":"COOKIE_BOT_TEMPLATE_INVALID"}"#.to_owned())?;
    if sites.is_empty()
        || sites
            .iter()
            .any(|site| site.as_str().map_or(true, |site| site.trim().is_empty()))
    {
        return Err(r#"{"code":"COOKIE_BOT_TEMPLATE_INVALID"}"#.to_owned());
    }
    object.insert("id".to_owned(), json!(id));
    Ok(record)
}

#[tauri::command]
pub async fn get_agent_console() -> Result<Value, String> {
    Err(agent_service_unavailable())
}

#[tauri::command]
pub async fn answer_agent_request(_request_id: u64, _answer: String) -> Result<(), String> {
    Err(agent_service_unavailable())
}

#[tauri::command]
pub async fn dismiss_agent_request(_request_id: u64) -> Result<(), String> {
    Err(agent_service_unavailable())
}

#[tauri::command]
pub async fn send_agent_note(_session_id: Option<String>, _note: String) -> Result<(), String> {
    Err(agent_service_unavailable())
}

#[tauri::command]
pub async fn take_over_profile(_profile_id: String, _note: Option<String>) -> Result<(), String> {
    Err(agent_service_unavailable())
}

#[tauri::command]
pub async fn hand_back_profile(_profile_id: String) -> Result<(), String> {
    Err(agent_service_unavailable())
}

#[tauri::command]
pub async fn set_agents_paused(paused: bool, note: Option<String>) -> Result<(), String> {
    let _ = (paused, note);
    Err(agent_service_unavailable())
}

#[tauri::command]
pub async fn clear_agent_activity() -> Result<(), String> {
    Err(agent_service_unavailable())
}

#[tauri::command]
pub async fn show_profile_window(_profile_id: String) -> Result<(), String> {
    Err(agent_service_unavailable())
}

// -------------------------------------------------------------
// Cookie Bot
// -------------------------------------------------------------

#[tauri::command]
pub async fn get_cookie_bot_schedules(_scope: Option<Value>) -> Result<Value, String> {
    Err(cookie_bot_worker_unavailable())
}

#[tauri::command]
pub async fn get_cookie_bot_schedule(profile_id: String) -> Result<Option<Value>, String> {
    let _ = profile_id;
    Err(cookie_bot_worker_unavailable())
}

#[tauri::command]
pub async fn save_cookie_bot_schedule(
    profile_id: Option<String>,
    schedule: Value,
) -> Result<Value, String> {
    let _ = (profile_id, schedule);
    Err(cookie_bot_worker_unavailable())
}

#[tauri::command]
pub async fn delete_cookie_bot_schedule(profile_id: String) -> Result<(), String> {
    let _ = profile_id;
    Err(cookie_bot_worker_unavailable())
}

#[tauri::command]
pub async fn check_cookie_bot_conflicts(_schedule: Option<Value>) -> Result<Value, String> {
    Err(cookie_bot_worker_unavailable())
}

#[tauri::command]
pub async fn get_cookie_bot_runs(_profile_id: Option<String>) -> Result<Value, String> {
    Err(cookie_bot_worker_unavailable())
}

#[tauri::command]
pub async fn run_cookie_bot_now(_profile_id: String) -> Result<Value, String> {
    Err(cookie_bot_worker_unavailable())
}

#[tauri::command]
pub async fn cancel_cookie_bot_run(
    _profile_id: String,
    _run_id: Option<String>,
) -> Result<(), String> {
    Err(cookie_bot_worker_unavailable())
}

#[tauri::command]
pub async fn get_cookie_bot_presets() -> Result<Value, String> {
    Err(cookie_bot_worker_unavailable())
}

#[tauri::command]
pub async fn get_cookie_bot_user_templates() -> Result<Value, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, template_json FROM cookie_bot_templates ORDER BY updated_at DESC, id")
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;

    let mut templates: Vec<Value> = Vec::new();
    for row in rows {
        let (id, raw) = row.map_err(|_| local_store_corrupt())?;
        let record: Value = serde_json::from_str(&raw).map_err(|_| local_store_corrupt())?;
        if record.get("id").and_then(Value::as_str) != Some(id.as_str()) {
            return Err(local_store_corrupt());
        }
        let record = cookie_bot_template_record(record, &id).map_err(|_| local_store_corrupt())?;
        templates.push(record);
    }
    Ok(json!({ "templates": templates }))
}

#[tauri::command]
pub async fn create_cookie_bot_user_template(template: Value) -> Result<Value, String> {
    let id = new_cookie_bot_template_id()?;
    let record = cookie_bot_template_record(template, &id)?;
    let raw = serde_json::to_string(&record).map_err(|_| local_store_corrupt())?;
    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO cookie_bot_templates (id, template_json, updated_at) VALUES (?1, ?2, ?3)",
        params![id, raw, now_secs()],
    )
    .map_err(|_| local_store_corrupt())?;
    Ok(record)
}

#[tauri::command]
pub async fn update_cookie_bot_user_template(
    template_id: String,
    template: Value,
) -> Result<Value, String> {
    if !is_cookie_bot_user_template_id(&template_id) {
        return Err(r#"{"code":"COOKIE_BOT_TEMPLATE_NOT_FOUND"}"#.to_owned());
    }
    let record = cookie_bot_template_record(template, &template_id)?;
    let raw = serde_json::to_string(&record).map_err(|_| local_store_corrupt())?;
    let conn = init_db().map_err(|e| e.to_string())?;
    let changed = conn
        .execute(
            "UPDATE cookie_bot_templates SET template_json = ?1, updated_at = ?2 WHERE id = ?3",
            params![raw, now_secs(), template_id],
        )
        .map_err(|e| e.to_string())?;
    if changed == 0 {
        return Err(r#"{"code":"COOKIE_BOT_TEMPLATE_NOT_FOUND"}"#.to_owned());
    }
    Ok(record)
}

#[tauri::command]
pub async fn delete_cookie_bot_user_template(template_id: String) -> Result<bool, String> {
    if !is_cookie_bot_user_template_id(&template_id) {
        return Ok(false);
    }
    let conn = init_db().map_err(|e| e.to_string())?;
    let changed = conn
        .execute(
            "DELETE FROM cookie_bot_templates WHERE id = ?1",
            params![template_id],
        )
        .map_err(|e| e.to_string())?;
    Ok(changed > 0)
}

#[tauri::command]
pub async fn get_remote_hours_quota() -> Result<Value, String> {
    Err(cookie_bot_worker_unavailable())
}

#[tauri::command]
pub async fn get_cookie_bot_usage() -> Result<Value, String> {
    Err(cookie_bot_worker_unavailable())
}

// -------------------------------------------------------------
// Recipes
// -------------------------------------------------------------

#[tauri::command]
pub async fn get_agent_recipes() -> Result<Vec<Value>, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, name, steps, created_at, updated_at FROM recipes ORDER BY created_at DESC",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut list = Vec::new();
    for row in rows {
        let (id, name, steps_raw, created_at, updated_at) =
            row.map_err(|_| local_store_corrupt())?;
        let steps: Value = serde_json::from_str(&steps_raw).map_err(|_| local_store_corrupt())?;
        list.push(json!({
            "id": id,
            "name": name,
            "steps": steps,
            "created_at": created_at,
            "updated_at": updated_at,
        }));
    }
    Ok(list)
}

#[tauri::command]
pub async fn create_agent_recipe(name: String, steps: Value) -> Result<Value, String> {
    let id = format!("rcp_{}", now_secs());
    let raw_steps = serde_json::to_string(&steps).unwrap_or_else(|_| "[]".to_string());
    let now = now_secs();

    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO recipes (id, name, steps, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, name, raw_steps, now, now],
    )
    .map_err(|e| e.to_string())?;

    Ok(json!({
        "id": id,
        "name": name,
        "steps": steps,
        "created_at": now,
        "updated_at": now,
    }))
}

#[tauri::command]
pub async fn update_agent_recipe(id: String, name: String, steps: Value) -> Result<Value, String> {
    let raw_steps = serde_json::to_string(&steps).unwrap_or_else(|_| "[]".to_string());
    let now = now_secs();

    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE recipes SET name = ?1, steps = ?2, updated_at = ?3 WHERE id = ?4",
        params![name, raw_steps, now, id],
    )
    .map_err(|e| e.to_string())?;

    Ok(json!({
        "id": id,
        "name": name,
        "steps": steps,
        "updated_at": now,
    }))
}

#[tauri::command]
pub async fn delete_agent_recipe(id: String) -> Result<(), String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM recipes WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn get_recipe_recording() -> Result<Value, String> {
    Err(agent_service_unavailable())
}

#[tauri::command]
pub async fn start_recipe_recording(profile_id: String) -> Result<(), String> {
    let _ = profile_id;
    Err(agent_service_unavailable())
}

#[tauri::command]
pub async fn stop_recipe_recording() -> Result<Value, String> {
    Err(agent_service_unavailable())
}

// -------------------------------------------------------------
// Remote Sessions
// -------------------------------------------------------------

#[tauri::command]
pub async fn list_remote_sessions() -> Result<Vec<Value>, String> {
    Err(agent_service_unavailable())
}

#[tauri::command]
pub async fn get_remote_session(session_id: String) -> Result<Option<Value>, String> {
    let _ = session_id;
    Err(agent_service_unavailable())
}

#[tauri::command]
pub async fn stop_remote_session(_session_id: String) -> Result<(), String> {
    Err(agent_service_unavailable())
}

#[tauri::command]
pub async fn start_remote_session_events() -> Result<(), String> {
    Err(agent_service_unavailable())
}

#[tauri::command]
pub async fn stop_remote_session_events() -> Result<(), String> {
    Err(agent_service_unavailable())
}

#[tauri::command]
pub async fn get_remote_session_events_status() -> Result<Value, String> {
    Err(agent_service_unavailable())
}

// -------------------------------------------------------------
// Schedules
// -------------------------------------------------------------

#[tauri::command]
pub async fn list_agent_schedules() -> Result<Vec<Value>, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, name, cron, profile_ids, recipe_id, enabled, created_at, updated_at FROM schedules ORDER BY created_at DESC")
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, i64>(7)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut list = Vec::new();
    for row in rows {
        let (id, name, cron, profile_ids, recipe_id, enabled, created_at, updated_at) =
            row.map_err(|_| local_store_corrupt())?;
        let profile_ids =
            serde_json::from_str::<Value>(&profile_ids).map_err(|_| local_store_corrupt())?;
        list.push(json!({
            "id": id,
            "name": name,
            "cron": cron,
            "profile_ids": profile_ids,
            "recipe_id": recipe_id,
            "enabled": enabled != 0,
            "created_at": created_at,
            "updated_at": updated_at,
        }));
    }
    Ok(list)
}

#[tauri::command]
pub async fn create_agent_schedule(schedule: Value) -> Result<Value, String> {
    let id = format!("sched_{}", now_secs());
    let name = schedule
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Schedule")
        .to_string();
    let cron = schedule
        .get("cron")
        .and_then(Value::as_str)
        .unwrap_or("0 0 * * *")
        .to_string();
    let p_ids = serde_json::to_string(
        &schedule
            .get("profile_ids")
            .or_else(|| schedule.get("profileIds")),
    )
    .unwrap_or_else(|_| "[]".to_string());
    let recipe_id = schedule
        .get("recipe_id")
        .or_else(|| schedule.get("recipeId"))
        .and_then(Value::as_str)
        .map(String::from);
    let enabled = schedule
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let now = now_secs();

    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO schedules (id, name, cron, profile_ids, recipe_id, enabled, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![id, name, cron, p_ids, recipe_id, if enabled { 1 } else { 0 }, now, now],
    ).map_err(|e| e.to_string())?;

    Ok(json!({
        "id": id,
        "name": name,
        "cron": cron,
        "enabled": enabled,
        "created_at": now,
        "updated_at": now,
    }))
}

#[tauri::command]
pub async fn update_agent_schedule(schedule: Value) -> Result<Value, String> {
    let id = schedule
        .get("id")
        .and_then(Value::as_str)
        .ok_or("Missing schedule id")?
        .to_string();
    let name = schedule
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Schedule")
        .to_string();
    let cron = schedule
        .get("cron")
        .and_then(Value::as_str)
        .unwrap_or("0 0 * * *")
        .to_string();
    let p_ids = serde_json::to_string(
        &schedule
            .get("profile_ids")
            .or_else(|| schedule.get("profileIds")),
    )
    .unwrap_or_else(|_| "[]".to_string());
    let recipe_id = schedule
        .get("recipe_id")
        .or_else(|| schedule.get("recipeId"))
        .and_then(Value::as_str)
        .map(String::from);
    let enabled = schedule
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let now = now_secs();

    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE schedules SET name = ?1, cron = ?2, profile_ids = ?3, recipe_id = ?4, enabled = ?5, updated_at = ?6 WHERE id = ?7",
        params![name, cron, p_ids, recipe_id, if enabled { 1 } else { 0 }, now, id],
    ).map_err(|e| e.to_string())?;

    Ok(schedule)
}

#[tauri::command]
pub async fn delete_agent_schedule(id: String) -> Result<(), String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM schedules WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn run_agent_schedule_now(id: String) -> Result<Value, String> {
    let _ = id;
    Err(agent_service_unavailable())
}

#[tauri::command]
pub async fn test_agent_schedule_notice(_id: String) -> Result<(), String> {
    Err(agent_service_unavailable())
}

#[tauri::command]
pub async fn list_agent_schedule_runs(_schedule_id: Option<String>) -> Result<Vec<Value>, String> {
    Err(agent_service_unavailable())
}

#[cfg(test)]
mod cookie_bot_template_tests {
    use super::{
        cookie_bot_template_record, is_cookie_bot_user_template_id, new_cookie_bot_template_id,
    };
    use serde_json::json;

    #[test]
    fn generated_user_template_id_matches_the_client_contract() {
        let first = new_cookie_bot_template_id().unwrap();
        let second = new_cookie_bot_template_id().unwrap();

        assert!(first.starts_with("user:"));
        assert!(second.starts_with("user:"));
        assert_ne!(first, second);
        let uuid = first.strip_prefix("user:").unwrap();
        assert_eq!(uuid.len(), 36);
        assert_eq!(uuid.as_bytes()[14], b'4');
        assert!(matches!(uuid.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
        assert!(is_cookie_bot_user_template_id(&first));
        assert!(is_cookie_bot_user_template_id("tmpl_legacy-id"));
        assert!(!is_cookie_bot_user_template_id("curated-template"));
    }

    #[test]
    fn user_template_record_requires_name_and_sites_and_overwrites_client_id() {
        let id = "user:4c5b0f2e-733e-4bcb-9a44-582fd231f0aa";
        let record = cookie_bot_template_record(
            json!({"id":"forged","name":"  My sites  ","sites":["https://example.test"]}),
            id,
        )
        .unwrap();

        assert_eq!(record["id"], id);
        assert_eq!(record["name"], "  My sites  ");
        assert_eq!(record["sites"][0], "https://example.test");
        assert!(
            cookie_bot_template_record(json!({"name":"  ","sites":["https://example.test"]}), id)
                .is_err()
        );
        assert!(cookie_bot_template_record(json!({"name":"ok","sites":[]}), id).is_err());
        assert!(cookie_bot_template_record(json!({"name":"ok","sites":[""]}), id).is_err());
    }
}
