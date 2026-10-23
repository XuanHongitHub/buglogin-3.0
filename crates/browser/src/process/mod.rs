use buglogin_domain::errors::Problem;
use std::process::Child;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use std::process::Command;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

#[cfg(target_os = "macos")]
use crate::platform::macos::{MacOsProcessGroup, MacOsProcessGroupController};
#[cfg(target_os = "windows")]
use crate::platform::windows::{WindowsJobController, WindowsJobProcess};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use crate::platform::{OwnedProcessTree, ProcessTreeIdentity, ProcessTreeStatus};
use crate::runtime::LaunchSpecification;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub executable: PathBuf,
    pub token: String,
}
impl ProcessIdentity {
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn new(pid: u32, executable: PathBuf) -> Result<Self, Problem> {
        let mut token_bytes = [0_u8; 16];
        getrandom::fill(&mut token_bytes).map_err(|_| identity_unavailable())?;
        let token = token_bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Ok(Self {
            pid,
            executable,
            token,
        })
    }
    pub(crate) fn encode(&self) -> String {
        format!(
            "{}\n{}\n{}\n",
            self.pid,
            self.token,
            self.executable.display()
        )
    }
    pub(crate) fn decode(value: &str) -> Result<Self, Problem> {
        let mut lines = value.lines();
        let pid = lines
            .next()
            .and_then(|value| value.parse().ok())
            .ok_or_else(invalid_identity)?;
        let token = lines
            .next()
            .filter(|value| !value.is_empty())
            .ok_or_else(invalid_identity)?;
        let executable = PathBuf::from(
            lines
                .next()
                .filter(|value| !value.is_empty())
                .ok_or_else(invalid_identity)?,
        );
        Ok(Self {
            pid,
            executable,
            token: token.to_owned(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessStatus {
    Running,
    Exited,
    Unknown,
}

pub trait ManagedProcess: Send + Sync {
    fn identity(&self) -> &ProcessIdentity;
    fn status(&mut self) -> Result<ProcessStatus, Problem>;
    fn stop(&mut self) -> Result<(), Problem>;
}

impl<T: ManagedProcess + ?Sized> ManagedProcess for Box<T> {
    fn identity(&self) -> &ProcessIdentity {
        (**self).identity()
    }
    fn status(&mut self) -> Result<ProcessStatus, Problem> {
        (**self).status()
    }
    fn stop(&mut self) -> Result<(), Problem> {
        (**self).stop()
    }
}

pub trait ProcessController: Send {
    fn launch(
        &self,
        specification: &LaunchSpecification,
    ) -> Result<Box<dyn ManagedProcess>, Problem>;
    fn inspect(&self, identity: &ProcessIdentity) -> ProcessStatus;
    fn terminate(&self, identity: &ProcessIdentity) -> Result<(), Problem>;
    fn approve_executable(&self, _path: &Path) -> Result<(), Problem> {
        Err(invalid_executable())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovedExecutable(PathBuf);
impl ApprovedExecutable {
    pub fn new(path: &Path) -> Result<Self, Problem> {
        let path = path.canonicalize().map_err(|_| invalid_executable())?;
        if !path.is_file() {
            return Err(invalid_executable());
        }
        Ok(Self(path))
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

pub struct SystemProcessController {
    approved: Arc<RwLock<BTreeSet<PathBuf>>>,
}
impl SystemProcessController {
    pub fn new(approved: impl IntoIterator<Item = ApprovedExecutable>) -> Self {
        Self {
            approved: Arc::new(RwLock::new(
                approved.into_iter().map(|item| item.0).collect(),
            )),
        }
    }

    fn approved_executable(&self, specification: &LaunchSpecification) -> Result<PathBuf, Problem> {
        let executable = specification
            .executable
            .canonicalize()
            .map_err(|_| invalid_executable())?;
        if !self
            .approved
            .read()
            .map_err(|_| invalid_executable())?
            .contains(&executable)
        {
            return Err(invalid_executable());
        }
        if !specification.profile_path.is_absolute() {
            return Err(invalid_profile_path());
        }
        Ok(executable)
    }
}
impl ProcessController for SystemProcessController {
    fn launch(
        &self,
        specification: &LaunchSpecification,
    ) -> Result<Box<dyn ManagedProcess>, Problem> {
        let executable = self.approved_executable(specification)?;
        if specification.runtime == buglogin_domain::entities::Runtime::Wayfern {
            crate::runtime::wayfern::hide_new_tab_shortcuts(&specification.profile_path)?;
        }
        #[cfg(target_os = "windows")]
        return Ok(Box::new(PlatformManagedProcess::new(
            WindowsJobController::system()?.launch(
                &executable,
                &specification.args,
                specification.runtime == buglogin_domain::entities::Runtime::Wayfern,
                &specification.profile_path,
            )?,
        )));
        #[cfg(target_os = "macos")]
        return Ok(Box::new(PlatformManagedProcess::new(
            MacOsProcessGroupController::system().launch(&executable, &specification.args)?,
        )));
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let child = Command::new(&executable)
            .args(&specification.args)
            .spawn()
            .map_err(|_| spawn_failed())?;
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        Ok(Box::new(ProcessHandle::from_child(child, executable)?))
    }
    fn inspect(&self, identity: &ProcessIdentity) -> ProcessStatus {
        #[cfg(target_os = "windows")]
        return decode_tree_identity(identity)
            .and_then(|identity| {
                WindowsJobController::system()
                    .ok()
                    .map(|controller| (controller, identity))
            })
            .map(|(controller, identity)| map_tree_status(controller.inspect(&identity)))
            .unwrap_or(ProcessStatus::Unknown);
        #[cfg(target_os = "macos")]
        return decode_tree_identity(identity)
            .map(|identity| {
                map_tree_status(MacOsProcessGroupController::system().inspect(&identity))
            })
            .unwrap_or(ProcessStatus::Unknown);
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        ProcessStatus::Unknown
    }
    fn terminate(&self, identity: &ProcessIdentity) -> Result<(), Problem> {
        #[cfg(target_os = "windows")]
        return WindowsJobController::system()?
            .terminate(&decode_tree_identity(identity).ok_or_else(identity_unverified)?);
        #[cfg(target_os = "macos")]
        return MacOsProcessGroupController::system()
            .terminate(&decode_tree_identity(identity).ok_or_else(identity_unverified)?);
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        Err(identity_unverified())
    }

    fn approve_executable(&self, path: &Path) -> Result<(), Problem> {
        let approved = ApprovedExecutable::new(path)?;
        self.approved
            .write()
            .map_err(|_| invalid_executable())?
            .insert(approved.0);
        Ok(())
    }
}

#[cfg(target_os = "windows")]
type PlatformProcessTree = WindowsJobProcess;
#[cfg(target_os = "macos")]
type PlatformProcessTree = MacOsProcessGroup;

#[cfg(any(target_os = "macos", target_os = "windows"))]
struct PlatformManagedProcess {
    tree: PlatformProcessTree,
    identity: ProcessIdentity,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl PlatformManagedProcess {
    fn new(tree: PlatformProcessTree) -> Self {
        let tree_identity = tree.identity();
        Self {
            identity: ProcessIdentity {
                pid: tree_identity.root.pid,
                executable: tree_identity.root.executable.clone(),
                token: format!(
                    "tree-v1:{}:{}",
                    tree_identity.tree_id, tree_identity.root.token
                ),
            },
            tree,
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl ManagedProcess for PlatformManagedProcess {
    fn identity(&self) -> &ProcessIdentity {
        &self.identity
    }

    fn status(&mut self) -> Result<ProcessStatus, Problem> {
        self.tree.status().map(map_tree_status)
    }

    fn stop(&mut self) -> Result<(), Problem> {
        self.tree.stop()
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn decode_tree_identity(identity: &ProcessIdentity) -> Option<ProcessTreeIdentity> {
    let value = identity.token.strip_prefix("tree-v1:")?;
    let (tree_id, token) = value.split_once(':')?;
    Some(ProcessTreeIdentity {
        root: ProcessIdentity {
            pid: identity.pid,
            executable: identity.executable.clone(),
            token: token.to_owned(),
        },
        tree_id: tree_id.parse().ok()?,
    })
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn map_tree_status(status: ProcessTreeStatus) -> ProcessStatus {
    match status {
        ProcessTreeStatus::Running => ProcessStatus::Running,
        ProcessTreeStatus::Exited => ProcessStatus::Exited,
        ProcessTreeStatus::Unknown => ProcessStatus::Unknown,
    }
}

pub struct ProcessHandle {
    child: Child,
    identity: ProcessIdentity,
}
impl ProcessHandle {
    #[deprecated(note = "use ProcessController::launch with an approved executable")]
    pub fn spawn(_program: &str, _args: &[String]) -> Result<Self, Problem> {
        Err(invalid_executable())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn from_child(mut child: Child, executable: PathBuf) -> Result<Self, Problem> {
        let identity = match ProcessIdentity::new(child.id(), executable) {
            Ok(identity) => identity,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        Ok(Self { child, identity })
    }
    pub fn id(&self) -> u32 {
        self.identity.pid
    }
    pub fn identity(&self) -> &ProcessIdentity {
        &self.identity
    }
}
impl ManagedProcess for ProcessHandle {
    fn identity(&self) -> &ProcessIdentity {
        &self.identity
    }
    fn status(&mut self) -> Result<ProcessStatus, Problem> {
        self.child
            .try_wait()
            .map(|status| {
                if status.is_some() {
                    ProcessStatus::Exited
                } else {
                    ProcessStatus::Running
                }
            })
            .map_err(|_| status_failed())
    }
    fn stop(&mut self) -> Result<(), Problem> {
        if self.status()? == ProcessStatus::Running {
            self.child.kill().map_err(|_| stop_failed())?;
            self.child.wait().map_err(|_| stop_failed())?;
        }
        Ok(())
    }
}
fn invalid_identity() -> Problem {
    Problem::new(
        "process_identity_invalid",
        "Process identity is invalid",
        500,
        "browser",
    )
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn identity_unavailable() -> Problem {
    Problem::new(
        "process_identity_unavailable",
        "Process identity could not be generated",
        503,
        "process",
    )
    .retryable(true)
}
fn identity_unverified() -> Problem {
    Problem::new(
        "process_identity_unverified",
        "Process identity could not be verified",
        409,
        "browser",
    )
}
fn invalid_executable() -> Problem {
    Problem::new(
        "runtime_executable_unapproved",
        "Runtime executable is not approved",
        403,
        "browser",
    )
}
fn invalid_profile_path() -> Problem {
    Problem::new(
        "profile_path_invalid",
        "Profile path must be absolute",
        400,
        "browser",
    )
}
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn spawn_failed() -> Problem {
    Problem::new(
        "process_spawn_failed",
        "Process spawn failed",
        500,
        "browser",
    )
}
fn status_failed() -> Problem {
    Problem::new(
        "process_status_failed",
        "Process status failed",
        500,
        "browser",
    )
}
fn stop_failed() -> Problem {
    Problem::new("process_stop_failed", "Process stop failed", 500, "browser")
}
