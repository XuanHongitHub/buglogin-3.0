use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use buglogin_domain::errors::Problem;
use buglogin_storage::{
    secrets::{OsVault, SecretKey, SecretValue},
    vault::NativeVault,
};
use getrandom::fill;
use ring::hmac as ring_hmac;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;
use zeroize::{Zeroize, Zeroizing};

const ROUTE_NAMESPACE: &str = "buglogin.sso.callback.handoff.v1";
const MAX_CALLBACK_BYTES: usize = 4096;
const MAX_ROUTES_REMEMBERED: usize = 8;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const IO_TIMEOUT: Duration = Duration::from_millis(750);

pub struct SsoCallbackHandoff {
    port: u16,
    secret: [u8; 32],
    listener: TcpListener,
    vault: Arc<Mutex<NativeVault>>,
    forwarded_state_keys: Mutex<Vec<String>>,
    listener_ready: AtomicBool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RouteRecord {
    port: u16,
    secret: String,
    expires_at: u64,
}

impl Drop for RouteRecord {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HandoffMessage {
    url: String,
    mac: String,
}

impl Drop for HandoffMessage {
    fn drop(&mut self) {
        self.url.zeroize();
        self.mac.zeroize();
    }
}

impl SsoCallbackHandoff {
    pub fn new() -> Result<Self, Problem> {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .map_err(|_| unavailable("desktop-sso-handoff"))?;
        let port = listener
            .local_addr()
            .map_err(|_| unavailable("desktop-sso-handoff"))?
            .port();
        let mut secret = [0_u8; 32];
        fill(&mut secret).map_err(|_| unavailable("desktop-sso-handoff"))?;
        Ok(Self {
            port,
            secret,
            listener,
            vault: Arc::new(Mutex::new(NativeVault::new())),
            forwarded_state_keys: Mutex::new(Vec::new()),
            listener_ready: AtomicBool::new(false),
        })
    }

    pub fn listener_ready(&self) -> bool {
        self.listener_ready.load(Ordering::Acquire)
    }

    pub fn register(&self, state: &str, expires_at: u64) -> Result<(), Problem> {
        if state.is_empty() || state.len() > 512 || expires_at <= now_seconds()? {
            return Err(invalid_route());
        }
        let mut record = RouteRecord {
            port: self.port,
            secret: hex(&self.secret),
            expires_at,
        };
        let bytes = serde_json::to_vec(&record).map_err(|_| unavailable("desktop-sso-handoff"))?;
        let value = SecretValue::new(bytes).map_err(|_| unavailable("desktop-sso-handoff"))?;
        let key = route_key(state)?;
        let mut vault = self
            .vault
            .lock()
            .map_err(|_| unavailable("desktop-sso-handoff"))?;
        match vault.read(&key) {
            Ok((stored, version)) => {
                let existing: RouteRecord =
                    serde_json::from_slice(stored.as_bytes()).map_err(|_| invalid_route())?;
                if existing.expires_at > now_seconds()? {
                    return Err(Problem::new(
                        "sso_callback_route_exists",
                        "A sign-in callback is already pending on this device",
                        409,
                        "desktop-sso-handoff",
                    ));
                }
                vault
                    .rotate(&key, version, value)
                    .map_err(|_| unavailable("desktop-sso-handoff"))?;
            }
            Err(buglogin_storage::secrets::SecretError::NotFound) => {
                vault
                    .create(&key, value)
                    .map_err(|_| unavailable("desktop-sso-handoff"))?;
            }
            Err(_) => return Err(unavailable("desktop-sso-handoff")),
        }
        record.secret.zeroize();
        Ok(())
    }

    pub fn remove(&self, state: &str) {
        let Ok(key) = route_key(state) else {
            return;
        };
        let Ok(mut vault) = self.vault.lock() else {
            return;
        };
        if let Ok((_, version)) = vault.read(&key) {
            let _ = vault.remove(&key, version);
        }
    }

