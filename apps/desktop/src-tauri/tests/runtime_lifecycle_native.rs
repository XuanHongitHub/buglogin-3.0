//! Native start/stop lifecycle checks against a real Wayfern (Chromium) binary.
//! Opt-in: set BUGLOGIN_WAYFERN_LIFECYCLE_BINARY (and BUGLOGIN_WAYFERN_LIFECYCLE_VERSION);
//! BUGLOGIN_WAYFERN_LIFECYCLE_STRICT=1 makes a missing binary fail instead of skip.
//! Profiles live in a temporary directory; native lifecycle cases use headed
//! launches where browser process identity and user-close behavior matter.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use buglogin_browser::runtime::RuntimeBinary;
use buglogin_browser::{
    process::{ApprovedExecutable, SystemProcessController},
    runtime::LaunchSpecification,
};
use buglogin_desktop_app::adapter::{AdapterState, RuntimeTarget, RuntimeTargets};
use buglogin_desktop_app::profile_runtime::{LaunchRequest, spawn_profile_runtime};
use buglogin_domain::{entities::Runtime, ids::ProfileId};
use buglogin_storage::blob::checksum;
use serde_json::{Value, json};

const PROCESS_TIMEOUT: Duration = Duration::from_secs(20);

#[tokio::test]
async fn wayfern_native_start_stop_operations() {
    if !cfg!(any(target_os = "windows", target_os = "macos")) {
        return;
    }
    let Some(binary) = env::var_os("BUGLOGIN_WAYFERN_LIFECYCLE_BINARY") else {
        if env::var("BUGLOGIN_WAYFERN_LIFECYCLE_STRICT")
            .ok()
            .as_deref()
            == Some("1")
        {
            panic!("BUGLOGIN_WAYFERN_LIFECYCLE_BINARY is required in strict mode");
        }
        return;
    };
    let binary = PathBuf::from(binary);
    let version =
        env::var("BUGLOGIN_WAYFERN_LIFECYCLE_VERSION").unwrap_or_else(|_| "146.0.7680.177".into());
    let bytes = fs::read(&binary).expect("read Wayfern binary");
    let run_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must be after Unix epoch")
        .as_nanos();
    let root = env::temp_dir().join(format!(
        "buglogin-wayfern-lifecycle-{}-{run_id}",
        std::process::id()
    ));
    fs::create_dir(&root).expect("create isolated lifecycle test root");
    let repository = root.join("repository");
    let profiles = root.join("profiles");
    let targets = || RuntimeTargets {
        wayfern: [(
            version.clone(),
            RuntimeTarget {
                binary: RuntimeBinary {
                    path: binary.clone(),
                    version: version.clone(),
                    expected_checksum: checksum(&bytes),
                },
                extra_args: vec!["--no-first-run".into(), "about:blank".into()],
            },
        )]
        .into(),
    };
    let state = AdapterState::open_with_targets(repository.clone(), profiles.clone(), targets())
        .expect("open desktop adapter");
    let mut report = Report::default();

    let a = create_profile(&state, "Lifecycle A", &version).await;
    let b = create_profile(&state, "Lifecycle B", &version).await;
    let a_path = profiles.join(&a);
    let b_path = profiles.join(&b);

    // 1. launch -> stop releases processes and the profile lock.
    let launched = launch(&state, &a, "basic").await;
    report.check(
        "launch completes",
        launched.is_ok(),
        format!("{launched:?}"),
    );
    report.check(
        "browser processes start",
        wait_for(|| !browser_pids(&a_path).is_empty()).await,
        format!("pids={:?}", browser_pids(&a_path)),
    );
    let stopped = stop(&state, &a, "basic").await;
    report.check("stop completes", stopped.is_ok(), format!("{stopped:?}"));
    report.check(
        "all browser processes exit after stop",
        wait_for(|| browser_pids(&a_path).is_empty()).await,
        format!("leftover={:?}", browser_pids(&a_path)),
    );
    report.check(
        "profile lock released after stop",
        !a_path.join(".buglogin-profile.lock").exists(),
        String::new(),
    );

    // 2. relaunch straight after a stop.
    let relaunched = launch(&state, &a, "relaunch").await;
    report.check(
        "relaunch right after stop",
        relaunched.is_ok(),
        format!("{relaunched:?}"),
    );
    wait_for(|| !browser_pids(&a_path).is_empty()).await;

    // 3. a second launch while running is refused without disturbing the first.
    let before = main_pid(&a_path);
    let double = launch(&state, &a, "double").await;
    report.check(
        "second launch while running is refused",
        double.is_err(),
        format!("{double:?}"),
    );
    report.check(
        "running browser untouched by refused launch",
        before.is_some() && main_pid(&a_path) == before,
        format!("before={before:?} after={:?}", main_pid(&a_path)),
    );

    // 4. the user closes the browser window (process exits on its own).
    if let Some(pid) = main_pid(&a_path) {
        #[cfg(windows)]
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status();
        #[cfg(not(windows))]
        let _ = Command::new("/bin/kill")
            .args(["-TERM", &pid.to_string()])
            .status();
    }
    let exited = wait_for(|| browser_pids(&a_path).is_empty()).await;
    report.check("browser exits after window close", exited, String::new());
    let started = Instant::now();
    let mut after_close = launch(&state, &a, "after-close-0").await;
    let mut attempts = 1;
    while after_close.is_err() && started.elapsed() < Duration::from_secs(45) {
        tokio::time::sleep(Duration::from_secs(3)).await;
        attempts += 1;
        after_close = launch(&state, &a, &format!("after-close-{attempts}")).await;
    }
    report.check(
        "relaunch right after the user closed the browser",
        attempts == 1 && after_close.is_ok(),
        format!(
            "first attempt failed; recovered={} after {attempts} attempts / {:?}; last={after_close:?}",
            after_close.is_ok(),
            started.elapsed()
        ),
    );
    if after_close.is_ok() {
        let _ = stop(&state, &a, "after-close").await;
        wait_for(|| browser_pids(&a_path).is_empty()).await;
    }

    // 5. stopping a profile that is not running answers cleanly.
    let idle_stop = stop(&state, &a, "idle").await;
    report.check(
        "stop on an idle profile returns a clean error",
        idle_stop
            .as_ref()
            .err()
            .is_some_and(|error| !error.contains("timed out")),
        format!("{idle_stop:?}"),
    );

    // 6. rapid on/off cycles.
    let mut cycle_failures = Vec::new();
    for cycle in 0..5 {
        if let Err(error) = launch(&state, &a, &format!("cycle-{cycle}")).await {
            cycle_failures.push(format!("launch {cycle}: {error}"));
            continue;
        }
        if let Err(error) = stop(&state, &a, &format!("cycle-{cycle}")).await {
            cycle_failures.push(format!("stop {cycle}: {error}"));
        }
    }
    let leftover = wait_for(|| browser_pids(&a_path).is_empty()).await;
    report.check(
        "5 rapid launch/stop cycles",
        cycle_failures.is_empty() && leftover,
        format!(
            "failures={cycle_failures:?} leftover={:?}",
            browser_pids(&a_path)
        ),
    );

    // 7. two profiles at once: stopping one leaves the other running.
    let la = launch_with_headless(&state, &a, "pair", false).await;
    let a_root_pid = la
        .as_ref()
        .ok()
        .and_then(|operation| operation["result"]["processId"].as_u64())
        .and_then(|pid| u32::try_from(pid).ok());
    let a_helper_pid = profile_helper_pid(&a_path);
    let lb = launch_with_headless(&state, &b, "pair", false).await;
    let b_root_pid = lb
        .as_ref()
        .ok()
        .and_then(|operation| operation["result"]["processId"].as_u64())
        .and_then(|pid| u32::try_from(pid).ok());
    let b_helper_pid = profile_helper_pid(&b_path);
    let both_running = wait_for(|| {
        profile_tree_is_running(a_root_pid, a_helper_pid, &a_path)
            && profile_tree_is_running(b_root_pid, b_helper_pid, &b_path)
    })
    .await;
    let sa = stop(&state, &a, "pair").await;
    let a_stopped = wait_for(|| profile_tree_is_stopped(a_root_pid, a_helper_pid, &a_path)).await;
    thread::sleep(Duration::from_millis(500));
    let b_alive_after_stop = profile_tree_is_running(b_root_pid, b_helper_pid, &b_path);
    report.check(
        "stopping profile A keeps profile B running",
        both_running && a_stopped && la.is_ok() && lb.is_ok() && sa.is_ok() && b_alive_after_stop,
        format!(
            "both_running={both_running} a_stopped={a_stopped} b_alive_after_stop={b_alive_after_stop} a_root_pid={a_root_pid:?} b_root_pid={b_root_pid:?} a_helper_pid={a_helper_pid:?} b_helper_pid={b_helper_pid:?} stopA={sa:?} aAncestryBeforeB={:?} bAncestryBeforeStop={:?} aProcesses={:?} bProcesses={:?}",
            a_root_pid.map(process_ancestry),
            b_root_pid.map(process_ancestry),
            process_lines(&a_path),
            process_lines(&b_path),
        ),
    );

    // 8. app shutdown with a browser still open cleans up its processes.
    let shutdown = state.shutdown().await;
    let cleaned = wait_for(|| browser_pids(&b_path).is_empty()).await;
    report.check(
        "app shutdown closes running browsers",
        shutdown.is_ok() && cleaned,
        format!("shutdown={shutdown:?} leftover={:?}", browser_pids(&b_path)),
    );
    report.check(
        "profile locks released after app shutdown",
        !b_path.join(".buglogin-profile.lock").exists(),
        String::new(),
    );

    // 9. the app starts again (the old state is gone, like a new process) and
    // the same profile launches.
    drop(state);
    let reopened = match AdapterState::open_with_targets(repository, profiles.clone(), targets()) {
        Ok(reopened) => reopened,
        Err(error) => {
            report.check(
                "desktop adapter reopens after shutdown",
                false,
                format!("{error:?}"),
            );
            kill_leftovers(&root);
            let _ = fs::remove_dir_all(&root);
            report.finish();
            return;
        }
    };
    let after_restart = launch(&reopened, &b, "after-restart").await;
    report.check(
        "profile launches after an app restart",
        after_restart.is_ok(),
        format!("{after_restart:?}"),
    );
    let _ = reopened.shutdown().await;
    wait_for(|| browser_pids(&b_path).is_empty()).await;

    kill_leftovers(&root);
    let _ = fs::remove_dir_all(&root);
    report.finish();
}

