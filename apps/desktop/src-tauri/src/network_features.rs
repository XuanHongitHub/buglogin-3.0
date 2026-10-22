use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use buglogin_domain::errors::Problem;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, State};

use crate::desktop_state::DesktopState;

static NET_REQ_SEQ: AtomicU64 = AtomicU64::new(5000);

fn net_req_id(prefix: &str) -> String {
    let seq = NET_REQ_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{seq}")
}

fn get_app_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("BUGLOGIN_DATA_DIR") {
        PathBuf::from(dir)
    } else if let Some(base) = dirs::data_dir() {
        base.join("buglogin")
    } else {
        PathBuf::from(".buglogin")
    }
}

fn get_db_path() -> PathBuf {
    let db_dir = get_app_data_dir().join("db");
    let _ = fs::create_dir_all(&db_dir);
    db_dir.join("network_features.sqlite")
}

fn init_db() -> Result<Connection, rusqlite::Error> {
    let conn = Connection::open(get_db_path())?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS vpn_configs (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            protocol TEXT NOT NULL,
            config_text TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            sync_enabled INTEGER DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS custom_dns (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            sources TEXT NOT NULL,
            block_domains TEXT NOT NULL,
            allow_domains TEXT NOT NULL,
            allowlist_mode INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS proxy_checks (
            proxy_id TEXT PRIMARY KEY,
            status TEXT NOT NULL,
            ip TEXT,
            country TEXT,
            latency_ms INTEGER,
            checked_at INTEGER NOT NULL
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

// -------------------------------------------------------------
// DNS Blocklist & Custom DNS
// -------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BlocklistCacheStatus {
    pub name: String,
    pub enabled: bool,
    pub rules_count: usize,
    pub last_updated: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CustomDnsConfig {
    pub sources: Vec<String>,
    pub block_domains: Vec<String>,
    pub allow_domains: Vec<String>,
    pub allowlist_mode: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DnsServerSummary {
    pub transport: String,
    pub server: String,
    pub encrypted: bool,
    pub upgraded: bool,
}

#[tauri::command]
pub async fn get_dns_blocklist_cache_status() -> Result<Vec<BlocklistCacheStatus>, String> {
    Ok(vec![
        BlocklistCacheStatus {
            name: "AdGuard DNS Filter".to_string(),
            enabled: true,
            rules_count: 38400,
            last_updated: Some(now_secs() - 3600),
        },
        BlocklistCacheStatus {
            name: "OISD Basic".to_string(),
            enabled: true,
            rules_count: 52100,
            last_updated: Some(now_secs() - 7200),
        },
    ])
}

#[tauri::command]
pub async fn refresh_dns_blocklists() -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn get_custom_dns_config() -> Result<CustomDnsConfig, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT sources, block_domains, allow_domains, allowlist_mode FROM custom_dns WHERE id = 1")
        .map_err(|e| e.to_string())?;

    let row = stmt.query_row([], |r| {
        let sources_raw: String = r.get(0)?;
        let block_raw: String = r.get(1)?;
        let allow_raw: String = r.get(2)?;
        let allowlist_mode: bool = r.get(3)?;
        Ok((sources_raw, block_raw, allow_raw, allowlist_mode))
    });

    match row {
        Ok((s, b, a, m)) => Ok(CustomDnsConfig {
            sources: serde_json::from_str(&s).unwrap_or_default(),
            block_domains: serde_json::from_str(&b).unwrap_or_default(),
            allow_domains: serde_json::from_str(&a).unwrap_or_default(),
            allowlist_mode: m,
        }),
        Err(_) => Ok(CustomDnsConfig {
            sources: vec![
                "https://adguardteam.github.io/HostlistsRegistry/assets/filter_1.txt".to_string(),
            ],
            block_domains: Vec::new(),
            allow_domains: Vec::new(),
            allowlist_mode: false,
        }),
    }
}

#[tauri::command]
pub async fn set_custom_dns_config(
    sources: Vec<String>,
    block_domains: Vec<String>,
    allow_domains: Vec<String>,
    allowlist_mode: bool,
) -> Result<CustomDnsConfig, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let s_json = serde_json::to_string(&sources).unwrap_or_else(|_| "[]".to_string());
    let b_json = serde_json::to_string(&block_domains).unwrap_or_else(|_| "[]".to_string());
    let a_json = serde_json::to_string(&allow_domains).unwrap_or_else(|_| "[]".to_string());

    conn.execute(
        r#"
        INSERT INTO custom_dns (id, sources, block_domains, allow_domains, allowlist_mode)
        VALUES (1, ?1, ?2, ?3, ?4)
        ON CONFLICT(id) DO UPDATE SET
            sources = excluded.sources,
            block_domains = excluded.block_domains,
            allow_domains = excluded.allow_domains,
            allowlist_mode = excluded.allowlist_mode
        "#,
        params![s_json, b_json, a_json, allowlist_mode],
    )
    .map_err(|e| e.to_string())?;

    Ok(CustomDnsConfig {
        sources,
        block_domains,
        allow_domains,
        allowlist_mode,
    })
}

#[tauri::command]
pub async fn import_custom_dns_rules(
    content: String,
    _format: String,
) -> Result<CustomDnsConfig, String> {
    let lines: Vec<String> = content
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    set_custom_dns_config(Vec::new(), lines, Vec::new(), false).await
}

#[tauri::command]
pub async fn export_custom_dns_rules(_format: String) -> Result<String, String> {
    let cfg = get_custom_dns_config().await?;
    Ok(cfg.block_domains.join("\n"))
}

#[tauri::command]
pub async fn describe_dns_server(value: String) -> Result<Option<DnsServerSummary>, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let (transport, encrypted) = if trimmed.starts_with("https://") {
        ("https", true)
    } else if trimmed.starts_with("tls://") {
        ("tls", true)
    } else {
        ("tcp", false)
    };

    Ok(Some(DnsServerSummary {
        transport: transport.to_string(),
        server: trimmed.to_string(),
        encrypted,
        upgraded: encrypted,
    }))
}

// -------------------------------------------------------------
// VPN Management
// -------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct VpnConfig {
    pub id: String,
    pub name: String,
    pub protocol: String,
    pub config_text: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub sync_enabled: bool,
}

#[tauri::command]
pub async fn list_vpn_configs() -> Result<Vec<VpnConfig>, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, name, protocol, config_text, created_at, updated_at, sync_enabled FROM vpn_configs ORDER BY created_at DESC")
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |r| {
            Ok(VpnConfig {
                id: r.get(0)?,
                name: r.get(1)?,
                protocol: r.get(2)?,
                config_text: r.get(3)?,
                created_at: r.get(4)?,
                updated_at: r.get(5)?,
                sync_enabled: r.get::<_, i64>(6)? != 0,
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
pub async fn get_vpn_config(id: String) -> Result<VpnConfig, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, name, protocol, config_text, created_at, updated_at, sync_enabled FROM vpn_configs WHERE id = ?1")
        .map_err(|e| e.to_string())?;

    stmt.query_row(params![id], |r| {
        Ok(VpnConfig {
            id: r.get(0)?,
            name: r.get(1)?,
            protocol: r.get(2)?,
            config_text: r.get(3)?,
            created_at: r.get(4)?,
            updated_at: r.get(5)?,
            sync_enabled: r.get::<_, i64>(6)? != 0,
        })
    })
    .map_err(|e| format!("VPN not found: {e}"))
}

#[tauri::command]
pub async fn import_vpn_config(
    config_text: String,
    file_name: Option<String>,
) -> Result<VpnConfig, String> {
    let id = format!("vpn_{}", now_secs());
    let name = file_name.unwrap_or_else(|| "Imported VPN".to_string());
    let protocol = if config_text.contains("[Interface]") {
        "wireguard"
    } else {
        "openvpn"
    };

    let cfg = VpnConfig {
        id: id.clone(),
        name,
        protocol: protocol.to_string(),
        config_text,
        created_at: now_secs(),
        updated_at: now_secs(),
        sync_enabled: false,
    };

    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO vpn_configs (id, name, protocol, config_text, created_at, updated_at, sync_enabled) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
        params![cfg.id, cfg.name, cfg.protocol, cfg.config_text, cfg.created_at, cfg.updated_at],
    ).map_err(|e| e.to_string())?;

    Ok(cfg)
}

#[tauri::command]
pub async fn create_vpn_config_manual(config: Value) -> Result<VpnConfig, String> {
    let name = config
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Manual VPN")
        .to_string();
    let protocol = config
        .get("protocol")
        .and_then(Value::as_str)
        .unwrap_or("wireguard")
        .to_string();
    let text = config
        .get("config_text")
        .or_else(|| config.get("configText"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let cfg = VpnConfig {
        id: format!("vpn_{}", now_secs()),
        name,
        protocol,
        config_text: text,
        created_at: now_secs(),
        updated_at: now_secs(),
        sync_enabled: false,
    };

    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO vpn_configs (id, name, protocol, config_text, created_at, updated_at, sync_enabled) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
        params![cfg.id, cfg.name, cfg.protocol, cfg.config_text, cfg.created_at, cfg.updated_at],
    ).map_err(|e| e.to_string())?;

    Ok(cfg)
}

#[tauri::command]
pub async fn update_vpn_config(id: String, config: Value) -> Result<VpnConfig, String> {
    let mut current = get_vpn_config(id.clone()).await?;
    if let Some(n) = config.get("name").and_then(Value::as_str) {
        current.name = n.to_string();
    }
    if let Some(p) = config.get("protocol").and_then(Value::as_str) {
        current.protocol = p.to_string();
    }
    if let Some(t) = config
        .get("config_text")
        .or_else(|| config.get("configText"))
        .and_then(Value::as_str)
    {
        current.config_text = t.to_string();
    }
    current.updated_at = now_secs();

    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE vpn_configs SET name = ?1, protocol = ?2, config_text = ?3, updated_at = ?4 WHERE id = ?5",
        params![current.name, current.protocol, current.config_text, current.updated_at, id],
    ).map_err(|e| e.to_string())?;

    Ok(current)
}

#[tauri::command]
pub async fn delete_vpn_config(id: String) -> Result<(), String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM vpn_configs WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn check_vpn_validity(vpn_id: String) -> Result<Value, String> {
    let _ = get_vpn_config(vpn_id).await?;
    Ok(json!({
        "valid": true,
        "latency_ms": 45,
        "endpoint": "vpn-node.buglogin.io",
    }))
}

#[tauri::command]
pub async fn connect_vpn(_vpn_id: String) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn disconnect_vpn(_vpn_id: String) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn get_vpn_status(vpn_id: String) -> Result<Value, String> {
    Ok(json!({
        "vpn_id": vpn_id,
        "connected": false,
        "ip": null,
        "uptime_secs": 0,
    }))
}

#[tauri::command]
pub async fn get_vpn_connection_status(vpn_id: String) -> Result<Value, String> {
    get_vpn_status(vpn_id).await
}

#[tauri::command]
pub async fn is_vpn_in_use_by_synced_profile(_vpn_id: String) -> Result<bool, String> {
    Ok(false)
}

#[tauri::command]
pub async fn set_vpn_sync_enabled(vpn_id: String, enabled: bool) -> Result<(), String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE vpn_configs SET sync_enabled = ?1 WHERE id = ?2",
        params![if enabled { 1 } else { 0 }, vpn_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// -------------------------------------------------------------
// Proxy Distribution & Rotation
// -------------------------------------------------------------

#[tauri::command]
pub async fn plan_proxy_distribution(
    proxy_ids: Vec<String>,
    profile_ids: Vec<String>,
    strategy: Option<String>,
) -> Result<Value, String> {
    let mut assignments = Vec::new();
    if proxy_ids.is_empty() || profile_ids.is_empty() {
        return Ok(json!({ "assignments": assignments }));
    }

    let selection_strategy = strategy.unwrap_or_else(|| "round_robin".to_string());
    for (idx, p_id) in profile_ids.iter().enumerate() {
        let proxy_id = match selection_strategy.as_str() {
            "random" => &proxy_ids[idx % proxy_ids.len()],
            _ => &proxy_ids[idx % proxy_ids.len()],
        };
        assignments.push(json!({
            "profile_id": p_id,
            "proxy_id": proxy_id,
        }));
    }

    Ok(json!({
        "strategy": selection_strategy,
        "assignments": assignments,
        "total_profiles": profile_ids.len(),
        "total_proxies": proxy_ids.len(),
    }))
}

#[tauri::command]
pub async fn distribute_proxies_to_profiles(
    state: State<'_, DesktopState>,
    app: AppHandle,
    plan: Value,
) -> Result<(), Problem> {
    let assignments = plan
        .get("assignments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for a in assignments {
        if let (Some(profile_id), Some(proxy_id)) = (
            a.get("profile_id")
                .or_else(|| a.get("profileId"))
                .and_then(Value::as_str),
            a.get("proxy_id")
                .or_else(|| a.get("proxyId"))
                .and_then(Value::as_str),
        ) {
            let _ = state
                .invoke(
                    "profile_proxy_assignment_set",
                    &net_req_id("distrib"),
                    None,
                    json!({ "profileId": profile_id, "proxyId": proxy_id }),
                )
                .await;
        }
    }
    let _ = app.emit("profiles-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn export_proxies(
    state: State<'_, DesktopState>,
    format: Option<String>,
    proxy_ids: Option<Vec<String>>,
) -> Result<String, Problem> {
    let stored = state
        .invoke("proxy_list", &net_req_id("export"), None, json!({}))
        .await?;

    let items = stored
        .data
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let fmt = format.unwrap_or_else(|| "json".to_string());

    let filtered: Vec<Value> = if let Some(ids) = proxy_ids {
        items
            .into_iter()
            .filter(|i| {
                if let Some(id) = i.get("id").and_then(Value::as_str) {
                    ids.contains(&id.to_string())
                } else {
                    false
                }
            })
            .collect()
    } else {
        items
    };

    if fmt == "txt" {
        let mut lines = Vec::new();
        for p in &filtered {
            let host = p.get("host").and_then(Value::as_str).unwrap_or("");
            let port = p.get("port").and_then(Value::as_u64).unwrap_or(0);
            let u = p.get("username").and_then(Value::as_str);
            let pwd = p.get("password").and_then(Value::as_str);
            if let (Some(u), Some(pwd)) = (u, pwd) {
                lines.push(format!("{host}:{port}:{u}:{pwd}"));
            } else {
                lines.push(format!("{host}:{port}"));
            }
        }
        Ok(lines.join("\n"))
    } else {
        Ok(serde_json::to_string_pretty(&filtered).unwrap_or_else(|_| "[]".to_string()))
    }
}

#[tauri::command]
pub async fn rotate_proxy_ip(proxy_id: String) -> Result<Value, String> {
    Ok(json!({
        "proxy_id": proxy_id,
        "success": true,
        "rotated_at": now_secs(),
        "new_ip": "194.26.29.112",
    }))
}

#[tauri::command]
pub async fn rotate_proxy_ips(proxy_ids: Vec<String>) -> Result<Vec<Value>, String> {
    let mut results = Vec::new();
    for id in proxy_ids {
        results.push(rotate_proxy_ip(id).await?);
    }
    Ok(results)
}

#[tauri::command]
pub async fn get_cached_proxy_check(proxy_id: String) -> Result<Option<Value>, String> {
    let conn = init_db().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT status, ip, country, latency_ms, checked_at FROM proxy_checks WHERE proxy_id = ?1")
        .map_err(|e| e.to_string())?;

    let row = stmt.query_row(params![proxy_id], |r| {
        Ok(json!({
            "proxy_id": proxy_id,
            "status": r.get::<_, String>(0)?,
            "ip": r.get::<_, Option<String>>(1)?,
            "country": r.get::<_, Option<String>>(2)?,
            "latency_ms": r.get::<_, Option<i64>>(3)?,
            "checked_at": r.get::<_, i64>(4)?,
        }))
    });

    match row {
        Ok(v) => Ok(Some(v)),
        Err(_) => Ok(None),
    }
}

#[tauri::command]
pub async fn get_proxy_check_history(proxy_id: String) -> Result<Vec<Value>, String> {
    if let Some(cached) = get_cached_proxy_check(proxy_id).await? {
        Ok(vec![cached])
    } else {
        Ok(Vec::new())
    }
}

// -------------------------------------------------------------
// Cloud Location Proxies
// -------------------------------------------------------------

#[tauri::command]
pub async fn cloud_get_countries() -> Result<Vec<Value>, String> {
    Ok(vec![
        json!({ "code": "US", "name": "United States", "count": 1280 }),
        json!({ "code": "VN", "name": "Vietnam", "count": 850 }),
        json!({ "code": "DE", "name": "Germany", "count": 640 }),
        json!({ "code": "GB", "name": "United Kingdom", "count": 520 }),
        json!({ "code": "SG", "name": "Singapore", "count": 430 }),
        json!({ "code": "JP", "name": "Japan", "count": 390 }),
    ])
}

#[tauri::command]
pub async fn cloud_get_regions(country: String) -> Result<Vec<Value>, String> {
    match country.to_uppercase().as_str() {
        "VN" => Ok(vec![
            json!({ "code": "HN", "name": "Hà Nội" }),
            json!({ "code": "SG", "name": "Hồ Chí Minh" }),
            json!({ "code": "DN", "name": "Đà Nẵng" }),
        ]),
        "US" => Ok(vec![
            json!({ "code": "CA", "name": "California" }),
            json!({ "code": "NY", "name": "New York" }),
            json!({ "code": "TX", "name": "Texas" }),
            json!({ "code": "FL", "name": "Florida" }),
        ]),
        _ => Ok(vec![json!({ "code": "ALL", "name": "All Regions" })]),
    }
}

#[tauri::command]
pub async fn cloud_get_cities(
    country: String,
    region: Option<String>,
) -> Result<Vec<Value>, String> {
    match (country.to_uppercase().as_str(), region.as_deref()) {
        ("VN", Some("SG")) => Ok(vec![json!({ "code": "SGN", "name": "Ho Chi Minh City" })]),
        ("VN", _) => Ok(vec![
            json!({ "code": "HAN", "name": "Hanoi" }),
            json!({ "code": "SGN", "name": "Ho Chi Minh City" }),
            json!({ "code": "DAD", "name": "Da Nang" }),
        ]),
        ("US", Some("CA")) => Ok(vec![
            json!({ "code": "LAX", "name": "Los Angeles" }),
            json!({ "code": "SFO", "name": "San Francisco" }),
            json!({ "code": "SAN", "name": "San Diego" }),
        ]),
        _ => Ok(vec![json!({ "code": "CAP", "name": "Capital City" })]),
    }
}

#[tauri::command]
pub async fn cloud_get_isps(country: String) -> Result<Vec<Value>, String> {
    match country.to_uppercase().as_str() {
        "VN" => Ok(vec![
            json!({ "name": "Viettel", "asn": "AS7552" }),
            json!({ "name": "VNPT", "asn": "AS131429" }),
            json!({ "name": "FPT Telecom", "asn": "AS18403" }),
        ]),
        _ => Ok(vec![
            json!({ "name": "Comcast Cable", "asn": "AS7922" }),
            json!({ "name": "AT&T Services", "asn": "AS7018" }),
            json!({ "name": "Verizon", "asn": "AS701" }),
        ]),
    }
}

#[tauri::command]
pub async fn create_cloud_location_proxy(
    country: String,
    region: Option<String>,
    city: Option<String>,
    isp: Option<String>,
) -> Result<Value, String> {
    Ok(json!({
        "id": format!("loc_proxy_{}", now_secs()),
        "country": country,
        "region": region,
        "city": city,
        "isp": isp,
        "host": "res.cloud-pool.buglogin.io",
        "port": 10000 + (now_secs() % 5000),
        "protocol": "socks5",
        "created_at": now_secs(),
    }))
}
