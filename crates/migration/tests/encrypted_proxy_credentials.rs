use std::collections::BTreeMap;

use buglogin_migration::{
    MigrationKey,
    encrypted_secrets::{
        ProxyCredentialSecret, decrypt_packaged_proxy_credentials, encrypt_proxy_credentials,
    },
    package::NeutralPackage,
};

#[test]
fn encrypts_decrypts_and_redacts_proxy_credentials() {
    let key = MigrationKey::from_bytes([61; 32]);
    let credentials = vec![ProxyCredentialSecret {
        credential_id: "11111111-1111-5111-8111-111111111111".into(),
        username: Some("secret-user".into()),
        password: Some("secret-password".into()),
    }];
    let encrypted = encrypt_proxy_credentials(&credentials, &key).unwrap();
    let index_json = serde_json::to_string(&encrypted.index).unwrap();
    assert!(!index_json.contains("plaintextSha256"));
    assert_eq!(
        encrypted.index.schema,
        "buglogin.encrypted-proxy-credentials/2"
    );
    let mut files = BTreeMap::new();
    files.insert(
        "encrypted/proxy-credentials/index.json".into(),
        serde_json::to_vec(&encrypted.index).unwrap(),
    );
    files.extend(encrypted.objects);
    let package = NeutralPackage::from_files(files);
    let package_bytes = package
        .files()
        .values()
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    assert!(
        !package_bytes
            .windows(b"secret-user".len())
            .any(|value| value == b"secret-user")
    );
    assert!(
        !package_bytes
            .windows(b"secret-password".len())
            .any(|value| value == b"secret-password")
    );
    let debug = format!("{:?}", credentials[0]);
    assert!(!debug.contains("secret-user"));
    assert!(!debug.contains("secret-password"));

    let decrypted = decrypt_packaged_proxy_credentials(&package, &key).unwrap();
    assert_eq!(decrypted, credentials);
    assert!(
        decrypt_packaged_proxy_credentials(&package, &MigrationKey::from_bytes([62; 32]),).is_err()
    );
}

#[test]
fn rejects_tampered_proxy_credential_object() {
    let key = MigrationKey::from_bytes([63; 32]);
    let encrypted = encrypt_proxy_credentials(
        &[ProxyCredentialSecret {
            credential_id: "22222222-2222-5222-8222-222222222222".into(),
            username: Some("user".into()),
            password: None,
        }],
        &key,
    )
    .unwrap();
    let mut files = BTreeMap::new();
    files.insert(
        "encrypted/proxy-credentials/index.json".into(),
        serde_json::to_vec(&encrypted.index).unwrap(),
    );
    files.extend(encrypted.objects);
    let object = files
        .iter_mut()
        .find(|(path, _)| path.as_str() != "encrypted/proxy-credentials/index.json")
        .unwrap()
        .1;
    object[0] ^= 1;
    let error =
        decrypt_packaged_proxy_credentials(&NeutralPackage::from_files(files), &key).unwrap_err();
    assert_eq!(error.code, "migration_proxy_credential_object_tampered");
}
