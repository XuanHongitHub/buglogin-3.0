use buglogin_browser::lifecycle::{LifecycleManager, LifecycleState};
use buglogin_browser::process::{
    ManagedProcess, ProcessController, ProcessHandle, ProcessIdentity, ProcessStatus,
};
use buglogin_browser::runtime::LaunchSpecification;
use buglogin_domain::{entities::Runtime, errors::Problem};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
struct FakeController {
    approved: PathBuf,
    processes: Arc<Mutex<HashMap<u32, (ProcessIdentity, ProcessStatus)>>>,
    fail_next_stop: Arc<AtomicBool>,
    managed_stop_attempts: Arc<AtomicUsize>,
    terminate_attempts: Arc<AtomicUsize>,
}
impl FakeController {
    fn new(approved: PathBuf) -> Self {
        Self {
            approved,
            processes: Arc::new(Mutex::new(HashMap::new())),
            fail_next_stop: Arc::new(AtomicBool::new(false)),
            managed_stop_attempts: Arc::new(AtomicUsize::new(0)),
            terminate_attempts: Arc::new(AtomicUsize::new(0)),
        }
    }
    fn fail_next_stop(&self) {
        self.fail_next_stop.store(true, Ordering::SeqCst);
    }
    fn mark_exited(&self, pid: u32) {
        self.processes.lock().unwrap().get_mut(&pid).unwrap().1 = ProcessStatus::Exited;
    }
}
struct FakeProcess {
    identity: ProcessIdentity,
    processes: Arc<Mutex<HashMap<u32, (ProcessIdentity, ProcessStatus)>>>,
    fail_next_stop: Arc<AtomicBool>,
    stop_attempts: Arc<AtomicUsize>,
}
impl ManagedProcess for FakeProcess {
    fn identity(&self) -> &ProcessIdentity {
        &self.identity
    }
    fn status(&mut self) -> Result<ProcessStatus, Problem> {
        Ok(self
            .processes
            .lock()
            .unwrap()
            .get(&self.identity.pid)
            .unwrap()
            .1)
    }
    fn stop(&mut self) -> Result<(), Problem> {
        self.stop_attempts.fetch_add(1, Ordering::SeqCst);
        if self.fail_next_stop.swap(false, Ordering::SeqCst) {
            return Err(Problem::new(
                "process_stop_failed",
                "Process stop failed",
                500,
                "test",
            ));
        }
        self.processes
            .lock()
            .unwrap()
            .get_mut(&self.identity.pid)
            .unwrap()
            .1 = ProcessStatus::Exited;
        Ok(())
    }
}
impl ProcessController for FakeController {
    fn launch(
        &self,
        specification: &LaunchSpecification,
    ) -> Result<Box<dyn ManagedProcess>, Problem> {
        if specification.executable != self.approved {
            return Err(Problem::new(
                "runtime_executable_unapproved",
                "Runtime executable is not approved",
                403,
                "test",
            ));
        }
        let identity = ProcessIdentity {
            pid: 42,
            executable: self.approved.clone(),
            token: "exact-start-token".into(),
        };
        self.processes
            .lock()
            .unwrap()
            .insert(identity.pid, (identity.clone(), ProcessStatus::Running));
        Ok(Box::new(FakeProcess {
            identity,
            processes: self.processes.clone(),
            fail_next_stop: self.fail_next_stop.clone(),
            stop_attempts: self.managed_stop_attempts.clone(),
        }))
    }
    fn inspect(&self, identity: &ProcessIdentity) -> ProcessStatus {
        self.processes
            .lock()
            .unwrap()
            .get(&identity.pid)
            .filter(|(known, _)| known == identity)
            .map(|(_, status)| *status)
            .unwrap_or(ProcessStatus::Unknown)
    }
    fn terminate(&self, identity: &ProcessIdentity) -> Result<(), Problem> {
        self.terminate_attempts.fetch_add(1, Ordering::SeqCst);
        if self.inspect(identity) != ProcessStatus::Running {
            return Err(Problem::new(
                "process_identity_unverified",
                "Process identity could not be verified",
                409,
                "test",
            ));
        }
        self.processes
            .lock()
            .unwrap()
            .get_mut(&identity.pid)
            .unwrap()
            .1 = ProcessStatus::Exited;
        Ok(())
    }
}

fn temp_profile(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("buglogin-lifecycle-{name}-{nonce}"));
    std::fs::create_dir_all(&path).unwrap();
    path.canonicalize().unwrap()
}
fn specification(executable: &Path, profile_path: &Path) -> LaunchSpecification {
    LaunchSpecification {
        runtime: Runtime::Wayfern,
        version: "test".into(),
        executable: executable.to_path_buf(),
        profile_path: profile_path.to_path_buf(),
        args: vec![],
    }
}

