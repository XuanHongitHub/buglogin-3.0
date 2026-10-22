#![cfg(feature = "integration-tests")]

use std::collections::BTreeMap;
use std::env;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use buglogin_storage::blob::checksum;
use buglogin_storage::control::{
    ControlAuditEntry, ControlDelete, ControlPut, ControlRecordStore, ExecuteControlCommand,
    HttpResponse, PostgresConfig, PostgresControlRecordStore,
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
    let delete_command = |key: String, expected_revision| ExecuteControlCommand {
        tenant_id: tenant_id.clone(),
        command_id: "storage.live-test.delete".into(),
        idempotency_key: key,
        expected_revisions: BTreeMap::new(),
        puts: vec![],
        deletes: vec![ControlDelete {
            tenant_id: tenant_id.clone(),
            record_type: "probe".into(),
            record_id: "one".into(),
            expected_revision: Some(expected_revision),
        }],
        audit: ControlAuditEntry {
            tenant_id: tenant_id.clone(),
            command_id: "storage.live-test.delete".into(),
            actor_id: "adapter-test".into(),
            allowed: true,
            detail: json!({"adapter": "postgres"}),
        },
        response: HttpResponse {
            status: 200,
            headers: BTreeMap::new(),
            body: json!({"deleted": true}),
        },
    };
    assert_eq!(
        reopened
            .execute(delete_command(
                format!("postgres-delete-stale-{run_id}"),
                stored.revision + 1,
            ))
            .unwrap_err()
            .code,
        "storage_revision_conflict"
    );
    assert!(
        reopened
            .get(&tenant_id, "probe", "one")
            .expect("read after stale delete")
            .is_some()
    );
    let control_revision = reopened
        .get("control", "control_state", "primary")
        .expect("read shared authorization revision")
        .map_or(0, |record| record.revision);
    let fenced_key = format!("postgres-auth-fence-{run_id}");
    let stale_fenced_write = ExecuteControlCommand {
        tenant_id: tenant_id.clone(),
        command_id: "storage.live-test.authorization-fence".into(),
        idempotency_key: fenced_key.clone(),
        expected_revisions: BTreeMap::from([(
            buglogin_storage::control::ControlRecordKey {
                tenant_id: "control".into(),
                record_type: "control_state".into(),
                record_id: "primary".into(),
            },
            control_revision + 1,
        )]),
        puts: vec![ControlPut {
            tenant_id: tenant_id.clone(),
            record_type: "probe".into(),
            record_id: "stale-auth-fence".into(),
            expected_revision: Some(0),
            value: json!({"mustNotWrite": true}),
        }],
        deletes: vec![],
        audit: ControlAuditEntry {
            tenant_id: tenant_id.clone(),
            command_id: "storage.live-test.authorization-fence".into(),
            actor_id: "adapter-test".into(),
            allowed: true,
            detail: json!({"adapter": "postgres"}),
        },
        response: HttpResponse {
            status: 200,
            headers: BTreeMap::new(),
            body: json!({"ok": true}),
        },
    };
    assert_eq!(
        reopened.execute(stale_fenced_write).unwrap_err().code,
        "storage_revision_conflict"
    );
    assert!(
        reopened
            .get(&tenant_id, "probe", "stale-auth-fence")
            .expect("read after stale authorization fence")
            .is_none()
    );
    reopened
        .execute(delete_command(
            format!("postgres-delete-current-{run_id}"),
            stored.revision,
        ))
        .expect("delete with current revision");
    assert!(
        reopened
            .get(&tenant_id, "probe", "one")
            .expect("read after current delete")
            .is_none()
    );
    let replay = reopened
        .replay(&tenant_id, &idempotency_key)
        .expect("read persisted replay")
        .expect("persisted replay exists");
    assert!(replay.replayed);
    assert_eq!(replay.response, first.response);
}

