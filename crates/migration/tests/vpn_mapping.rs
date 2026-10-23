use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use buglogin_domain::ids::migration_uuid;
use buglogin_migration::legacy::map_legacy_vpns;
use buglogin_sync::crypto::{AuthenticatedChunk, ClientKey};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "buglogin-vpn-mapping-{}-{}",
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
fn decrypts_and_maps_wireguard_and_openvpn_without_entity_plaintext() {
    let source = Temp::new();
    let key = [17_u8; 32];
    let wireguard = "[Interface]\nPrivateKey = private\nAddress = 10.0.0.2/24\n[Peer]\nPublicKey = public\nEndpoint = vpn.example.com:51820\n";
    let openvpn = "client\nremote vpn.example.com 1194\n<ca>\ncertificate\n</ca>\n<cert>\ncertificate\n</cert>\n<key>\nprivate\n</key>\n";
    let (wireguard_ciphertext, wireguard_nonce) = encrypt_legacy(key, wireguard);
    let (openvpn_ciphertext, openvpn_nonce) = encrypt_legacy(key, openvpn);
    source.write("vpn/.vpn_key", &key);
    source.write(
        "vpn/vpn_configs.json",
        format!(
            r#"{{"version":1,"configs":[{{"id":"wg","name":"WireGuard","vpn_type":"WireGuard","encrypted_data":"{wireguard_ciphertext}","nonce":"{wireguard_nonce}","created_at":1784592000,"last_used":1784592060,"sync_enabled":true,"last_sync":1784592070}},{{"id":"ovpn","name":"OpenVPN","vpn_type":"OpenVPN","encrypted_data":"{openvpn_ciphertext}","nonce":"{openvpn_nonce}","created_at":1784592000,"last_used":null,"sync_enabled":false}}]}}"#
        )
        .as_bytes(),
    );

    let mapped = map_legacy_vpns(source.path(), "vpn-source").unwrap();
    assert_eq!(mapped.records.len(), 2);
    assert_eq!(mapped.configs.len(), 2);
    let wireguard_record = mapped
        .records
        .iter()
        .find(|record| record.source_ref.id == "wg")
        .unwrap();
    assert_eq!(
        wireguard_record.id,
        migration_uuid("buglogin-legacy", "vpn-source", "vpn", "wg").to_string()
    );
    assert_eq!(wireguard_record.data.kind, "wireguard");
    assert_eq!(wireguard_record.data.sync_mode, "managed");
    assert_eq!(
        wireguard_record.data.config_ref,
        migration_uuid("buglogin-legacy", "vpn-source", "vpn-config", "wg").to_string()
    );
    let entity_json = serde_json::to_string(&mapped.records).unwrap();
    assert!(!entity_json.contains("PrivateKey"));
    assert!(!entity_json.contains("private"));
    assert_eq!(
        mapped
            .configs
            .iter()
            .find(|config| config.vpn_id == wireguard_record.id)
            .unwrap()
            .config_data(),
        wireguard
    );
    assert!(!format!("{:?}", mapped.configs).contains("PrivateKey"));
}

#[test]
fn rejects_wrong_key_nonce_version_duplicate_and_tamper() {
    let cases = ["wrong-key", "nonce", "version", "duplicate", "tamper"];
    for case in cases {
        let source = Temp::new();
        let key = [23_u8; 32];
        let config = "[Interface]\nPrivateKey = private\nAddress = 10.0.0.2/24\n[Peer]\nPublicKey = public\nEndpoint = host:51820\n";
        let (mut ciphertext, mut nonce) = encrypt_legacy(key, config);
        let mut stored_key = key.to_vec();
        let mut version = 1;
        let duplicate = case == "duplicate";
        match case {
            "wrong-key" => stored_key = vec![24_u8; 32],
            "nonce" => nonce = "AA==".into(),
            "version" => version = 2,
            "tamper" => {
                let replacement = if ciphertext.starts_with('A') {
                    "B"
                } else {
                    "A"
                };
                ciphertext.replace_range(0..1, replacement);
            }
            _ => {}
        }
        source.write("vpn/.vpn_key", &stored_key);
        let second = if duplicate {
            format!(
                r#",{{"id":"b","name":"B","vpn_type":"WireGuard","encrypted_data":"{ciphertext}","nonce":"{nonce}","created_at":1}}"#
            )
        } else {
            String::new()
        };
        source.write(
            "vpn/vpn_configs.json",
            format!(r#"{{"version":{version},"configs":[{{"id":"a","name":"A","vpn_type":"WireGuard","encrypted_data":"{ciphertext}","nonce":"{nonce}","created_at":1}}{second}]}}"#).as_bytes(),
        );
        assert_eq!(
            map_legacy_vpns(source.path(), "fail-closed")
                .unwrap_err()
                .code,
            "migration_vpn_invalid",
            "case {case}"
        );
    }
}

#[test]
fn rejects_invalid_configs_and_openvpn_external_paths() {
    let cases = [
        (
            "WireGuard",
            "[Interface]\nPrivateKey=x\n[Peer]\nPublicKey=y\nEndpoint=z\n",
        ),
        ("OpenVPN", "client\nremote vpn.example.com\nca ca.pem\n"),
        ("OpenVPN", "client\ndev tun\n"),
        (
            "OpenVPN",
            "[Interface]\nPrivateKey=x\nAddress=x\n[Peer]\nPublicKey=y\nEndpoint=z\n",
        ),
    ];
    for (index, (vpn_type, config)) in cases.into_iter().enumerate() {
        let source = Temp::new();
        let key = [31_u8; 32];
        let (ciphertext, nonce) = encrypt_legacy(key, config);
        source.write("vpn/.vpn_key", &key);
        source.write(
            "vpn/vpn_configs.json",
            format!(r#"{{"version":1,"configs":[{{"id":"vpn","name":"VPN","vpn_type":"{vpn_type}","encrypted_data":"{ciphertext}","nonce":"{nonce}","created_at":1}}]}}"#).as_bytes(),
        );
        assert_eq!(
            map_legacy_vpns(source.path(), "invalid-config")
                .unwrap_err()
                .code,
            "migration_vpn_invalid",
            "case {index}"
        );
    }
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
        let a = chunk[0];
        let b = *chunk.get(1).unwrap_or(&0);
        let c = *chunk.get(2).unwrap_or(&0);
        encoded.push(TABLE[(a >> 2) as usize] as char);
        encoded.push(TABLE[((a & 0x03) << 4 | b >> 4) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            TABLE[((b & 0x0f) << 2 | c >> 6) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            TABLE[(c & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    encoded
}
