#![cfg(any(target_os = "windows", target_os = "macos"))]

use buglogin_browser::process::{ApprovedExecutable, ProcessController};
use buglogin_browser::runtime::{
    independent_chromium::fury::{FuryChromiumAdapter, FuryIdentity},
    RuntimeAdapter, RuntimeBinary,
};
use buglogin_domain::entities::{FingerprintOperatingSystem, ProfileFingerprintTarget};
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "buglogin-fury-contract-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("browser"), b"fury-contract-fixture").unwrap();
        Self(root)
    }
    fn binary(&self) -> RuntimeBinary {
        RuntimeBinary {
            path: self.0.join("browser"),
            version: "155.0.8059.39".into(),
            expected_checksum: buglogin_storage::blob::checksum(b"fury-contract-fixture"),
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
        } else {
            FingerprintOperatingSystem::Macos
        },
        operating_system_version: None,
    }
}
fn capture() -> Value {
    serde_json::from_str(if cfg!(target_os = "windows") {
        include_str!("fixtures/fury-smoke-windows.json")
    } else {
        include_str!("fixtures/fury-smoke-macos.json")
    })
    .unwrap()
}
fn identity() -> FuryIdentity {
    FuryIdentity::generate(
        "155.0.8059.39".into(),
        buglogin_storage::blob::checksum(b"fury-contract-fixture"),
        target(),
        capture(),
    )
    .unwrap()
}

#[test]
fn native_identity_persists_and_does_not_expose_its_config_in_argv() {
    let fixture = Fixture::new();
    let original = identity();
    let adapter = FuryChromiumAdapter::new(original.clone()).unwrap();
    let binary = adapter.validate_binary(&fixture.binary()).unwrap();
    let spec = adapter
        .launch_specification(&binary, &fixture.0, &["--headless=new".into()])
        .unwrap();
    assert_eq!(FuryIdentity::load(&fixture.0).unwrap().unwrap(), original);
    assert_eq!(
        spec.args
            .iter()
            .filter(|arg| arg.starts_with("--user-data-dir="))
            .count(),
        1
    );
    assert!(!spec.args.iter().any(|arg| arg.contains("fury-fp")
        || arg.contains("canvasSeed")
        || arg.contains("UNMASKED_RENDERER_WEBGL")));
    let config = fs::read(fixture.0.join("BugLogin/fury-fingerprint.json")).unwrap();
    assert_eq!(
        adapter
            .launch_specification(&binary, &fixture.0, &["--headless=new".into()])
            .unwrap(),
        spec
    );

    let mut substituted = fixture.binary();
    substituted.path = fixture.0.join("other-browser");
    fs::write(&substituted.path, b"substituted-fury-contract-fixture").unwrap();
    substituted.expected_checksum =
        buglogin_storage::blob::checksum(b"substituted-fury-contract-fixture");
    assert_eq!(
        adapter.validate_binary(&substituted).unwrap_err().code,
        "fury_checksum_mismatch"
    );
    assert_eq!(
        fs::read(fixture.0.join("BugLogin/fury-fingerprint.json")).unwrap(),
        config
    );
}

#[test]
fn artifact_replacement_after_validation_is_rejected_at_process_launch() {
    let fixture = Fixture::new();
    let adapter = FuryChromiumAdapter::new(identity()).unwrap();
    let binary = adapter.validate_binary(&fixture.binary()).unwrap();
    let specification = adapter
        .launch_specification(&binary, &fixture.0, &[])
        .unwrap();
    fs::write(&fixture.0.join("browser"), b"replaced-after-validation").unwrap();

    let approved = ApprovedExecutable::new(&fixture.0.join("browser")).unwrap();
    let controller =
        buglogin_browser::runtime::independent_chromium::fury::FuryProcessController::new([
            approved,
        ]);
    assert_eq!(
        controller.launch(&specification).err().unwrap().code,
        "fury_checksum_mismatch"
    );
}

#[test]
fn conflicting_or_modified_identity_never_silently_regenerates() {
    let fixture = Fixture::new();
    let adapter = FuryChromiumAdapter::new(identity()).unwrap();
    let binary = adapter.validate_binary(&fixture.binary()).unwrap();
    adapter
        .launch_specification(&binary, &fixture.0, &[])
        .unwrap();
    let other = FuryChromiumAdapter::new(identity()).unwrap();
    assert_eq!(
        other
            .launch_specification(&binary, &fixture.0, &[])
            .unwrap_err()
            .code,
        "fury_identity_conflict"
    );
    fs::write(fixture.0.join("BugLogin/fury-fingerprint.json"), b"{}").unwrap();
    assert_eq!(
        adapter
            .launch_specification(&binary, &fixture.0, &[])
            .unwrap_err()
            .code,
        "fury_config_conflict"
    );
}

