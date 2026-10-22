use buglogin_domain::{
    ids::{AccountId, MemberId, MemberInvitationId, WorkspaceId},
    member::MemberRole,
    member_invitation::{MemberInvitation, MemberInvitationState},
};
use sha2::{Digest, Sha256};

const INVITATION_CODE: &str = "test-bearer-code";

fn code_hash() -> String {
    Sha256::digest(INVITATION_CODE.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn invitation(role: MemberRole, expires_at: u64) -> MemberInvitation {
    MemberInvitation::issue(
        MemberInvitationId::new("inv_test_1").unwrap(),
        WorkspaceId::new("ws_test_1").unwrap(),
        " Admin@Example.COM ",
        role,
        expires_at,
        AccountId::new("acct_owner").unwrap(),
        code_hash(),
        100,
    )
    .unwrap()
}

#[test]
fn invitation_canonicalizes_email_and_persists_only_a_code_digest() {
    let value = invitation(MemberRole::Member, 200);
    let serialized = serde_json::to_value(&value).unwrap();

    assert_eq!(value.canonical_email, "admin@example.com");
    assert_eq!(value.state, MemberInvitationState::Issued);
    assert_eq!(value.token_hash, code_hash());
    assert!(serialized.get("code").is_none());
    assert!(value.verify_code(INVITATION_CODE));
    assert!(!value.verify_code("wrong-code"));
}

#[test]
fn invitation_rejects_expired_excessive_and_owner_grants() {
    assert!(
        MemberInvitation::issue(
            MemberInvitationId::new("inv_expired").unwrap(),
            WorkspaceId::new("ws_test_1").unwrap(),
            "admin@example.com",
            MemberRole::Member,
            100,
            AccountId::new("acct_owner").unwrap(),
            "ab".repeat(32),
            100,
        )
        .is_err()
    );

    assert!(
        MemberInvitation::issue(
            MemberInvitationId::new("inv_too_long").unwrap(),
            WorkspaceId::new("ws_test_1").unwrap(),
            "admin@example.com",
            MemberRole::Member,
            100 + 30 * 24 * 60 * 60 + 1,
            AccountId::new("acct_owner").unwrap(),
            "ab".repeat(32),
            100,
        )
        .is_err()
    );

    assert!(
        MemberInvitation::issue(
            MemberInvitationId::new("inv_owner").unwrap(),
            WorkspaceId::new("ws_test_1").unwrap(),
            "admin@example.com",
            MemberRole::Owner,
            200,
            AccountId::new("acct_owner").unwrap(),
            "ab".repeat(32),
            100,
        )
        .is_err()
    );
}

#[test]
fn redemption_is_revision_checked_expiring_and_single_use() {
    let value = invitation(MemberRole::Admin, 200);
    assert!(
        value
            .redeem(
                0,
                AccountId::new("acct_member").unwrap(),
                MemberId::new("mem_1").unwrap(),
                101
            )
            .is_err()
    );

    let redeemed = value
        .redeem(
            1,
            AccountId::new("acct_member").unwrap(),
            MemberId::new("mem_1").unwrap(),
            199,
        )
        .unwrap();
    assert_eq!(redeemed.state, MemberInvitationState::Redeemed);
    assert_eq!(redeemed.revision, 2);
    assert_eq!(
        redeemed.redeemed_by.as_ref().unwrap().as_str(),
        "acct_member"
    );
    assert!(
        redeemed
            .redeem(
                2,
                AccountId::new("acct_member").unwrap(),
                MemberId::new("mem_2").unwrap(),
                199,
            )
            .is_err()
    );
    assert!(
        value
            .redeem(
                1,
                AccountId::new("acct_member").unwrap(),
                MemberId::new("mem_3").unwrap(),
                200,
            )
            .is_err()
    );
}

#[test]
fn revocation_requires_current_revision_and_blocks_redeem() {
    let value = invitation(MemberRole::Member, 200);
    assert!(value.revoke(0).is_err());

    let revoked = value.revoke(1).unwrap();
    assert_eq!(revoked.state, MemberInvitationState::Revoked);
    assert_eq!(revoked.revision, 2);
    assert!(
        revoked
            .redeem(
                2,
                AccountId::new("acct_member").unwrap(),
                MemberId::new("mem_1").unwrap(),
                101
            )
            .is_err()
    );
}
