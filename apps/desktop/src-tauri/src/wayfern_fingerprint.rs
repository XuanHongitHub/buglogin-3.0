use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::TcpListener,
    path::Path,
    time::Duration,
};

use crate::clock::unix_time_nanos;
use buglogin_domain::{
    entities::{
        FingerprintOperatingSystem, PROFILE_RUNTIME_CONFIG_SCHEMA_VERSION,
        ProfileFingerprintTarget, WayfernProfileRuntimeConfig, validate_wayfern_fingerprint_target,
    },
    errors::Problem,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::{sleep, timeout};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use url::Url;

const CDP_TIMEOUT: Duration = Duration::from_secs(8);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const FINGERPRINT_PATH: &str = "BugLogin/fingerprint.json";

pub fn configure_local_cdp_args(
    args: &mut Vec<String>,
    correlation_id: &str,
) -> Result<u16, Problem> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|_| {
        problem(
            "wayfern_cdp_unavailable",
            "A local Wayfern control port could not be allocated",
            503,
            correlation_id,
        )
    })?;
    let port = listener
        .local_addr()
        .map_err(|_| {
            problem(
                "wayfern_cdp_unavailable",
                "A local Wayfern control port could not be allocated",
                503,
                correlation_id,
            )
        })?
        .port();
    drop(listener);

    args.retain(|arg| {
        !arg.starts_with("--remote-debugging-port=")
            && !arg.starts_with("--remote-debugging-address=")
    });
    args.push(format!("--remote-debugging-port={port}"));
    args.push("--remote-debugging-address=127.0.0.1".to_owned());
    Ok(port)
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredFingerprint {
    schema_version: u8,
    target: ProfileFingerprintTarget,
    fingerprint: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    identity_id: Option<String>,
}

pub fn target_from_runtime_config(
    runtime_config: Option<&Value>,
    correlation_id: &str,
) -> Result<Option<ProfileFingerprintTarget>, Problem> {
    let Some(value) = runtime_config else {
        return Ok(None);
    };
    if value.get("wayfern").is_none() {
        return Ok(None);
    }
    let config: WayfernProfileRuntimeConfig =
        serde_json::from_value(value.clone()).map_err(|_| {
            problem(
                "profile_runtime_config_invalid",
                "Wayfern profile configuration is invalid",
                409,
                correlation_id,
            )
        })?;
    if config.schema_version != PROFILE_RUNTIME_CONFIG_SCHEMA_VERSION {
        return Err(problem(
            "profile_runtime_config_version_unsupported",
            "Wayfern profile configuration version is unsupported",
            409,
            correlation_id,
        ));
    }
    let target = config.wayfern.fingerprint_target;
    validate_wayfern_fingerprint_target(&target).map_err(|error| {
        let (code, title, status) = error.problem_details();
        problem(code, title, status, correlation_id)
    })?;
    Ok(Some(target))
}

