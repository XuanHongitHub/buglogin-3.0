use buglogin_domain::{
    account::{
        Account, AccountIdentity, AccountStatus, IdentityVerification, canonical_email,
        canonical_username,
    },
    ids::AccountId,
};

fn account() -> Account {
    Account::new(
        AccountId::from_str("acct_1").expect("valid account ID"),
        "2026-07-21T00:00:00Z",
    )
}

#[test]
fn identity_canonicalizes_without_destroying_display_values() {
    let identity = AccountIdentity::new(
        AccountId::from_str("acct_1").unwrap(),
        "  Nguyen.Admin  ",
        "  Admin@Example.COM  ",
        "2026-07-22T00:00:00Z",
    )
    .unwrap();

    assert_eq!(identity.username, "Nguyen.Admin");
    assert_eq!(identity.canonical_username, "nguyen.admin");
    assert_eq!(identity.email, "Admin@Example.COM");
    assert_eq!(identity.canonical_email, "admin@example.com");
    assert_eq!(identity.verification, IdentityVerification::Pending);
}

#[test]
fn identity_rejects_ambiguous_or_malformed_values() {
    assert!(canonical_username("ab").is_err());
    assert!(canonical_username("has space").is_err());
    assert!(canonical_username("name@example").is_err());
    assert!(canonical_email("missing-at.example.com").is_err());
    assert!(canonical_email("name@localhost").is_err());
}

#[test]
fn identity_verification_is_revision_checked_and_idempotent() {
    let identity = AccountIdentity::new(
        AccountId::from_str("acct_1").unwrap(),
        "owner",
        "owner@example.com",
        "2026-07-22T00:00:00Z",
    )
    .unwrap();
    assert!(identity.verify(2, "2026-07-22T00:01:00Z").is_err());

    let verified = identity.verify(1, "2026-07-22T00:01:00Z").unwrap();
    assert_eq!(verified.verification, IdentityVerification::Verified);
    assert_eq!(verified.revision, 2);
    assert_eq!(
        verified.verify(2, "2026-07-22T00:02:00Z").unwrap(),
        verified
    );
}

#[test]
fn account_is_session_independent_and_serializes_canonically() {
    let value = serde_json::to_value(account()).expect("serialize account");

    assert_eq!(value["id"], "acct_1");
    assert_eq!(value["status"], "active");
    assert_eq!(value["revision"], 1);
    assert!(value.get("session").is_none());
    assert!(value.get("token").is_none());
    assert!(value.get("secret").is_none());
}

#[test]
fn account_status_transitions_are_revision_checked() {
    let suspended = account()
        .transition_status(1, AccountStatus::Suspended, "2026-07-21T00:01:00Z")
        .expect("active to suspended");
    assert_eq!(suspended.revision, 2);
    assert_eq!(suspended.status, AccountStatus::Suspended);

    let active = suspended
        .transition_status(2, AccountStatus::Active, "2026-07-21T00:02:00Z")
        .expect("suspended to active");
    assert_eq!(active.revision, 3);

    let error = active
        .transition_status(1, AccountStatus::Disabled, "2026-07-21T00:03:00Z")
        .expect_err("stale revision must be rejected");
    assert_eq!(error.code, "revision_conflict");
    assert_eq!(error.fields["expectedRevision"], 1);
    assert_eq!(error.fields["actualRevision"], 3);
}

#[test]
fn disabled_account_is_terminal() {
    assert!(
        AccountStatus::Disabled
            .transition(AccountStatus::Active)
            .is_err()
    );
    assert!(
        AccountStatus::Active
            .transition(AccountStatus::Active)
            .is_err()
    );
}
#[test]
fn account_revision_cannot_overflow() {
    let account = Account {
        revision: u64::MAX,
        ..account()
    };
    let error = account
        .transition_status(u64::MAX, AccountStatus::Suspended, "2026-07-21T00:01:00Z")
        .expect_err("revision overflow must be rejected");
    assert_eq!(error.code, "revision_exhausted");
}
