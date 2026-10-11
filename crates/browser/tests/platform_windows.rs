#![cfg(target_os = "windows")]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use buglogin_browser::platform::windows::{StopPolicy, WindowsJobController};
use buglogin_browser::platform::{OwnedProcessTree, ProcessTreeStatus};
use buglogin_browser::process::{
    ApprovedExecutable, ProcessController, ProcessStatus, SystemProcessController,
};
use buglogin_browser::runtime::LaunchSpecification;
use buglogin_domain::entities::Runtime;

static FILE_COUNTER: AtomicU64 = AtomicU64::new(1);

fn powershell() -> PathBuf {
    PathBuf::from(std::env::var_os("SystemRoot").unwrap())
        .join("System32/WindowsPowerShell/v1.0/powershell.exe")
}

fn child_pid_file(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "buglogin-platform-{name}-{}-{}.pid",
        std::process::id(),
        FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

fn child_args(pid_file: &Path) -> Vec<String> {
    let path = pid_file.display().to_string().replace('\'', "''");
    let script = format!("Set-Content -LiteralPath '{path}' -Value $PID; Start-Sleep -Seconds 60");
    let output = Command::new(powershell())
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($env:BUGLOGIN_TEST_SCRIPT))",
        ])
        .env("BUGLOGIN_TEST_SCRIPT", script)
        .output()
        .unwrap();
    assert!(output.status.success());
    vec![
        "-NoProfile".into(),
        "-NonInteractive".into(),
        "-EncodedCommand".into(),
        String::from_utf8(output.stdout).unwrap().trim().to_owned(),
    ]
}

fn launch_tree(controller: &WindowsJobController, pid_file: &Path) -> impl OwnedProcessTree {
    controller
        .launch(&powershell(), &child_args(pid_file))
        .unwrap()
}

fn wait_for_pid_file(path: &Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(value) = std::fs::read_to_string(path) {
            return value.trim().parse().unwrap();
        }
        assert!(Instant::now() < deadline, "child PID file was not written");
        thread::sleep(Duration::from_millis(50));
    }
}

