#[path = "../src/secrets/mod.rs"]
#[allow(dead_code)]
mod secrets;
#[path = "../src/vault/mod.rs"]
mod vault;

#[cfg(not(any(windows, target_os = "macos")))]
use secrets::SecretError;
use secrets::{OsVault, SecretKey, SecretValue};
use vault::NativeVault;

fn key() -> SecretKey {
    SecretKey::new("tests", format!("vault-{}", std::process::id())).unwrap()
}

#[cfg(not(any(windows, target_os = "macos")))]
#[test]
fn unsupported_os_fails_closed() {
    let mut vault = NativeVault::new();
    let error = vault
        .create(&key(), SecretValue::new(b"secret".to_vec()).unwrap())
        .unwrap_err();
    assert_eq!(error, SecretError::Unavailable);
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn native_vault_round_trip_and_rotation() {
    let mut vault = NativeVault::new();
    let key = key();
    let created = vault
        .create(&key, SecretValue::new(b"old".to_vec()).unwrap())
        .unwrap();
    assert_eq!(vault.read(&key).unwrap().0.as_bytes(), b"old");
    let rotated = vault
        .rotate(&key, created, SecretValue::new(b"new".to_vec()).unwrap())
        .unwrap();
    assert_eq!(vault.read(&key).unwrap().0.as_bytes(), b"new");
    assert_eq!(vault.remove(&key, rotated.current), Ok(()));
}