#[test]
fn transport_and_identity_switches_cannot_be_injected() {
    let fixture = Fixture::new();
    let adapter = FuryChromiumAdapter::new(identity()).unwrap();
    let binary = adapter.validate_binary(&fixture.binary()).unwrap();
    for argument in [
        "--fury-fp-fd=3",
        "--fury-fp-file-handle=4",
        "-fury-fp-handle=7",
        "--fingerprint=0",
        "--user-data-dir=other",
        "-guest",
    ] {
        assert_eq!(
            adapter
                .launch_specification(&binary, &fixture.0, &[argument.into()])
                .unwrap_err()
                .code,
            "fury_identity_override_forbidden"
        );
    }
    assert!(!fixture.0.join("BugLogin").exists());
}

#[test]
fn foreign_profiles_are_untouched_and_invalid_configs_rejected() {
    let fixture = Fixture::new();
    let adapter = FuryChromiumAdapter::new(identity()).unwrap();
    let binary = adapter.validate_binary(&fixture.binary()).unwrap();
    fs::write(fixture.0.join("Local State"), b"existing-data").unwrap();
    assert_eq!(
        adapter
            .launch_specification(&binary, &fixture.0, &[])
            .unwrap_err()
            .code,
        "fury_profile_engine_conflict"
    );
    assert!(!fixture.0.join("BugLogin").exists());
    for field in ["platform", "hardwareConcurrency", "deviceMemory"] {
        let mut invalid = capture();
        invalid["navigator"][field] = Value::from("invalid");
        assert!(FuryIdentity::generate(
            "155.0.8059.39".into(),
            buglogin_storage::blob::checksum(b"fury-contract-fixture"),
            target(),
            invalid,
        )
        .is_err());
    }
    let mut invalid = capture();
    invalid["clientHints"]["fullVersion"] = Value::from("154.0.0.1");
    assert!(FuryIdentity::generate(
        "155.0.8059.39".into(),
        buglogin_storage::blob::checksum(b"fury-contract-fixture"),
        target(),
        invalid,
    )
    .is_err());
}

#[test]
fn pinned_core_contract_is_required_and_profile_noise_is_stable() {
    let fixture = Fixture::new();
    let adapter = FuryChromiumAdapter::new(identity()).unwrap();
    let binary = adapter.validate_binary(&fixture.binary()).unwrap();
    adapter
        .launch_specification(&binary, &fixture.0, &[])
        .unwrap();
    let config_path = fixture.0.join("BugLogin/fury-fingerprint.json");
    let config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    for path in [
        "clientHints.formFactors",
        "gpu.webglParams.UNMASKED_RENDERER_WEBGL",
        "audio.sampleRate",
        "fontsHidden",
        "webrtc.ipHandlingPolicy",
        "permissions.notifications",
        "engine.jsHeapSizeLimit",
    ] {
        assert!(
            lookup(&config, path).is_some(),
            "missing core field: {path}"
        );
    }
    let first_noise = config["noise"].clone();
    adapter
        .launch_specification(&binary, &fixture.0, &[])
        .unwrap();
    let second: Value = serde_json::from_slice(&fs::read(config_path).unwrap()).unwrap();
    assert_eq!(first_noise, second["noise"]);

    let identity_path = fixture.0.join("BugLogin/fury-identity.json");
    let mut stored: Value = serde_json::from_slice(&fs::read(&identity_path).unwrap()).unwrap();
    stored["fingerprint"]["noise"]["canvasSeed"] = Value::from(
        stored["fingerprint"]["noise"]["canvasSeed"]
            .as_i64()
            .unwrap()
            .wrapping_add(1),
    );
    fs::write(identity_path, serde_json::to_vec(&stored).unwrap()).unwrap();
    assert!(FuryIdentity::load(&fixture.0).is_err());

    let mut incomplete = capture();
    incomplete.as_object_mut().unwrap().remove("fontsHidden");
    assert!(FuryIdentity::generate(
        "155.0.8059.39".into(),
        buglogin_storage::blob::checksum(b"fury-contract-fixture"),
        target(),
        incomplete,
    )
    .is_err());
}

fn lookup<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(value, |node, part| node.get(part))
}
