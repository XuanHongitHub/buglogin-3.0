use buglogin_browser::cookies::{
    CookieError, CookieInput, CookieSameSite, CookieScope, CookieService,
    authority::{CookieAuthority, CookieAuthorityError, IdempotencyKey, ProfileRuntimeState},
    wayfern::{self, EncryptedCookieInput, ProfileCookieState, WayfernCookieError},
};
use buglogin_domain::{
    ids::{ProfileId, ResourceId, WorkspaceId},
    lease::Lease,
    permissions::{Principal, Role},
};
use buglogin_storage::secrets::{
    OsVault, RotationReceipt, SecretError, SecretKey, SecretValue, SecretVersion,
};
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
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
        SecretVersion::new(self.next).expect("test version is non-zero")
    }
}

#[derive(Clone, Default)]
struct SharedVault {
    values: Arc<Mutex<HashMap<SecretKey, (SecretValue, SecretVersion)>>>,
    next: Arc<Mutex<u64>>,
}

impl OsVault for SharedVault {
    fn read(&self, key: &SecretKey) -> Result<(SecretValue, SecretVersion), SecretError> {
        self.values
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or(SecretError::NotFound)
    }

    fn create(
        &mut self,
        key: &SecretKey,
        value: SecretValue,
    ) -> Result<SecretVersion, SecretError> {
        let mut values = self.values.lock().unwrap();
        if values.contains_key(key) {
            return Err(SecretError::VersionConflict);
        }
        let version = self.next_version();
        values.insert(key.clone(), (value, version));
        Ok(version)
    }

    fn rotate(
        &mut self,
        key: &SecretKey,
        expected: SecretVersion,
        value: SecretValue,
    ) -> Result<RotationReceipt, SecretError> {
        let mut values = self.values.lock().unwrap();
        let current = values
            .get(key)
            .map(|(_, version)| *version)
            .ok_or(SecretError::NotFound)?;
        if current != expected {
            return Err(SecretError::VersionConflict);
        }
        let next = self.next_version();
        values.insert(key.clone(), (value, next));
        Ok(RotationReceipt {
            previous: current,
            current: next,
        })
    }

    fn remove(&mut self, key: &SecretKey, expected: SecretVersion) -> Result<(), SecretError> {
        let mut values = self.values.lock().unwrap();
        let current = values
            .get(key)
            .map(|(_, version)| *version)
            .ok_or(SecretError::NotFound)?;
        if current != expected {
            return Err(SecretError::VersionConflict);
        }
        values.remove(key);
        Ok(())
    }
}

