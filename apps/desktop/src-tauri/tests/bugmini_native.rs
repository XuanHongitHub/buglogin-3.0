use std::{
    env, fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use buglogin_browser::{
    cookies::{
        CookieInput, CookieSameSite,
        wayfern::{ProfileCookieState, materialize},
    },
    runtime::RuntimeBinary,
};
use buglogin_desktop_app::adapter::{AdapterState, RuntimeTarget, RuntimeTargets};
use buglogin_storage::{blob::checksum, secrets::SecretValue};
use serde_json::{Value, json};

#[tokio::test]
async fn bugmini_native_profile_lifecycle() {
    if !cfg!(target_os = "windows") {
        return;
    }
    let Some(binary) = env::var_os("BUGLOGIN_BUGMINI_TEST_BINARY") else {
        if env::var("BUGLOGIN_BUGMINI_TEST_STRICT").ok().as_deref() == Some("1") {
            panic!("BUGLOGIN_BUGMINI_TEST_BINARY is required in strict mode");
        }
        return;
    };
    let binary = PathBuf::from(binary);
    let bytes = fs::read(&binary).expect("read Bugmini binary");
    let root = env::temp_dir().join(format!("buglogin-bugmini-native-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let repository = root.join("repository");
    let profiles = root.join("profiles");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind cookie observer");
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let (request_sender, request_receiver) = mpsc::channel();
    let (observer_stop_sender, observer_stop_receiver) = mpsc::channel();
    let observer = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline && observer_stop_receiver.try_recv().is_err() {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
                    let mut request = vec![0; 16 * 1024];
                    let bytes = stream.read(&mut request).unwrap_or(0);
                    let request = String::from_utf8_lossy(&request[..bytes]);
                    if request.starts_with("GET ") {
                        let _ = request_sender.send(request.into_owned());
                    }
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK",
                    );
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(_) => break,
            }
        }
    });
    let state = match AdapterState::open_with_targets(
        repository,
        profiles.clone(),
        RuntimeTargets {
            wayfern: Default::default(),
            bugmini: [(
                "146.0.7680.177.4".to_owned(),
                RuntimeTarget {
                    binary: RuntimeBinary {
                        path: binary,
                        version: "146.0.7680.177.4".into(),
                        expected_checksum: checksum(&bytes),
                    },
                    extra_args: vec![
                        "--no-first-run".into(),
                        format!("http://{address}/cookie-check"),
                    ],
                },
            )]
            .into(),
        },
    ) {
        Ok(state) => state,
        Err(error) => {
            let _ = observer_stop_sender.send(());
            let _ = observer.join();
            let _ = fs::remove_dir_all(&root);
            panic!("open desktop adapter: {error}");
        }
    };

    let result: Result<(), String> = async {
        let created = state
            .invoke(
                "profile_create",
                "native-bugmini-create",
                Some("native-bugmini-create-idem"),
                json!({"name":"Native Bugmini","runtime":"bugmini","runtimeVersion":"146.0.7680.177.4"}),
            )
            .await
            .map_err(|error| format!("create profile: {error}"))?;
        let profile_id = created.data["id"]
            .as_str()
            .ok_or_else(|| "created profile ID is missing".to_owned())?
            .to_owned();
        let profile_path = profiles.join(&profile_id);
        materialize(
            &profile_path,
            ProfileCookieState::Stopped,
            &[CookieInput {
                name: "buglogin_migrated".into(),
                domain: "127.0.0.1".into(),
                path: "/".into(),
                secure: false,
                http_only: true,
                expires_at: None,
                same_site: CookieSameSite::Lax,
                value: SecretValue::new(b"session-ok".to_vec()).unwrap(),
            }],
            1_784_764_800,
        )
        .map_err(|error| format!("materialize migrated cookie: {error}"))?;
        let launched = state
            .invoke(
                "profile_launch",
                "native-bugmini-launch",
                Some("native-bugmini-launch-idem"),
                json!({"profileId": profile_id, "headless": true}),
            )
            .await
            .map_err(|error| format!("launch profile: {error}"))?;
        let operation = wait_operation(&state, launched.data["operationId"].clone()).await?;
        if operation["status"] != "completed" {
            return Err(format!("launch operation failed: {operation}"));
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut requests = Vec::new();
        let mut cookie_observed = false;
        while Instant::now() < deadline {
            match request_receiver.recv_timeout(Duration::from_millis(250)) {
                Ok(request) => {
                    cookie_observed |= request
                        .to_ascii_lowercase()
                        .contains("cookie: buglogin_migrated=session-ok");
                    requests.push(request);
                    if cookie_observed {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        if !cookie_observed {
            return Err(format!(
                "Bugmini did not send the materialized cookie; requests: {requests:#?}"
            ));
        }
        let stopped = state
            .invoke(
                "profile_stop",
                "native-bugmini-stop",
                Some("native-bugmini-stop-idem"),
                json!({"id": profile_id}),
            )
            .await
            .map_err(|error| format!("stop profile: {error}"))?;
        let stop_operation = wait_operation(&state, stopped.data["operationId"].clone()).await?;
        if stop_operation["status"] != "completed" {
            return Err(format!("stop operation failed: {stop_operation}"));
        }
        Ok(())
    }
    .await;

    let shutdown = state
        .shutdown()
        .await
        .map_err(|error| format!("shutdown desktop adapter: {error}"));
    let _ = observer_stop_sender.send(());
    let observer_result = observer
        .join()
        .map_err(|_| "cookie observer panicked".to_owned());
    let cleanup =
        fs::remove_dir_all(&root).map_err(|error| format!("remove temp profile: {error}"));
    if let Err(error) = result {
        panic!("{error}; shutdown={shutdown:?}; observer={observer_result:?}; cleanup={cleanup:?}");
    }
    shutdown.unwrap();
    observer_result.unwrap();
    cleanup.unwrap();
}

async fn wait_operation(state: &AdapterState, operation_id: Value) -> Result<Value, String> {
    for _ in 0..120 {
        let operation = state
            .invoke(
                "operation_get",
                "native-bugmini-operation",
                None,
                json!({"operationId": operation_id}),
            )
            .await
            .map_err(|error| format!("read operation: {error}"))?;
        if matches!(
            operation.data["status"].as_str(),
            Some("completed" | "failed")
        ) {
            return Ok(operation.data);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err("operation did not complete".to_owned())
}
