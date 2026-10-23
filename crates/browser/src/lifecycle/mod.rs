use crate::process::{ManagedProcess, ProcessController, ProcessIdentity, ProcessStatus};
use crate::runtime::LaunchSpecification;
use buglogin_domain::errors::Problem;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const PROFILE_LOCK: &str = ".buglogin-profile.lock";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleState {
    Ready,
    Launching,
    Running,
    Stopping,
    Stopped,
    Failed,
    Recovering,
}

pub struct Lifecycle {
    state: LifecycleState,
    process: Option<Box<dyn ManagedProcess>>,
}
impl Lifecycle {
    pub fn new() -> Self {
        Self {
            state: LifecycleState::Ready,
            process: None,
        }
    }
    pub fn state(&self) -> LifecycleState {
        self.state
    }
    pub fn attach<P: ManagedProcess + 'static>(&mut self, process: P) -> Result<(), Problem> {
        if self.state != LifecycleState::Ready {
            return Err(invalid_state());
        }
        self.process = Some(Box::new(process));
        self.state = LifecycleState::Running;
        Ok(())
    }
    pub fn stop(&mut self) -> Result<(), Problem> {
        if self.state != LifecycleState::Running {
            return Err(invalid_state());
        }
        self.state = LifecycleState::Stopping;
        if let Some(process) = self.process.as_mut() {
            process.stop()?;
        }
        self.process = None;
        self.state = LifecycleState::Stopped;
        Ok(())
    }
}
impl Default for Lifecycle {
    fn default() -> Self {
        Self::new()
    }
}

struct ProfileLock {
    path: PathBuf,
    file: File,
}
impl ProfileLock {
    fn acquire(profile_path: &Path) -> Result<Self, Problem> {
        if !profile_path.is_absolute() || !profile_path.is_dir() {
            return Err(invalid_profile_path());
        }
        let path = profile_path.join(PROFILE_LOCK);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| already_locked())?;
        Ok(Self { path, file })
    }
    fn set_identity(&mut self, identity: &ProcessIdentity) -> Result<(), Problem> {
        self.file
            .write_all(identity.encode().as_bytes())
            .map_err(|_| lock_failed())?;
        self.file.sync_all().map_err(|_| lock_failed())
    }
}
impl Drop for ProfileLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

struct ActiveProcess {
    process: Box<dyn ManagedProcess>,
    _lock: ProfileLock,
}

pub struct LifecycleManager<C: ProcessController> {
    controller: C,
    active: HashMap<PathBuf, ActiveProcess>,
}
impl<C: ProcessController> LifecycleManager<C> {
    pub fn new(controller: C) -> Self {
        Self {
            controller,
            active: HashMap::new(),
        }
    }

    pub fn approve_executable(&self, path: &std::path::Path) -> Result<(), Problem> {
        self.controller.approve_executable(path)
    }

    pub fn launch(
        &mut self,
        profile_path: &Path,
        specification: &LaunchSpecification,
    ) -> Result<ProcessIdentity, Problem> {
        let key = profile_path.to_path_buf();
        if self.active.contains_key(&key) {
            return Err(already_running());
        }
        match self.reconcile_profile(profile_path)? {
            LifecycleState::Ready | LifecycleState::Stopped => {}
            LifecycleState::Recovering => return Err(identity_unverified()),
            // A verified live process owned by another manager holds the lock.
            LifecycleState::Running => return Err(already_locked()),
            _ => return Err(already_running()),
        }
        let mut lock = ProfileLock::acquire(profile_path)?;
        let mut process = self.controller.launch(specification)?;
        let identity = process.identity().clone();
        if let Err(error) = lock.set_identity(&identity) {
            let _ = process.stop();
            return Err(error);
        }
        self.active.insert(
            key,
            ActiveProcess {
                process,
                _lock: lock,
            },
        );
        Ok(identity)
    }

    pub fn stop(&mut self, profile_path: &Path) -> Result<(), Problem> {
        if let Some(active) = self.active.get_mut(profile_path) {
            active.process.stop()?;
            self.active.remove(profile_path);
            return Ok(());
        }
        let lock_path = profile_path.join(PROFILE_LOCK);
        let identity = read_identity(&lock_path)?;
        if self.controller.inspect(&identity) != ProcessStatus::Running {
            return Err(identity_unverified());
        }
        self.controller.terminate(&identity)?;
        std::fs::remove_file(lock_path).map_err(|_| lock_failed())
    }

    pub fn reconcile(&mut self) -> Vec<(PathBuf, LifecycleState)> {
        let mut changed = Vec::new();
        self.active.retain(|path, active| {
            match active.process.status().unwrap_or(ProcessStatus::Unknown) {
                ProcessStatus::Running => true,
                ProcessStatus::Exited => {
                    changed.push((path.clone(), LifecycleState::Ready));
                    false
                }
                ProcessStatus::Unknown => {
                    changed.push((path.clone(), LifecycleState::Recovering));
                    true
                }
            }
        });
        changed
    }

    pub fn reconcile_profile(&self, profile_path: &Path) -> Result<LifecycleState, Problem> {
        if self.active.contains_key(profile_path) {
            return Ok(LifecycleState::Running);
        }
        let lock_path = profile_path.join(PROFILE_LOCK);
        if !lock_path.exists() {
            return Ok(LifecycleState::Ready);
        }
        let identity = read_identity(&lock_path)?;
        match self.controller.inspect(&identity) {
            ProcessStatus::Running => Ok(LifecycleState::Running),
            ProcessStatus::Unknown => Ok(LifecycleState::Recovering),
            ProcessStatus::Exited => {
                std::fs::remove_file(lock_path).map_err(|_| lock_failed())?;
                Ok(LifecycleState::Ready)
            }
        }
    }

    pub fn process_is_verified_running(&mut self, profile_path: &Path) -> bool {
        self.active
            .get_mut(profile_path)
            .is_some_and(|active| matches!(active.process.status(), Ok(ProcessStatus::Running)))
    }
}

fn read_identity(path: &Path) -> Result<ProcessIdentity, Problem> {
    let value = std::fs::read_to_string(path).map_err(|_| lock_failed())?;
    ProcessIdentity::decode(&value)
}
fn invalid_state() -> Problem {
    Problem::new(
        "invalid_lifecycle_state",
        "Invalid lifecycle state",
        409,
        "browser",
    )
}
fn invalid_profile_path() -> Problem {
    Problem::new(
        "profile_path_invalid",
        "Profile path must be an existing absolute directory",
        400,
        "browser",
    )
}
fn already_locked() -> Problem {
    Problem::new(
        "profile_already_locked",
        "Profile is already locked",
        409,
        "browser",
    )
}
fn already_running() -> Problem {
    Problem::new(
        "profile_already_running",
        "Profile is already running",
        409,
        "browser",
    )
}
fn identity_unverified() -> Problem {
    Problem::new(
        "process_identity_unverified",
        "Process identity could not be verified",
        409,
        "browser",
    )
}
fn lock_failed() -> Problem {
    Problem::new(
        "profile_lock_failed",
        "Profile lock operation failed",
        500,
        "browser",
    )
}
