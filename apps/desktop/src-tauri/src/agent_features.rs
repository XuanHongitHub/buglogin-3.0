use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
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

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// -------------------------------------------------------------
// In-Memory Agent Console State
// -------------------------------------------------------------

static AGENT_PAUSED: Mutex<Option<Value>> = Mutex::new(None);
static RECORDING_STATE: Mutex<Option<(String, Vec<Value>)>> = Mutex::new(None);

#[tauri::command]
pub async fn get_agent_console() -> Result<Value, String> {
    let paused = AGENT_PAUSED.lock().unwrap().clone();
    Ok(json!({
        "sessions": [],
        "activity": [],
        "threads": [],
        "held_profiles": [],
        "paused": paused,
        "remote_bridge": {
            "status": "idle",
            "url": "ws://127.0.0.1:8765",
            "connected_clients": 0,
        }
    }))
}

#[tauri::command]
pub async fn answer_agent_request(_request_id: u64, _answer: String) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn dismiss_agent_request(_request_id: u64) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn send_agent_note(_session_id: Option<String>, _note: String) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn take_over_profile(_profile_id: String, _note: Option<String>) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn hand_back_profile(_profile_id: String) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn set_agents_paused(paused: bool, note: Option<String>) -> Result<(), String> {
    let mut lock = AGENT_PAUSED.lock().unwrap();
    if paused {
        *lock = Some(json!({
            "paused_at": now_millis(),
            "note": note,
        }));
    } else {
        *lock = None;
    }
    Ok(())
}

#[tauri::command]
pub async fn clear_agent_activity() -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn show_profile_window(_profile_id: String) -> Result<(), String> {
    Ok(())
}

// -------------------------------------------------------------
// Cookie Bot
// -------------------------------------------------------------

#[tauri::command]
pub async fn get_cookie_bot_schedules(_scope: Option<Value>) -> Result<Value, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT profile_id, schedule_json FROM cookie_bot_schedules")
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |r| {
            let json_str: String = r.get(1)?;
            let val: Value = serde_json::from_str(&json_str).unwrap_or_else(|_| json!({}));
            Ok(val)
        })
        .map_err(|e| e.to_string())?;

    let mut schedules = Vec::new();
    for row in rows {
        if let Ok(item) = row {
            schedules.push(item);
        }
    }
    Ok(json!({ "schedules": schedules }))
}

#[tauri::command]
pub async fn get_cookie_bot_schedule(profile_id: String) -> Result<Option<Value>, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT schedule_json FROM cookie_bot_schedules WHERE profile_id = ?1")
        .map_err(|e| e.to_string())?;

    let row = stmt.query_row(params![profile_id], |r| {
        let json_str: String = r.get(0)?;
        let val: Value = serde_json::from_str(&json_str).unwrap_or_else(|_| json!({}));
        Ok(val)
    });

    match row {
        Ok(v) => Ok(Some(v)),
        Err(_) => Ok(None),
    }
}

#[tauri::command]
pub async fn save_cookie_bot_schedule(
    profile_id: Option<String>,
    schedule: Value,
) -> Result<Value, String> {
    let p_id = profile_id
        .or_else(|| {
            schedule
                .get("profileId")
                .or_else(|| schedule.get("profile_id"))
                .and_then(Value::as_str)
                .map(String::from)
        })
        .ok_or("Missing profileId in schedule")?;

    let raw = serde_json::to_string(&schedule).unwrap_or_else(|_| "{}".to_string());
    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute(
        r#"
        INSERT INTO cookie_bot_schedules (profile_id, schedule_json, updated_at)
        VALUES (?1, ?2, ?3)
        ON CONFLICT(profile_id) DO UPDATE SET
            schedule_json = excluded.schedule_json,
            updated_at = excluded.updated_at
        "#,
        params![p_id, raw, now_secs()],
    )
    .map_err(|e| e.to_string())?;

    Ok(json!({ "saved": true, "profile_id": p_id }))
}

#[tauri::command]
pub async fn delete_cookie_bot_schedule(profile_id: String) -> Result<(), String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM cookie_bot_schedules WHERE profile_id = ?1",
        params![profile_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn check_cookie_bot_conflicts(_schedule: Option<Value>) -> Result<Value, String> {
    Ok(json!({ "conflicts": [] }))
}

#[tauri::command]
pub async fn get_cookie_bot_runs(_profile_id: Option<String>) -> Result<Value, String> {
    Ok(json!({ "runs": [] }))
}

#[tauri::command]
pub async fn run_cookie_bot_now(_profile_id: String) -> Result<Value, String> {
    Ok(json!({
        "status": "started",
        "run_id": format!("cbrun_{}", now_secs()),
    }))
}

#[tauri::command]
pub async fn cancel_cookie_bot_run(
    _profile_id: String,
    _run_id: Option<String>,
) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn get_cookie_bot_presets() -> Result<Value, String> {
    Ok(json!({
        "presets": [
            {
                "id": "ecommerce_warm",
                "name": "E-Commerce Warming (Amazon, eBay, Walmart)",
                "sites": ["amazon.com", "ebay.com", "walmart.com"],
                "duration_minutes": 15,
            },
            {
                "id": "social_warm",
                "name": "Social Media Warming (Twitter, Reddit, YouTube)",
                "sites": ["twitter.com", "reddit.com", "youtube.com"],
                "duration_minutes": 20,
            }
        ]
    }))
}

#[tauri::command]
pub async fn get_cookie_bot_user_templates() -> Result<Value, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT template_json FROM cookie_bot_templates")
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |r| {
            let json_str: String = r.get(0)?;
            let val: Value = serde_json::from_str(&json_str).unwrap_or_else(|_| json!({}));
            Ok(val)
        })
        .map_err(|e| e.to_string())?;

    let mut templates = Vec::new();
    for row in rows {
        if let Ok(item) = row {
            templates.push(item);
        }
    }
    Ok(json!({ "templates": templates }))
}