    /// Returns true only when the callback belongs to another running instance and
    /// that instance acknowledged the handoff. The caller may process local URLs.
    pub fn forward(&self, callback_url: &str) -> Result<bool, Problem> {
        let state = match callback_state(callback_url) {
            Some(state) => state,
            None => return Ok(false),
        };
        let state_key = route_record_name(&state);
        if self
            .forwarded_state_keys
            .lock()
            .map_err(|_| unavailable("desktop-sso-handoff"))?
            .iter()
            .any(|known| known == &state_key)
        {
            return Ok(true);
        }
        let Some(route) = self.load_route(&state)? else {
            return Ok(false);
        };
        if route.port == self.port {
            return Ok(false);
        }
        if callback_url.len() > MAX_CALLBACK_BYTES {
            return Err(invalid_callback());
        }
        let secret = decode_hex_32(&route.secret).ok_or_else(invalid_route)?;
        let key = ring_hmac::Key::new(ring_hmac::HMAC_SHA256, &secret);
        let signature = ring_hmac::sign(&key, callback_url.as_bytes());
        let message = HandoffMessage {
            url: callback_url.to_owned(),
            mac: hex(signature.as_ref()),
        };
        let bytes = serde_json::to_vec(&message).map_err(|_| invalid_callback())?;
        if bytes.len() > MAX_CALLBACK_BYTES + 256 {
            return Err(invalid_callback());
        }
        let mut stream = TcpStream::connect_timeout(
            &SocketAddr::from((Ipv4Addr::LOCALHOST, route.port)),
            CONNECT_TIMEOUT,
        )
        .map_err(|_| handoff_failed())?;
        stream
            .set_read_timeout(Some(IO_TIMEOUT))
            .map_err(|_| handoff_failed())?;
        stream
            .set_write_timeout(Some(IO_TIMEOUT))
            .map_err(|_| handoff_failed())?;
        stream
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .and_then(|_| stream.write_all(&bytes))
            .map_err(|_| handoff_failed())?;
        let mut ack = [0_u8; 1];
        stream.read_exact(&mut ack).map_err(|_| handoff_failed())?;
        if ack[0] != 1 {
            return Err(handoff_failed());
        }
        self.remove(&state);
        let mut remembered = self
            .forwarded_state_keys
            .lock()
            .map_err(|_| unavailable("desktop-sso-handoff"))?;
        if remembered.len() == MAX_ROUTES_REMEMBERED {
            remembered.remove(0);
        }
        remembered.push(state_key);
        Ok(true)
    }

    pub fn start_listener<F>(&self, mut on_callback: F) -> Result<(), Problem>
    where
        F: FnMut(String) + Send + 'static,
    {
        self.listener
            .set_nonblocking(true)
            .map_err(|_| unavailable("desktop-sso-handoff"))?;
        let listener = self
            .listener
            .try_clone()
            .map_err(|_| unavailable("desktop-sso-handoff"))?;
        let port = self.port;
        let secret = self.secret;
        let vault = Arc::clone(&self.vault);
        thread::Builder::new()
            .name("buglogin-sso-callback-handoff".into())
            .spawn(move || {
                loop {
                    let (mut stream, _) = match listener.accept() {
                        Ok(connection) => connection,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(20));
                            continue;
                        }
                        Err(_) => break,
                    };
                    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
                    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
                    let accepted = receive_callback(&mut stream, port, &secret, &vault)
                        .ok()
                        .flatten();
                    if let Some((state, mut url)) = accepted {
                        on_callback(std::mem::take(&mut url));
                        remove_route_from_vault(&vault, &state);
                        let _ = stream.write_all(&[1]);
                        url.zeroize();
                    } else {
                        let _ = stream.write_all(&[0]);
                    }
                }
            })
            .map_err(|_| unavailable("desktop-sso-handoff"))?;
        self.listener_ready.store(true, Ordering::Release);
        Ok(())
    }

    fn load_route(&self, state: &str) -> Result<Option<RouteRecord>, Problem> {
        let key = route_key(state)?;
        let mut vault = self
            .vault
            .lock()
            .map_err(|_| unavailable("desktop-sso-handoff"))?;
        let (value, version) = match vault.read(&key) {
            Ok(record) => record,
            Err(buglogin_storage::secrets::SecretError::NotFound) => return Ok(None),
            Err(_) => return Err(unavailable("desktop-sso-handoff")),
        };
        let mut route: RouteRecord =
            serde_json::from_slice(value.as_bytes()).map_err(|_| invalid_route())?;
        if route.port == 0 || decode_hex_32(&route.secret).is_none() {
            return Err(invalid_route());
        }
        if route.expires_at <= now_seconds()? {
            let _ = vault.remove(&key, version);
            return Ok(None);
        }
        route.secret.shrink_to_fit();
        Ok(Some(route))
    }
}

