use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use buglogin_domain::entities::Vpn;
use buglogin_migration::{
    MigrationKey, activate_package_with_vault,
    inspect::inspect_source,
    legacy::map_legacy_vpns,
    package::{NeutralPackage, PackageMetadata, build_encrypted_profile_package},
    plan_package_activation_with_vault,
    verify::verify_package,
};
use buglogin_storage::{
    local::LocalRepository,
    secrets::{OsVault, RotationReceipt, SecretError, SecretKey, SecretValue, SecretVersion},
};
use buglogin_sync::crypto::{AuthenticatedChunk, ClientKey};

#[derive(Default)]
struct FakeVault {
    values: BTreeMap<(String, String), (SecretValue, SecretVersion)>,
}

impl OsVault for FakeVault {
    fn read(&self, key: &SecretKey) -> Result<(SecretValue, SecretVersion), SecretError> {
        self.values
            .get(&(key.namespace().into(), key.name().into()))
            .cloned()
            .ok_or(SecretError::NotFound)
    }

    fn create(
        &mut self,
        key: &SecretKey,
        value: SecretValue,
    ) -> Result<SecretVersion, SecretError> {
        let version = SecretVersion::new(1).unwrap();
        if self
            .values
            .insert(
                (key.namespace().into(), key.name().into()),
                (value, version),
            )
            .is_some()
        {
            return Err(SecretError::VersionConflict);
        }
        Ok(version)
    }

    fn rotate(
        &mut self,
        _key: &SecretKey,
        _expected: SecretVersion,
        _replacement: SecretValue,
    ) -> Result<RotationReceipt, SecretError> {
        Err(SecretError::OperationFailed)
    }

    fn remove(&mut self, key: &SecretKey, expected: SecretVersion) -> Result<(), SecretError> {
        let map_key = (key.namespace().into(), key.name().into());
        let (_, version) = self.values.get(&map_key).ok_or(SecretError::NotFound)?;
        if *version != expected {
            return Err(SecretError::VersionConflict);
        }
        self.values.remove(&map_key);
        Ok(())
    }
}

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "buglogin-package-vpn-{label}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, relative: &str, value: &[u8]) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn package_plan_and_activation_wire_vpn_records_and_encrypted_configs() {
    let source = Temp::new("source");
    source.write(
        "version.json",
        br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"package-vpn","os":"windows","architecture":"x86_64"}"#,
    );
    let legacy_key = [73_u8; 32];
    let config = "[Interface]\nPrivateKey = package-private-key\nAddress = 10.0.0.2/24\n[Peer]\nPublicKey = package-public-key\nEndpoint = vpn.example.com:51820\n";
    let (ciphertext, nonce) = encrypt_legacy(legacy_key, config);
    source.write("vpn/.vpn_key", &legacy_key);
    source.write(
        "vpn/vpn_configs.json",
        format!(r#"{{"version":1,"configs":[{{"id":"wg","name":"Package VPN","vpn_type":"WireGuard","encrypted_data":"{ciphertext}","nonce":"{nonce}","created_at":1,"last_used":null}}]}}"#).as_bytes(),
    );

    let (report, snapshot) = inspect_source(source.path()).unwrap();
    let key = MigrationKey::from_bytes([74; 32]);
    let package = build_encrypted_profile_package(
        source.path(),
        &report,
        &snapshot,
        &PackageMetadata {
            created_at: "2026-07-21T00:00:00Z".into(),
            tool_version: "0.1.0".into(),
            tool_commit: "0123456789abcdef0123456789abcdef01234567".into(),
        },
        &key,
    )
    .unwrap();
    verify_package(&package).expect("verify encrypted VPN package");
    let manifest: serde_json::Value =
        serde_json::from_slice(package.file("manifest.json").unwrap()).unwrap();
    assert_eq!(manifest["security"]["contains_secrets"], true);
    assert_eq!(manifest["security"]["encrypted"], true);
    let mapped = map_legacy_vpns(source.path(), &report.source.instance_id).unwrap();
    let vpn_id = mapped.records[0].id.clone();
    let config_ref = mapped.records[0].data.config_ref.clone();
    let destination = Temp::new("destination");
    fs::remove_dir(destination.path()).unwrap();
    let mut vault = FakeVault::default();

    let plan =
        plan_package_activation_with_vault(destination.path(), &package, &key, &vault).unwrap();
    assert!(!destination.path().exists());
    assert_eq!(plan.create_vpn_config_ids, vec![config_ref.clone()]);
    assert_eq!(plan.create_vpn_ids, vec![vpn_id.clone()]);

    let first = activate_package_with_vault(
        destination.path(),
        "package-vpn",
        &package,
        &key,
        &mut vault,
    )
    .unwrap();
    assert_eq!(first.created_vpn_configs, 1);
    assert_eq!(first.created_vpns, 1);
    assert!(
        LocalRepository::open(destination.path())
            .unwrap()
            .get::<Vpn>(&vpn_id)
            .unwrap()
            .is_some()
    );
    let secret_key = SecretKey::new("vpn-configs", config_ref.clone()).unwrap();
    let (secret, version) = vault.read(&secret_key).unwrap();
    assert_eq!(version.generation(), 1);
    let stored: buglogin_migration::encrypted_vpn_configs::VpnConfigSecret =
        serde_json::from_slice(secret.as_bytes()).unwrap();
    assert_eq!(stored.config_data, config);

    let rerun = activate_package_with_vault(
        destination.path(),
        "package-vpn",
        &package,
        &key,
        &mut vault,
    )
    .unwrap();
    assert_eq!(rerun.reused_vpn_configs, 1);
    assert_eq!(rerun.reused_vpns, 1);
}

