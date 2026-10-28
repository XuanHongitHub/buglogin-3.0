use std::{
    fmt,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use buglogin_domain::{entities::ProxyProtocol, errors::Problem};
use buglogin_storage::secrets::SecretValue;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Semaphore,
    sync::oneshot,
    task::{JoinHandle, JoinSet},
    time::timeout,
};

const MAX_HEADERS: usize = 64 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_CONCURRENT_PROXY_CHECKS: usize = 4;
static PROXY_CHECK_LIMITER: Semaphore = Semaphore::const_new(MAX_CONCURRENT_PROXY_CHECKS);

#[derive(Clone, Eq, PartialEq)]
pub struct ProxyBridgeSpec {
    pub protocol: ProxyProtocol,
    pub host: String,
    pub port: u16,
    pub credential: Option<SecretValue>,
}

impl fmt::Debug for ProxyBridgeSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProxyBridgeSpec")
            .field("protocol", &self.protocol)
            .field("host", &self.host)
            .field("port", &self.port)
            .field(
                "credential",
                &self.credential.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

pub struct ProxyBridgeHandle {
    proxy_url: String,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl fmt::Debug for ProxyBridgeHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProxyBridgeHandle")
            .field("proxy_url", &self.proxy_url)
            .finish()
    }
}

impl ProxyBridgeHandle {
    pub async fn start(spec: ProxyBridgeSpec) -> Result<Self, Problem> {
        if spec.host.trim().is_empty() || spec.port == 0 {
            return Err(problem(
                "proxy_bridge_upstream_invalid",
                "Proxy bridge upstream is invalid",
                422,
            ));
        }
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(|_| {
            problem(
                "proxy_bridge_bind_failed",
                "Proxy bridge could not bind loopback",
                503,
            )
        })?;
        let port = listener
            .local_addr()
            .map_err(|_| {
                problem(
                    "proxy_bridge_bind_failed",
                    "Proxy bridge address unavailable",
                    503,
                )
            })?
            .port();
        let (stop_sender, mut stop_receiver) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    _ = &mut stop_receiver => {
                        connections.abort_all();
                        while connections.join_next().await.is_some() {}
                        break;
                    }
                    Some(_) = connections.join_next(), if !connections.is_empty() => {}
                    accepted = listener.accept() => match accepted {
                        Ok((stream, _)) => {
                            let connection_spec = spec.clone();
                            connections.spawn(async move {
                                let _ = serve_connection(stream, connection_spec).await;
                            });
                        }
                        Err(_) => {
                            connections.abort_all();
                            while connections.join_next().await.is_some() {}
                            break;
                        }
                    },
                }
            }
        });
        Ok(Self {
            proxy_url: format!("http://127.0.0.1:{port}"),
            stop: Some(stop_sender),
            task: Some(task),
        })
    }

    pub fn proxy_url(&self) -> &str {
        &self.proxy_url
    }

    pub async fn stop(mut self) -> Result<(), Problem> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.await.map_err(|_| {
                problem(
                    "proxy_bridge_stop_failed",
                    "Proxy bridge task failed during stop",
                    500,
                )
            })?;
        }
        Ok(())
    }

    pub fn stop_in_background(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            tauri::async_runtime::spawn(async move {
                if let Err(error) = task.await {
                    tauri_plugin_log::log::warn!("Proxy bridge shutdown task failed: {}", error);
                }
            });
        }
    }
}

pub async fn check_proxy_connectivity(spec: ProxyBridgeSpec) -> Result<u64, Problem> {
    check_proxy_connectivity_with_limiter(spec, &PROXY_CHECK_LIMITER).await
}

