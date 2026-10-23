use buglogin_storage::blob::{checksum, read_verified, write_verified};
use std::{fs, path::PathBuf};

fn root(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("buglogin-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn blob_round_trip_verifies_checksum() {
    let root = root("blob-roundtrip");
    let data = b"profile-data";
    let hash = checksum(data);
    write_verified(
        &root,
        PathBuf::from("profiles/data.bin").as_path(),
        data,
        &hash,
    )
    .unwrap();
    assert_eq!(
        read_verified(&root, PathBuf::from("profiles/data.bin").as_path(), &hash).unwrap(),
        data
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn checksum_mismatch_never_publishes() {
    let root = root("blob-mismatch");
    let path = PathBuf::from("bad.bin");
    assert_eq!(
        write_verified(&root, &path, b"data", "00")
            .unwrap_err()
            .code,
        "blob_checksum_mismatch"
    );
    assert!(!root.join(path).exists());
    let _ = fs::remove_dir_all(root);
}