impl Drop for SsoCallbackHandoff {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

fn receive_callback(
    stream: &mut TcpStream,
    port: u16,
    secret: &[u8; 32],
    vault: &Arc<Mutex<NativeVault>>,
) -> Result<Option<(String, String)>, Problem> {
    let mut size = [0_u8; 4];
    stream
        .read_exact(&mut size)
        .map_err(|_| invalid_callback())?;
    let size = u32::from_be_bytes(size) as usize;
    if size == 0 || size > MAX_CALLBACK_BYTES + 256 {
        return Err(invalid_callback());
    }
    let mut bytes = vec![0_u8; size];
    stream
        .read_exact(&mut bytes)
        .map_err(|_| invalid_callback())?;
    let mut message: HandoffMessage =
        serde_json::from_slice(&bytes).map_err(|_| invalid_callback())?;
    bytes.zeroize();
    if message.url.len() > MAX_CALLBACK_BYTES {
        return Err(invalid_callback());
    }
    let Some(state) = callback_state(&message.url) else {
        return Err(invalid_callback());
    };
    let expected_mac = decode_hex_32(&message.mac).ok_or_else(invalid_callback)?;
    let key = ring_hmac::Key::new(ring_hmac::HMAC_SHA256, secret);
    ring_hmac::verify(&key, message.url.as_bytes(), &expected_mac)
        .map_err(|_| invalid_callback())?;
    let route = load_route_from_vault(vault, &state)?;
    let Some(route) = route else {
        return Ok(None);
    };
    if route.port != port || decode_hex_32(&route.secret).as_ref() != Some(secret) {
        return Err(invalid_callback());
    }
    Ok(Some((state.to_string(), std::mem::take(&mut message.url))))
}

fn load_route_from_vault(
    vault: &Arc<Mutex<NativeVault>>,
    state: &str,
) -> Result<Option<RouteRecord>, Problem> {
    let key = route_key(state)?;
    let mut vault = vault
        .lock()
        .map_err(|_| unavailable("desktop-sso-handoff"))?;
    let (value, version) = match vault.read(&key) {
        Ok(record) => record,
        Err(buglogin_storage::secrets::SecretError::NotFound) => return Ok(None),
        Err(_) => return Err(unavailable("desktop-sso-handoff")),
    };
    let route: RouteRecord =
        serde_json::from_slice(value.as_bytes()).map_err(|_| invalid_route())?;
    if route.expires_at <= now_seconds()? {
        let _ = vault.remove(&key, version);
        return Ok(None);
    }
    Ok(Some(route))
}

fn remove_route_from_vault(vault: &Arc<Mutex<NativeVault>>, state: &str) {
    let Ok(key) = route_key(state) else {
        return;
    };
    let Ok(mut vault) = vault.lock() else {
        return;
    };
    if let Ok((_, version)) = vault.read(&key) {
        let _ = vault.remove(&key, version);
    }
}

fn route_key(state: &str) -> Result<SecretKey, Problem> {
    if state.is_empty() || state.len() > 512 {
        return Err(invalid_route());
    }
    SecretKey::new(ROUTE_NAMESPACE, route_record_name(state)).map_err(|_| invalid_route())
}

fn route_record_name(state: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"buglogin-sso-callback-route-v1:");
    digest.update(state.as_bytes());
    hex(&digest.finalize())
}

fn callback_state(callback: &str) -> Option<Zeroizing<String>> {
    if callback.len() > MAX_CALLBACK_BYTES {
        return None;
    }
    let url = Url::parse(callback).ok()?;
    if url.scheme() != "buglogin"
        || url.host_str() != Some("sso")
        || url.path() != "/callback"
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let states: Vec<_> = url
        .query_pairs()
        .filter(|(key, _)| key == "state")
        .collect();
    let tokens: Vec<_> = url
        .query_pairs()
        .filter(|(key, _)| key == "token")
        .collect();
    if states.len() != 1
        || tokens.len() != 1
        || states[0].1.is_empty()
        || states[0].1.len() > 512
        || tokens[0].1.is_empty()
    {
        return None;
    }
    Some(Zeroizing::new(states[0].1.to_string()))
}

fn decode_hex_32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut output = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        output[index] = (hex_value(pair[0])? << 4) | hex_value(pair[1])?;
    }
    Some(output)
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

fn now_seconds() -> Result<u64, Problem> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| unavailable("desktop-sso-handoff"))
}

fn invalid_route() -> Problem {
    Problem::new(
        "sso_callback_route_invalid",
        "The sign-in callback route is invalid",
        400,
        "desktop-sso-handoff",
    )
}

fn invalid_callback() -> Problem {
    Problem::new(
        "sso_callback_invalid",
        "The sign-in callback could not be verified",
        400,
        "desktop-sso-handoff",
    )
}

fn handoff_failed() -> Problem {
    Problem::new(
        "sso_callback_handoff_failed",
        "Could not return the sign-in callback to the app that started it",
        503,
        "desktop-sso-handoff",
    )
}

fn unavailable(correlation_id: &str) -> Problem {
    Problem::new(
        "sso_callback_handoff_unavailable",
        "Secure callback routing is unavailable on this device",
        503,
        correlation_id,
    )
}

#[cfg(test)]
mod tests {
    use super::{callback_state, route_record_name};

    #[test]
    fn accepts_only_single_well_formed_buglogin_sso_callback() {
        let state =
            callback_state("buglogin://sso/callback?state=opaque-state&token=provider-token")
                .unwrap();
        assert_eq!(state.as_str(), "opaque-state");
    }

    #[test]
    fn rejects_non_callback_duplicate_or_incomplete_callback_urls() {
        for callback in [
            "buglogin://invite/opaque-state",
            "buglogin://other/callback?state=s&token=t",
            "buglogin://sso/other?state=s&token=t",
            "buglogin://sso/callback?state=s&state=s2&token=t",
            "buglogin://sso/callback?state=s&token=t&token=t2",
            "buglogin://sso/callback?state=s",
            "buglogin://sso/callback?state=s&token=",
        ] {
            assert!(callback_state(callback).is_none(), "accepted {callback}");
        }
    }

    #[test]
    fn route_key_is_one_way_and_bounded_to_vault_component_size() {
        let route_name = route_record_name("state-that-must-not-be-stored-as-key");
        assert_eq!(route_name.len(), 64);
        assert!(!route_name.contains("state-that-must-not-be-stored-as-key"));
    }
}