pub async fn check_proxy_exit_ip(spec: ProxyBridgeSpec) -> Result<(String, u64), Problem> {
    let _permit = PROXY_CHECK_LIMITER.try_acquire().map_err(|_| {
        problem(
            "proxy_check_busy",
            "Too many proxy connectivity checks are already running",
            429,
        )
    })?;
    let bridge = ProxyBridgeHandle::start(spec).await?;
    let started = Instant::now();
    let result = async {
        let proxy = reqwest::Proxy::all(bridge.proxy_url()).map_err(|_| ())?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .proxy(proxy)
            .timeout(IO_TIMEOUT)
            .build()
            .map_err(|_| ())?;
        let response = client
            .get("https://api.ipify.org")
            .send()
            .await
            .map_err(|_| ())?;
        if !response.status().is_success() {
            return Err(());
        }
        let ip = response.text().await.map_err(|_| ())?.trim().to_owned();
        ip.parse::<std::net::IpAddr>().map_err(|_| ())?;
        Ok::<_, ()>((
            ip,
            started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        ))
    }
    .await;
    let stopped = bridge.stop().await;
    if stopped.is_err() {
        return Err(problem(
            "proxy_check_cleanup_failed",
            "Proxy check could not close its local tunnel",
            500,
        ));
    }
    result.map_err(|_| {
        problem(
            "proxy_check_connection_failed",
            "Proxy could not fetch its public IP address",
            502,
        )
    })
}

