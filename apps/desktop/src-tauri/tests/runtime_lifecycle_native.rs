//! Native start/stop lifecycle checks against a real Wayfern (Chromium) binary.
//! Opt-in: set BUGLOGIN_WAYFERN_LIFECYCLE_BINARY (and BUGLOGIN_WAYFERN_LIFECYCLE_VERSION);
//! BUGLOGIN_WAYFERN_LIFECYCLE_STRICT=1 makes a missing binary fail instead of skip.
//! Profiles live in a temporary directory and browsers run headless.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use buglogin_browser::runtime::RuntimeBinary;
use buglogin_desktop_app::adapter::{AdapterState, RuntimeTarget, RuntimeTargets};
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
    let root = env::temp_dir().join(format!("buglogin-wayfern-lifecycle-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
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
    let la = launch(&state, &a, "pair").await;
    let lb = launch(&state, &b, "pair").await;
    wait_for(|| !browser_pids(&a_path).is_empty() && !browser_pids(&b_path).is_empty()).await;
    let sa = stop(&state, &a, "pair").await;
    wait_for(|| browser_pids(&a_path).is_empty()).await;
    report.check(
        "stopping profile A keeps profile B running",
        la.is_ok() && lb.is_ok() && sa.is_ok() && !browser_pids(&b_path).is_empty(),
        format!(
            "a={la:?} b={lb:?} stopA={sa:?} bPids={:?}",
            browser_pids(&b_path)
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
    let accepted = state
        .invoke(
            "profile_launch",
            "lifecycle-launch",
            Some(&format!("lifecycle-launch-{profile_id}-{label}")),
            json!({"profileId": profile_id, "headless": true}),
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

fn process_lines(profile_path: &Path) -> Vec<(u32, String)> {
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

fn kill_leftovers(root: &Path) {
    for (pid, _) in process_lines(root) {
        let _ = Command::new("/bin/kill")
            .args(["-KILL", &pid.to_string()])
            .status();
    }
}