#[test]
fn package_rejects_vpn_records_without_exact_encrypted_config_set() {
    let source = Temp::new("mismatch");
    source.write(
        "version.json",
        br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"package-vpn-mismatch","os":"windows","architecture":"x86_64"}"#,
    );
    let legacy_key = [75_u8; 32];
    let config = "[Interface]\nPrivateKey = private\nAddress = 10.0.0.2/24\n[Peer]\nPublicKey = public\nEndpoint = host:51820\n";
    let (ciphertext, nonce) = encrypt_legacy(legacy_key, config);
    source.write("vpn/.vpn_key", &legacy_key);
    source.write(
        "vpn/vpn_configs.json",
        format!(r#"{{"version":1,"configs":[{{"id":"wg","name":"VPN","vpn_type":"WireGuard","encrypted_data":"{ciphertext}","nonce":"{nonce}","created_at":1}}]}}"#).as_bytes(),
    );
    let (report, snapshot) = inspect_source(source.path()).unwrap();
    let key = MigrationKey::from_bytes([76; 32]);
    let package = build_encrypted_profile_package(
        source.path(),
        &report,
        &snapshot,
        &PackageMetadata {
            created_at: "2026-07-21T00:00:00Z".into(),
            tool_version: "0.1.0".into(),
            tool_commit: "0123456789abcdef0123456789abcdef01234567".into(),
        },
        &key,
    )
    .unwrap();
    let mut files = package.files().clone();
    files.remove("entities/vpns.jsonl");
    let package = NeutralPackage::from_files(files);

    assert_eq!(
        plan_package_activation_with_vault(
            Temp::new("blocked").path(),
            &package,
            &key,
            &FakeVault::default(),
        )
        .unwrap_err()
        .code,
        "migration_vpn_config_set_mismatch"
    );
}

fn encrypt_legacy(key: [u8; 32], plaintext: &str) -> (String, String) {
    let chunk = AuthenticatedChunk::encrypt(&ClientKey::from_bytes(key), plaintext.as_bytes(), &[])
        .unwrap();
    let mut ciphertext = chunk.ciphertext;
    ciphertext.extend_from_slice(&chunk.tag);
    (encode_base64(&ciphertext), encode_base64(&chunk.nonce))
}

fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = *chunk.get(1).unwrap_or(&0);
        let third = *chunk.get(2).unwrap_or(&0);
        encoded.push(TABLE[(first >> 2) as usize] as char);
        encoded.push(TABLE[((first & 0x03) << 4 | second >> 4) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            TABLE[((second & 0x0f) << 2 | third >> 6) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            TABLE[(third & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    encoded
}
