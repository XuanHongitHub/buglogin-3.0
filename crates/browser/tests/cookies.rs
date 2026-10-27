// This integration test includes the whole implementation module to exercise
// crate-private cookie behavior; not every production method is used here.
#![allow(dead_code)]

#[path = "../src/cookies/mod.rs"]
mod cookies;

use buglogin_domain::{
    ids::{ProfileId, WorkspaceId},
    permissions::{Principal, Role},
};
use buglogin_storage::secrets::{
    OsVault, RotationReceipt, SecretError, SecretKey, SecretValue, SecretVersion,
};
use cookies::{CookieError, CookieInput, CookieSameSite, CookieScope, CookieService};
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Default)]
struct FakeVault {
    values: HashMap<SecretKey, (SecretValue, SecretVersion)>,
    next: u64,
}

impl OsVault for FakeVault {
    fn read(&self, key: &SecretKey) -> Result<(SecretValue, SecretVersion), SecretError> {
        self.values.get(key).cloned().ok_or(SecretError::NotFound)
    }

    fn create(
        &mut self,
        key: &SecretKey,
        value: SecretValue,
    ) -> Result<SecretVersion, SecretError> {
        if self.values.contains_key(key) {
            return Err(SecretError::VersionConflict);
        }
        let version = self.next_version();
        self.values.insert(key.clone(), (value, version));
        Ok(version)
    }

    fn rotate(
        &mut self,
        key: &SecretKey,
        expected: SecretVersion,
        value: SecretValue,
    ) -> Result<RotationReceipt, SecretError> {
        let current = self
            .values
            .get(key)
            .map(|(_, version)| *version)
            .ok_or(SecretError::NotFound)?;
        if current != expected {
            return Err(SecretError::VersionConflict);
        }
        let next = self.next_version();
        self.values.insert(key.clone(), (value, next));
        Ok(RotationReceipt {
            previous: current,
            current: next,
        })
    }

    fn remove(&mut self, key: &SecretKey, expected: SecretVersion) -> Result<(), SecretError> {
        let current = self
            .values
            .get(key)
            .map(|(_, version)| *version)
            .ok_or(SecretError::NotFound)?;
        if current != expected {
            return Err(SecretError::VersionConflict);
        }
        self.values.remove(key);
        Ok(())
    }
}

impl FakeVault {
    fn next_version(&mut self) -> SecretVersion {
        self.next += 1;
        SecretVersion::new(self.next).unwrap()
    }
}

fn scope(workspace: &str, profile: &str) -> CookieScope {
    CookieScope {
        workspace_id: WorkspaceId::from_str(workspace).unwrap(),
        profile_id: ProfileId::from_str(profile).unwrap(),
    }
}

fn principal() -> Principal {
    Principal {
        roles: BTreeSet::from([Role::LocalOwner]),
        ..Principal::default()
    }
}

fn cookie(value: &[u8]) -> CookieInput {
    CookieInput {
        name: "sid".into(),
        domain: ".Example.COM".into(),
        path: "/".into(),
        secure: true,
        http_only: true,
        expires_at: None,
        same_site: CookieSameSite::Lax,
        value: SecretValue::new(value.to_vec()).unwrap(),
    }
}
fn named_cookie(name: &str, value: &[u8]) -> CookieInput {
    CookieInput {
        name: name.into(),
        ..cookie(value)
    }
}

#[test]
fn import_normalizes_and_lists_without_secret_values() {
    let mut service = CookieService::new(FakeVault::default());
    let report = service
        .import(
            scope("ws_1", "prf_1"),
            [cookie(b"secret-value")],
            &principal(),
            &[],
        )
        .unwrap();
    assert_eq!(report.inserted, 1);
    let summaries = service
        .list(&scope("ws_1", "prf_1"), &principal(), &[])
        .unwrap();
    assert_eq!(summaries[0].domain, "example.com");
    assert!(summaries[0].value_present);
    let debug = format!("{summaries:?}");
    assert!(!debug.contains("secret-value"));
}

#[test]
fn copy_and_clone_preserve_values_inside_vault_only() {
    let mut service = CookieService::new(FakeVault::default());
    let source = scope("ws_1", "prf_1");
    let destination = scope("ws_1", "prf_2");
    service
        .import(source.clone(), [cookie(b"secret-value")], &principal(), &[])
        .unwrap();
    assert_eq!(
        service
            .copy(&source, destination.clone(), &principal(), &[])
            .unwrap()
            .copied,
        1
    );
    assert_eq!(
        service
            .clone_to(&source, scope("ws_1", "prf_3"), &principal(), &[])
            .unwrap()
            .copied,
        1
    );
    assert_eq!(
        service.list(&destination, &principal(), &[]).unwrap().len(),
        1
    );
}

