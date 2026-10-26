use std::{
    collections::BTreeSet,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, State};
use url::Url;

use crate::desktop_state::DesktopState;
use crate::profile_cookie_manager::{
    UnifiedCookieItem, export_profile_cookies, import_profile_cookies,
    import_profile_cookies_replacing_domains,
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
#[serde(rename_all = "camelCase")]
pub struct CookieAnalysisIssue {
    pub severity: String,
    pub code: String,
    pub message: String,
    pub source: Option<String>,
    pub params: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PastedCookiePreview {
    pub domain: String,
    pub name: String,
    pub path: String,
    pub expires: i64,
    pub is_secure: bool,
    pub is_http_only: bool,
    pub same_site: i32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CookieAnalysis {
    pub cookies: Vec<PastedCookiePreview>,
    pub issues: Vec<CookieAnalysisIssue>,
    pub format: Option<String>,
    pub site_required: bool,
    pub expired_count: usize,
    pub replace_delete_count: Option<usize>,
    pub clears_on_close: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CookiePasteImportResult {
    pub added: usize,
    pub overwritten: usize,
    pub deleted: usize,
    pub skipped: usize,
    pub issues: Vec<CookieAnalysisIssue>,
}

#[derive(Debug, Clone)]
struct ParsedPastedCookie {
    name: String,
    value: String,
    domain: String,
    path: String,
    expires: i64,
    secure: bool,
    http_only: bool,
    same_site: i32,
}

impl ParsedPastedCookie {
    fn preview(&self) -> PastedCookiePreview {
        PastedCookiePreview {
            domain: self.domain.clone(),
            name: self.name.clone(),
            path: self.path.clone(),
            expires: self.expires,
            is_secure: self.secure,
            is_http_only: self.http_only,
            same_site: self.same_site,
        }
    }
    fn as_value(&self) -> Value {
        json!({"name": self.name, "value": self.value, "domain": self.domain,
            "path": self.path, "expires": self.expires, "secure": self.secure,
            "httpOnly": self.http_only, "sameSite": match self.same_site { 1 => "lax", 2 => "strict", _ => "no_restriction" }})
    }
}

fn cookie_issue(code: &str, severity: &str) -> CookieAnalysisIssue {
    CookieAnalysisIssue {
        severity: severity.into(),
        code: code.into(),
        message: code.into(),
        source: None,
        params: None,
    }
}

fn cookie_site(site: Option<&str>) -> Option<String> {
    let site = site?.trim();
    if site.is_empty() {
        return None;
    }
    let value = if site.contains("://") {
        site.to_owned()
    } else {
        format!("https://{site}")
    };
    Url::parse(&value)
        .ok()?
        .host_str()
        .map(|host| host.trim_start_matches('.').to_owned())
}

fn normalize_expiry(expiry: i64) -> i64 {
    if expiry > 100_000_000_000_000 {
        expiry / 1_000_000
    } else if expiry > 100_000_000_000 {
        expiry / 1_000
    } else {
        expiry.max(0)
    }
}

fn push_cookie_pair(
    pair: &str,
    domain: &str,
    path: &str,
    expires: i64,
    secure: bool,
    http_only: bool,
    same_site: i32,
    cookies: &mut Vec<ParsedPastedCookie>,
    issues: &mut Vec<CookieAnalysisIssue>,
) {
    let Some((name, value)) = pair.trim().split_once('=') else {
        if !pair.trim().is_empty() {
            issues.push(cookie_issue("NAME_VALUE_NO_PAIR", "warning"));
        }
        return;
    };
    let name = name.trim();
    if name.is_empty() {
        issues.push(cookie_issue("NAME_MISSING", "warning"));
        return;
    }
    cookies.push(ParsedPastedCookie {
        name: name.into(),
        value: value.trim().trim_matches('"').into(),
        domain: domain.into(),
        path: path.into(),
        expires,
        secure,
        http_only,
        same_site,
    });
}

fn parse_pasted_cookie_text(
    content: &str,
    site: Option<&str>,
) -> (
    Vec<ParsedPastedCookie>,
    Vec<CookieAnalysisIssue>,
    Option<String>,
) {
    let input = content.trim();
    let mut cookies = Vec::new();
    let mut issues = Vec::new();
    if input.is_empty() {
        return (cookies, vec![cookie_issue("EMPTY_INPUT", "error")], None);
    }
    let fallback_domain = cookie_site(site);

    if input.starts_with('{') || input.starts_with('[') {
        let value: Value = match serde_json::from_str(input) {
            Ok(value) => value,
            Err(_) => {
                return (
                    cookies,
                    vec![cookie_issue("JSON_PARSE_FAILED", "error")],
                    Some("json".into()),
                );
            }
        };
        let list = match &value {
            Value::Array(items) => Some(items.as_slice()),
            Value::Object(object) => ["cookies", "items", "data", "cookieList"]
                .iter()
                .find_map(|key| {
                    object
                        .get(*key)
                        .and_then(Value::as_array)
                        .map(Vec::as_slice)
                })
                .or_else(|| {
                    if object.contains_key("name") {
                        None
                    } else {
                        None
                    }
                }),
            _ => None,
        };
        let one = if value.is_object() && value.get("name").is_some() {
            Some(&value)
        } else {
            None
        };
        let items: Vec<&Value> = list
            .map(|items| items.iter().collect())
            .unwrap_or_else(|| one.into_iter().collect());
        if items.is_empty() {
            return (
                cookies,
                vec![cookie_issue("JSON_NOT_COOKIE_LIST", "error")],
                Some("json".into()),
            );
        }
        for item in items {
            let Some(name) = item
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                issues.push(cookie_issue("NAME_MISSING", "warning"));
                continue;
            };
            let Some(value) = item.get("value").and_then(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .or_else(|| v.as_i64().map(|n| n.to_string()))
            }) else {
                issues.push(cookie_issue("VALUE_INVALID", "warning"));
                continue;
            };
            let mut domain = item
                .get("domain")
                .or_else(|| item.get("host"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_owned();
            if domain.is_empty() {
                domain = fallback_domain.clone().unwrap_or_default();
            }
            let path = item
                .get("path")
                .and_then(Value::as_str)
                .filter(|p| !p.is_empty())
                .unwrap_or("/")
                .to_owned();
            let expires = if item.get("session").and_then(Value::as_bool) == Some(true) {
                0
            } else {
                normalize_expiry(
                    item.get("expirationDate")
                        .or_else(|| item.get("expires"))
                        .or_else(|| item.get("expiresAt"))
                        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|n| n as i64)))
                        .unwrap_or(0),
                )
            };
            let same_site = match item
                .get("sameSite")
                .or_else(|| item.get("same_site"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str()
            {
                "lax" => 1,
                "strict" => 2,
                _ => 0,
            };
            cookies.push(ParsedPastedCookie {
                name: name.into(),
                value,
                domain,
                path,
                expires,
                secure: item.get("secure").and_then(Value::as_bool).unwrap_or(false),
                http_only: item
                    .get("httpOnly")
                    .or_else(|| item.get("http_only"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                same_site,
            });
        }
        return (cookies, issues, Some("json".into()));
    }

    let mut format = "nameValue";
    for raw in input.lines() {
        let mut line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let http_only = line.starts_with("#HttpOnly_");
        if http_only {
            line = line.trim_start_matches("#HttpOnly_");
        } else if line.starts_with('#') {
            continue;
        }
        let columns: Vec<&str> = line.split('\t').collect();
        if columns.len() >= 7 {
            format = "netscape";
            let domain = columns[0].trim();
            let secure = columns[3].eq_ignore_ascii_case("true");
            let expires = columns[4].parse::<i64>().map(normalize_expiry).unwrap_or(0);
            let before = cookies.len();
            push_cookie_pair(
                &format!("{}={}", columns[5], columns[6]),
                domain,
                columns[2],
                expires,
                secure,
                http_only,
                0,
                &mut cookies,
                &mut issues,
            );
            if cookies.len() > before {
                continue;
            }
        } else if line.to_ascii_lowercase().starts_with("set-cookie:") {
            let mut parts = line.split(';');
            let pair = parts
                .next()
                .unwrap_or("")
                .split_once(':')
                .map(|(_, v)| v.trim())
                .unwrap_or("");
            let mut domain = fallback_domain.clone().unwrap_or_default();
            let mut path = "/".to_owned();
            let mut secure = false;
            let mut is_http_only = false;
            let mut same_site = 0;
            let mut expires = 0;
            for attr in parts {
                let (key, val) = attr.trim().split_once('=').unwrap_or((attr.trim(), ""));
                match key.trim().to_ascii_lowercase().as_str() {
                    "domain" => domain = val.trim().to_owned(),
                    "path" => path = val.trim().to_owned(),
                    "secure" => secure = true,
                    "httponly" => is_http_only = true,
                    "samesite" => {
                        same_site = match val.trim().to_ascii_lowercase().as_str() {
                            "lax" => 1,
                            "strict" => 2,
                            _ => 0,
                        }
                    }
                    "max-age" => {
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() as i64;
                        expires = val
                            .trim()
                            .parse::<i64>()
                            .ok()
                            .map(|seconds| {
                                if seconds <= 0 {
                                    now.saturating_sub(1)
                                } else {
                                    now.saturating_add(seconds)
                                }
                            })
                            .unwrap_or(0);
                    }
                    "expires" => {
                        expires = httpdate::parse_http_date(val.trim())
                            .ok()
                            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0)
                    }
                    _ => {}
                }
            }
            push_cookie_pair(
                pair,
                &domain,
                &path,
                expires,
                secure,
                http_only || is_http_only,
                same_site,
                &mut cookies,
                &mut issues,
            );
        } else {
            let value = if line.to_ascii_lowercase().starts_with("cookie:") {
                line.split_once(':')
                    .map(|(_, value)| value.trim())
                    .unwrap_or(line)
            } else {
                line
            };
            for pair in value.split(';') {
                push_cookie_pair(
                    pair,
                    fallback_domain.as_deref().unwrap_or(""),
                    "/",
                    0,
                    false,
                    false,
                    0,
                    &mut cookies,
                    &mut issues,
                );
            }
        }
    }
    (cookies, issues, Some(format.into()))
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
    site: Option<String>,
) -> Result<CookieAnalysis, String> {
    let (parsed, issues, format) = parse_pasted_cookie_text(&content, site.as_deref());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let expired_count = parsed
        .iter()
        .filter(|cookie| cookie.expires > 0 && cookie.expires <= now)
        .count();
    let site_required = parsed.iter().any(|cookie| cookie.domain.is_empty());
    Ok(CookieAnalysis {
        cookies: parsed.iter().map(ParsedPastedCookie::preview).collect(),
        issues,
        format,
        site_required,
        expired_count,
        replace_delete_count: None,
        clears_on_close: false,
        blocked_by: None,
    })
}

#[tauri::command]
pub async fn import_pasted_cookies(
    state: State<'_, DesktopState>,
    app: AppHandle,
    profile_id: String,
    content: String,
    site: Option<String>,
    mode: Option<String>,
    include_expired: Option<bool>,
) -> Result<CookiePasteImportResult, String> {
    let (mut parsed, issues, _) = parse_pasted_cookie_text(&content, site.as_deref());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let expired_count = parsed
        .iter()
        .filter(|cookie| cookie.expires > 0 && cookie.expires <= now)
        .count();
    if include_expired != Some(true) {
        parsed.retain(|cookie| cookie.expires == 0 || cookie.expires > now);
    }
    if parsed.iter().any(|cookie| cookie.domain.is_empty()) {
        return Err("Hãy nhập trang web chứa cookie trước khi nhập.".to_string());
    }
    if parsed.is_empty() {
        return Err("Không tìm thấy cookie hợp lệ để nhập.".to_string());
    }
    if mode
        .as_deref()
        .is_some_and(|mode| mode != "merge" && mode != "replaceMatchingSites")
    {
        return Err("Chế độ nhập cookie không hợp lệ.".to_string());
    }
    let json_content = serde_json::to_string(
        &parsed
            .iter()
            .map(ParsedPastedCookie::as_value)
            .collect::<Vec<_>>(),
    )
    .map_err(|e| e.to_string())?;

    let res = if mode.as_deref() == Some("replaceMatchingSites") {
        let domains = parsed
            .iter()
            .map(|cookie| cookie.domain.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        import_profile_cookies_replacing_domains(state, app, profile_id, json_content, domains)
            .await
            .map_err(|e| e.title)?
    } else {
        import_profile_cookies(state, app, profile_id, json_content)
            .await
            .map_err(|e| e.title)?
    };
    let count = res
        .get("cookies_imported")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let replaced = res
        .get("cookies_replaced")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    if count == 0 && replaced == 0 {
        return Err("Trình nhập không lưu được cookie nào.".to_string());
    }
    Ok(CookiePasteImportResult {
        added: res
            .get("cookies_added")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| count.saturating_sub(replaced) as u64) as usize,
        overwritten: replaced,
        deleted: res
            .get("cookies_deleted")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
        skipped: issues
            .iter()
            .filter(|issue| issue.severity != "info")
            .count()
            + if include_expired == Some(true) {
                0
            } else {
                expired_count
            },
        issues,
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
