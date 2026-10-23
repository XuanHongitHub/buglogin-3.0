use buglogin_api::auth::AuthService;
use buglogin_domain::ids::AccountId;

#[test]
fn token_is_one_time_visible_hashed_expiring_and_revocable() {
    let mut auth = AuthService::default();
    let account = AccountId::from_str("acct_1").unwrap();
    let token = auth.issue(account.clone(), 100).unwrap();
    assert_eq!(auth.authenticate(&token, 99).unwrap(), account);
    assert_eq!(
        auth.authenticate("wrong", 99).unwrap_err().code,
        "invalid_session"
    );
    assert_eq!(
        auth.authenticate(&token, 100).unwrap_err().code,
        "session_expired"
    );
    let token = auth.issue(account, 200).unwrap();
    auth.revoke(&token, 150).unwrap();
    assert_eq!(
        auth.authenticate(&token, 151).unwrap_err().code,
        "session_revoked"
    );
}
