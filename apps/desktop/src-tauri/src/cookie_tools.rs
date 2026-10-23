use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, State};

use crate::desktop_state::DesktopState;
use crate::profile_cookie_manager::{
    UnifiedCookieItem, export_profile_cookies, import_profile_cookies,
};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CookieReadResult {
    pub profile_id: String,
    pub browser_type: String,
    pub domains: Vec<DomainCookies>,
    pub total_count: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DomainCookies {
    pub domain: String,
    pub cookies: Vec<UnifiedCookieItem>,
    pub cookie_count: usize,
}

fn group_cookie_values(cookies: &[Value]) -> Vec<DomainCookies> {
    let mut grouped = std::collections::BTreeMap::<String, Vec<UnifiedCookieItem>>::new();
    for cookie in cookies {
        let domain = cookie
            .get("domain")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let same_site = match cookie.get("sameSite") {
            Some(Value::Number(value)) => value.as_i64().unwrap_or_default() as i32,
            Some(Value::String(value)) => match value.as_str() {
                "lax" => 1,
                "strict" => 2,
                _ => 0,
            },
            _ => 0,
        };
        grouped
            .entry(domain.clone())
            .or_default()
            .push(UnifiedCookieItem {
                name: cookie
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                value: cookie
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                domain,
                path: cookie
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or("/")
                    .to_string(),
                expires: cookie
                    .get("expirationDate")
                    .or_else(|| cookie.get("expires"))
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
                is_secure: cookie
                    .get("secure")
                    .or_else(|| cookie.get("is_secure"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                is_http_only: cookie
                    .get("httpOnly")
                    .or_else(|| cookie.get("is_http_only"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                same_site,
            });
    }
    grouped
        .into_iter()
        .map(|(domain, cookies)| DomainCookies {
            cookie_count: cookies.len(),
            domain,
            cookies,
        })
        .collect()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CookieCopyRequest {
    pub source_profile_id: String,
    pub target_profile_ids: Vec<String>,
    #[serde(default)]
    pub selected_domains: Option<Vec<String>>,
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CookieCopyResult {
    pub target_profile_id: String,
    pub copied_count: usize,
    pub replaced_count: usize,
    pub success: bool,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CookieAnalysisIssue {
    pub severity: String,
    pub code: String,
    pub message: String,
    pub source: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CookieAnalysis {
    pub valid_cookies: Vec<Value>,
    pub issues: Vec<CookieAnalysisIssue>,
    pub total_parsed: usize,
    pub valid_count: usize,
    pub expired_count: usize,
    pub format: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CookiePasteImportResult {
    pub imported_count: usize,
    pub replaced_count: usize,
    pub skipped_count: usize,
    pub issues: Vec<CookieAnalysisIssue>,
}

#[tauri::command]
pub async fn read_profile_cookies(
    state: State<'_, DesktopState>,
    profile_id: String,
) -> Result<CookieReadResult, String> {
    let cookies_json = export_profile_cookies(state, profile_id.clone(), "json".to_string())
        .await
        .map_err(|e| e.title)?;
    let cookies: Vec<Value> = serde_json::from_str(&cookies_json).unwrap_or_default();

    Ok(CookieReadResult {
        profile_id,
        browser_type: "wayfern".to_string(),
        domains: group_cookie_values(&cookies),
        total_count: cookies.len(),
    })
}

#[tauri::command]
pub async fn copy_profile_cookies(
    state: State<'_, DesktopState>,
    app: AppHandle,
    request: CookieCopyRequest,
) -> Result<Vec<CookieCopyResult>, String> {
    let source_json =
        export_profile_cookies(state.clone(), request.source_profile_id, "json".to_string())
            .await
            .map_err(|e| e.title)?;
    let cookies: Vec<Value> = serde_json::from_str(&source_json).unwrap_or_default();

    let filtered_cookies: Vec<Value> = if let Some(selected) = request.selected_domains {
        cookies
            .into_iter()
            .filter(|c| {
                if let Some(d) = c.get("domain").and_then(Value::as_str) {
                    selected
                        .iter()
                        .any(|sel| d.ends_with(sel) || sel.ends_with(d))
                } else {
                    false
                }
            })
            .collect()
    } else {
        cookies
    };

    let filtered_json =
        serde_json::to_string(&filtered_cookies).unwrap_or_else(|_| "[]".to_string());
    let mut results = Vec::new();

    for target_id in request.target_profile_ids {
        match import_profile_cookies(
            state.clone(),
            app.clone(),
            target_id.clone(),
            filtered_json.clone(),
        )
        .await
        {
            Ok(res) => {
                let count = res
                    .get("cookies_imported")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize;
                results.push(CookieCopyResult {
                    target_profile_id: target_id,
                    copied_count: count,
                    replaced_count: 0,
                    success: true,
                    error: None,
                });
            }
            Err(e) => {
                results.push(CookieCopyResult {
                    target_profile_id: target_id,
                    copied_count: 0,
                    replaced_count: 0,
                    success: false,
                    error: Some(e.title),
                });
            }
        }
    }

    Ok(results)
}

#[tauri::command]
pub async fn analyze_pasted_cookies(
    _profile_id: String,
    content: String,
    _site: Option<String>,
) -> Result<CookieAnalysis, String> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(CookieAnalysis {
            valid_cookies: Vec::new(),
            issues: Vec::new(),
            total_parsed: 0,
            valid_count: 0,
            expired_count: 0,
            format: "empty".to_string(),
        });
    }

    if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(trimmed) {
        let valid_count = items.len();
        return Ok(CookieAnalysis {
            valid_cookies: items,
            issues: Vec::new(),
            total_parsed: valid_count,
            valid_count,
            expired_count: 0,
            format: "json".to_string(),
        });
    }

    let mut valid_cookies = Vec::new();
    let mut issues = Vec::new();

    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 7 {
            valid_cookies.push(json!({
                "domain": parts[0],
                "path": parts[2],
                "secure": parts[3].eq_ignore_ascii_case("true"),
                "expires": parts[4].parse::<i64>().unwrap_or(0),
                "name": parts[5],
                "value": parts[6],
            }));
        } else if let Some((name, val)) = line.split_once('=') {
            valid_cookies.push(json!({
                "name": name.trim(),
                "value": val.trim(),
                "domain": "",
                "path": "/",
            }));
        } else {
            issues.push(CookieAnalysisIssue {
                severity: "warning".to_string(),
                code: "MALFORMED_LINE".to_string(),
                message: format!("Skipped unrecognizable line: {}", line),
                source: Some(line.to_string()),
            });
        }
    }

    let valid_count = valid_cookies.len();
    Ok(CookieAnalysis {
        total_parsed: valid_count + issues.len(),
        valid_count,
        expired_count: 0,
        format: if !valid_cookies.is_empty() {
            "netscape"
        } else {
            "unknown"
        }
        .to_string(),
        valid_cookies,
        issues,
    })
}

#[tauri::command]
pub async fn import_pasted_cookies(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
    content: String,
    site: Option<String>,
    _mode: Option<String>,
    _include_expired: Option<bool>,
) -> Result<CookiePasteImportResult, String> {
    let analysis = analyze_pasted_cookies(profile_id.clone(), content, site).await?;
    let json_content = serde_json::to_string(&analysis.valid_cookies).map_err(|e| e.to_string())?;

    let res = import_profile_cookies(state, app, profile_id, json_content)
        .await
        .map_err(|e| e.title)?;
    let count = res
        .get("cookies_imported")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    Ok(CookiePasteImportResult {
        imported_count: count,
        replaced_count: 0,
        skipped_count: analysis.issues.len(),
        issues: analysis.issues,
    })
}

#[cfg(test)]
mod tests {
    use super::group_cookie_values;
    use serde_json::json;

    #[test]
    fn cookie_export_groups_values_by_domain_and_preserves_fields() {
        let domains = group_cookie_values(&[
            json!({
                "domain": ".example.com",
                "name": "session",
                "value": "secret",
                "path": "/",
                "expirationDate": 1_900_000_000,
                "secure": true,
                "httpOnly": true,
                "sameSite": "strict"
            }),
            json!({"domain": ".example.com", "name": "prefs", "value": "a=b"}),
            json!({"domain": "other.test", "name": "id", "value": "42"}),
        ]);

        assert_eq!(domains.len(), 2);
        assert_eq!(domains[0].domain, ".example.com");
        assert_eq!(domains[0].cookie_count, 2);
        assert_eq!(domains[0].cookies[0].same_site, 2);
        assert!(domains[0].cookies[0].is_secure);
        assert!(domains[0].cookies[0].is_http_only);
        assert_eq!(domains[0].cookies[1].path, "/");
    }
}
