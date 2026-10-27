use std::{fs, path::PathBuf};

use buglogin_browser::runtime::{
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
    assert_eq!(spec.args.len(), 3);
    let user_data_dir = spec.args[0]
        .strip_prefix("--user-data-dir=")
        .expect("launch spec must set the isolated user data directory");
    assert!(PathBuf::from(user_data_dir).ends_with("profile_1"));
    assert_eq!(&spec.args[1..], ["--headless", "--disable-background-mode"]);
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
