//! Local profile password lock: Argon2id hashes (legacy salted SHA-256 records
//! still verify and are upgraded) and a short single-use unlock window that
//! launch/export commands must consume, so the lock is enforced in Rust.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use argon2::password_hash::{PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Argon2, PasswordHash};
use buglogin_domain::errors::Problem;
use serde_json::Value;

const UNLOCK_TTL_SECS: i64 = 120;
const MAX_PHC_BYTES: usize = 256;

static UNLOCKS: LazyLock<Mutex<UnlockWindow>> =
    LazyLock::new(|| Mutex::new(UnlockWindow::default()));

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum PasswordCheck {
    NotSet,
    Valid { upgrade: Option<String> },
    Invalid,
}

pub(crate) fn hash_password(password: &str) -> Result<String, Problem> {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|_| unavailable())?;
    let salt = SaltString::encode_b64(&salt).map_err(|_| unavailable())?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| unavailable())
}

pub(crate) fn check_password(settings: Option<&Value>, password: &str) -> PasswordCheck {
    let field = |name: &str| {
        settings
            .and_then(|settings| settings.get(name))
            .and_then(Value::as_str)
            .unwrap_or("")
    };
    let stored = field("password_hash");
    if stored.is_empty() {
        return PasswordCheck::NotSet;
    }
    if stored.starts_with("$argon2") {
        let verified = stored.len() <= MAX_PHC_BYTES
            && PasswordHash::new(stored).is_ok_and(|hash| {
                Argon2::default()
                    .verify_password(password.as_bytes(), &hash)
                    .is_ok()
            });
        return if verified {
            PasswordCheck::Valid { upgrade: None }
        } else {
            PasswordCheck::Invalid
        };
    }
    if constant_time_eq(&legacy_hash(password, field("password_salt")), stored) {
        PasswordCheck::Valid {
            upgrade: hash_password(password).ok(),
        }
    } else {
        PasswordCheck::Invalid
    }
}

/// Replacing an existing password must go through change (old password required).
pub(crate) fn ensure_password_can_be_set(settings: Option<&Value>) -> Result<(), Problem> {
    if check_password(settings, "") == PasswordCheck::NotSet {
        return Ok(());
    }
    Err(Problem::new(
        "profile_password_exists",
        "Profile already has a password; change it with the current password",
        409,
        "profile-password",
    ))
}

fn legacy_hash(password: &str, salt: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update(password.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

#[derive(Default)]
pub(crate) struct UnlockWindow {
    granted_at: HashMap<String, i64>,
}

impl UnlockWindow {
    pub(crate) fn grant(&mut self, profile_id: &str, now: i64) {
        self.granted_at.insert(profile_id.to_owned(), now);
    }

    pub(crate) fn is_unlocked(&self, profile_id: &str, now: i64) -> bool {
        self.granted_at
            .get(profile_id)
            .is_some_and(|granted| now - granted <= UNLOCK_TTL_SECS)
    }

    pub(crate) fn consume(&mut self, profile_id: &str, now: i64) -> bool {
        let unlocked = self.is_unlocked(profile_id, now);
        self.granted_at.remove(profile_id);
        unlocked
    }
}

pub(crate) fn grant_unlock(profile_id: &str) {
    if let Ok(mut unlocks) = UNLOCKS.lock() {
        unlocks.grant(profile_id, now_secs());
    }
}

pub(crate) fn is_unlocked(profile_id: &str) -> bool {
    UNLOCKS
        .lock()
        .is_ok_and(|unlocks| unlocks.is_unlocked(profile_id, now_secs()))
}

/// Uses up the profile's unlock; locked profiles need a fresh unlock per launch/export.
pub(crate) fn take_unlock(profile_id: &str) -> bool {
    UNLOCKS
        .lock()
        .is_ok_and(|mut unlocks| unlocks.consume(profile_id, now_secs()))
}

pub(crate) fn locked_problem() -> Problem {
    Problem::new(
        "profile_locked",
        "Profile is password protected; unlock it first",
        423,
        "profile-password",
    )
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn unavailable() -> Problem {
    Problem::new(
        "profile_password_unavailable",
        "Profile password hashing is unavailable",
        500,
        "profile-password",
    )
}

#[cfg(test)]
mod tests {
    use super::{
        PasswordCheck, UnlockWindow, check_password, ensure_password_can_be_set, hash_password,
    };
    use serde_json::json;
    use sha2::{Digest, Sha256};

    #[test]
    fn new_hashes_are_argon2id_and_verify() {
        let phc = hash_password("correct horse battery").unwrap();
        assert!(phc.starts_with("$argon2id$"));
        let settings = json!({ "password_hash": phc });

        assert_eq!(
            check_password(Some(&settings), "correct horse battery"),
            PasswordCheck::Valid { upgrade: None }
        );
        assert_eq!(
            check_password(Some(&settings), "wrong horse battery"),
            PasswordCheck::Invalid
        );
    }

    #[test]
    fn legacy_sha256_records_verify_and_request_an_upgrade() {
        let mut hasher = Sha256::new();
        hasher.update(b"0123abcd");
        hasher.update(b"legacy-password");
        let legacy = format!("{:x}", hasher.finalize());
        let settings = json!({ "password_salt": "0123abcd", "password_hash": legacy });

        match check_password(Some(&settings), "legacy-password") {
            PasswordCheck::Valid {
                upgrade: Some(upgraded),
            } => assert!(upgraded.starts_with("$argon2id$")),
            other => panic!("expected an upgrade, got {other:?}"),
        }
        assert_eq!(
            check_password(Some(&settings), "not-the-password"),
            PasswordCheck::Invalid
        );
    }

    #[test]
    fn missing_hash_means_no_password() {
        assert_eq!(check_password(None, "anything"), PasswordCheck::NotSet);
        assert_eq!(
            check_password(Some(&json!({})), "anything"),
            PasswordCheck::NotSet
        );
    }

    #[test]
    fn an_unlock_is_single_use_and_expires() {
        let mut window = UnlockWindow::default();
        assert!(!window.is_unlocked("prf_a", 100));

        window.grant("prf_a", 100);
        assert!(window.is_unlocked("prf_a", 150));
        assert!(!window.is_unlocked("prf_b", 150));
        assert!(window.consume("prf_a", 150));
        assert!(!window.consume("prf_a", 151));

        window.grant("prf_a", 200);
        assert!(!window.consume("prf_a", 200 + 121));
    }

    #[test]
    fn setting_a_password_never_overwrites_an_existing_one() {
        assert!(ensure_password_can_be_set(None).is_ok());
        assert!(ensure_password_can_be_set(Some(&json!({}))).is_ok());
        let existing = json!({ "password_hash": hash_password("first-password").unwrap() });
        assert_eq!(
            ensure_password_can_be_set(Some(&existing))
                .unwrap_err()
                .code,
            "profile_password_exists"
        );
    }
}
