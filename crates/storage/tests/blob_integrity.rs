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

#[test]
fn concurrent_writes_to_the_same_blob_do_not_share_staging_files() {
    let root = root("blob-concurrent");
    let path = PathBuf::from("profiles/shared.bin");
    let data = vec![0x5a; 128 * 1024];
    let expected = checksum(&data);

    std::thread::scope(|scope| {
        for _ in 0..8 {
            let root = &root;
            let path = &path;
            let data = &data;
            let expected = &expected;
            scope.spawn(move || write_verified(root, path, data, expected).unwrap());
        }
    });

    assert_eq!(read_verified(&root, &path, &expected).unwrap(), data);
    assert_eq!(
        fs::read_dir(root.join("profiles"))
            .unwrap()
            .filter_map(Result::ok)
            .count(),
        1,
        "temporary files must be removed after concurrent publication"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn verified_blob_write_atomically_replaces_an_old_cache_entry() {
    let root = root("blob-replace");
    let path = PathBuf::from("cache/chunk.bin");
    fs::create_dir_all(root.join("cache")).unwrap();
    fs::write(root.join(&path), b"stale cache entry").unwrap();
    let data = b"refreshed cache entry";
    let expected = checksum(data);

    write_verified(&root, &path, data, &expected).unwrap();

    assert_eq!(read_verified(&root, &path, &expected).unwrap(), data);
    let _ = fs::remove_dir_all(root);
}