#[tauri::command]
pub async fn create_cookie_bot_user_template(template: Value) -> Result<Value, String> {
    let id = format!("tmpl_{}", now_secs());
    let mut record = template;
    if let Some(obj) = record.as_object_mut() {
        obj.insert("id".to_string(), json!(id));
    }
    let raw = serde_json::to_string(&record).unwrap_or_else(|_| "{}".to_string());
    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO cookie_bot_templates (id, template_json, updated_at) VALUES (?1, ?2, ?3)",
        params![id, raw, now_secs()],
    )
    .map_err(|e| e.to_string())?;
    Ok(record)
}

#[tauri::command]
pub async fn update_cookie_bot_user_template(
    template_id: String,
    template: Value,
) -> Result<Value, String> {
    let raw = serde_json::to_string(&template).unwrap_or_else(|_| "{}".to_string());
    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE cookie_bot_templates SET template_json = ?1, updated_at = ?2 WHERE id = ?3",
        params![raw, now_secs(), template_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(template)
}

#[tauri::command]
pub async fn delete_cookie_bot_user_template(template_id: String) -> Result<(), String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM cookie_bot_templates WHERE id = ?1",
        params![template_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn get_remote_hours_quota() -> Result<Value, String> {
    Ok(json!({
        "total_hours": 100,
        "used_hours": 0,
        "remaining_hours": 100,
    }))
}

#[tauri::command]
pub async fn get_cookie_bot_usage() -> Result<Value, String> {
    Ok(json!({
        "usage": [],
        "total_runs": 0,
    }))
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
            let id: String = r.get(0)?;
            let name: String = r.get(1)?;
            let steps_raw: String = r.get(2)?;
            let created_at: i64 = r.get(3)?;
            let updated_at: i64 = r.get(4)?;
            let steps: Value = serde_json::from_str(&steps_raw).unwrap_or_else(|_| json!([]));
            Ok(json!({
                "id": id,
                "name": name,
                "steps": steps,
                "created_at": created_at,
                "updated_at": updated_at,
            }))
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
    let lock = RECORDING_STATE.lock().unwrap();
    match &*lock {
        Some((profile_id, steps)) => Ok(json!({
            "recording": true,
            "profile_id": profile_id,
            "steps": steps,
        })),
        None => Ok(json!({
            "recording": false,
            "steps": [],
        })),
    }
}

#[tauri::command]
pub async fn start_recipe_recording(profile_id: String) -> Result<(), String> {
    let mut lock = RECORDING_STATE.lock().unwrap();
    *lock = Some((profile_id, Vec::new()));
    Ok(())
}

#[tauri::command]
pub async fn stop_recipe_recording() -> Result<Value, String> {
    let mut lock = RECORDING_STATE.lock().unwrap();
    let steps = match lock.take() {
        Some((_, steps)) => steps,
        None => Vec::new(),
    };
    Ok(json!({ "steps": steps }))
}

// -------------------------------------------------------------
// Remote Sessions
// -------------------------------------------------------------

#[tauri::command]
pub async fn list_remote_sessions() -> Result<Vec<Value>, String> {
    Ok(Vec::new())
}

#[tauri::command]
pub async fn get_remote_session(session_id: String) -> Result<Option<Value>, String> {
    let _ = session_id;
    Ok(None)
}

#[tauri::command]
pub async fn stop_remote_session(_session_id: String) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn start_remote_session_events() -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn stop_remote_session_events() -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn get_remote_session_events_status() -> Result<Value, String> {
    Ok(json!({ "active": false }))
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
            let p_ids: String = r.get(3)?;
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "name": r.get::<_, String>(1)?,
                "cron": r.get::<_, String>(2)?,
                "profile_ids": serde_json::from_str::<Value>(&p_ids).unwrap_or_else(|_| json!([])),
                "recipe_id": r.get::<_, Option<String>>(4)?,
                "enabled": r.get::<_, i64>(5)? != 0,
                "created_at": r.get::<_, i64>(6)?,
                "updated_at": r.get::<_, i64>(7)?,
            }))
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
    Ok(json!({
        "schedule_id": id,
        "status": "triggered",
        "run_id": format!("srun_{}", now_secs()),
    }))
}

#[tauri::command]
pub async fn test_agent_schedule_notice(_id: String) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn list_agent_schedule_runs(_schedule_id: Option<String>) -> Result<Vec<Value>, String> {
    Ok(Vec::new())
}
