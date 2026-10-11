use std::collections::BTreeMap;

use buglogin_domain::ids::{ResourceId, WorkspaceId};
use buglogin_sync::coordination::{
    AuthorizationRequest, CoordinationAuth, CoordinationAuthorizer, CoordinationEvent,
    CoordinationOperation, DenyAllAuthorizer, EventResource, LeaseGrant, Presence,
};
use serde_json::json;

#[test]
fn protocol_encoding_is_camel_case_and_round_trips() {
    let presence = Presence {
        session_id: "session-1".into(),
        actor_id: "actor-1".into(),
        connected_at_ms: 10,
        last_seen_at_ms: 11,
        expires_at_ms: 21,
        metadata: BTreeMap::from([("device".into(), "desktop".into())]),
    };
    assert_eq!(
        presence.encode().unwrap(),
        r#"{"sessionId":"session-1","actorId":"actor-1","connectedAtMs":10,"lastSeenAtMs":11,"expiresAtMs":21,"metadata":{"device":"desktop"}}"#
    );
    assert_eq!(
        Presence::decode(&presence.encode().unwrap()).unwrap(),
        presence
    );

    let event = event("actor-1", "evt-1", 7);
    let encoded = event.encode().unwrap();
    assert!(encoded.contains(r#""eventVersion":"1.0""#));
    assert!(encoded.contains(r#""resourceType":"profile""#));
    assert_eq!(CoordinationEvent::decode(&encoded).unwrap(), event);
}

#[test]
fn credentials_are_redacted() {
    let auth = CoordinationAuth::new("actor-1", "secret-token").unwrap();
    assert!(!format!("{auth:?}").contains("secret-token"));
}

#[test]
fn lease_grant_serializes_fence_and_redacts_owner_token() {
    let lease = LeaseGrant {
        resource_id: ResourceId::new("resource-1").unwrap(),
        workspace_id: WorkspaceId::new("workspace-1").unwrap(),
        owner_id: "actor-1".into(),
        owner_token: "secret-owner-token".into(),
        fencing_token: 42,
        issued_at_ms: 10,
        expires_at_ms: 20,
    };
    let encoded = serde_json::to_string(&lease).unwrap();

    assert_eq!(
        encoded,
        r#"{"resourceId":"resource-1","workspaceId":"workspace-1","ownerId":"actor-1","ownerToken":"secret-owner-token","fencingToken":42,"issuedAtMs":10,"expiresAtMs":20}"#
    );
    assert_eq!(serde_json::from_str::<LeaseGrant>(&encoded).unwrap(), lease);
    assert!(!format!("{lease:?}").contains("secret-owner-token"));
}

#[tokio::test]
async fn authorization_defaults_to_deny() {
    let auth = CoordinationAuth::new("actor-1", "credential").unwrap();
    let workspace = WorkspaceId::new("workspace-1").unwrap();
    let result = DenyAllAuthorizer
        .authorize(
            &auth,
            AuthorizationRequest {
                operation: CoordinationOperation::EventRead,
                workspace_id: &workspace,
                resource_id: None,
            },
        )
        .await;
    assert_eq!(result.unwrap_err().status, 403);
}

fn event(actor_id: &str, event_id: &str, revision: u64) -> CoordinationEvent {
    CoordinationEvent {
        event_id: event_id.into(),
        event_version: "1.0".into(),
        name: "sync.manifest.updated".into(),
        occurred_at_ms: 1,
        correlation_id: "cor-1".into(),
        causation_id: "cmd-1".into(),
        actor_id: actor_id.into(),
        resource: EventResource {
            resource_type: "profile".into(),
            id: "profile-1".into(),
            revision,
        },
        data: json!({"generation": revision}),
    }
}

#[cfg(feature = "redis-tests")]
mod live_redis {
    use super::*;
    use async_trait::async_trait;
    use buglogin_domain::{
        errors::Problem,
        ids::{ResourceId, WorkspaceId},
    };
    use buglogin_sync::coordination::{
        AuthorizationRequest, CoordinationAuthorizer, CoordinationBackend, CoordinationScope,
        RedisCoordination, RedisCoordinationConfig,
    };
    use std::{sync::Arc, time::Duration};

    #[tokio::test]
    async fn coordinates_leases_presence_and_idempotent_events() {
        let Some(url) = redis_test_url() else {
            return;
        };
        let namespace = format!("test-{}", std::process::id());
        let backend = RedisCoordination::connect(
            RedisCoordinationConfig::new(url, namespace),
            Arc::new(AllowAll),
        )
        .await
        .unwrap();
        let workspace = WorkspaceId::new("workspace-1").unwrap();
        let resource = ResourceId::new("resource-1").unwrap();
        let auth = CoordinationAuth::new("actor-1", "test-credential").unwrap();
        let scope = || CoordinationScope {
            workspace_id: &workspace,
            auth: &auth,
        };

        let lease = backend
            .acquire_lease(scope(), &resource, Duration::from_secs(5))
            .await
            .unwrap();
        let renewed = backend
            .renew_lease(
                scope(),
                &resource,
                &lease.owner_token,
                Duration::from_secs(10),
            )
            .await
            .unwrap();
        assert!(renewed.expires_at_ms > lease.expires_at_ms);
        assert_eq!(renewed.fencing_token, lease.fencing_token);

        backend
            .heartbeat_presence(
                scope(),
                "session-1",
                BTreeMap::new(),
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        assert_eq!(backend.list_presence(scope()).await.unwrap().len(), 1);

        let first = backend
            .publish_event(
                scope(),
                "idempotency-key-0001",
                event("actor-1", "evt-1", 1),
            )
            .await
            .unwrap();
        let retry = backend
            .publish_event(
                scope(),
                "idempotency-key-0001",
                event("actor-1", "evt-1", 1),
            )
            .await
            .unwrap();
        assert!(!first.duplicate);
        assert!(retry.duplicate);
        assert_eq!(first.stream_id, retry.stream_id);
        assert_eq!(
            backend.read_events(scope(), None, 10).await.unwrap().len(),
            1
        );
        backend
            .release_lease(scope(), &resource, &renewed.owner_token)
            .await
            .unwrap();
        let reacquired = backend
            .acquire_lease(scope(), &resource, Duration::from_secs(5))
            .await
            .unwrap();
        assert!(reacquired.fencing_token > renewed.fencing_token);
        backend
            .release_lease(scope(), &resource, &reacquired.owner_token)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn idempotent_lease_acquire_replays_live_token_and_rejects_fingerprint_conflict() {
        let Some(url) = redis_test_url() else {
            return;
        };
        let namespace = format!("test-lease-idempotency-{}", std::process::id());
        let backend = RedisCoordination::connect(
            RedisCoordinationConfig::new(url, namespace),
            Arc::new(AllowAll),
        )
        .await
        .unwrap();
        let workspace = WorkspaceId::new("workspace-1").unwrap();
        let resource = ResourceId::new("resource-idempotent-1").unwrap();
        let auth = CoordinationAuth::new("actor-1", "test-credential").unwrap();
        let scope = || CoordinationScope {
            workspace_id: &workspace,
            auth: &auth,
        };
        let first = backend
            .acquire_lease_idempotent(
                scope(),
                &resource,
                Duration::from_secs(10),
                "lease-idempotency-key-0001",
            )
            .await
            .unwrap();
        let retry = backend
            .acquire_lease_idempotent(
                scope(),
                &resource,
                Duration::from_secs(10),
                "lease-idempotency-key-0001",
            )
            .await
            .unwrap();
        assert_eq!(first, retry);
        let conflict = backend
            .acquire_lease_idempotent(
                scope(),
                &resource,
                Duration::from_secs(20),
                "lease-idempotency-key-0001",
            )
            .await
            .unwrap_err();
        assert_eq!(conflict.code, "idempotency_conflict");
        backend
            .release_lease(scope(), &resource, &first.owner_token)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn idempotent_lease_acquire_retries_after_expiry_with_new_fencing_token() {
        let Some(url) = redis_test_url() else {
            return;
        };
        let namespace = format!("test-lease-idempotency-expiry-{}", std::process::id());
        let backend = RedisCoordination::connect(
            RedisCoordinationConfig::new(url, namespace),
            Arc::new(AllowAll),
        )
        .await
        .unwrap();
        let workspace = WorkspaceId::new("workspace-1").unwrap();
        let resource = ResourceId::new("resource-idempotent-expiry-1").unwrap();
        let auth = CoordinationAuth::new("actor-1", "test-credential").unwrap();
        let scope = || CoordinationScope {
            workspace_id: &workspace,
            auth: &auth,
        };
        let first = backend
            .acquire_lease_idempotent(
                scope(),
                &resource,
                Duration::from_secs(1),
                "lease-idempotency-expiry-0001",
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        let retry = backend
            .acquire_lease_idempotent(
                scope(),
                &resource,
                Duration::from_secs(1),
                "lease-idempotency-expiry-0001",
            )
            .await
            .unwrap();
        assert!(retry.fencing_token > first.fencing_token);
        assert_ne!(retry.owner_token, first.owner_token);
        backend
            .release_lease(scope(), &resource, &retry.owner_token)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn manifest_atomic_commit_flow() {
        let Some(url) = redis_test_url() else {
            return;
        };
        let namespace = format!("test-manifest-{}", std::process::id());
        let backend = RedisCoordination::connect(
            RedisCoordinationConfig::new(url, namespace),
            Arc::new(AllowAll),
        )
        .await
        .unwrap();

        let workspace = WorkspaceId::new("workspace-1").unwrap();
        let resource = ResourceId::new("resource-1").unwrap();
        let auth = CoordinationAuth::new("actor-1", "test-credential").unwrap();
        let scope = || CoordinationScope {
            workspace_id: &workspace,
            auth: &auth,
        };

        let current = backend.current_manifest(scope(), &resource).await.unwrap();
        assert!(current.is_none());

        let mut m1 = buglogin_sync::manifest::SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            1,
            1,
            None,
            b"data-1",
            vec![],
        )
        .unwrap();

        let err = backend
            .commit_manifest(
                scope(),
                &resource,
                "0000000000000000000000000000000000000000000000000000000000000000",
                m1.clone(),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, "lease_not_found");

        let lease1 = backend
            .acquire_lease(scope(), &resource, Duration::from_secs(10))
            .await
            .unwrap();
        m1.fencing_token = lease1.fencing_token;

        let bad_token = "1111111111111111111111111111111111111111111111111111111111111111";
        let err = backend
            .commit_manifest(scope(), &resource, bad_token, m1.clone())
            .await
            .unwrap_err();
        assert_eq!(err.code, "lease_not_owner");

        let mut m1_wrong_fence = m1.clone();
        m1_wrong_fence.fencing_token = lease1.fencing_token + 10;
        let err = backend
            .commit_manifest(scope(), &resource, &lease1.owner_token, m1_wrong_fence)
            .await
            .unwrap_err();
        assert_eq!(err.code, "stale_manifest_fencing_token");

        backend
            .commit_manifest(scope(), &resource, &lease1.owner_token, m1.clone())
            .await
            .unwrap();

        let current = backend
            .current_manifest(scope(), &resource)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.generation, 1);
        assert_eq!(current.parent_generation, None);

        backend
            .commit_manifest(scope(), &resource, &lease1.owner_token, m1.clone())
            .await
            .unwrap();

        let mut m1_diff = m1.clone();
        m1_diff.root_checksum =
            "2222222222222222222222222222222222222222222222222222222222222222".to_owned();
        let err = backend
            .commit_manifest(scope(), &resource, &lease1.owner_token, m1_diff)
            .await
            .unwrap_err();
        assert_eq!(err.code, "idempotency_conflict");

        let m3 = buglogin_sync::manifest::SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            lease1.fencing_token,
            3,
            Some(2),
            b"data-3",
            vec![],
        )
        .unwrap();
        let err = backend
            .commit_manifest(scope(), &resource, &lease1.owner_token, m3)
            .await
            .unwrap_err();
        assert_eq!(err.code, "stale_manifest_generation");

        let m2 = buglogin_sync::manifest::SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            lease1.fencing_token,
            2,
            Some(1),
            b"data-2",
            vec![],
        )
        .unwrap();
        backend
            .commit_manifest(scope(), &resource, &lease1.owner_token, m2)
            .await
            .unwrap();

        let current = backend
            .current_manifest(scope(), &resource)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.generation, 2);
        assert_eq!(current.parent_generation, Some(1));

        backend
            .release_lease(scope(), &resource, &lease1.owner_token)
            .await
            .unwrap();
        let lease2 = backend
            .acquire_lease(scope(), &resource, Duration::from_secs(10))
            .await
            .unwrap();
        assert!(lease2.fencing_token > lease1.fencing_token);

        let m3_old_fence = buglogin_sync::manifest::SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            lease1.fencing_token,
            3,
            Some(2),
            b"data-3",
            vec![],
        )
        .unwrap();
        let err = backend
            .commit_manifest(scope(), &resource, &lease2.owner_token, m3_old_fence)
            .await
            .unwrap_err();
        assert_eq!(err.code, "stale_manifest_fencing_token");

        let m3_new_fence = buglogin_sync::manifest::SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            lease2.fencing_token,
            3,
            Some(2),
            b"data-3",
            vec![],
        )
        .unwrap();
        backend
            .commit_manifest(scope(), &resource, &lease2.owner_token, m3_new_fence)
            .await
            .unwrap();

        let current = backend
            .current_manifest(scope(), &resource)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.generation, 3);
        assert_eq!(current.fencing_token, lease2.fencing_token);
    }

    #[tokio::test]
    async fn reconnect_stream_gap_triggers_resnapshot() {
        let Some(url) = redis_test_url() else {
            return;
        };
        let namespace = format!("test-gap-{}", std::process::id());
        let backend = RedisCoordination::connect(
            RedisCoordinationConfig::new(url, namespace),
            Arc::new(AllowAll),
        )
        .await
        .unwrap();

        let workspace = WorkspaceId::new("workspace-1").unwrap();
        let auth = CoordinationAuth::new("actor-1", "test-credential").unwrap();
        let scope = || CoordinationScope {
            workspace_id: &workspace,
            auth: &auth,
        };

        let first = backend
            .publish_event(scope(), "idem-gap-1", event("actor-1", "evt-1", 1))
            .await
            .unwrap();

        // cursor "1-0" (1ms) chắc chắn nhỏ hơn timestamp thực tế của first event vừa tạo (>1500000000000)
        let err = backend
            .read_events(scope(), Some("1-0"), 10)
            .await
            .unwrap_err();
        assert_eq!(err.code, "sync_history_gap_resnapshot_required");

        // cursor đúng bằng entry ID của first event thì bình thường (không có gap)
        let ok_events = backend
            .read_events(scope(), Some(&first.stream_id), 10)
            .await
            .unwrap();
        assert!(ok_events.is_empty());
    }

    struct AllowAll;

    fn redis_test_url() -> Option<String> {
        match std::env::var("BUGLOGIN_REDIS_TEST_URL") {
            Ok(url) if !url.trim().is_empty() => Some(url),
            _ if std::env::var("BUGLOGIN_LIVE_ADAPTER_STRICT")
                .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true")) =>
            {
                panic!("missing required live-test variable BUGLOGIN_REDIS_TEST_URL")
            }
            _ => None,
        }
    }

    #[async_trait]
    impl CoordinationAuthorizer for AllowAll {
        async fn authorize(
            &self,
            _auth: &CoordinationAuth,
            _request: AuthorizationRequest<'_>,
        ) -> Result<(), Problem> {
            Ok(())
        }
    }
}
