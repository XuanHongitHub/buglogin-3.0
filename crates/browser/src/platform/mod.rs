use crate::process::ProcessIdentity;
use buglogin_domain::errors::Problem;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessTreeIdentity {
    pub root: ProcessIdentity,
    pub tree_id: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessTreeStatus {
    Running,
    Exited,
    Unknown,
}

pub trait OwnedProcessTree: Send {
    fn identity(&self) -> &ProcessTreeIdentity;
    fn status(&mut self) -> Result<ProcessTreeStatus, Problem>;
    fn stop(&mut self) -> Result<(), Problem>;
}

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;