impl SharedVault {
    fn next_version(&self) -> SecretVersion {
        let mut next = self.next.lock().unwrap();
        *next += 1;
        SecretVersion::new(*next).expect("test version is non-zero")
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

fn cookie(name: &str, value: &[u8]) -> CookieInput {
    CookieInput {
        name: name.into(),
        domain: ".Example.COM".into(),
        path: "/".into(),
        secure: true,
        http_only: true,
        expires_at: None,
        same_site: CookieSameSite::Lax,
        value: SecretValue::new(value.to_vec()).unwrap(),
    }
}

fn lease(scope: &CookieScope, fencing_token: u64, expires_at: u64) -> Lease {
    Lease {
        lease_id: format!("lease-{fencing_token}"),
        fencing_token,
        resource_id: ResourceId::from_str(&format!("res-{}", scope.profile_id.as_str())).unwrap(),
        workspace_id: scope.workspace_id.clone(),
        owner_id: "desktop-1".into(),
        token: format!("token-{fencing_token}"),
        issued_at: 1,
        expires_at,
    }
}

fn temporary_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "buglogin-cookie-authority-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn authority() -> (CookieAuthority<FakeVault>, CookieScope, Lease, Principal) {
    let target = scope("ws_1", "prf_1");
    let current_lease = lease(&target, 7, 100);
    let mut authority = CookieAuthority::new(CookieService::new(FakeVault::default()));
    authority.set_profile_state(target.clone(), ProfileRuntimeState::Stopped);
    authority
        .set_current_lease(target.clone(), current_lease.clone())
        .unwrap();
    (authority, target, current_lease, principal())
}

#[test]
fn staged_import_is_one_use_and_idempotent_replay_is_safe() {
    let (mut authority, target, current_lease, principal) = authority();
    let stage = authority
        .stage_import(target.clone(), [cookie("sid", b"secret")])
        .unwrap();
    assert_eq!(stage.as_str().len(), 64);
    assert!(stage.as_str().bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(format!("{stage:?}").contains("[REDACTED]"));
    let key = IdempotencyKey::new("cookie-import-001").unwrap();

    let first = authority
        .import(&stage, 0, &current_lease, 10, &principal, &[], key.clone())
        .unwrap();
    assert_eq!(first.report.inserted, 1);
    assert_eq!(first.generation, 1);
    assert!(!first.replayed);

    let replay = authority
        .import(&stage, 0, &current_lease, 10, &principal, &[], key.clone())
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.generation, 1);
    assert_eq!(authority.generation(&target), 1);
    assert_eq!(
        authority.import(
            &stage,
            0,
            &current_lease,
            10,
            &principal,
            &[],
            IdempotencyKey::new("cookie-import-002b").unwrap(),
        ),
        Err(CookieAuthorityError::StagedImportNotFound)
    );

    let conflicting = authority.import(&stage, 1, &current_lease, 10, &principal, &[], key);
    assert_eq!(conflicting, Err(CookieAuthorityError::IdempotencyConflict));
}

#[test]
fn stale_generation_rejects_without_consuming_stage() {
    let (mut authority, target, current_lease, principal) = authority();
    let first_stage = authority
        .stage_import(target.clone(), [cookie("sid", b"first")])
        .unwrap();
    authority
        .import(
            &first_stage,
            0,
            &current_lease,
            10,
            &principal,
            &[],
            IdempotencyKey::new("cookie-import-002").unwrap(),
        )
        .unwrap();

    let second_stage = authority
        .stage_import(target.clone(), [cookie("sid", b"second")])
        .unwrap();
    assert_eq!(
        authority.import(
            &second_stage,
            0,
            &current_lease,
            10,
            &principal,
            &[],
            IdempotencyKey::new("cookie-import-003").unwrap(),
        ),
        Err(CookieAuthorityError::GenerationMismatch {
            expected: 0,
            actual: 1,
        })
    );
    let committed = authority
        .import(
            &second_stage,
            1,
            &current_lease,
            10,
            &principal,
            &[],
            IdempotencyKey::new("cookie-import-004").unwrap(),
        )
        .unwrap();
    assert_eq!(committed.generation, 2);
}

#[test]
fn stopped_state_lease_and_fencing_are_fail_closed() {
    let (mut authority, target, current_lease, principal) = authority();
    let stage = authority
        .stage_import(target.clone(), [cookie("sid", b"secret")])
        .unwrap();

    authority.set_profile_state(target.clone(), ProfileRuntimeState::Running);
    assert_eq!(
        authority.import(
            &stage,
            0,
            &current_lease,
            10,
            &principal,
            &[],
            IdempotencyKey::new("cookie-import-005").unwrap(),
        ),
        Err(CookieAuthorityError::ProfileRunning)
    );

    authority.set_profile_state(target.clone(), ProfileRuntimeState::Stopped);
    assert_eq!(
        authority.import(
            &stage,
            0,
            &lease(&target, 7, 10),
            10,
            &principal,
            &[],
            IdempotencyKey::new("cookie-import-006").unwrap(),
        ),
        Err(CookieAuthorityError::LeaseExpired)
    );
    assert_eq!(
        authority.import(
            &stage,
            0,
            &lease(&target, 6, 100),
            10,
            &principal,
            &[],
            IdempotencyKey::new("cookie-import-007").unwrap(),
        ),
        Err(CookieAuthorityError::LeaseFencingMismatch)
    );
    assert_eq!(
        authority.import(
            &stage,
            0,
            &lease(&target, 8, 100),
            10,
            &principal,
            &[],
            IdempotencyKey::new("cookie-import-008").unwrap(),
        ),
        Err(CookieAuthorityError::LeaseFencingMismatch)
    );
}

#[test]
fn copy_requires_both_profiles_stopped_and_destination_authority() {
    let (mut authority, source, source_lease, principal) = authority();
    let destination = scope("ws_2", "prf_2");
    let destination_lease = lease(&destination, 9, 100);
    authority.set_profile_state(destination.clone(), ProfileRuntimeState::Stopped);
    authority
        .set_current_lease(destination.clone(), destination_lease.clone())
        .unwrap();
    authority
        .stage_import(source.clone(), [cookie("sid", b"secret")])
        .map(|stage| {
            authority
                .import(
                    &stage,
                    0,
                    &source_lease,
                    10,
                    &principal,
                    &[],
                    IdempotencyKey::new("cookie-import-009").unwrap(),
                )
                .unwrap();
        })
        .unwrap();

    authority.set_profile_state(source.clone(), ProfileRuntimeState::Running);
    assert_eq!(
        authority.copy(
            &source,
            destination.clone(),
            0,
            &destination_lease,
            10,
            &principal,
            &[],
            IdempotencyKey::new("cookie-copy-001").unwrap(),
        ),
        Err(CookieAuthorityError::ProfileRunning)
    );
}

#[test]
fn copy_advances_destination_cas_and_replays_without_duplicate_write() {
    let (mut authority, source, source_lease, principal) = authority();
    let destination = scope("ws_2", "prf_2");
    let destination_lease = lease(&destination, 9, 100);
    authority.set_profile_state(destination.clone(), ProfileRuntimeState::Stopped);
    authority
        .set_current_lease(destination.clone(), destination_lease.clone())
        .unwrap();
    let stage = authority
        .stage_import(source.clone(), [cookie("sid", b"secret")])
        .unwrap();
    authority
        .import(
            &stage,
            0,
            &source_lease,
            10,
            &principal,
            &[],
            IdempotencyKey::new("cookie-copy-source").unwrap(),
        )
        .unwrap();

    let key = IdempotencyKey::new("cookie-copy-002").unwrap();
    let first = authority
        .copy(
            &source,
            destination.clone(),
            0,
            &destination_lease,
            10,
            &principal,
            &[],
            key.clone(),
        )
        .unwrap();
    assert_eq!(first.report.copied, 1);
    assert_eq!(first.generation, 1);
    let replay = authority
        .copy(
            &source,
            destination.clone(),
            0,
            &destination_lease,
            10,
            &principal,
            &[],
            key,
        )
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.generation, 1);
    assert_eq!(
        authority
            .list(&destination, &principal, &[])
            .unwrap()
            .cookies
            .len(),
        1
    );