#[tokio::test]
async fn wayfern_stopping_one_profile_keeps_another_running() {
    if !cfg!(any(target_os = "windows", target_os = "macos")) {
        return;
    }
    let Some(binary) = env::var_os("BUGLOGIN_WAYFERN_LIFECYCLE_BINARY") else {
        if env::var("BUGLOGIN_WAYFERN_LIFECYCLE_STRICT")
            .ok()
            .as_deref()
            == Some("1")
        {
            panic!("BUGLOGIN_WAYFERN_LIFECYCLE_BINARY is required in strict mode");
        }
        return;
    };
    let binary = PathBuf::from(binary);
    let version =
        env::var("BUGLOGIN_WAYFERN_LIFECYCLE_VERSION").unwrap_or_else(|_| "146.0.7680.177".into());
    let bytes = fs::read(&binary).expect("read Wayfern binary");
    let run_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must be after Unix epoch")
        .as_nanos();
    let root = env::temp_dir().join(format!(
        "buglogin-wayfern-parallel-{}-{run_id}",
        std::process::id()
    ));
    fs::create_dir(&root).expect("create isolated parallel test root");
    let repository = root.join("repository");
    let profiles = root.join("profiles");
    let targets = || RuntimeTargets {
        wayfern: [(
            version.clone(),
            RuntimeTarget {
                binary: RuntimeBinary {
                    path: binary.clone(),
                    version: version.clone(),
                    expected_checksum: checksum(&bytes),
                },
                extra_args: vec!["--no-first-run".into(), "about:blank".into()],
            },
        )]
        .into(),
    };
    let state = AdapterState::open_with_targets(repository, profiles.clone(), targets())
        .expect("open desktop adapter");
    let a = create_profile(&state, "Parallel A", &version).await;
    let b = create_profile(&state, "Parallel B", &version).await;
    let a_path = profiles.join(&a);
    let b_path = profiles.join(&b);
    let la = launch_with_headless(&state, &a, "parallel-isolation", false).await;
    let lb = launch_with_headless(&state, &b, "parallel-isolation", false).await;
    let a_root_pid = la
        .as_ref()
        .ok()
        .and_then(|operation| operation["result"]["processId"].as_u64())
        .and_then(|pid| u32::try_from(pid).ok());
    let a_helper_pid = profile_helper_pid(&a_path);
    let b_root_pid = lb
        .as_ref()
        .ok()
        .and_then(|operation| operation["result"]["processId"].as_u64())
        .and_then(|pid| u32::try_from(pid).ok());
    let b_helper_pid = profile_helper_pid(&b_path);
    let both_alive_before_stop = wait_for(|| {
        profile_tree_is_running(a_root_pid, a_helper_pid, &a_path)
            && profile_tree_is_running(b_root_pid, b_helper_pid, &b_path)
    })
    .await;
    let a_alive_immediately_before_stop =
        profile_tree_is_running(a_root_pid, a_helper_pid, &a_path);
    let b_alive_immediately_before_stop =
        profile_tree_is_running(b_root_pid, b_helper_pid, &b_path);
    let stop_a = stop(&state, &a, "parallel-isolation").await;
    let a_stopped = wait_for(|| profile_tree_is_stopped(a_root_pid, a_helper_pid, &a_path)).await;
    let b_alive_after_stop = profile_tree_is_running(b_root_pid, b_helper_pid, &b_path);
    let active_after_stop = state.active_profiles().await;
    let shutdown = state.shutdown().await;
    kill_leftovers(&root);
    let _ = fs::remove_dir_all(&root);
    assert!(
        both_alive_before_stop
            && a_alive_immediately_before_stop
            && b_alive_immediately_before_stop
            && a_stopped
            && stop_a.is_ok()
            && b_alive_after_stop
            && shutdown.is_ok(),
        "both_alive_before_stop={both_alive_before_stop} a_alive_immediately_before_stop={a_alive_immediately_before_stop} b_alive_immediately_before_stop={b_alive_immediately_before_stop} a_stopped={a_stopped} b_alive_after_stop={b_alive_after_stop} a_root_pid={a_root_pid:?} b_root_pid={b_root_pid:?} a_helper_pid={a_helper_pid:?} b_helper_pid={b_helper_pid:?} stop_a={stop_a:?} active_after_stop={active_after_stop:?} shutdown={shutdown:?}",
    );
}

