use crate::{errors::Problem, ids::AccountId};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityVerification {
    Pending,
    Verified,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountIdentity {
    pub account_id: AccountId,
    pub username: String,
    pub canonical_username: String,
    pub email: String,
    pub canonical_email: String,
    pub verification: IdentityVerification,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
}

impl AccountIdentity {
    pub fn new(
        account_id: AccountId,
        username: impl Into<String>,
        email: impl Into<String>,
        created_at: impl Into<String>,
    ) -> Result<Self, Problem> {
        let username = username.into();
        let email = email.into();
        let canonical_username = canonical_username(&username)?;
        let canonical_email = canonical_email(&email)?;
        let created_at = created_at.into();
        Ok(Self {
            account_id,
            username: username.trim().into(),
            canonical_username,
            email: email.trim().into(),
            canonical_email,
            verification: IdentityVerification::Pending,
            revision: 1,
            created_at: created_at.clone(),
            updated_at: created_at,
        })
    }

    pub fn verify(
        &self,
        expected_revision: u64,
        updated_at: impl Into<String>,
    ) -> Result<Self, Problem> {
        if self.revision != expected_revision {
            return Err(revision_conflict(expected_revision, self.revision));
        }
        if self.verification == IdentityVerification::Verified {
            return Ok(self.clone());
        }
        let mut next = self.clone();
        next.verification = IdentityVerification::Verified;
        next.revision = next_revision(self.revision)?;
        next.updated_at = updated_at.into();
        Ok(next)
    }
}

pub fn canonical_username(value: &str) -> Result<String, Problem> {
    canonical_identity_value(value, "username", 3, 64, false)
}

pub fn canonical_email(value: &str) -> Result<String, Problem> {
    let canonical = canonical_identity_value(value, "email", 3, 254, true)?;
    let (local, domain) = canonical
        .split_once('@')
        .ok_or_else(|| invalid_identity("email"))?;
    if local.is_empty()
        || domain.is_empty()
        || domain.starts_with('.')
        || domain.ends_with('.')
        || !domain.contains('.')
    {
        return Err(invalid_identity("email"));
    }
    Ok(canonical)
}

fn canonical_identity_value(
    value: &str,
    field: &str,
    min: usize,
    max: usize,
    allow_at: bool,
) -> Result<String, Problem> {
    let trimmed = value.trim();
    let length = trimmed.chars().count();
    if !(min..=max).contains(&length)
        || trimmed.chars().any(|character| {
            character.is_control() || character.is_whitespace() || (!allow_at && character == '@')
        })
    {
        return Err(invalid_identity(field));
    }
    Ok(trimmed.chars().flat_map(char::to_lowercase).collect())
}

fn invalid_identity(field: &str) -> Problem {
    Problem::new(
        format!("invalid_{field}"),
        format!("Invalid {field}"),
        400,
        "identity",
    )
}

fn revision_conflict(expected_revision: u64, actual_revision: u64) -> Problem {
    Problem::new("revision_conflict", "Revision conflict", 409, "domain")
        .with_field("expectedRevision", expected_revision)
        .with_field("actualRevision", actual_revision)
}

fn next_revision(revision: u64) -> Result<u64, Problem> {
    revision.checked_add(1).ok_or_else(|| {
        Problem::new("revision_exhausted", "Revision exhausted", 409, "domain")
            .with_field("actualRevision", revision)
    })
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountStatus {
    Active,
    Suspended,
    Disabled,
}

impl AccountStatus {
    pub fn transition(self, next: Self) -> Result<Self, Problem> {
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(Problem::invalid_transition(
                "account",
                self.to_string(),
                next.to_string(),
            ))
        }
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Active, Self::Suspended)
                | (Self::Active, Self::Disabled)
                | (Self::Suspended, Self::Active)
                | (Self::Suspended, Self::Disabled)
        )
    }
}

impl fmt::Display for AccountStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::Disabled => "disabled",
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: AccountId,
    pub status: AccountStatus,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
}

impl Account {
    pub fn new(id: AccountId, created_at: impl Into<String>) -> Self {
        let created_at = created_at.into();
        Self {
            id,
            status: AccountStatus::Active,
            revision: 1,
            created_at: created_at.clone(),
            updated_at: created_at,
        }
    }

    pub fn transition_status(
        &self,
        expected_revision: u64,
        next: AccountStatus,
        updated_at: impl Into<String>,
    ) -> Result<Self, Problem> {
        if self.revision != expected_revision {
            return Err(revision_conflict(expected_revision, self.revision));
        }

        let revision = next_revision(self.revision)?;

        Ok(Self {
            id: self.id.clone(),
            status: self.status.transition(next)?,
            revision,
            created_at: self.created_at.clone(),
            updated_at: updated_at.into(),
        })
    }
}