async fn check_proxy_connectivity_with_limiter(
    spec: ProxyBridgeSpec,
    limiter: &Semaphore,
) -> Result<u64, Problem> {
    let _permit = limiter.try_acquire().map_err(|_| {
        problem(
            "proxy_check_busy",
            "Too many proxy connectivity checks are already running",
            429,
        )
    })?;
    let bridge = ProxyBridgeHandle::start(spec).await?;
    let local_address = bridge
        .proxy_url()
        .strip_prefix("http://")
        .ok_or_else(|| {
            problem(
                "proxy_check_unavailable",
                "Proxy check could not start",
                503,
            )
        })?
        .to_owned();
    let started = Instant::now();
    let check = timeout(IO_TIMEOUT, async {
        let mut client = TcpStream::connect(&local_address).await.map_err(|_| ())?;
        client
            .write_all(b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n")
            .await
            .map_err(|_| ())?;
        let response = read_headers(&mut client).await?;
        let status_line = std::str::from_utf8(&response)
            .ok()
            .and_then(|response| response.split("\r\n").next())
            .ok_or(())?;
        let mut fields = status_line.split_ascii_whitespace();
        let version = fields.next().ok_or(())?;
        let status = fields
            .next()
            .and_then(|value| value.parse::<u16>().ok())
            .ok_or(())?;
        if !matches!(version, "HTTP/1.0" | "HTTP/1.1") || !(200..300).contains(&status) {
            return Err(());
        }
        Ok::<(), ()>(())
    })
    .await;
    let stopped = bridge.stop().await;
    if stopped.is_err() {
        return Err(problem(
            "proxy_check_cleanup_failed",
            "Proxy check could not close its local tunnel",
            500,
        ));
    }
    match check {
        Ok(Ok(())) => Ok(started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64),
        Ok(Err(())) => Err(problem(
            "proxy_check_connection_failed",
            "Proxy could not establish an HTTPS tunnel to the connectivity check target",
            502,
        )),
        Err(_) => Err(problem(
            "proxy_check_timeout",
            "Proxy connectivity check timed out",
            504,
        )),
    }
}

impl Drop for ProxyBridgeHandle {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

trait ProxyStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> ProxyStream for T {}
type BoxedStream = Box<dyn ProxyStream>;

async fn serve_connection(mut client: TcpStream, spec: ProxyBridgeSpec) -> Result<(), ()> {
    let request = read_headers(&mut client).await.map_err(|_| ())?;
    let parsed = parse_request(&request).ok_or(())?;
    let credentials = basic_authorization(spec.credential.as_ref()).map_err(|_| ())?;
    match spec.protocol {
        ProxyProtocol::Http | ProxyProtocol::Https => {
            let mut upstream = connect_http_upstream(&spec).await?;
            let upstream_request = build_upstream_request(&parsed, credentials.as_deref());
            write_all(&mut *upstream, &upstream_request).await?;
            if parsed.is_connect {
                let response = read_headers_stream(&mut *upstream).await?;
                if !response.starts_with(b"HTTP/1.1 2") && !response.starts_with(b"HTTP/1.0 2") {
                    let _ = client.write_all(&response).await;
                    return Err(());
                }
                client.write_all(&response).await.map_err(|_| ())?;
                if !parsed.trailing.is_empty() {
                    write_all(&mut *upstream, &parsed.trailing).await?;
                }
            }
            tokio::io::copy_bidirectional(&mut client, &mut upstream)
                .await
                .map_err(|_| ())?;
        }
        ProxyProtocol::Socks4 | ProxyProtocol::Socks5 => {
            let (host, port) = target_host_port(&parsed).ok_or(())?;
            let mut upstream = connect_socks_upstream(&spec, &host, port).await?;
            if parsed.is_connect {
                client
                    .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                    .await
                    .map_err(|_| ())?;
            } else {
                let mut forwarded = parsed;
                forwarded.target = origin_target(&forwarded.target).ok_or(())?;
                let request = build_upstream_request(&forwarded, None);
                write_all(&mut *upstream, &request).await?;
            }
            tokio::io::copy_bidirectional(&mut client, &mut upstream)
                .await
                .map_err(|_| ())?;
        }
    }
    Ok(())
}

async fn connect_http_upstream(spec: &ProxyBridgeSpec) -> Result<BoxedStream, ()> {
    let stream = timeout(
        IO_TIMEOUT,
        TcpStream::connect((spec.host.as_str(), spec.port)),
    )
    .await
    .map_err(|_| ())
    .and_then(|result| result.map_err(|_| ()))?;
    if spec.protocol == ProxyProtocol::Https {
        let connector = native_tls::TlsConnector::new().map_err(|_| ())?;
        let connector = tokio_native_tls::TlsConnector::from(connector);
        let stream = timeout(IO_TIMEOUT, connector.connect(&spec.host, stream))
            .await
            .map_err(|_| ())
            .and_then(|result| result.map_err(|_| ()))?;
        Ok(Box::new(stream))
    } else {
        Ok(Box::new(stream))
    }
}

async fn connect_socks_upstream(
    spec: &ProxyBridgeSpec,
    target_host: &str,
    target_port: u16,
) -> Result<BoxedStream, ()> {
    let mut stream = timeout(
        IO_TIMEOUT,
        TcpStream::connect((spec.host.as_str(), spec.port)),
    )
    .await
    .map_err(|_| ())
    .and_then(|result| result.map_err(|_| ()))?;
    let credentials = parse_credential(spec.credential.as_ref()).map_err(|_| ())?;
    match spec.protocol {
        ProxyProtocol::Socks5 => {
            socks5_connect(&mut stream, target_host, target_port, credentials).await?
        }
        ProxyProtocol::Socks4 => {
            socks4_connect(&mut stream, target_host, target_port, credentials).await?
        }
        _ => return Err(()),
    }
    Ok(Box::new(stream))
}

async fn socks5_connect(
    stream: &mut TcpStream,
    host: &str,
    port: u16,
    credentials: Option<CredentialParts>,
) -> Result<(), ()> {
    let methods = if credentials.is_some() {
        vec![0x00, 0x02]
    } else {
        vec![0x00]
    };
    let mut greeting = vec![0x05, methods.len() as u8];
    greeting.extend_from_slice(&methods);
    write_all(stream, &greeting).await?;
    let mut selected = [0u8; 2];
    read_exact(stream, &mut selected).await?;
    if selected[0] != 0x05 || selected[1] == 0xff {
        return Err(());
    }
    if selected[1] == 0x02 {
        let credentials = credentials.ok_or(())?;
        let username = credentials
            .username
            .as_ref()
            .map(SecretValue::as_bytes)
            .unwrap_or_default();
        let password = credentials
            .password
            .as_ref()
            .map(SecretValue::as_bytes)
            .unwrap_or_default();
        if username.len() > 255 || password.len() > 255 {
            return Err(());
        }
        let mut auth = vec![0x01, username.len() as u8];
        auth.extend_from_slice(username);
        auth.push(password.len() as u8);
        auth.extend_from_slice(password);
        write_all(stream, &auth).await?;
        let mut response = [0u8; 2];
        read_exact(stream, &mut response).await?;
        if response != [0x01, 0x00] {
            return Err(());
        }
    }
    let mut request = vec![0x05, 0x01, 0x00];
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        match ip {
            std::net::IpAddr::V4(ip) => {
                request.push(0x01);
                request.extend_from_slice(&ip.octets());
            }
            std::net::IpAddr::V6(ip) => {
                request.push(0x04);
                request.extend_from_slice(&ip.octets());
            }
        }
    } else {
        if host.len() > 255 {
            return Err(());
        }
        request.push(0x03);
        request.push(host.len() as u8);
        request.extend_from_slice(host.as_bytes());
    }
    request.extend_from_slice(&port.to_be_bytes());
    write_all(stream, &request).await?;
    let mut response = [0u8; 4];
    read_exact(stream, &mut response).await?;
    if response[0] != 0x05 || response[1] != 0x00 {
        return Err(());
    }
    let address_len = match response[3] {
        0x01 => 4,
        0x04 => 16,
        0x03 => {
            let mut length = [0u8; 1];
            read_exact(stream, &mut length).await?;
            length[0] as usize
        }
        _ => return Err(()),
    };
    let mut discarded = vec![0u8; address_len + 2];
    read_exact(stream, &mut discarded).await
}

async fn socks4_connect(
    stream: &mut TcpStream,
    host: &str,
    port: u16,
    credentials: Option<CredentialParts>,
) -> Result<(), ()> {
    if credentials
        .as_ref()
        .and_then(|value| value.password.as_ref())
        .is_some_and(|password| !password.is_empty())
    {
        return Err(());
    }
    let username = credentials
        .as_ref()
        .and_then(|value| value.username.as_ref())
        .map(SecretValue::as_bytes)
        .unwrap_or_default();
    if username.len() > 255 || host.len() > 255 {
        return Err(());
    }
    let mut request = vec![0x04, 0x01];
    request.extend_from_slice(&port.to_be_bytes());
    if let Ok(std::net::IpAddr::V4(ip)) = host.parse() {
        request.extend_from_slice(&ip.octets());
        request.extend_from_slice(username);
        request.push(0);
    } else {
        request.extend_from_slice(&[0, 0, 0, 1]);
        request.extend_from_slice(username);
        request.push(0);
        request.extend_from_slice(host.as_bytes());
        request.push(0);
    }
    write_all(stream, &request).await?;
    let mut response = [0u8; 8];
    read_exact(stream, &mut response).await?;
    if response[1] != 0x5a {
        return Err(());
    }
    Ok(())
}

async fn write_all(stream: &mut dyn ProxyStream, bytes: &[u8]) -> Result<(), ()> {
    timeout(IO_TIMEOUT, stream.write_all(bytes))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}

async fn read_exact(stream: &mut TcpStream, bytes: &mut [u8]) -> Result<(), ()> {
    timeout(IO_TIMEOUT, stream.read_exact(bytes))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;
    Ok(())
}

async fn read_headers_stream(stream: &mut dyn ProxyStream) -> Result<Vec<u8>, ()> {
    let mut bytes = Vec::with_capacity(4096);
    let mut chunk = [0u8; 2048];
    loop {
        let count = timeout(IO_TIMEOUT, stream.read(&mut chunk))
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;
        if count == 0 {
            return Err(());
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(bytes);
        }
        if bytes.len() > MAX_HEADERS {
            return Err(());
        }
    }
}

struct CredentialParts {
    username: Option<SecretValue>,
    password: Option<SecretValue>,
}

fn parse_credential(secret: Option<&SecretValue>) -> Result<Option<CredentialParts>, ()> {
    let Some(secret) = secret else {
        return Ok(None);
    };
    let value: Credential = serde_json::from_slice(secret.as_bytes()).map_err(|_| ())?;
    let username = value
        .username
        .filter(|value| !value.is_empty())
        .map(|value| SecretValue::new(value.into_bytes()).map_err(|_| ()))
        .transpose()?;
    let password = value
        .password
        .filter(|value| !value.is_empty())
        .map(|value| SecretValue::new(value.into_bytes()).map_err(|_| ()))
        .transpose()?;
    if username.is_none() && password.is_none() {
        return Err(());
    }
    Ok(Some(CredentialParts { username, password }))
}

fn target_host_port(request: &ParsedRequest) -> Option<(String, u16)> {
    if request.is_connect {
        let (host, port) = request.target.rsplit_once(':')?;
        return Some((
            host.trim_matches(&['[', ']'][..]).to_owned(),
            port.parse().ok()?,
        ));
    }
    let url = url::Url::parse(&request.target).ok()?;
    Some((url.host_str()?.to_owned(), url.port_or_known_default()?))
}

fn origin_target(target: &str) -> Option<String> {
    let url = url::Url::parse(target).ok()?;
    Some(url[url::Position::BeforePath..].to_owned())
}

struct ParsedRequest {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    trailing: Vec<u8>,
    is_connect: bool,
}

fn parse_request(bytes: &[u8]) -> Option<ParsedRequest> {
    let header_end = bytes.windows(4).position(|window| window == b"\r\n\r\n")? + 4;
    let text = std::str::from_utf8(&bytes[..header_end]).ok()?;
    let mut lines = text.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_owned();
    let target = parts.next()?.to_owned();
    let version = parts.next()?;
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return None;
    }
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':')?;
        if !name.eq_ignore_ascii_case("proxy-authorization")
            && !name.eq_ignore_ascii_case("proxy-connection")
        {
            headers.push((name.trim().to_owned(), value.trim().to_owned()));
        }
    }
    Some(ParsedRequest {
        is_connect: method.eq_ignore_ascii_case("CONNECT"),
        method,
        target,
        headers,
        trailing: bytes[header_end..].to_vec(),
    })
}

