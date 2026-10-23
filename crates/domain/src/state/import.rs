use crate::errors::Problem;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportState {
    Detected,
    Previewed,
    Copying,
    Validating,
    Completed,
    Partial,
    Failed,
}

impl ImportState {
    pub fn transition(self, next: Self) -> Result<Self, Problem> {
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(Problem::invalid_transition(
                "import",
                self.to_string(),
                next.to_string(),
            ))
        }
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Detected, Self::Previewed)
                | (Self::Detected, Self::Failed)
                | (Self::Previewed, Self::Copying)
                | (Self::Previewed, Self::Failed)
                | (Self::Copying, Self::Validating)
                | (Self::Copying, Self::Failed)
                | (Self::Validating, Self::Completed)
                | (Self::Validating, Self::Partial)
                | (Self::Validating, Self::Failed)
        )
    }
}

impl fmt::Display for ImportState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Detected => "detected",
            Self::Previewed => "previewed",
            Self::Copying => "copying",
            Self::Validating => "validating",
            Self::Completed => "completed",
            Self::Partial => "partial",
            Self::Failed => "failed",
        })
    }
}
