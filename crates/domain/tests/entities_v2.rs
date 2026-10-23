use buglogin_domain::{
    entities::{
        BrowserCompatibility, Extension, ExtensionGroup, ExtensionPackageType, SyncMode, Vpn,
        VpnKind,
    },
    ids::{ArtifactId, CredentialId, ExtensionGroupId, ExtensionId, VpnId},
};

#[test]
fn vpn_and_extension_entities_match_domain_v2_wire_format() {
    let vpn = Vpn {
        id: VpnId::new("vpn_1").unwrap(),
        name: "Office".into(),
        kind: VpnKind::Wireguard,
        config_ref: CredentialId::new("vpn_config_1").unwrap(),
        last_used_at: None,
        sync_mode: Some(SyncMode::Local),
        created_at: "2026-07-21T00:00:00Z".into(),
        updated_at: "2026-07-21T00:00:00Z".into(),
    };
    let vpn_json = serde_json::to_value(&vpn).unwrap();
    assert_eq!(vpn_json["kind"], "wireguard");
    assert_eq!(vpn_json["configRef"], "vpn_config_1");
    assert!(vpn_json.get("configData").is_none());

    let extension = Extension {
        id: ExtensionId::new("ext_1").unwrap(),
        name: "Captcha".into(),
        file_name: Some("captcha.xpi".into()),
        package_type: ExtensionPackageType::Xpi,
        artifact_ref: ArtifactId::new("artifact_1").unwrap(),
        artifact_sha256: "a".repeat(64),
        compatibility: vec![BrowserCompatibility::Firefox],
        sync_mode: Some(SyncMode::Managed),
        created_at: "2026-07-21T00:00:00Z".into(),
        updated_at: "2026-07-21T00:00:00Z".into(),
    };
    let group = ExtensionGroup {
        id: ExtensionGroupId::new("ext_group_1").unwrap(),
        name: "Firefox".into(),
        extension_ids: vec![extension.id.clone()],
        sync_mode: Some(SyncMode::Managed),
        created_at: "2026-07-21T00:00:00Z".into(),
        updated_at: "2026-07-21T00:00:00Z".into(),
    };
    assert_eq!(
        serde_json::to_value(&extension).unwrap()["artifactRef"],
        "artifact_1"
    );
    assert_eq!(
        serde_json::to_value(&group).unwrap()["extensionIds"][0],
        "ext_1"
    );
}