#[test]
fn postgres_account_scoped_records_do_not_cross_account_or_record_type() {
    let Some(database_url) = optional_env("BUGLOGIN_POSTGRES_TEST_URL") else {
        return;
    };
    let run_id = run_id();
    let account_a = format!("account-a-{run_id}");
    let account_b = format!("account-b-{run_id}");
    let refresh_a_id = format!("refresh-a-{run_id}");
    let refresh_b_id = format!("refresh-b-{run_id}");
    let session_a_id = format!("session-a-{run_id}");
    let tenant_id = "identity";
    let config = PostgresConfig::new_with_tls_root_certificate(
        database_url,
        env::var_os("BUGLOGIN_POSTGRES_TLS_ROOT_CERT").map(PathBuf::from),
    )
    .expect("test URL must require PostgreSQL TLS");
    let mut store = PostgresControlRecordStore::connect(&config).expect("connect PostgreSQL TLS");
    store
        .ensure_schema()
        .expect("create control schema and account index");
    let record = |record_type: &str, record_id: String, account_id: &str| ControlPut {
        tenant_id: tenant_id.into(),
        record_type: record_type.into(),
        record_id,
        expected_revision: Some(0),
        value: json!({"accountId": account_id}),
    };
    let put_key = format!("postgres-account-scope-{run_id}");
    store
        .execute(ExecuteControlCommand {
            tenant_id: tenant_id.into(),
            command_id: "storage.live-test.account-scope.insert".into(),
            idempotency_key: put_key,
            expected_revisions: BTreeMap::new(),
            puts: vec![
                record("device_refresh", refresh_a_id.clone(), &account_a),
                record("device_refresh", refresh_b_id.clone(), &account_b),
                record("session", session_a_id.clone(), &account_a),
            ],
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: tenant_id.into(),
                command_id: "storage.live-test.account-scope.insert".into(),
                actor_id: "adapter-test".into(),
                allowed: true,
                detail: json!({"runId": run_id}),
            },
            response: HttpResponse {
                status: 201,
                headers: BTreeMap::new(),
                body: json!({"ok": true}),
            },
        })
        .expect("write account-scoped fixture records");

    let rows = store
        .list_for_account(tenant_id, "device_refresh", &account_a)
        .expect("query records by account index");
    let deletes = [
        ("device_refresh", refresh_a_id.clone()),
        ("device_refresh", refresh_b_id),
        ("session", session_a_id),
    ]
    .into_iter()
    .map(|(record_type, record_id)| ControlDelete {
        tenant_id: tenant_id.into(),
        record_type: record_type.into(),
        record_id,
        expected_revision: Some(1),
    })
    .collect();
    store
        .execute(ExecuteControlCommand {
            tenant_id: tenant_id.into(),
            command_id: "storage.live-test.account-scope.delete".into(),
            idempotency_key: format!("postgres-account-scope-delete-{run_id}"),
            expected_revisions: BTreeMap::new(),
            puts: vec![],
            deletes,
            audit: ControlAuditEntry {
                tenant_id: tenant_id.into(),
                command_id: "storage.live-test.account-scope.delete".into(),
                actor_id: "adapter-test".into(),
                allowed: true,
                detail: json!({"runId": run_id}),
            },
            response: HttpResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: json!({"deleted": true}),
            },
        })
        .expect("remove account-scoped fixture records");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].record_id, refresh_a_id);
}

#[test]
fn postgres_session_advisory_lock_is_exclusive_across_store_handles() {
    let Some(database_url) = optional_env("BUGLOGIN_POSTGRES_TEST_URL") else {
        return;
    };
    let config = PostgresConfig::new_with_tls_root_certificate(
        database_url,
        env::var_os("BUGLOGIN_POSTGRES_TLS_ROOT_CERT").map(PathBuf::from),
    )
    .expect("test URL must require PostgreSQL TLS");
    let mut first = PostgresControlRecordStore::connect(&config).expect("connect first store");
    let mut second = PostgresControlRecordStore::connect(&config).expect("connect second store");
    let lock_id = run_id() as i64;

    assert!(first.try_advisory_lock(lock_id).expect("first lock claim"));
    assert!(
        !first
            .try_advisory_lock(lock_id)
            .expect("same-session reentrant claim")
    );
    assert!(
        !second
            .try_advisory_lock(lock_id)
            .expect("competing lock claim")
    );
    first
        .release_advisory_lock(lock_id)
        .expect("release first lock");
    assert!(second.try_advisory_lock(lock_id).expect("retry lock claim"));
    second
        .release_advisory_lock(lock_id)
        .expect("release second lock");
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
        _ => panic!("{name} is required when integration-tests is enabled"),
    }
}

fn required_companion_env(name: &str) -> Option<String> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ => panic!("{name} is required when BUGLOGIN_S3_TEST_ENDPOINT is set"),
    }
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
