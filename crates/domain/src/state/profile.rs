use crate::entities::ProfileState;
use crate::errors::Problem;
use std::fmt;

pub use crate::entities::ProfileState as ProfileLifecycle;

impl ProfileState {
    pub fn allows_proxy_change(&self) -> bool {
        matches!(self, Self::Created | Self::Ready | Self::Failed)
    }

    pub fn transition(self, next: Self) -> Result<Self, Problem> {
        if self.can_transition_to(&next) {
            Ok(next)
        } else {
            Err(Problem::invalid_transition(
                "profile",
                self.to_string(),
                next.to_string(),
            ))
        }
    }

    pub fn can_transition_to(&self, next: &Self) -> bool {
        matches!(
            (self, next),
            (Self::Created, Self::Ready)
                | (Self::Ready, Self::Launching)
                | (Self::Launching, Self::Running)
                | (Self::Launching, Self::Failed)
                | (Self::Running, Self::Stopping)
                | (Self::Running, Self::Failed)
                | (Self::Stopping, Self::Ready)
                | (Self::Stopping, Self::Failed)
                | (Self::Failed, Self::Recovering)
                | (Self::Recovering, Self::Ready)
                | (Self::Recovering, Self::Failed)
        )
    }
}

impl fmt::Display for ProfileState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Created => "created",
            Self::Ready => "ready",
            Self::Launching => "launching",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Failed => "failed",
            Self::Recovering => "recovering",
        })
    }
}
