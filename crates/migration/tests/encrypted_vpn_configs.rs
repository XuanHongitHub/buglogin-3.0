use std::collections::BTreeMap;

use buglogin_domain::entities::VpnKind;
use buglogin_migration::{MigrationKey, package::NeutralPackage};
use buglogin_storage::blob::checksum;

mod package {
    pub use buglogin_migration::package::NeutralPackage;
}

#[path = "../src/encrypted_vpn_configs.rs"]
mod encrypted_vpn_configs;

use encrypted_vpn_configs::{VpnConfigSecret, decrypt_packaged_vpn_configs, encrypt_vpn_configs};

fn secret(config_ref: &str, vpn_id: &str, config_data: &str) -> VpnConfigSecret {
    VpnConfigSecret {
        schema: "buglogin.vpn-config-secret/1".into(),
        config_ref: config_ref.into(),
        vpn_id: vpn_id.into(),
        kind: VpnKind::Wireguard,
        config_sha256: checksum(config_data.as_bytes()),
        config_data: config_data.into(),
    }
}

fn package(encrypted: encrypted_vpn_configs::EncryptedVpnConfigs) -> NeutralPackage {
    let mut files = BTreeMap::new();
    files.insert(
        "encrypted/vpn-configs/index.json".into(),
        serde_json::to_vec(&encrypted.index).unwrap(),
    );
    files.extend(encrypted.objects);
    NeutralPackage::from_files(files)
}

#[test]
fn encrypts_decrypts_and_redacts_vpn_configs() {
    let key = MigrationKey::from_bytes([101; 32]);
    let raw_config = "[Interface]\nPrivateKey = production-private-key\n[Peer]\nPublicKey = peer";
    let configs = vec![secret(
        "11111111-1111-5111-8111-111111111111",
        "22222222-2222-5222-8222-222222222222",
        raw_config,
    )];
    let encrypted = encrypt_vpn_configs(&configs, &key).unwrap();
    assert_eq!(
        encrypted.index.configs[0].object_key,
        "encrypted/vpn-configs/11111111-1111-5111-8111-111111111111/0"
    );
    let package = package(encrypted);
    let package_bytes = package
        .files()
        .values()
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    assert!(!contains(&package_bytes, raw_config.as_bytes()));
    assert!(!format!("{:?}", configs[0]).contains("production-private-key"));
    assert_eq!(
        decrypt_packaged_vpn_configs(&package, &key).unwrap(),
        configs
    );
    assert!(decrypt_packaged_vpn_configs(&package, &MigrationKey::from_bytes([102; 32])).is_err());
}

#[test]
fn rejects_tamper_and_non_exact_object_sets() {
    let key = MigrationKey::from_bytes([103; 32]);
    let encrypted = encrypt_vpn_configs(
        &[secret(
            "33333333-3333-5333-8333-333333333333",
            "44444444-4444-5444-8444-444444444444",
            "client\ndev tun\n<key>private</key>",
        )],
        &key,
    )
    .unwrap();
    let mut tampered_files = package(encrypted.clone()).files().clone();
    tampered_files
        .iter_mut()
        .find(|(path, _)| path.as_str() != "encrypted/vpn-configs/index.json")
        .unwrap()
        .1[0] ^= 1;
    let error = decrypt_packaged_vpn_configs(&NeutralPackage::from_files(tampered_files), &key)
        .unwrap_err();
    assert_eq!(error.code, "migration_vpn_config_object_tampered");

    let mut extra_files = package(encrypted).files().clone();
    extra_files.insert("encrypted/vpn-configs/orphan/0".into(), vec![1]);
    let error =
        decrypt_packaged_vpn_configs(&NeutralPackage::from_files(extra_files), &key).unwrap_err();
    assert_eq!(error.code, "migration_vpn_config_object_set_mismatch");
}

#[test]
fn rejects_hash_and_index_binding_mismatches() {
    let key = MigrationKey::from_bytes([104; 32]);
    let mut invalid = secret(
        "55555555-5555-5555-8555-555555555555",
        "66666666-6666-5666-8666-666666666666",
        "[Interface]\nPrivateKey = secret",
    );
    invalid.config_sha256 = "0".repeat(64);
    assert_eq!(
        encrypt_vpn_configs(&[invalid], &key).unwrap_err().code,
        "migration_vpn_config_invalid"
    );

    let encrypted = encrypt_vpn_configs(
        &[secret(
            "77777777-7777-5777-8777-777777777777",
            "88888888-8888-5888-8888-888888888888",
            "[Interface]\nPrivateKey = secret",
        )],
        &key,
    )
    .unwrap();
    let mut files = package(encrypted).files().clone();
    let mut index: encrypted_vpn_configs::EncryptedVpnConfigIndex =
        serde_json::from_slice(files.get("encrypted/vpn-configs/index.json").unwrap()).unwrap();
    index.configs[0].vpn_id = "99999999-9999-5999-8999-999999999999".into();
    files.insert(
        "encrypted/vpn-configs/index.json".into(),
        serde_json::to_vec(&index).unwrap(),
    );
    let error = decrypt_packaged_vpn_configs(&NeutralPackage::from_files(files), &key).unwrap_err();
    assert_eq!(error.code, "migration_vpn_config_index_mismatch");
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
