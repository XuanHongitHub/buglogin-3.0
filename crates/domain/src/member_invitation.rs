use crate::{
    account::canonical_email,
    errors::Problem,
    ids::{AccountId, MemberId, MemberInvitationId, WorkspaceId},
    member::MemberRole,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberInvitationState {
    Issued,
    Redeemed,
    Revoked,
}

/// Durable workspace invitation. The bearer code is never stored here; only its
/// SHA-256 digest is persisted and compared by the Rust API.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemberInvitation {
    pub id: MemberInvitationId,
    pub workspace_id: WorkspaceId,
    pub canonical_email: String,
    pub role: MemberRole,
    pub expires_at: u64,
    pub created_by: AccountId,
    pub token_hash: String,
    pub state: MemberInvitationState,
    pub redeemed_by: Option<AccountId>,
    pub member_id: Option<MemberId>,
    pub revision: u64,
}

impl MemberInvitation {
    #[allow(clippy::too_many_arguments)]
    pub fn issue(
        id: MemberInvitationId,
        workspace_id: WorkspaceId,
        email: impl Into<String>,
        role: MemberRole,
        expires_at: u64,
        created_by: AccountId,
        token_hash: impl Into<String>,
        now: u64,
    ) -> Result<Self, Problem> {
        let email = email.into();
        let canonical_email = canonical_email(&email)?;
        let token_hash = token_hash.into();
        if expires_at <= now || expires_at.saturating_sub(now) > 30 * 24 * 60 * 60 {
            return Err(invalid_invitation("expiry"));
        }
        if token_hash.len() != 64 || !token_hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid_invitation("tokenHash"));
        }
        if matches!(role, MemberRole::Owner) {
            return Err(invalid_invitation("role"));
        }
        Ok(Self {
            id,
            workspace_id,
            canonical_email,
            role,
            expires_at,
            created_by,
            token_hash: token_hash.to_ascii_lowercase(),
            state: MemberInvitationState::Issued,
            redeemed_by: None,
            member_id: None,
            revision: 1,
        })
    }

    pub fn redeem(
        &self,
        expected_revision: u64,
        account_id: AccountId,
        member_id: MemberId,
        now: u64,
    ) -> Result<Self, Problem> {
        if self.revision != expected_revision {
            return Err(Problem::new(
                "revision_conflict",
                "Invitation changed; retry the request",
                409,
                "member_invitation",
            ));
        }
        if self.state != MemberInvitationState::Issued || now >= self.expires_at {
            return Err(Problem::new(
                "invitation_unavailable",
                "Invitation is expired, revoked, or already used",
                410,
                "member_invitation",
            ));
        }
        let mut next = self.clone();
        next.state = MemberInvitationState::Redeemed;
        next.redeemed_by = Some(account_id);
        next.member_id = Some(member_id);
        next.revision = self.revision.checked_add(1).ok_or_else(|| {
            Problem::new(
                "revision_exhausted",
                "Invitation revision exhausted",
                409,
                "member_invitation",
            )
        })?;
        Ok(next)
    }

    pub fn verify_code(&self, code: &str) -> bool {
        if self.token_hash.len() != 64 {
            return false;
        }
        let digest = Sha256::digest(code.as_bytes());
        let mut expected = [0_u8; 32];
        for (index, pair) in self.token_hash.as_bytes().chunks_exact(2).enumerate() {
            let Some(high) = hex_nibble(pair[0]) else {
                return false;
            };
            let Some(low) = hex_nibble(pair[1]) else {
                return false;
            };
            expected[index] = (high << 4) | low;
        }
        bool::from(expected.ct_eq(digest.as_slice()))
    }

    pub fn revoke(&self, expected_revision: u64) -> Result<Self, Problem> {
        if self.revision != expected_revision {
            return Err(Problem::new(
                "revision_conflict",
                "Invitation changed; retry the request",
                409,
                "member_invitation",
            ));
        }
        if self.state != MemberInvitationState::Issued {
            return Err(Problem::new(
                "invitation_unavailable",
                "Invitation is expired, revoked, or already used",
                410,
                "member_invitation",
            ));
        }
        let mut next = self.clone();
        next.state = MemberInvitationState::Revoked;
        next.revision = self.revision.checked_add(1).ok_or_else(|| {
            Problem::new(
                "revision_exhausted",
                "Invitation revision exhausted",
                409,
                "member_invitation",
            )
        })?;
        Ok(next)
    }
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn invalid_invitation(field: &str) -> Problem {
    Problem::new(
        "member_invitation_invalid",
        "Member invitation is invalid",
        400,
        "member_invitation",
    )
    .with_field(field, "invalid")
}
