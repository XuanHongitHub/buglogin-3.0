use super::model::{ProxyProtocol, ProxyRow};
use std::collections::HashSet;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct ParseRowError {
    pub line: usize,
    pub code: &'static str,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct ParseReport {
    pub rows: Vec<ProxyRow>,
    pub errors: Vec<ParseRowError>,
    pub duplicates: usize,
}

pub fn parse_lines(input: &str) -> ParseReport {
    let mut report = ParseReport::default();
    let mut seen = HashSet::new();
    for (index, raw) in input.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        match parse_one(line) {
            Ok(row) => {
                let key = format!(
                    "{:?}|{}|{}|{}|{}",
                    row.protocol,
                    row.host,
                    row.port,
                    row.username.as_deref().unwrap_or(""),
                    row.password.as_deref().unwrap_or("")
                );
                if !seen.insert(key) {
                    report.duplicates += 1;
                } else {
                    report.rows.push(row);
                }
            }
            Err(code) => report.errors.push(ParseRowError {
                line: index + 1,
                code,
            }),
        }
    }
    report
}

fn parse_one(input: &str) -> Result<ProxyRow, &'static str> {
    if !input.contains("://") && input.split(':').count() == 4 && !input.starts_with('[') {
        let parts = input.split(':').collect::<Vec<_>>();
        let host_first_port = parts[1].parse::<u16>().ok().filter(|port| *port > 0);
        let credentials_first_port = parts[3].parse::<u16>().ok().filter(|port| *port > 0);
        let (host, port, username, password) = match (host_first_port, credentials_first_port) {
            (Some(port), None) => (parts[0], port, parts[2], parts[3]),
            (None, Some(port)) => (parts[2], port, parts[0], parts[1]),
            (Some(port), Some(_)) => (parts[0], port, parts[2], parts[3]),
            (None, None) => return Err("invalid_port"),
        };
        if host.is_empty() || username.is_empty() {
            return Err("invalid_proxy");
        }
        return Ok(ProxyRow {
            protocol: ProxyProtocol::Http,
            host: host.to_owned(),
            port,
            username: Some(username.to_owned()),
            password: Some(password.to_owned()),
        });
    }
    let (protocol, rest) = if let Some((scheme, value)) = input.split_once("://") {
        (protocol(scheme)?, value)
    } else {
        (ProxyProtocol::Http, input)
    };
    let (credentials, endpoint) = match rest.rsplit_once('@') {
        Some((auth, host)) => (Some(auth), host),
        None => (None, rest),
    };
    let (host, port) = split_host_port(endpoint)?;
    let (username, password) = credentials
        .map(parse_auth)
        .transpose()?
        .unwrap_or((None, None));
    Ok(ProxyRow {
        protocol,
        host,
        port,
        username,
        password,
    })
}

fn protocol(value: &str) -> Result<ProxyProtocol, &'static str> {
    match value.to_ascii_lowercase().as_str() {
        "http" => Ok(ProxyProtocol::Http),
        "https" => Ok(ProxyProtocol::Https),
        "socks4" => Ok(ProxyProtocol::Socks4),
        "socks" | "socks5" => Ok(ProxyProtocol::Socks5),
        _ => Err("unsupported_protocol"),
    }
}
fn parse_auth(value: &str) -> Result<(Option<String>, Option<String>), &'static str> {
    let (user, pass) = value
        .split_once(':')
        .map_or((value, None), |(user, pass)| (user, Some(pass)));
    if user.is_empty() {
        return Err("invalid_credentials");
    }
    Ok((Some(user.to_owned()), pass.map(str::to_owned)))
}
fn split_host_port(value: &str) -> Result<(String, u16), &'static str> {
    let (host, port) = if let Some(stripped) = value.strip_prefix('[') {
        let (host, rest) = stripped.split_once(']').ok_or("invalid_host")?;
        (host, rest.strip_prefix(':').ok_or("invalid_port")?)
    } else {
        value.rsplit_once(':').ok_or("invalid_port")?
    };
    if host.is_empty() {
        return Err("invalid_host");
    }
    let port = port.parse::<u16>().map_err(|_| "invalid_port")?;
    if port == 0 {
        return Err("invalid_port");
    }
    Ok((host.to_owned(), port))
}
