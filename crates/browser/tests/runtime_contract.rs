use std::{fs, path::PathBuf};

use buglogin_browser::runtime::{
    bugmini::BugminiAdapter,
    contract::{RuntimeAdapter, RuntimeBinary, RuntimeCapability},
    wayfern::WayfernAdapter,
};
use buglogin_domain::{entities::Runtime, ids::ProfileId};
use buglogin_storage::blob::checksum;

fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("buglogin-runtime-{label}-{}", std::process::id()))
}

#[test]
fn wayfern_validates_checksum_and_builds_launch_specification() {
    let root = temp_root("valid");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let binary_path = root.join("wayfern");
    let bytes = b"wayfern-test-binary";
    fs::write(&binary_path, bytes).unwrap();
    let adapter = WayfernAdapter;
    let binary = adapter
        .validate_binary(&RuntimeBinary {
            path: binary_path.clone(),
            version: "1.0.0".into(),
            expected_checksum: checksum(bytes),
        })
        .unwrap();
    let profile_path = adapter
        .profile_path(&root, &ProfileId::from_str("profile_1").unwrap())
        .unwrap();
    let spec = adapter
        .launch_specification(&binary, &profile_path, &["--headless".into()])
        .unwrap();
    assert_eq!(adapter.runtime(), Runtime::Wayfern);
    assert!(adapter.capabilities().contains(&RuntimeCapability::Proxy));
    assert_eq!(spec.executable, binary_path);
    assert_eq!(spec.args[0], "--user-data-dir");
    assert_eq!(spec.args[2], "--headless");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn bugmini_uses_verified_chromium_launch_contract_without_claiming_fingerprint() {
    let root = temp_root("bugmini");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let binary_path = root.join("chrome.exe");
    let bytes = b"bugmini-test-binary";
    fs::write(&binary_path, bytes).unwrap();
    let adapter = BugminiAdapter;
    let binary = adapter
        .validate_binary(&RuntimeBinary {
            path: binary_path.clone(),
            version: "146.0.7680.177.4".into(),
            expected_checksum: checksum(bytes),
        })
        .unwrap();
    let profile_path = adapter
        .profile_path(&root, &ProfileId::from_str("profile_1").unwrap())
        .unwrap();
    let specification = adapter
        .launch_specification(
            &binary,
            &profile_path,
            &["--remote-debugging-port=0".into()],
        )
        .unwrap();

    assert_eq!(adapter.runtime(), Runtime::Bugmini);
    assert_eq!(specification.runtime, Runtime::Bugmini);
    assert_eq!(specification.executable, binary_path);
    assert!(adapter.capabilities().contains(&RuntimeCapability::Cookies));
    assert!(
        !adapter
            .capabilities()
            .contains(&RuntimeCapability::Fingerprint)
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn wayfern_rejects_checksum_mismatch_without_launching() {
    let root = temp_root("checksum-mismatch");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let binary_path = root.join("wayfern");
    fs::write(&binary_path, b"wayfern-test-binary").unwrap();
    let result = WayfernAdapter.validate_binary(&RuntimeBinary {
        path: binary_path,
        version: "1.0.0".into(),
        expected_checksum: "0".repeat(64),
    });
    assert_eq!(result.unwrap_err().code, "runtime_checksum_mismatch");
    let _ = fs::remove_dir_all(root);
}