    assert_eq!(
        authority.copy(
            &source,
            destination,
            0,
            &destination_lease,
            10,
            &principal,
            &[],
            IdempotencyKey::new("cookie-copy-003").unwrap(),
        ),
        Err(CookieAuthorityError::GenerationMismatch {
            expected: 0,
            actual: 1,
        })
    );
}

#[test]
fn unknown_profile_state_and_invalid_staged_reference_fail_closed() {
    let (mut authority, target, current_lease, principal) = authority();
    let unknown = scope("ws_9", "prf_9");
    assert_eq!(
        authority.list(&unknown, &principal, &[]),
        Err(CookieAuthorityError::ProfileStateUnknown)
    );
    assert_eq!(
        buglogin_browser::cookies::authority::StagedImportId::from_opaque("not-an-id"),
        Err(CookieAuthorityError::InvalidStagedImportId)
    );
    let invalid_lease = Lease {
        workspace_id: WorkspaceId::from_str("other-workspace").unwrap(),
        ..current_lease
    };
    assert_eq!(
        authority.set_current_lease(target, invalid_lease),
        Err(CookieAuthorityError::InvalidLease)
    );
}

#[test]
fn authority_restart_restores_generation_and_replay_but_discards_staged_input() {
    let root = temporary_path("restart");
    let vault = SharedVault::default();
    let target = scope("ws_1", "prf_1");
    let current_lease = lease(&target, 7, 100);
    let principal = principal();
    let stage;
    let key = IdempotencyKey::new("cookie-restart-001").unwrap();

    {
        let mut authority = CookieAuthority::open(vault.clone(), &root).unwrap();
        authority.set_profile_state(target.clone(), ProfileRuntimeState::Stopped);
        authority
            .set_current_lease(target.clone(), current_lease.clone())
            .unwrap();
        stage = authority
            .stage_import(target.clone(), [cookie("sid", b"secret")])
            .unwrap();
        assert_eq!(
            authority
                .import(&stage, 0, &current_lease, 10, &principal, &[], key.clone())
                .unwrap()
                .generation,
            1
        );
    }

    {
        let mut authority = CookieAuthority::open(vault, &root).unwrap();
        authority.set_profile_state(target.clone(), ProfileRuntimeState::Stopped);
        authority
            .set_current_lease(target.clone(), current_lease.clone())
            .unwrap();
        assert_eq!(authority.generation(&target), 1);
        assert!(
            authority
                .import(&stage, 0, &current_lease, 10, &principal, &[], key)
                .unwrap()
                .replayed
        );

        let new_stage = authority
            .stage_import(target.clone(), [cookie("sid2", b"secret-2")])
            .unwrap();
        assert_eq!(
            authority
                .import(
                    &new_stage,
                    1,
                    &current_lease,
                    10,
                    &principal,
                    &[],
                    IdempotencyKey::new("cookie-restart-002").unwrap(),
                )
                .unwrap()
                .generation,
            2
        );
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn authority_restart_rejects_uncommitted_staged_import() {
    let root = temporary_path("staged-restart");
    let vault = SharedVault::default();
    let target = scope("ws_1", "prf_1");
    let current_lease = lease(&target, 7, 100);
    let principal = principal();
    let stage;

    {
        let mut authority = CookieAuthority::open(vault.clone(), &root).unwrap();
        authority.set_profile_state(target.clone(), ProfileRuntimeState::Stopped);
        authority
            .set_current_lease(target.clone(), current_lease.clone())
            .unwrap();
        stage = authority
            .stage_import(target.clone(), [cookie("sid", b"secret")])
            .unwrap();
    }

    {
        let mut authority = CookieAuthority::open(vault, &root).unwrap();
        authority.set_profile_state(target.clone(), ProfileRuntimeState::Stopped);
        authority
            .set_current_lease(target.clone(), current_lease.clone())
            .unwrap();
        assert_eq!(
            authority.import(
                &stage,
                0,
                &current_lease,
                10,
                &principal,
                &[],
                IdempotencyKey::new("cookie-staged-restart-001").unwrap(),
            ),
            Err(CookieAuthorityError::StagedImportNotFound)
        );
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cookie_authority_lock_rejects_second_open_and_releases_on_drop() {
    let root = temporary_path("lock");
    let vault = SharedVault::default();
    let authority = CookieAuthority::open(vault.clone(), &root).unwrap();
    assert!(matches!(
        CookieAuthority::open(vault.clone(), &root),
        Err(CookieAuthorityError::Cookie(CookieError::Locked))
    ));
    drop(authority);
    assert!(CookieAuthority::open(vault, &root).is_ok());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn authority_commit_and_wayfern_materialization_are_separate_transactions() {
    let authority_root = temporary_path("wayfern-gap-authority");
    let profile_root = temporary_path("wayfern-gap-profile");
    let target = scope("ws_1", "prf_1");
    let current_lease = lease(&target, 7, 100);
    let principal = principal();
    let mut authority = CookieAuthority::open(FakeVault::default(), &authority_root).unwrap();
    authority.set_profile_state(target.clone(), ProfileRuntimeState::Stopped);
    authority
        .set_current_lease(target.clone(), current_lease.clone())
        .unwrap();

    let stage = authority
        .stage_import(target.clone(), [cookie("sid", &[0, 1, 2, 3])])
        .unwrap();
    let committed = authority
        .import(
            &stage,
            0,
            &current_lease,
            10,
            &principal,
            &[],
            IdempotencyKey::new("cookie-wayfern-gap-001").unwrap(),
        )
        .unwrap();

    assert_eq!(committed.generation, 1);
    assert!(authority.list(&target, &principal, &[]).unwrap().cookies[0].value_present);
    assert_eq!(
        wayfern::materialize(
            &profile_root,
            ProfileCookieState::Stopped,
            &[cookie("sid", &[0, 1, 2, 3])],
            1_700_000_000,
        ),
        Err(WayfernCookieError::PlaintextPersistenceForbidden)
    );
    assert!(!wayfern::database_path(&profile_root).exists());

    let second_stage = authority
        .stage_import(target.clone(), [cookie("sid-2", &[4, 5, 6, 7])])
        .unwrap();
    let encrypted = EncryptedCookieInput::new(
        "sid-2".into(),
        ".example.com".into(),
        "/".into(),
        true,
        true,
        None,
        CookieSameSite::Lax,
        b"v10\x01opaque-ciphertext",
    )
    .unwrap();
    assert_eq!(
        wayfern::materialize_encrypted(
            &profile_root,
            ProfileCookieState::Stopped,
            &[encrypted],
            1_700_000_000,
        )
        .unwrap()
        .inserted,
        1
    );
    assert_eq!(
        authority.import(
            &second_stage,
            1,
            &current_lease,
            100,
            &principal,
            &[],
            IdempotencyKey::new("cookie-wayfern-gap-002").unwrap(),
        ),
        Err(CookieAuthorityError::LeaseExpired)
    );
    assert_eq!(authority.generation(&target), 1);
    assert_eq!(wayfern::read_metadata(&profile_root).unwrap().len(), 1);

    fs::remove_dir_all(authority_root).unwrap();
    fs::remove_dir_all(profile_root).unwrap();
}