#[test]
fn stop_failure_retains_process_and_profile_lock() {
    let profile = temp_profile("stop-failure");
    let executable = profile.join("approved-runtime");
    std::fs::write(&executable, b"test").unwrap();
    let controller = FakeController::new(executable.clone());
    let mut manager = LifecycleManager::new(controller.clone());
    let spec = specification(&executable, &profile);
    manager.launch(&profile, &spec).unwrap();
    controller.fail_next_stop();

    assert_eq!(
        manager.stop(&profile).unwrap_err().code,
        "process_stop_failed"
    );
    assert!(profile.join(".buglogin-profile.lock").exists());
    assert_eq!(
        LifecycleManager::new(controller.clone())
            .launch(&profile, &spec)
            .unwrap_err()
            .code,
        "profile_already_locked"
    );

    manager.stop(&profile).unwrap();
    assert_eq!(controller.managed_stop_attempts.load(Ordering::SeqCst), 2);
    assert_eq!(controller.terminate_attempts.load(Ordering::SeqCst), 0);
    assert!(!profile.join(".buglogin-profile.lock").exists());
    std::fs::remove_dir_all(profile).unwrap();
}

#[test]
fn concurrent_managers_cannot_launch_same_profile() {
    let profile = temp_profile("concurrent-managers");
    let executable = profile.join("approved-runtime");
    std::fs::write(&executable, b"test").unwrap();
    let controller = FakeController::new(executable.clone());
    let mut first = LifecycleManager::new(controller.clone());
    let mut second = LifecycleManager::new(controller);
    let spec = specification(&executable, &profile);

    first.launch(&profile, &spec).unwrap();
    assert_eq!(
        second.launch(&profile, &spec).unwrap_err().code,
        "profile_already_locked"
    );

    first.stop(&profile).unwrap();
    std::fs::remove_dir_all(profile).unwrap();
}

#[test]
fn canonical_launch_locks_profile_and_stop_releases_it() {
    let profile = temp_profile("launch-stop");
    let executable = profile.join("approved-runtime");
    std::fs::write(&executable, b"test").unwrap();
    let controller = FakeController::new(executable.clone());
    let mut manager = LifecycleManager::new(controller);
    let spec = specification(&executable, &profile);

    let identity = manager.launch(&profile, &spec).unwrap();
    assert_eq!(identity.token, "exact-start-token");
    assert!(profile.join(".buglogin-profile.lock").exists());
    assert_eq!(
        manager.launch(&profile, &spec).unwrap_err().code,
        "profile_already_running"
    );
    manager.stop(&profile).unwrap();
    assert!(!profile.join(".buglogin-profile.lock").exists());
    std::fs::remove_dir_all(profile).unwrap();
}

#[test]
fn canonical_launch_rejects_unapproved_executable() {
    let profile = temp_profile("unapproved");
    let approved = profile.join("approved-runtime");
    let unapproved = profile.join("user-input.exe");
    std::fs::write(&approved, b"test").unwrap();
    std::fs::write(&unapproved, b"test").unwrap();
    let mut manager = LifecycleManager::new(FakeController::new(approved));

    let error = manager
        .launch(&profile, &specification(&unapproved, &profile))
        .unwrap_err();
    assert_eq!(error.code, "runtime_executable_unapproved");
    assert!(!profile.join(".buglogin-profile.lock").exists());
    std::fs::remove_dir_all(profile).unwrap();
}

#[test]
fn reconcile_cleans_exited_process_without_trusting_pid_alone() {
    let profile = temp_profile("reconcile");
    let executable = profile.join("approved-runtime");
    std::fs::write(&executable, b"test").unwrap();
    let controller = FakeController::new(executable.clone());
    let mut manager = LifecycleManager::new(controller.clone());
    let identity = manager
        .launch(&profile, &specification(&executable, &profile))
        .unwrap();
    controller.mark_exited(identity.pid);

    assert_eq!(
        manager.reconcile(),
        vec![(profile.clone(), LifecycleState::Ready)]
    );
    assert!(!profile.join(".buglogin-profile.lock").exists());
    std::fs::remove_dir_all(profile).unwrap();
}

#[test]
fn stale_lock_with_unverified_identity_remains_recovering() {
    let profile = temp_profile("identity");
    let executable = profile.join("approved-runtime");
    std::fs::write(&executable, b"test").unwrap();
    std::fs::write(
        profile.join(".buglogin-profile.lock"),
        format!("42\nwrong-token\n{}\n", executable.display()),
    )
    .unwrap();
    let manager = LifecycleManager::new(FakeController::new(executable));

    assert_eq!(
        manager.reconcile_profile(&profile).unwrap(),
        LifecycleState::Recovering
    );
    assert!(profile.join(".buglogin-profile.lock").exists());
    std::fs::remove_dir_all(profile).unwrap();
}

#[test]
fn direct_process_spawn_fails_closed() {
    #[allow(deprecated)]
    let error = match ProcessHandle::spawn("user-controlled.exe", &[]) {
        Ok(_) => panic!("direct spawn must fail closed"),
        Err(error) => error,
    };
    assert_eq!(error.code, "runtime_executable_unapproved");
}
