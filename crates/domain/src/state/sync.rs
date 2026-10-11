use crate::errors::Problem;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncState {
    Idle,
    Checking,
    Uploading,
    Downloading,
    Conflict,
    Completed,
    Failed,
}

impl SyncState {
    pub fn transition(self, next: Self) -> Result<Self, Problem> {
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(Problem::invalid_transition(
                "sync",
                self.to_string(),
                next.to_string(),
            ))
        }
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Idle, Self::Checking)
                | (Self::Checking, Self::Uploading)
                | (Self::Checking, Self::Downloading)
                | (Self::Checking, Self::Conflict)
                | (Self::Checking, Self::Failed)
                | (Self::Uploading, Self::Completed)
                | (Self::Uploading, Self::Failed)
                | (Self::Downloading, Self::Completed)
                | (Self::Downloading, Self::Failed)
                | (Self::Conflict, Self::Checking)
                | (Self::Completed, Self::Checking)
                | (Self::Failed, Self::Checking)
        )
    }
}

impl fmt::Display for SyncState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Idle => "idle",
            Self::Checking => "checking",
            Self::Uploading => "uploading",
            Self::Downloading => "downloading",
            Self::Conflict => "conflict",
            Self::Completed => "completed",
            Self::Failed => "failed",
        })
    }
}
