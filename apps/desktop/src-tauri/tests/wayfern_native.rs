use std::{
    env, fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use buglogin_browser::runtime::RuntimeBinary;
use buglogin_desktop_app::adapter::{AdapterState, RuntimeTarget, RuntimeTargets};
use buglogin_storage::blob::checksum;
use serde_json::{Value, json};

#[tokio::test]
#[ignore = "requires an installed trusted Wayfern runtime; run explicitly with BUGLOGIN_WAYFERN_TEST_STRICT=1"]
async fn wayfern_native_profile_lifecycle() {
    if !cfg!(any(target_os = "windows", target_os = "macos")) {
        return;
    }
    let Some(binary) = env::var_os("BUGLOGIN_WAYFERN_TEST_BINARY") else {
        if env::var("BUGLOGIN_WAYFERN_TEST_STRICT").ok().as_deref() == Some("1") {
            panic!("BUGLOGIN_WAYFERN_TEST_BINARY is required in strict mode");
        }
        return;
    };
    let binary = PathBuf::from(binary);
    let bytes = fs::read(&binary).expect("read Wayfern binary");
    let version =
        env::var("BUGLOGIN_WAYFERN_TEST_VERSION").unwrap_or_else(|_| "154.0.8037.57".to_owned());
    let root = env::temp_dir().join(format!("buglogin-wayfern-native-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let repository = root.join("repository");
    let profiles = root.join("profiles");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind Wayfern observer");
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let (request_sender, request_receiver) = mpsc::channel();
    let (observer_stop_sender, observer_stop_receiver) = mpsc::channel();
    let observer = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(90);
        while Instant::now() < deadline && observer_stop_receiver.try_recv().is_err() {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
                    let mut request = vec![0; 16 * 1024];
                    let bytes = stream.read(&mut request).unwrap_or(0);
                    let request = String::from_utf8_lossy(&request[..bytes]);
                    if request.starts_with("GET /profile-check ") {
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
            wayfern: [(
                version.clone(),
                RuntimeTarget {
                    binary: RuntimeBinary {
                        path: binary,
                        version: version.clone(),
                        expected_checksum: checksum(&bytes),
                    },
                    extra_args: vec![
                        "--no-first-run".into(),
                        format!("http://{address}/profile-check"),
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
                "native-wayfern-create",
                Some("native-wayfern-create-idem"),
                json!({
                    "name":"Native Wayfern",
                    "runtime":"wayfern",
                    "runtimeVersion":version,
                    "fingerprintTarget":{"operatingSystem":"windows"}
                }),
            )
            .await
            .map_err(|error| format!("create profile: {error}"))?;
        let profile_id = created.data["id"]
            .as_str()
            .ok_or_else(|| "created profile ID is missing".to_owned())?
            .to_owned();
        let launched = state
            .invoke(
                "profile_launch",
                "native-wayfern-launch",
                Some("native-wayfern-launch-idem"),
                json!({"profileId": profile_id, "headless": false}),
            )
            .await
            .map_err(|error| format!("launch profile: {error}"))?;
        let operation = wait_operation(&state, launched.data["operationId"].clone()).await?;
        if operation["status"] != "completed" {
            return Err(format!("launch operation failed: {operation}"));
        }

        let deadline = Instant::now() + Duration::from_secs(45);
        let mut request_observed = false;
        while Instant::now() < deadline {
            match request_receiver.recv_timeout(Duration::from_millis(250)) {
                Ok(request) => {
                    request_observed |= request.starts_with("GET /profile-check ");
                    if request_observed {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        if !request_observed {
            return Err(
                "Wayfern did not open the loopback page from the profile launch".to_owned(),
            );
        }

        let stopped = state
            .invoke(
                "profile_stop",
                "native-wayfern-stop",
                Some("native-wayfern-stop-idem"),
                json!({"profileId": profile_id}),
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
        .map_err(|_| "Wayfern observer panicked".to_owned());
    let cleanup =
        fs::remove_dir_all(&root).map_err(|error| format!("remove temporary profile: {error}"));
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
                "native-wayfern-operation",
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
    Err("Wayfern operation did not complete".to_owned())
}
