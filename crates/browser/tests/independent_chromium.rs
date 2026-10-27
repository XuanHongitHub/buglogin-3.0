use buglogin_browser::runtime::{
    RuntimeAdapter, RuntimeBinary, RuntimeCapability,
    independent_chromium::{ChromiumIdentity, IndependentChromiumAdapter},
};
use buglogin_domain::{
    entities::{FingerprintOperatingSystem, ProfileFingerprintTarget, Runtime},
    ids::ProfileId,
};
use buglogin_storage::blob::checksum;
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "buglogin-independent-adapter-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("browser"), b"adapter-checksum-fixture").unwrap();
        Self(root)
    }
    fn binary(&self) -> RuntimeBinary {
        RuntimeBinary {
            path: self.0.join("browser"),
            version: "148.0.7778.215".into(),
            expected_checksum: checksum(b"adapter-checksum-fixture"),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn target() -> ProfileFingerprintTarget {
    ProfileFingerprintTarget {
        operating_system: if cfg!(target_os = "windows") {
            FingerprintOperatingSystem::Windows
        } else if cfg!(target_os = "macos") {
            FingerprintOperatingSystem::Macos
        } else {
            FingerprintOperatingSystem::Linux
        },
        operating_system_version: None,
    }
}
fn identity() -> ChromiumIdentity {
    ChromiumIdentity::generate("148.0.7778.215".into(), target()).unwrap()
}

#[test]
fn restart_preserves_identity_and_normal_launch_cannot_rotate_it() {
    let fixture = Fixture::new();
    let original = identity();
    let adapter = IndependentChromiumAdapter::new(original.clone()).unwrap();
    let binary = adapter.validate_binary(&fixture.binary()).unwrap();
    let profile = adapter
        .profile_path(&fixture.0, &ProfileId::new("prf_restart").unwrap())
        .unwrap();
    fs::create_dir(&profile).unwrap();
    let first = adapter
        .launch_specification(&binary, &profile, &["--headless=new".into()])
        .unwrap();
    let persisted = fs::read(profile.join("BugLogin/independent-chromium.json")).unwrap();
    let loaded = ChromiumIdentity::load(&profile).unwrap().unwrap();
    assert_eq!(loaded, original);
    let restarted = IndependentChromiumAdapter::new(loaded).unwrap();
    assert_eq!(
        first,
        restarted
            .launch_specification(&binary, &profile, &["--headless=new".into()])
            .unwrap()
    );
    let mut replacement: serde_json::Value = serde_json::from_slice(&persisted).unwrap();
    replacement["seed"] = serde_json::json!(if original.seed() == 1 { 2 } else { 1 });
    let replacement =
        IndependentChromiumAdapter::new(serde_json::from_value(replacement).unwrap()).unwrap();
    assert_eq!(
        replacement
            .launch_specification(&binary, &profile, &[])
            .unwrap_err()
            .code,
        "chromium_identity_conflict"
    );
    assert_eq!(
        fs::read(profile.join("BugLogin/independent-chromium.json")).unwrap(),
        persisted
    );
    assert_eq!(first.runtime, Runtime::Bugium);
    assert!(
        adapter
            .capabilities()
            .contains(&RuntimeCapability::Fingerprint)
    );
}

#[test]
fn identity_and_profile_overrides_are_rejected_before_profile_mutation() {
    let fixture = Fixture::new();
    let adapter = IndependentChromiumAdapter::new(identity()).unwrap();
    let binary = adapter.validate_binary(&fixture.binary()).unwrap();
    for argument in [
        "--user-data-dir=other",
        "--USER-DATA-DIR",
        "--profile-directory=other",
        "--fingerprint=42",
        "--fingerprint-platform=macos",
        "--user-agent=other",
        "--disable-spoofing",
        "--incognito",
        "--guest",
        "-user-data-dir=other",
        "-fingerprint=42",
        "-fingerprint-platform=macos",
        " -user-agent=other ",
    ] {
        assert_eq!(
            adapter
                .launch_specification(&binary, &fixture.0, &[argument.into()])
                .unwrap_err()
                .code,
            "chromium_identity_override_forbidden"
        );
    }
    #[cfg(target_os = "windows")]
    for argument in ["/user-data-dir=other", "/fingerprint=42", "/guest"] {
        assert_eq!(
            adapter
                .launch_specification(&binary, &fixture.0, &[argument.into()])
                .unwrap_err()
                .code,
            "chromium_identity_override_forbidden"
        );
    }
    assert!(!fixture.0.join("BugLogin").exists());
}

#[test]
fn binary_version_and_checksum_are_pinned() {
    let fixture = Fixture::new();
    let adapter = IndependentChromiumAdapter::new(identity()).unwrap();
    let mut binary = fixture.binary();
    binary.expected_checksum = "0".repeat(64);
    assert_eq!(
        adapter.validate_binary(&binary).unwrap_err().code,
        "runtime_checksum_mismatch"
    );
    binary = fixture.binary();
    binary.version = "151.0.0.1".into();
    assert_eq!(
        adapter.validate_binary(&binary).unwrap_err().code,
        "chromium_identity_version_mismatch"
    );
    let mut validated = adapter.validate_binary(&fixture.binary()).unwrap();
    validated.version = "151.0.0.1".into();
    assert_eq!(
        adapter
            .launch_specification(&validated, &fixture.0, &[])
            .unwrap_err()
            .code,
        "chromium_identity_version_mismatch"
    );
}

#[test]
fn known_canvas_crashing_engine_cannot_be_selected() {
    assert_eq!(
        ChromiumIdentity::generate("150.0.7871.186".into(), target())
            .unwrap_err()
            .code,
        "chromium_engine_regression"
    );
}

#[test]
fn existing_wayfern_and_untracked_chromium_profiles_are_never_reidentified() {
    for marker in ["BugLogin/fingerprint.json", "Local State"] {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.0.join(marker).parent().unwrap()).unwrap();
        fs::write(fixture.0.join(marker), b"existing-profile-data").unwrap();
        let adapter = IndependentChromiumAdapter::new(identity()).unwrap();
        let binary = adapter.validate_binary(&fixture.binary()).unwrap();
        assert_eq!(
            adapter
                .launch_specification(&binary, &fixture.0, &[])
                .unwrap_err()
                .code,
            "chromium_profile_engine_conflict"
        );
        assert_eq!(
            fs::read(fixture.0.join(marker)).unwrap(),
            b"existing-profile-data"
        );
        assert!(
            !fixture
                .0
                .join("BugLogin/independent-chromium.json")
                .exists()
        );
        assert!(
            !fixture
                .0
                .join("BugLogin/.independent-chromium.lock")
                .exists()
        );
    }
}

#[test]
fn oversized_stored_identity_is_rejected_without_regeneration() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("BugLogin")).unwrap();
    fs::write(
        fixture.0.join("BugLogin/independent-chromium.json"),
        vec![b' '; 16 * 1024 + 1],
    )
    .unwrap();
    assert_eq!(
        ChromiumIdentity::load(&fixture.0).unwrap_err().code,
        "chromium_identity_invalid"
    );
}

