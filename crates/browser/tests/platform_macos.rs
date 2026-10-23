#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use buglogin_browser::platform::macos::{MacOsProcessGroupController, StopPolicy};
use buglogin_browser::platform::{OwnedProcessTree, ProcessTreeStatus};
use buglogin_browser::process::{
    ApprovedExecutable, ProcessController, ProcessStatus, SystemProcessController,
};
use buglogin_browser::runtime::LaunchSpecification;
use buglogin_domain::entities::Runtime;

static FILE_COUNTER: AtomicU64 = AtomicU64::new(1);

fn child_pid_file(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "buglogin-platform-{name}-{}-{}.pid",
        std::process::id(),
        FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

fn launch_tree(controller: &MacOsProcessGroupController, pid_file: &Path) -> impl OwnedProcessTree {
    controller
        .launch(
            Path::new("/bin/sh"),
            &[
                "-c".into(),
                "sleep 60 & echo $! > \"$1\"; wait".into(),
                "buglogin-platform-test".into(),
                pid_file.display().to_string(),
            ],
        )
        .unwrap()
}

fn wait_for_pid_file(path: &Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(value) = std::fs::read_to_string(path) {
            return value.trim().parse().unwrap();
        }
        assert!(Instant::now() < deadline, "child PID file was not written");
        thread::sleep(Duration::from_millis(50));
    }
}

fn process_exists(pid: u32) -> bool {
    Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
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
fn process_group_stop_cleans_descendants() {
    let pid_file = child_pid_file("stop");
    let controller = MacOsProcessGroupController::system().with_stop_policy(StopPolicy {
        graceful_timeout: Duration::from_millis(250),
        poll_interval: Duration::from_millis(20),
    });
    let mut tree = launch_tree(&controller, &pid_file);
    let child_pid = wait_for_pid_file(&pid_file);
    assert_eq!(tree.status().unwrap(), ProcessTreeStatus::Running);
    tree.stop().unwrap();
    wait_for_absent(child_pid);
    let _ = std::fs::remove_file(&pid_file);
}

#[test]
fn identity_mismatch_fails_closed() {
    let pid_file = child_pid_file("identity");
    let controller = MacOsProcessGroupController::system();
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
fn system_controller_uses_owned_process_group() {
    let pid_file = child_pid_file("controller");
    let executable = PathBuf::from("/bin/sh").canonicalize().unwrap();
    let controller = SystemProcessController::new([ApprovedExecutable::new(&executable).unwrap()]);
    let specification = LaunchSpecification {
        runtime: Runtime::Wayfern,
        version: "test".into(),
        executable,
        profile_path: std::env::temp_dir(),
        args: vec![
            "-c".into(),
            "sleep 60 & echo $! > \"$1\"; wait".into(),
            "buglogin-platform-test".into(),
            pid_file.display().to_string(),
        ],
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
