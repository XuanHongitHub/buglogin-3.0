#![cfg(feature = "integration-tests")]

use std::collections::BTreeMap;
use std::env;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use buglogin_storage::blob::checksum;
use buglogin_storage::control::{
    ControlAuditEntry, ControlPut, ControlRecordStore, ExecuteControlCommand, HttpResponse,
    PostgresConfig, PostgresControlRecordStore,
};
use buglogin_storage::remote::{RemoteChunkStore, S3Config, S3RemoteChunkStore};
use serde_json::json;

#[test]
fn postgres_tls_adapter_persists_and_replays_atomically() {
    let Some(database_url) = optional_env("BUGLOGIN_POSTGRES_TEST_URL") else {
        return;
    };
    let run_id = run_id();
    let tenant_id = format!("live-test-{run_id}");
    let idempotency_key = format!("postgres-live-{run_id}");
    println!("postgres_canary_tenant={tenant_id}");
    let command = ExecuteControlCommand {
        tenant_id: tenant_id.clone(),
        command_id: "storage.live-test".into(),
        idempotency_key: idempotency_key.clone(),
        expected_revisions: BTreeMap::new(),
        puts: vec![ControlPut {
            tenant_id: tenant_id.clone(),
            record_type: "probe".into(),
            record_id: "one".into(),
            expected_revision: Some(0),
            value: json!({"runId": run_id}),
        }],
        deletes: vec![],
        audit: ControlAuditEntry {
            tenant_id: tenant_id.clone(),
            command_id: "storage.live-test".into(),
            actor_id: "adapter-test".into(),
            allowed: true,
            detail: json!({"adapter": "postgres"}),
        },
        response: HttpResponse {
            status: 201,
            headers: BTreeMap::new(),
            body: json!({"ok": true}),
        },
    };

    let config = PostgresConfig::new_with_tls_root_certificate(
        database_url,
        env::var_os("BUGLOGIN_POSTGRES_TLS_ROOT_CERT").map(PathBuf::from),
    )
    .expect("test URL must require PostgreSQL TLS");
    let mut store = PostgresControlRecordStore::connect(&config).expect("connect PostgreSQL TLS");
    store.ensure_schema().expect("create control schema");
    let first = store.execute(command).expect("execute control command");
    assert!(!first.replayed);
    drop(store);

    let mut reopened =
        PostgresControlRecordStore::connect(&config).expect("reconnect PostgreSQL TLS");
    let stored = reopened
        .get(&tenant_id, "probe", "one")
        .expect("read persisted record")
        .expect("persisted record exists");
    assert_eq!(stored.value, json!({"runId": run_id}));
    let replay = reopened
        .replay(&tenant_id, &idempotency_key)
        .expect("read persisted replay")
        .expect("persisted replay exists");
    assert!(replay.replayed);
    assert_eq!(replay.response, first.response);
}

#[test]
fn s3_https_adapter_round_trips_checksum_metadata() {
    let Some(endpoint) = optional_env("BUGLOGIN_S3_TEST_ENDPOINT") else {
        return;
    };
    let Some(bucket) = required_companion_env("BUGLOGIN_S3_TEST_BUCKET") else {
        return;
    };
    let Some(access_key) = required_companion_env("BUGLOGIN_S3_TEST_ACCESS_KEY") else {
        return;
    };
    let Some(secret_key) = required_companion_env("BUGLOGIN_S3_TEST_SECRET_KEY") else {
        return;
    };
    let run_id = run_id();
    let bytes = format!("buglogin-s3-live-probe-{run_id}").into_bytes();
    let digest = checksum(&bytes);
    let object_key = format!("probes/{run_id}");
    let config = S3Config::new(
        endpoint,
        bucket,
        env::var("BUGLOGIN_S3_TEST_REGION").unwrap_or_else(|_| "us-east-1".into()),
        "adapter-test",
        env::var("BUGLOGIN_S3_TEST_PREFIX").unwrap_or_else(|_| "buglogin-live-tests".into()),
        access_key,
        secret_key,
        env::var("BUGLOGIN_S3_TEST_SESSION_TOKEN").ok(),
        bool_env("BUGLOGIN_S3_TEST_PATH_STYLE"),
    )
    .expect("test endpoint must use HTTPS");
    let mut store = S3RemoteChunkStore::new(config).expect("create S3 adapter");

    assert_eq!(
        store.checksum(&object_key).expect("head probe object"),
        None
    );
    store
        .put(&object_key, &bytes, &digest)
        .expect("put probe object");
    store
        .put(&object_key, &bytes, &digest)
        .expect("idempotent probe put");
    assert_eq!(
        store.checksum(&object_key).expect("read checksum metadata"),
        Some(digest.clone())
    );
    assert_eq!(
        store
            .get(&object_key, &digest)
            .expect("get probe object")
            .bytes,
        bytes
    );
    assert!(
        store
            .delete_if_checksum(&object_key, &digest)
            .expect("delete probe object")
    );
    assert_eq!(
        store.checksum(&object_key).expect("verify probe deletion"),
        None
    );
}

fn optional_env(name: &str) -> Option<String> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ if strict() => panic!("missing required live-test variable {name}"),
        _ => None,
    }
}

fn required_companion_env(name: &str) -> Option<String> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ => panic!("{name} is required when BUGLOGIN_S3_TEST_ENDPOINT is set"),
    }
}

fn strict() -> bool {
    bool_env("BUGLOGIN_LIVE_ADAPTER_STRICT")
}

fn bool_env(name: &str) -> bool {
    env::var(name).is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

fn run_id() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time must follow Unix epoch")
        .as_nanos()
}