#[test]
fn corrupt_identity_and_unknown_targets_never_fall_back_to_a_new_device() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("BugLogin")).unwrap();
    fs::write(
        fixture.0.join("BugLogin/independent-chromium.json"),
        b"broken",
    )
    .unwrap();
    assert_eq!(
        ChromiumIdentity::load(&fixture.0).unwrap_err().code,
        "chromium_identity_invalid"
    );
    let mut unsupported = target();
    unsupported.operating_system = FingerprintOperatingSystem::Android;
    assert_eq!(
        ChromiumIdentity::generate("148.0.7778.215".into(), unsupported)
            .unwrap_err()
            .code,
        "chromium_fingerprint_target_unsupported"
    );
    assert_eq!(
        ChromiumIdentity::generate("150".into(), target())
            .unwrap_err()
            .code,
        "chromium_identity_invalid"
    );
}

#[test]
fn profile_isolation_keeps_identity_under_each_confined_directory() {
    let fixture = Fixture::new();
    for id in ["prf_isolated_a", "prf_isolated_b"] {
        let adapter = IndependentChromiumAdapter::new(identity()).unwrap();
        let binary = adapter.validate_binary(&fixture.binary()).unwrap();
        let profile = adapter
            .profile_path(&fixture.0, &ProfileId::new(id).unwrap())
            .unwrap();
        fs::create_dir(&profile).unwrap();
        let specification = adapter
            .launch_specification(&binary, &profile, &[])
            .unwrap();
        assert_eq!(
            specification
                .args
                .iter()
                .filter(|arg| arg.starts_with("--user-data-dir="))
                .count(),
            1
        );
        assert!(specification.args[0].ends_with(id));
        assert!(profile.join("BugLogin/independent-chromium.json").is_file());
    }
    assert!(!fixture.0.join("BugLogin").exists());
}
