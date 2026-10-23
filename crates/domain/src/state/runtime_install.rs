use crate::entities::RuntimeInstallState;
use crate::errors::Problem;
use std::fmt;

pub use crate::entities::RuntimeInstallState as RuntimeInstallLifecycle;

impl RuntimeInstallState {
    pub fn transition(self, next: Self) -> Result<Self, Problem> {
        if self.can_transition_to(&next) {
            Ok(next)
        } else {
            Err(Problem::invalid_transition(
                "runtime_install",
                self.to_string(),
                next.to_string(),
            ))
        }
    }

    pub fn can_transition_to(&self, next: &Self) -> bool {
        matches!(
            (self, next),
            (Self::Queued, Self::Downloading)
                | (Self::Queued, Self::Cancelled)
                | (Self::Queued, Self::Failed)
                | (Self::Downloading, Self::Verifying)
                | (Self::Downloading, Self::Cancelled)
                | (Self::Downloading, Self::Failed)
                | (Self::Verifying, Self::Installing)
                | (Self::Verifying, Self::Failed)
                | (Self::Installing, Self::Ready)
                | (Self::Installing, Self::Failed)
        )
    }
}

impl fmt::Display for RuntimeInstallState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Queued => "queued",
            Self::Downloading => "downloading",
            Self::Verifying => "verifying",
            Self::Installing => "installing",
            Self::Ready => "ready",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        })
    }
}
