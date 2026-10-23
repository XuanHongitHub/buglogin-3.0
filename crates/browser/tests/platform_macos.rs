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
            Path::new("/bin/bash"),
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
    let executable = PathBuf::from("/bin/bash").canonicalize().unwrap();
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

#[test]
fn stopping_a_tree_whose_browser_already_exited_succeeds() {
    let pid_file = child_pid_file("self-exit");
    let controller = MacOsProcessGroupController::system();
    let mut tree = launch_tree(&controller, &pid_file);
    let child_pid = wait_for_pid_file(&pid_file);
    let root_pid = tree.identity().root.pid;
    let _ = Command::new("/bin/kill")
        .args(["-TERM", &root_pid.to_string()])
        .status();
    let _ = Command::new("/bin/kill")
        .args(["-TERM", &child_pid.to_string()])
        .status();
    wait_for_absent(root_pid);
    wait_for_absent(child_pid);

    let result = tree.stop();

    assert!(result.is_ok(), "stop after the browser exited: {result:?}");
    let _ = std::fs::remove_file(&pid_file);
}

#[test]
fn stop_lets_the_browser_finish_a_graceful_shutdown() {
    let marker = child_pid_file("graceful-marker");
    let controller = MacOsProcessGroupController::system().with_stop_policy(StopPolicy {
        graceful_timeout: Duration::from_secs(8),
        poll_interval: Duration::from_millis(50),
    });
    let mut tree = controller
        .launch(
            Path::new("/bin/bash"),
            &[
                "-c".into(),
                "trap 'trap \"\" TERM; end=$((SECONDS+4)); while [ $SECONDS -lt $end ]; do :; done; echo flushed > \"$1\"; exit 0' TERM; while true; do sleep 0.1; done".into(),
                "buglogin-graceful-test".into(),
                marker.display().to_string(),
            ],
        )
        .unwrap();
    thread::sleep(Duration::from_millis(300));

    tree.stop().unwrap();

    assert_eq!(
        std::fs::read_to_string(&marker).unwrap_or_default().trim(),
        "flushed",
        "the browser was killed before it finished shutting down"
    );
    let _ = std::fs::remove_file(&marker);
}