fn build_upstream_request(request: &ParsedRequest, authorization: Option<&str>) -> Vec<u8> {
    let mut output = format!("{} {} HTTP/1.1\r\n", request.method, request.target);
    let mut has_host = false;
    for (name, value) in &request.headers {
        if name.eq_ignore_ascii_case("proxy-authorization")
            || name.eq_ignore_ascii_case("proxy-connection")
        {
            continue;
        }
        if name.eq_ignore_ascii_case("host") {
            has_host = true;
        }
        output.push_str(name);
        output.push_str(": ");
        output.push_str(value);
        output.push_str("\r\n");
    }
    if request.is_connect && !has_host {
        output.push_str("Host: ");
        output.push_str(&request.target);
        output.push_str("\r\n");
    }
    if let Some(authorization) = authorization {
        output.push_str("Proxy-Authorization: Basic ");
        output.push_str(authorization);
        output.push_str("\r\n");
    }
    output.push_str("\r\n");
    let mut output = output.into_bytes();
    if !request.is_connect {
        output.extend_from_slice(&request.trailing);
    }
    output
}

fn basic_authorization(secret: Option<&SecretValue>) -> Result<Option<String>, ()> {
    let Some(secret) = secret else {
        return Ok(None);
    };
    let value = parse_credential(Some(secret))?.ok_or(())?;
    let username = value
        .username
        .as_ref()
        .map(SecretValue::as_bytes)
        .unwrap_or_default();
    let password = value
        .password
        .as_ref()
        .map(SecretValue::as_bytes)
        .unwrap_or_default();
    let mut cleartext = Vec::with_capacity(username.len() + password.len() + 1);
    cleartext.extend_from_slice(username);
    cleartext.push(b':');
    cleartext.extend_from_slice(password);
    let encoded = STANDARD.encode(&cleartext);
    cleartext.fill(0);
    Ok(Some(encoded))
}

