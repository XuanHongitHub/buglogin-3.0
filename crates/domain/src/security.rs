use serde_json::Value;

/// Validate the canonical text form used for opaque 32-byte tokens.
/// Tokens are case-sensitive strings; generated values use lowercase hex only.
pub fn is_lower_hex_token_64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Detect secret-bearing fields and values in caller-controlled JSON before
/// storing data that may later be returned to other authorized readers.
pub fn contains_secret(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, child)| {
            (is_secret_field(key) && !child.is_null()) || contains_secret(child)
        }),
        Value::Array(values) => values.iter().any(contains_secret),
        Value::String(value) => {
            let value = value.trim();
            let lower = value.to_ascii_lowercase();
            lower.starts_with("bearer ")
                || lower.starts_with("basic ")
                || contains_scheme_less_proxy_credentials(value)
                || url::Url::parse(value)
                    .ok()
                    .is_some_and(|url| !url.username().is_empty() || url.password().is_some())
        }
        _ => false,
    }
}

fn contains_scheme_less_proxy_credentials(value: &str) -> bool {
    let token = value.split_whitespace().next().unwrap_or_default();
    let authority = token
        .split_once("://")
        .map_or(token, |(_, rest)| rest)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let Some((userinfo, host)) = authority.split_once('@') else {
        return false;
    };
    !host.is_empty() && userinfo.contains(':')
}

pub fn is_secret_field(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().replace(['-', '_'], "").as_str(),
        "password"
            | "proxypassword"
            | "username"
            | "proxyusername"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "apikey"
            | "secret"
            | "clientsecret"
            | "privatekey"
            | "cookie"
            | "cookies"
            | "credential"
            | "credentials"
            | "authorization"
    )
}

#[cfg(test)]
mod tests {
    use super::is_lower_hex_token_64;

    #[test]
    fn opaque_token_wire_form_is_exactly_lowercase_hex() {
        assert!(is_lower_hex_token_64(&"0123456789abcdef".repeat(4)));
        assert!(!is_lower_hex_token_64(&"0123456789ABCDEF".repeat(4)));
        assert!(!is_lower_hex_token_64(&"a".repeat(63)));
        assert!(!is_lower_hex_token_64(&format!("{}g", "a".repeat(63))));
    }
}