fn process_exists(pid: u32) -> bool {
    Command::new(powershell())
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("Get-Process -Id {pid} -ErrorAction Stop | Out-Null"),
        ])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn wait_for_absent(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while process_exists(pid) {
        assert!(
            Instant::now() < deadline,
            "descendant process leaked: {pid}"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn job_launch_preserves_argument_boundaries() {
    let output = child_pid_file("argv-output").with_extension("json");
    let script = child_pid_file("argv-script").with_extension("ps1");
    let escaped_output = output.display().to_string().replace('\'', "''");
    std::fs::write(
        &script,
        format!(
            "ConvertTo-Json -Compress -InputObject @($args) | Set-Content -LiteralPath '{escaped_output}'; Start-Sleep -Seconds 60"
        ),
    )
    .unwrap();
    let expected = vec![
        "--user-data-dir=C:\\path with space\\profile".to_owned(),
        "http://127.0.0.1:43210/cookie-check".to_owned(),
        "--headless".to_owned(),
    ];
    let mut args = vec![
        "-NoProfile".to_owned(),
        "-NonInteractive".to_owned(),
        "-File".to_owned(),
        script.display().to_string(),
    ];
    args.extend(expected.iter().cloned());
    let controller = WindowsJobController::system().unwrap();
    let tree = controller.launch(&powershell(), &args).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !output.exists() {
        assert!(Instant::now() < deadline, "argument output was not written");
        thread::sleep(Duration::from_millis(50));
    }
    let actual: Vec<String> = serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();

    assert_eq!(actual, expected);
    drop(tree);
    let _ = std::fs::remove_file(script);
    let _ = std::fs::remove_file(output);
}

#[test]
fn job_stop_cleans_the_owned_process_tree() {
    let pid_file = child_pid_file("stop");
    let controller = WindowsJobController::system()
        .unwrap()
        .with_stop_policy(StopPolicy {
            graceful_timeout: Duration::from_millis(250),
            poll_interval: Duration::from_millis(20),
        });
    let mut tree = launch_tree(&controller, &pid_file);
    let child_pid = wait_for_pid_file(&pid_file);

    assert_eq!(tree.status().unwrap(), ProcessTreeStatus::Running);
    tree.stop().unwrap();
    assert_eq!(tree.status().unwrap(), ProcessTreeStatus::Exited);
    wait_for_absent(child_pid);
    let _ = std::fs::remove_file(&pid_file);
}

#[test]
fn dropping_job_handle_cleans_descendants() {
    let pid_file = child_pid_file("drop");
    let controller = WindowsJobController::system().unwrap();
    let child_pid = {
        let tree = launch_tree(&controller, &pid_file);
        let child_pid = wait_for_pid_file(&pid_file);
        assert_eq!(
            controller.inspect(tree.identity()),
            ProcessTreeStatus::Running
        );
        child_pid
    };

    wait_for_absent(child_pid);
    let _ = std::fs::remove_file(&pid_file);
}

#[test]
fn identity_mismatch_fails_closed() {
    let pid_file = child_pid_file("identity");
    let controller = WindowsJobController::system().unwrap();
    let tree = launch_tree(&controller, &pid_file);
    let child_pid = wait_for_pid_file(&pid_file);
    let mut identity = tree.identity().clone();
    identity.root.token.push_str("-wrong");

    assert_eq!(controller.inspect(&identity), ProcessTreeStatus::Unknown);
    let error = controller.terminate(&identity).unwrap_err();
    assert_eq!(error.code, "process_identity_unverified");
    drop(tree);
    wait_for_absent(child_pid);
    let _ = std::fs::remove_file(&pid_file);
}

#[test]
fn invalid_temp_prevents_unowned_process_launch() {
    const CHILD_MODE: &str = "BUGLOGIN_INVALID_TEMP_CHILD";
    const ENCODED_COMMAND: &str = "BUGLOGIN_INVALID_TEMP_COMMAND";
    const PID_FILE: &str = "BUGLOGIN_INVALID_TEMP_PID_FILE";

    if std::env::var_os(CHILD_MODE).is_some() {
        let pid_file = PathBuf::from(std::env::var_os(PID_FILE).unwrap());
        let executable = powershell().canonicalize().unwrap();
        let controller =
            SystemProcessController::new([ApprovedExecutable::new(&executable).unwrap()]);
        let specification = LaunchSpecification {
            runtime: Runtime::Wayfern,
            version: "test".into(),
            executable,
            profile_path: std::env::current_dir().unwrap(),
            args: vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-EncodedCommand".into(),
                std::env::var(ENCODED_COMMAND).unwrap(),
            ],
        };
        let error = match controller.launch(&specification) {
            Ok(_) => panic!("launch unexpectedly succeeded with invalid TEMP"),
            Err(error) => error,
        };

        assert_eq!(error.code, "process_tree_launch_failed");
        assert!(!pid_file.exists());
        return;
    }

    let invalid_temp = child_pid_file("invalid-temp-root");
    let pid_file = child_pid_file("invalid-temp-child");
    std::fs::write(&invalid_temp, b"not a directory").unwrap();
    let encoded_command = child_args(&pid_file).pop().unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "invalid_temp_prevents_unowned_process_launch",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_MODE, "1")
        .env(ENCODED_COMMAND, encoded_command)
        .env(PID_FILE, &pid_file)
        .env("TEMP", &invalid_temp)
        .env("TMP", &invalid_temp)
        .status()
        .unwrap();

    assert!(status.success());
    assert!(!pid_file.exists());
    let _ = std::fs::remove_file(&invalid_temp);
}

#[test]
fn system_controller_uses_job_owned_process_tree() {
    let pid_file = child_pid_file("controller");
    let executable = powershell().canonicalize().unwrap();
    let controller = SystemProcessController::new([ApprovedExecutable::new(&executable).unwrap()]);
    let specification = LaunchSpecification {
        runtime: Runtime::Wayfern,
        version: "test".into(),
        executable,
        profile_path: std::env::temp_dir(),
        args: child_args(&pid_file),
    };
    let mut process = controller.launch(&specification).unwrap();
    let child_pid = wait_for_pid_file(&pid_file);

    assert_eq!(
        controller.inspect(process.identity()),
        ProcessStatus::Running
    );
    process.stop().unwrap();
    assert_eq!(process.status().unwrap(), ProcessStatus::Exited);
    wait_for_absent(child_pid);
    let _ = std::fs::remove_file(&pid_file);
}