#[derive(serde::Deserialize)]
struct Credential {
    username: Option<String>,
    password: Option<String>,
}

async fn read_headers(stream: &mut TcpStream) -> Result<Vec<u8>, ()> {
    let mut bytes = Vec::with_capacity(4096);
    let mut chunk = [0u8; 2048];
    loop {
        let count = timeout(IO_TIMEOUT, stream.read(&mut chunk))
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;
        if count == 0 {
            return Err(());
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(bytes);
        }
        if bytes.len() > MAX_HEADERS {
            return Err(());
        }
    }
}

fn problem(code: &'static str, title: &'static str, status: u16) -> Problem {
    Problem::new(code, title, status, "desktop-proxy-bridge")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;

    #[test]
    fn upstream_request_redacts_browser_auth_and_adds_bridge_auth() {
        let request = ParsedRequest {
            method: "CONNECT".into(),
            target: "target.example:443".into(),
            headers: vec![("Proxy-Authorization".into(), "Basic leaked".into())],
            trailing: Vec::new(),
            is_connect: true,
        };
        let output = String::from_utf8(build_upstream_request(&request, Some("safe"))).unwrap();
        assert!(output.contains("Proxy-Authorization: Basic safe"));
        assert!(!output.contains("leaked"));
    }

    #[tokio::test]
    async fn authenticated_http_connect_uses_upstream_auth_without_exposing_it_locally() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let upstream_address = listener.local_addr().unwrap();
        let seen = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let seen_server = Arc::clone(&seen);
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_headers(&mut stream).await.unwrap();
            *seen_server.lock().await = request.clone();
            stream
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await
                .unwrap();
            let mut payload = [0u8; 4];
            stream.read_exact(&mut payload).await.unwrap();
            stream.write_all(&payload).await.unwrap();
        });
        let secret = SecretValue::new(
            serde_json::to_vec(&serde_json::json!({"username":"alice","password":"secret"}))
                .unwrap(),
        )
        .unwrap();
        let bridge = ProxyBridgeHandle::start(ProxyBridgeSpec {
            protocol: ProxyProtocol::Http,
            host: upstream_address.ip().to_string(),
            port: upstream_address.port(),
            credential: Some(secret),
        })
        .await
        .unwrap();
        let proxy_address = bridge.proxy_url().strip_prefix("http://").unwrap();
        let mut client = TcpStream::connect(proxy_address).await.unwrap();
        client
            .write_all(b"CONNECT target.example:443 HTTP/1.1\r\nHost: target.example:443\r\n\r\n")
            .await
            .unwrap();
        let response = read_headers(&mut client).await.unwrap();
        assert!(response.starts_with(b"HTTP/1.1 200"));
        client.write_all(b"ping").await.unwrap();
        let mut echoed = [0u8; 4];
        client.read_exact(&mut echoed).await.unwrap();
        assert_eq!(&echoed, b"ping");
        let request = String::from_utf8(seen.lock().await.clone()).unwrap();
        assert!(request.contains("Proxy-Authorization: Basic "));
        assert!(!request.contains("alice"));
        assert!(!request.contains("secret"));
        bridge.stop().await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn stop_in_background_signals_shutdown_without_waiting_for_task_completion() {
        let (stop, mut stopped) = oneshot::channel();
        let (finish, wait_for_finish) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _ = (&mut stopped).await;
            let _ = wait_for_finish.await;
        });
        let bridge = ProxyBridgeHandle {
            proxy_url: "http://127.0.0.1:0".into(),
            stop: Some(stop),
            task: Some(task),
        };

        tokio::time::timeout(Duration::from_millis(250), async move {
            bridge.stop_in_background();
        })
        .await
        .expect("bridge shutdown must not block its caller");

        finish.send(()).unwrap();
    }

    #[tokio::test]
    async fn connectivity_check_uses_the_proxy_tunnel_and_returns_latency() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let upstream_address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_headers(&mut stream).await.unwrap();
            assert!(request.starts_with(b"CONNECT example.com:443 HTTP/1.1"));
            stream
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await
                .unwrap();
            let mut byte = [0u8; 1];
            let _ = stream.read(&mut byte).await;
        });
        let latency = check_proxy_connectivity(ProxyBridgeSpec {
            protocol: ProxyProtocol::Http,
            host: upstream_address.ip().to_string(),
            port: upstream_address.port(),
            credential: None,
        })
        .await
        .unwrap();
        assert!(latency < 15_000);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn connectivity_check_reports_proxy_rejection_without_exposing_response() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let upstream_address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = read_headers(&mut stream).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n")
                .await
                .unwrap();
        });
        let error = check_proxy_connectivity(ProxyBridgeSpec {
            protocol: ProxyProtocol::Http,
            host: upstream_address.ip().to_string(),
            port: upstream_address.port(),
            credential: None,
        })
        .await
        .unwrap_err();
        assert_eq!(error.code, "proxy_check_connection_failed");
        assert!(!error.title.contains("407"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn connectivity_check_fails_fast_when_concurrency_limit_is_reached() {
        let limiter = Semaphore::new(1);
        let _permit = limiter.try_acquire().unwrap();
        let error = check_proxy_connectivity_with_limiter(
            ProxyBridgeSpec {
                protocol: ProxyProtocol::Http,
                host: "127.0.0.1".to_owned(),
                port: 1,
                credential: None,
            },
            &limiter,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "proxy_check_busy");
        assert_eq!(error.status, 429);
    }

    #[tokio::test]
    async fn socks5_username_password_handshake_stays_inside_bridge() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut greeting = [0u8; 4];
            stream.read_exact(&mut greeting[..2]).await.unwrap();
            assert_eq!(&greeting[..2], &[0x05, 0x02]);
            let mut methods = [0u8; 2];
            stream.read_exact(&mut methods).await.unwrap();
            assert_eq!(methods, [0x00, 0x02]);
            stream.write_all(&[0x05, 0x02]).await.unwrap();
            let mut auth = [0u8; 2];
            stream.read_exact(&mut auth).await.unwrap();
            assert_eq!(auth, [0x01, 0x05]);
            let mut username = [0u8; 5];
            stream.read_exact(&mut username).await.unwrap();
            assert_eq!(&username, b"alice");
            let mut password_length = [0u8; 1];
            stream.read_exact(&mut password_length).await.unwrap();
            assert_eq!(password_length[0], 6);
            let mut password = [0u8; 6];
            stream.read_exact(&mut password).await.unwrap();
            assert_eq!(&password, b"secret");
            stream.write_all(&[0x01, 0x00]).await.unwrap();
            let mut connect = [0u8; 4];
            stream.read_exact(&mut connect).await.unwrap();
            assert_eq!(connect, [0x05, 0x01, 0x00, 0x03]);
            let mut length = [0u8; 1];
            stream.read_exact(&mut length).await.unwrap();
            let mut host = vec![0u8; length[0] as usize];
            stream.read_exact(&mut host).await.unwrap();
            assert_eq!(&host, b"target.example");
            let mut port = [0u8; 2];
            stream.read_exact(&mut port).await.unwrap();
            assert_eq!(u16::from_be_bytes(port), 443);
            stream
                .write_all(&[0x05, 0x00, 0x00, 0x01, 127, 0, 0, 1, 0, 1])
                .await
                .unwrap();
        });
        let secret = SecretValue::new(
            serde_json::to_vec(&serde_json::json!({"username":"alice","password":"secret"}))
                .unwrap(),
        )
        .unwrap();
        let parts = parse_credential(Some(&secret)).unwrap();
        let mut client = TcpStream::connect(address).await.unwrap();
        socks5_connect(&mut client, "target.example", 443, parts)
            .await
            .unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn socks4a_uses_username_and_rejects_password() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 8];
            stream.read_exact(&mut request).await.unwrap();
            assert_eq!(&request[..2], &[0x04, 0x01]);
            assert_eq!(u16::from_be_bytes([request[2], request[3]]), 80);
            let mut username = Vec::new();
            loop {
                let mut byte = [0u8; 1];
                stream.read_exact(&mut byte).await.unwrap();
                if byte[0] == 0 {
                    break;
                }
                username.push(byte[0]);
            }
            assert_eq!(&username, b"alice");
            let mut host = Vec::new();
            loop {
                let mut byte = [0u8; 1];
                stream.read_exact(&mut byte).await.unwrap();
                if byte[0] == 0 {
                    break;
                }
                host.push(byte[0]);
            }
            assert_eq!(&host, b"target.example");
            stream
                .write_all(&[0x00, 0x5a, 0, 0, 0, 0, 0, 0])
                .await
                .unwrap();
        });
        let secret =
            SecretValue::new(serde_json::to_vec(&serde_json::json!({"username":"alice"})).unwrap())
                .unwrap();
        let parts = parse_credential(Some(&secret)).unwrap();
        let mut client = TcpStream::connect(address).await.unwrap();
        socks4_connect(&mut client, "target.example", 80, parts)
            .await
            .unwrap();
        server.await.unwrap();
    }
}