#[test]
fn validation_happens_before_any_write_and_scope_is_enforced() {
    let mut service = CookieService::new(FakeVault::default());
    let target = scope("ws_1", "prf_1");
    let mut invalid = cookie(b"secret-value");
    invalid.path = "relative".into();
    assert_eq!(
        service.import(target.clone(), [invalid], &principal(), &[]),
        Err(CookieError::InvalidPath)
    );
    assert_eq!(service.list(&target, &principal(), &[]).unwrap().len(), 0);
    assert_eq!(
        service
            .list(&scope("ws_2", "prf_1"), &principal(), &[])
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn unauthorized_access_fails_closed() {
    let mut service = CookieService::new(FakeVault::default());
    let denied = Principal::default();
    assert_eq!(
        service.import(
            scope("ws_1", "prf_1"),
            [cookie(b"secret-value")],
            &denied,
            &[]
        ),
        Err(CookieError::Forbidden)
    );
}

#[test]
fn cookie_vault_keys_are_collision_resistant_and_scope_bound() {
    let first = cookies::key_for(
        &scope("ws_1", "prf_1"),
        &cookies::CookieIdentity {
            name: "sid".into(),
            domain: "example.com".into(),
            path: "/".into(),
        },
    );
    let second = cookies::key_for(
        &scope("ws_2", "prf_1"),
        &cookies::CookieIdentity {
            name: "sid".into(),
            domain: "example.com".into(),
            path: "/".into(),
        },
    );
    assert_eq!(first.namespace(), "cookies");
    assert_eq!(first.name().len(), 64);
    assert_ne!(first, second);
}

#[test]
fn metadata_survives_restart_without_persisting_secret_values() {
    let root = temporary_path("restart");
    let target = scope("ws_1", "prf_1");
    let mut service = CookieService::open(FakeVault::default(), &root).unwrap();
    service
        .import(target.clone(), [cookie(b"secret-value")], &principal(), &[])
        .unwrap();
    let vault = service.into_vault();
    let reopened = CookieService::open(vault, &root).unwrap();
    let summaries = reopened.list(&target, &principal(), &[]).unwrap();
    assert_eq!(summaries.len(), 1);
    assert!(summaries[0].value_present);
    let persisted = fs::read_dir(root.join("revisions"))
        .unwrap()
        .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect::<String>();
    assert!(!persisted.contains("secret-value"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_metadata_commit_rolls_back_new_slot_and_preserves_active_cookie() {
    let root = temporary_path("rollback");
    let target = scope("ws_1", "prf_1");
    let mut service = CookieService::open(FakeVault::default(), &root).unwrap();
    service
        .import(target.clone(), [cookie(b"old-secret")], &principal(), &[])
        .unwrap();
    fs::remove_dir_all(&root).unwrap();
    fs::write(&root, b"blocks cookie metadata directory").unwrap();
    assert_eq!(
        service.import(target.clone(), [cookie(b"new-secret")], &principal(), &[]),
        Err(CookieError::Storage)
    );
    let summaries = service.list(&target, &principal(), &[]).unwrap();
    assert_eq!(summaries.len(), 1);
    assert!(summaries[0].value_present);
    let vault = service.into_vault();
    assert_eq!(vault.values.len(), 1);
    assert_eq!(
        vault.values.values().next().unwrap().0.as_bytes(),
        b"old-secret"
    );
    fs::remove_file(root).unwrap();
}

fn temporary_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "buglogin-cookie-{label}-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn failed_batch_commit_removes_every_staged_secret() {
    let root = temporary_path("batch-rollback");
    fs::write(&root, b"blocks cookie metadata directory").unwrap();
    let target = scope("ws_1", "prf_1");
    let mut service = CookieService::open(FakeVault::default(), &root).unwrap();
    assert_eq!(
        service.import(
            target.clone(),
            [
                named_cookie("first", b"first-secret"),
                named_cookie("second", b"second-secret")
            ],
            &principal(),
            &[],
        ),
        Err(CookieError::Storage)
    );
    assert!(service.list(&target, &principal(), &[]).unwrap().is_empty());
    assert!(service.into_vault().values.is_empty());
    fs::remove_file(root).unwrap();
}