#[tokio::test]
async fn profile_runtime_handle_stops_only_the_requested_wayfern_profile() {
    if !cfg!(any(target_os = "windows", target_os = "macos")) {
        return;
    }
    let Some(binary) = env::var_os("BUGLOGIN_WAYFERN_LIFECYCLE_BINARY") else {
        if env::var("BUGLOGIN_WAYFERN_LIFECYCLE_STRICT")
            .ok()
            .as_deref()
            == Some("1")
        {
            panic!("BUGLOGIN_WAYFERN_LIFECYCLE_BINARY is required in strict mode");
        }
        return;
    };
    let binary = PathBuf::from(binary)
        .canonicalize()
        .expect("canonical Wayfern binary");
    let root = env::temp_dir().join(format!(
        "buglogin-wayfern-runtime-handle-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let a_path = root.join("a");
    let b_path = root.join("b");
    fs::create_dir_all(&a_path).expect("create profile A path");
    fs::create_dir_all(&b_path).expect("create profile B path");
    let controller = SystemProcessController::new([ApprovedExecutable::new(&binary).unwrap()]);
    let runtime = spawn_profile_runtime(controller, None);
    let launch = |profile_id: &str, profile_path: &Path| LaunchRequest {
        profile_id: ProfileId::from_str(profile_id).unwrap(),
        idempotency_key: format!("native-{profile_id}"),
        profile_path: profile_path.to_path_buf(),
        specification: LaunchSpecification {
            runtime: Runtime::Wayfern,
            version: "native-test".into(),
            executable: binary.clone(),
            profile_path: profile_path.to_path_buf(),
            args: vec![
                format!("--user-data-dir={}", profile_path.display()),
                "--no-first-run".into(),
                "--no-default-browser-check".into(),
                "--headless".into(),
                format!("--wayfern-profile-label={profile_id}"),
                "--wayfern-profile-color=4fd123".into(),
                "about:blank".into(),
            ],
        },
        lease_scope: None,
        preacquired_lease: None,
        proxy_bridge: None,
    };
    let a_id = ProfileId::from_str("prf_native_runtime_a").unwrap();
    let b_id = ProfileId::from_str("prf_native_runtime_b").unwrap();
    let a = runtime
        .launch(launch(a_id.as_str(), &a_path))
        .await
        .expect("launch A");
    let b = runtime
        .launch(launch(b_id.as_str(), &b_path))
        .await
        .expect("launch B");
    let b_helper_pid = profile_helper_pid(&b_path);
    assert!(wayfern_process_exists(a.process_id, &a_path));
    assert!(wayfern_process_exists(b.process_id, &b_path));
    let b_tree_running_before_stop =
        profile_tree_is_running(Some(b.process_id), b_helper_pid, &b_path);
    runtime.stop(a_id).await.expect("stop A");
    thread::sleep(Duration::from_millis(500));
    let b_alive_after_stop = profile_tree_is_running(Some(b.process_id), b_helper_pid, &b_path);
    let b_processes_after_stop = process_lines(&b_path);
    let _ = runtime.stop(b_id).await;
    let _ = fs::remove_dir_all(&root);
    assert!(
        b_tree_running_before_stop && b_alive_after_stop,
        "ProfileRuntimeHandle stopping A lost B: root_pid={} root_alive={} helper_pid={b_helper_pid:?} tree_running_before_stop={b_tree_running_before_stop} tree_running_after_stop={b_alive_after_stop} after={b_processes_after_stop:?}",
        b.process_id,
        wayfern_process_exists(b.process_id, &b_path),
    );
}

#[derive(Default)]
struct Report {
    lines: Vec<String>,
    failures: usize,
}

impl Report {
    fn check(&mut self, name: &str, passed: bool, detail: String) {
        if !passed {
            self.failures += 1;
        }
        self.lines.push(format!(
            "{} {name}{}",
            if passed { "PASS" } else { "FAIL" },
            if passed || detail.is_empty() {
                String::new()
            } else {
                format!(" — {detail}")
            }
        ));
    }

    fn finish(self) {
        println!("{}", self.lines.join("\n"));
        assert_eq!(
            self.failures,
            0,
            "lifecycle checks failed:\n{}",
            self.lines.join("\n")
        );
    }
}

async fn create_profile(state: &AdapterState, name: &str, version: &str) -> String {
    let created = state
        .invoke(
            "profile_create",
            "lifecycle-create",
            Some(&format!(
                "lifecycle-create-{}",
                name.replace(char::is_whitespace, "-")
            )),
            json!({"name": name, "runtime": "wayfern", "runtimeVersion": version}),
        )
        .await
        .expect("create profile");
    created.data["id"].as_str().expect("profile id").to_owned()
}

async fn launch(state: &AdapterState, profile_id: &str, label: &str) -> Result<Value, String> {
    launch_with_headless(state, profile_id, label, false).await
}

async fn launch_with_headless(
    state: &AdapterState,
    profile_id: &str,
    label: &str,
    headless: bool,
) -> Result<Value, String> {
    let accepted = state
        .invoke(
            "profile_launch",
            "lifecycle-launch",
            Some(&format!("lifecycle-launch-{profile_id}-{label}")),
            json!({"profileId": profile_id, "headless": headless}),
        )
        .await
        .map_err(|error| format!("{}: {}", error.code, error.title))?;
    finished(state, accepted.data["operationId"].clone()).await
}

async fn stop(state: &AdapterState, profile_id: &str, label: &str) -> Result<Value, String> {
    let accepted = state
        .invoke(
            "profile_stop",
            "lifecycle-stop",
            Some(&format!("lifecycle-stop-{profile_id}-{label}")),
            json!({"profileId": profile_id}),
        )
        .await
        .map_err(|error| format!("{}: {}", error.code, error.title))?;
    finished(state, accepted.data["operationId"].clone()).await
}

async fn finished(state: &AdapterState, operation_id: Value) -> Result<Value, String> {
    for _ in 0..160 {
        let operation = state
            .invoke(
                "operation_get",
                "lifecycle-operation",
                None,
                json!({"operationId": operation_id}),
            )
            .await
            .map_err(|error| format!("{}: {}", error.code, error.title))?;
        match operation.data["status"].as_str() {
            Some("completed") => return Ok(operation.data),
            Some("failed" | "cancelled") => {
                return Err(format!(
                    "{}: {}",
                    operation.data["error"]["code"], operation.data["error"]["title"]
                ));
            }
            _ => tokio::time::sleep(Duration::from_millis(250)).await,
        }
    }
    Err("operation timed out".to_owned())
}

async fn wait_for(mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + PROCESS_TIMEOUT;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    condition()
}

/// Every process (browser and helpers) whose command line names the profile directory.
fn browser_pids(profile_path: &Path) -> Vec<u32> {
    process_lines(profile_path)
        .into_iter()
        .map(|(pid, _)| pid)
        .collect()
}

/// The browser process itself (not a helper/renderer).
fn main_pid(profile_path: &Path) -> Option<u32> {
    process_lines(profile_path)
        .into_iter()
        .find(|(_, command)| !command.contains("Helper") && !command.contains("--type="))
        .map(|(pid, _)| pid)
}

#[cfg(windows)]
fn process_exists(pid: u32) -> bool {
    Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("Get-Process -Id {pid} -ErrorAction Stop | Out-Null"),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(windows)]
fn wayfern_process_exists(pid: u32, profile_path: &Path) -> bool {
    let binary = env::var_os("BUGLOGIN_WAYFERN_LIFECYCLE_BINARY")
        .map(PathBuf::from)
        .and_then(|path| path.canonicalize().ok());
    let _profile_path = profile_path;
    let Some(binary) = binary else {
        return false;
    };
    let script = r#"
$process = Get-CimInstance Win32_Process -Filter "ProcessId = $env:BUGLOGIN_TEST_PID" -ErrorAction SilentlyContinue
if ($null -ne $process -and
    [String]::Equals($process.ExecutablePath, $env:BUGLOGIN_TEST_EXE, [StringComparison]::OrdinalIgnoreCase)) { exit 0 }
exit 1
"#;
    Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .env("BUGLOGIN_TEST_PID", pid.to_string())
        .env(
            "BUGLOGIN_TEST_EXE",
            binary.to_string_lossy().trim_start_matches(r"\\?\"),
        )
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(not(windows))]
fn wayfern_process_exists(pid: u32, profile_path: &Path) -> bool {
    process_lines(profile_path)
        .iter()
        .any(|(running_pid, _)| *running_pid == pid)
}

#[cfg(not(windows))]
fn process_exists(pid: u32) -> bool {
    Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(windows)]
fn process_ancestry(pid: u32) -> Vec<String> {
    let script = r#"
$current = [int]$env:BUGLOGIN_TEST_PID
$rows = @()
for ($i = 0; $i -lt 5 -and $current -gt 0; $i++) {
  $process = Get-CimInstance Win32_Process -Filter "ProcessId = $current" -ErrorAction SilentlyContinue
  if ($null -eq $process) { break }
  $rows += [pscustomobject]@{ pid = $process.ProcessId; parent = $process.ParentProcessId; name = $process.Name }
  $current = [int]$process.ParentProcessId
}
ConvertTo-Json -InputObject @($rows) -Compress
"#;
    let output = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .env("BUGLOGIN_TEST_PID", pid.to_string())
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_slice::<Value>(&output.stdout) else {
        return Vec::new();
    };
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| {
            format!(
                "pid={} parent={} name={}",
                row["pid"], row["parent"], row["name"]
            )
        })
        .collect()
}

#[cfg(not(windows))]
fn process_ancestry(_pid: u32) -> Vec<String> {
    Vec::new()
}

#[cfg(windows)]
fn profile_helper_pid(profile_path: &Path) -> Option<u32> {
    let lock = fs::read_to_string(profile_path.join(".buglogin-profile.lock")).ok()?;
    let token = lock.lines().nth(1)?;
    token
        .strip_prefix("tree-v1:")?
        .split(':')
        .next()?
        .parse()
        .ok()
}

#[cfg(windows)]
fn profile_tree_is_running(root_pid: Option<u32>, helper_pid: Option<u32>, path: &Path) -> bool {
    root_pid.is_some_and(|pid| wayfern_process_exists(pid, path))
        && helper_pid.is_some_and(process_exists)
}

#[cfg(windows)]
fn profile_tree_is_stopped(root_pid: Option<u32>, helper_pid: Option<u32>, _path: &Path) -> bool {
    root_pid.is_none_or(|pid| !process_exists(pid))
        && helper_pid.is_none_or(|pid| !process_exists(pid))
}

#[cfg(not(windows))]
fn profile_helper_pid(_profile_path: &Path) -> Option<u32> {
    None
}

#[cfg(not(windows))]
fn profile_tree_is_running(root_pid: Option<u32>, _helper_pid: Option<u32>, path: &Path) -> bool {
    root_pid.is_some_and(|pid| wayfern_process_exists(pid, path)) && !browser_pids(path).is_empty()
}

#[cfg(not(windows))]
fn profile_tree_is_stopped(_root_pid: Option<u32>, _helper_pid: Option<u32>, path: &Path) -> bool {
    browser_pids(path).is_empty()
}

fn process_lines(profile_path: &Path) -> Vec<(u32, String)> {
    #[cfg(windows)]
    {
        let output = Command::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                r#"
Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; using System.Text; public static class BugLoginPath { [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] public static extern uint GetShortPathName(string longPath, StringBuilder shortPath, uint size); }'
$buffer = New-Object System.Text.StringBuilder(1024)
[void][BugLoginPath]::GetShortPathName($env:BUGLOGIN_TEST_PROFILE, $buffer, 1024)
$longPath = $env:BUGLOGIN_TEST_PROFILE.ToLowerInvariant()
$shortPath = $buffer.ToString().ToLowerInvariant()
$extendedPrefix = '\\?\'
if ($longPath.StartsWith($extendedPrefix)) { $longPath = $longPath.Substring($extendedPrefix.Length) }
if ($shortPath.StartsWith($extendedPrefix)) { $shortPath = $shortPath.Substring($extendedPrefix.Length) }
Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -and ($_.CommandLine.ToLowerInvariant().Contains($longPath) -or ($shortPath -and $_.CommandLine.ToLowerInvariant().Contains($shortPath))) } | Select-Object ProcessId, ParentProcessId, CommandLine | ConvertTo-Json -Compress
"#,
            ])
            .env("BUGLOGIN_TEST_PROFILE", profile_path.canonicalize().unwrap_or_else(|_| profile_path.to_path_buf()))
            .output()
            .expect("list Windows processes");
        if !output.status.success() || output.stdout.is_empty() {
            return Vec::new();
        }
        let Ok(value) = serde_json::from_slice::<Value>(&output.stdout) else {
            return Vec::new();
        };
        let rows = value.as_array().cloned().unwrap_or_else(|| vec![value]);
        rows.into_iter()
            .filter_map(|row| {
                Some((
                    u32::try_from(row.get("ProcessId")?.as_u64()?).ok()?,
                    row.get("CommandLine")?.as_str()?.to_owned(),
                ))
            })
            .collect::<Vec<_>>()
    }

    #[cfg(not(windows))]
    {
        let output = Command::new("/bin/ps")
            .args(["-axww", "-o", "pid=,command="])
            .output()
            .expect("list processes");
        let needle = profile_path.to_string_lossy().into_owned();
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| line.contains(&needle) && !line.contains("/bin/ps"))
            .filter_map(|line| {
                let line = line.trim_start();
                let (pid, command) = line.split_once(' ')?;
                Some((pid.parse().ok()?, command.to_owned()))
            })
            .collect()
    }
}

fn kill_leftovers(root: &Path) {
    let profiles = root.join("profiles");
    let profile_paths = fs::read_dir(profiles)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    for pid in profile_paths
        .iter()
        .flat_map(|profile_path| process_lines(profile_path))
        .map(|(pid, _)| pid)
    {
        #[cfg(windows)]
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status();
        #[cfg(not(windows))]
        let _ = Command::new("/bin/kill")
            .args(["-KILL", &pid.to_string()])
            .status();
    }
}
