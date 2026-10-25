use buglogin_domain::ids::{ProfileId, migration_uuid};

#[test]
fn ids_validate_and_round_trip() {
    let id = ProfileId::new("prf_123").expect("valid ID");
    let json = serde_json::to_string(&id).expect("serialize");
    assert_eq!(json, "\"prf_123\"");
    let restored: ProfileId = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(restored, id);
    let parsed: ProfileId = "prf_123".parse().expect("parse through FromStr");
    assert_eq!(parsed, id);
}

#[test]
fn ids_reject_invalid_values() {
    assert!(ProfileId::new("").is_err());
    assert!(ProfileId::new("_bad").is_err());
    assert!(ProfileId::new("bad/value").is_err());
}

#[test]
fn migration_uuid_is_deterministic() {
    assert_eq!(
        migration_uuid("legacy", "machine-a", "profile", "42"),
        migration_uuid("legacy", "machine-a", "profile", "42")
    );
    assert_ne!(
        migration_uuid("legacy", "machine-a", "profile", "42"),
        migration_uuid("legacy", "machine-a", "profile", "43")
    );
}