pub fn clear_stale_cdp_marker(profile_path: &Path, correlation_id: &str) -> Result<(), Problem> {
    let marker = profile_path.join("DevToolsActivePort");
    match fs::symlink_metadata(&marker) {
        Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => {
            fs::remove_file(marker).map_err(|_| {
                problem(
                    "wayfern_cdp_marker_cleanup_failed",
                    "Stale Wayfern control marker could not be removed",
                    500,
                    correlation_id,
                )
            })
        }
        Ok(_) => Err(problem(
            "wayfern_cdp_marker_invalid",
            "Wayfern control marker path is invalid",
            409,
            correlation_id,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(problem(
            "wayfern_cdp_marker_unavailable",
            "Wayfern control marker path is unavailable",
            500,
            correlation_id,
        )),
    }
}

pub async fn apply_or_generate(
    profile_path: &Path,
    target: &ProfileFingerprintTarget,
    runtime_version: &str,
    cdp_port: u16,
    correlation_id: &str,
) -> Result<(), Problem> {
    ensure_profile_directory(profile_path, correlation_id)?;
    let fingerprint_path = profile_path.join(FINGERPRINT_PATH);
    let stored = match fs::read(&fingerprint_path) {
        Ok(bytes) => Some(
            serde_json::from_slice::<StoredFingerprint>(&bytes).map_err(|_| {
                problem(
                    "profile_fingerprint_data_invalid",
                    "Stored profile fingerprint data is invalid",
                    409,
                    correlation_id,
                )
            })?,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => {
            return Err(problem(
                "profile_fingerprint_data_unavailable",
                "Stored profile fingerprint data cannot be read",
                500,
                correlation_id,
            ));
        }
    };

    let should_persist = stored.is_none();
    let (fingerprint, identity_id) = match stored {
        Some(stored) if stored.schema_version == 1 && stored.target == *target => {
            (stored.fingerprint, stored.identity_id)
        }
        Some(_) => {
            return Err(problem(
                "profile_fingerprint_target_changed",
                "The profile fingerprint target changed; regenerate it before launch",
                409,
                correlation_id,
            ));
        }
        None => generate(target, runtime_version, cdp_port, correlation_id).await?,
    };
    if identity_id.is_none() {
        validate_generated_target(&fingerprint, target, correlation_id)?;
    }

    let websocket_url = cdp_websocket_url(cdp_port, correlation_id).await?;
    let (method, params) = match identity_id.as_deref() {
        Some(identity_id) => (
            "Wayfern.setIdentity",
            json!({
                "identityId": identity_id,
                "operatingSystem": target.operating_system,
            }),
        ),
        None => ("Wayfern.setFingerprint", fingerprint.clone()),
    };
    let applied = cdp_command(&websocket_url, method, params)
        .await
        .map_err(|_| {
            problem(
                "wayfern_fingerprint_apply_failed",
                "Wayfern rejected the profile fingerprint",
                409,
                correlation_id,
            )
        })?;

    let readback = if let Some(identity_id) = identity_id.as_deref() {
        if applied
            .get("identityId")
            .and_then(Value::as_str)
            .is_some_and(|observed| observed != identity_id)
        {
            return Err(problem(
                "wayfern_identity_readback_mismatch",
                "Wayfern applied a different profile identity",
                409,
                correlation_id,
            ));
        }
        applied
    } else {
        cdp_command(&websocket_url, "Wayfern.getFingerprint", json!({}))
            .await
            .map_err(|_| {
                problem(
                    "wayfern_fingerprint_readback_failed",
                    "Wayfern did not confirm the applied profile fingerprint",
                    409,
                    correlation_id,
                )
            })?
    };
    let readback_fingerprint = readback
        .get("identity")
        .or_else(|| readback.get("fingerprint"))
        .cloned()
        .unwrap_or_else(|| readback.clone());
    if identity_id.is_none()
        || readback_fingerprint.get("userAgent").is_some()
        || readback_fingerprint.get("platform").is_some()
        || readback_fingerprint.get("operatingSystem").is_some()
    {
        validate_generated_target(&readback_fingerprint, target, correlation_id)?;
    }

    if should_persist {
        persist_fingerprint(
            profile_path,
            target,
            readback_fingerprint,
            identity_id,
            correlation_id,
        )?;
    }
    Ok(())
}

async fn generate(
    target: &ProfileFingerprintTarget,
    runtime_version: &str,
    cdp_port: u16,
    correlation_id: &str,
) -> Result<(Value, Option<String>), Problem> {
    let websocket_url = cdp_websocket_url(cdp_port, correlation_id).await?;
    let operating_system = serde_json::to_value(&target.operating_system).map_err(|_| {
        problem(
            "profile_fingerprint_target_invalid",
            "Fingerprint target is invalid",
            400,
            correlation_id,
        )
    })?;
    let (fingerprint, identity_id) = if supports_identity_api(runtime_version) {
        let result = cdp_command(
            &websocket_url,
            "Wayfern.createIdentity",
            json!({ "operatingSystem": operating_system }),
        )
        .await
        .map_err(|_| {
            problem(
                "wayfern_fingerprint_provider_unavailable",
                "Wayfern could not create a reproducible profile identity",
                409,
                correlation_id,
            )
        })?;
        let identity_id = result
            .get("identityId")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .map(str::to_owned)
            .ok_or_else(|| {
                problem(
                    "wayfern_identity_missing",
                    "Wayfern did not return a profile identity ID",
                    409,
                    correlation_id,
                )
            })?;
        let fingerprint = result
            .get("identity")
            .or_else(|| result.get("fingerprint"))
            .cloned()
            .unwrap_or(result);
        (fingerprint, Some(identity_id))
    } else {
        cdp_command(
            &websocket_url,
            "Wayfern.refreshFingerprint",
            json!({ "operatingSystem": operating_system }),
        )
        .await
        .map_err(|_| {
            problem(
                "wayfern_fingerprint_provider_unavailable",
                "The configured Wayfern runtime cannot generate this fingerprint target",
                409,
                correlation_id,
            )
        })?;
        let result = cdp_command(&websocket_url, "Wayfern.getFingerprint", json!({}))
            .await
            .map_err(|_| {
                problem(
                    "wayfern_fingerprint_readback_failed",
                    "Wayfern did not return its generated profile fingerprint",
                    409,
                    correlation_id,
                )
            })?;
        (result.get("fingerprint").cloned().unwrap_or(result), None)
    };
    if identity_id.is_none() {
        validate_generated_target(&fingerprint, target, correlation_id)?;
    }
    Ok((fingerprint, identity_id))
}

fn supports_identity_api(version: &str) -> bool {
    let major = version
        .split('.')
        .next()
        .and_then(|component| component.parse::<u32>().ok())
        .unwrap_or_default();
    major >= 151
}

async fn cdp_websocket_url(cdp_port: u16, correlation_id: &str) -> Result<String, Problem> {
    if cdp_port == 0 {
        return Err(problem(
            "wayfern_cdp_unavailable",
            "Wayfern local control port is invalid",
            503,
            correlation_id,
        ));
    }
    let endpoint = format!("http://127.0.0.1:{cdp_port}/json/list");
    let start = tokio::time::Instant::now();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .no_proxy()
        .build()
        .map_err(|_| {
            problem(
                "wayfern_cdp_unavailable",
                "Wayfern CDP unavailable",
                503,
                correlation_id,
            )
        })?;
    let mut endpoint_responded = false;
    loop {
        if let Ok(response) = client.get(&endpoint).send().await
            && response.status().is_success()
        {
            endpoint_responded = true;
            if let Ok(targets) = response.json::<Vec<Value>>().await
                && let Some(websocket_url) = targets
                    .iter()
                    .find_map(|target| target.get("webSocketDebuggerUrl").and_then(Value::as_str))
            {
                let parsed = Url::parse(websocket_url).map_err(|_| {
                    problem(
                        "wayfern_cdp_target_invalid",
                        "Wayfern returned an invalid CDP target",
                        502,
                        correlation_id,
                    )
                })?;
                if parsed.scheme() != "ws"
                    || !matches!(parsed.host_str(), Some("127.0.0.1" | "localhost"))
                    || parsed.port() != Some(cdp_port)
                {
                    return Err(problem(
                        "wayfern_cdp_target_invalid",
                        "Wayfern returned a non-local CDP target",
                        502,
                        correlation_id,
                    ));
                }
                return Ok(parsed.to_string());
            }
        }
        if start.elapsed() >= STARTUP_TIMEOUT {
            let (code, title) = if endpoint_responded {
                (
                    "wayfern_cdp_target_missing",
                    "Wayfern has no CDP page target",
                )
            } else {
                ("wayfern_cdp_unavailable", "Wayfern CDP unavailable")
            };
            return Err(problem(code, title, 503, correlation_id));
        }
        sleep(Duration::from_millis(100)).await;
    }
}

async fn cdp_command(url: &str, method: &str, params: Value) -> Result<Value, ()> {
    let (mut socket, _) = timeout(CDP_TIMEOUT, connect_async(url))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;
    let id = 1_u64;
    socket
        .send(Message::Text(
            json!({ "id": id, "method": method, "params": params })
                .to_string()
                .into(),
        ))
        .await
        .map_err(|_| ())?;
    loop {
        let message = timeout(CDP_TIMEOUT, socket.next())
            .await
            .map_err(|_| ())?
            .ok_or(())?
            .map_err(|_| ())?;
        match message {
            Message::Text(text) => {
                let value: Value = serde_json::from_str(&text).map_err(|_| ())?;
                if value.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }
                if value.get("error").is_some() {
                    return Err(());
                }
                return Ok(value.get("result").cloned().unwrap_or(Value::Null));
            }
            Message::Ping(data) => socket.send(Message::Pong(data)).await.map_err(|_| ())?,
            Message::Close(_) => return Err(()),
            _ => {}
        }
    }
}

fn validate_generated_target(
    fingerprint: &Value,
    target: &ProfileFingerprintTarget,
    correlation_id: &str,
) -> Result<(), Problem> {
    let version_matches = match target.operating_system_version.as_deref() {
        Some(expected) => fingerprint
            .get("platformVersion")
            .and_then(Value::as_str)
            .is_some_and(|actual| version_prefix_matches(actual, expected)),
        None => true,
    };
    let user_agent = fingerprint
        .get("userAgent")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let platform = fingerprint
        .get("platform")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let reported_os = fingerprint
        .get("operatingSystem")
        .or_else(|| fingerprint.get("os"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let operating_system_matches = match target.operating_system {
        FingerprintOperatingSystem::Windows => {
            user_agent.contains("Windows NT")
                || platform.eq_ignore_ascii_case("Win32")
                || reported_os == "windows"
        }
        FingerprintOperatingSystem::Macos => {
            user_agent.contains("Mac OS X")
                || platform.starts_with("Mac")
                || reported_os == "macos"
                || reported_os == "mac"
        }
        FingerprintOperatingSystem::Linux => {
            user_agent.contains("X11")
                || user_agent.contains("Linux")
                || platform.starts_with("Linux")
                || reported_os == "linux"
        }
        FingerprintOperatingSystem::Android => {
            user_agent.contains("Android")
                || platform.contains("Android")
                || reported_os == "android"
        }
        FingerprintOperatingSystem::Ios => {
            (user_agent.contains("iPhone") || user_agent.contains("iPad"))
                && (platform.contains("iPhone")
                    || platform.contains("iPad")
                    || reported_os == "ios")
                || (reported_os == "ios"
                    && (platform.contains("iPhone") || platform.contains("iPad")))
        }
    };
    if !fingerprint.is_object() || !version_matches || !operating_system_matches {
        return Err(problem(
            "wayfern_fingerprint_target_mismatch",
            "Wayfern generated a fingerprint that does not match the requested operating system version",
            409,
            correlation_id,
        ));
    }
    Ok(())
}

fn version_prefix_matches(actual: &str, expected: &str) -> bool {
    let actual = actual.replace('_', ".");
    actual == expected
        || actual
            .strip_prefix(expected)
            .is_some_and(|tail| tail.starts_with('.'))
}

fn persist_fingerprint(
    profile_path: &Path,
    target: &ProfileFingerprintTarget,
    fingerprint: Value,
    identity_id: Option<String>,
    correlation_id: &str,
) -> Result<(), Problem> {
    let directory = profile_path.join("BugLogin");
    fs::create_dir_all(&directory).map_err(|_| {
        problem(
            "profile_fingerprint_storage_failed",
            "Profile fingerprint could not be stored",
            500,
            correlation_id,
        )
    })?;
    let output = serde_json::to_vec(&StoredFingerprint {
        schema_version: 1,
        target: target.clone(),
        fingerprint,
        identity_id,
    })
    .map_err(|_| {
        problem(
            "profile_fingerprint_storage_failed",
            "Profile fingerprint could not be stored",
            500,
            correlation_id,
        )
    })?;
    let now = unix_time_nanos(correlation_id)?;
    let temporary = directory.join(format!(".fingerprint-{}-{now}.tmp", std::process::id()));
    let destination = directory.join("fingerprint.json");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|_| {
            problem(
                "profile_fingerprint_storage_failed",
                "Profile fingerprint could not be stored",
                500,
                correlation_id,
            )
        })?;
    file.write_all(&output)
        .and_then(|_| file.sync_all())
        .map_err(|_| {
            problem(
                "profile_fingerprint_storage_failed",
                "Profile fingerprint could not be stored",
                500,
                correlation_id,
            )
        })?;
    fs::rename(&temporary, &destination).map_err(|_| {
        let _ = fs::remove_file(&temporary);
        problem(
            "profile_fingerprint_storage_failed",
            "Profile fingerprint could not be stored",
            500,
            correlation_id,
        )
    })
}

fn ensure_profile_directory(profile_path: &Path, correlation_id: &str) -> Result<(), Problem> {
    let metadata = fs::symlink_metadata(profile_path).map_err(|_| {
        problem(
            "profile_fingerprint_storage_failed",
            "Profile data directory is unavailable",
            500,
            correlation_id,
        )
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(problem(
            "profile_fingerprint_storage_failed",
            "Profile data directory is invalid",
            409,
            correlation_id,
        ));
    }
    let directory = profile_path.join("BugLogin");
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(problem(
            "profile_fingerprint_storage_failed",
            "Profile fingerprint directory is invalid",
            409,
            correlation_id,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(problem(
            "profile_fingerprint_storage_failed",
            "Profile fingerprint directory is unavailable",
            500,
            correlation_id,
        )),
    }
}

fn problem(code: &'static str, title: &'static str, status: u16, correlation_id: &str) -> Problem {
    Problem::new(code, title, status, correlation_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_api_is_selected_for_wayfern_151_and_newer() {
        assert!(!supports_identity_api("150.9.9999.99"));
        assert!(supports_identity_api("151.0.0.0"));
        assert!(supports_identity_api("154.0.8037.57"));
        assert!(!supports_identity_api("unknown"));
    }

    #[test]
    fn identity_payload_can_be_validated_from_platform_without_user_agent() {
        let target = ProfileFingerprintTarget {
            operating_system: FingerprintOperatingSystem::Windows,
            operating_system_version: None,
        };
        validate_generated_target(&json!({"platform":"Win32"}), &target, "test").unwrap();
    }

    #[test]
    fn local_cdp_args_use_one_explicit_loopback_port() {
        let mut args = vec![
            "--remote-debugging-port=0".to_owned(),
            "--remote-debugging-port=9222".to_owned(),
            "--remote-debugging-address=0.0.0.0".to_owned(),
            "--lang=en-US".to_owned(),
        ];

        let port = configure_local_cdp_args(&mut args, "test-cdp").unwrap();

        assert_ne!(port, 0);
        assert_eq!(
            args.iter()
                .filter(|arg| arg.starts_with("--remote-debugging-port="))
                .count(),
            1
        );
        assert!(args.contains(&format!("--remote-debugging-port={port}")));
        assert_eq!(
            args.iter()
                .filter(|arg| arg.starts_with("--remote-debugging-address="))
                .count(),
            1
        );
        assert!(args.contains(&"--remote-debugging-address=127.0.0.1".to_owned()));
        assert!(args.contains(&"--lang=en-US".to_owned()));
    }

    #[tokio::test]
    async fn cdp_discovery_uses_the_explicit_port_without_devtools_marker() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let body =
            format!(r#"[{{"webSocketDebuggerUrl":"ws://127.0.0.1:{port}/devtools/page/test"}}]"#);
        let body_length = body.len();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {body_length}\r\nConnection: close\r\n\r\n{body}"
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });

        let websocket_url = cdp_websocket_url(port, "test-cdp").await.unwrap();

        assert_eq!(
            websocket_url,
            format!("ws://127.0.0.1:{port}/devtools/page/test")
        );
        server.await.unwrap();
    }
}
