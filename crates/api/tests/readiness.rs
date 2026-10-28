use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use buglogin_api::http::{ApiState, router};
use buglogin_browser::profile::ProfileService;
use buglogin_domain::errors::Problem;
use buglogin_storage::control::MemoryControlRecordStore;
use buglogin_storage::remote::{ConcurrentRemoteChunkStore, RemoteChunk};
use tower::ServiceExt;

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .header("x-correlation-id", "readiness-test")
        .body(Body::empty())
        .unwrap()
}

#[derive(Clone)]
struct GatedReadinessStore {
    entered: Arc<AtomicBool>,
    release: Arc<(Mutex<bool>, Condvar)>,
}

impl GatedReadinessStore {
    fn new() -> Self {
        Self {
            entered: Arc::new(AtomicBool::new(false)),
            release: Arc::new((Mutex::new(false), Condvar::new())),
        }
    }

    fn block_readiness(&self) {
        self.entered.store(true, Ordering::SeqCst);
        let (released, changed) = &*self.release;
        let mut released = released.lock().unwrap();
        while !*released {
            released = changed.wait(released).unwrap();
        }
    }

    fn release_readiness(&self) {
        let (released, changed) = &*self.release;
        *released.lock().unwrap() = true;
        changed.notify_all();
    }
}

impl ConcurrentRemoteChunkStore for GatedReadinessStore {
    fn readiness(&self) -> Result<(), Problem> {
        self.block_readiness();
        Ok(())
    }

    fn checksum(&self, _object_key: &str) -> Result<Option<String>, Problem> {
        Ok(None)
    }

    fn put(
        &self,
        _object_key: &str,
        _bytes: &[u8],
        _expected_checksum: &str,
    ) -> Result<(), Problem> {
        Ok(())
    }

    fn get(&self, _object_key: &str, _expected_checksum: &str) -> Result<RemoteChunk, Problem> {
        Err(Problem::new("not_found", "Not found", 404, "test"))
    }

    fn delete_if_checksum(
        &self,
        _object_key: &str,
        _expected_checksum: &str,
    ) -> Result<bool, Problem> {
        Ok(false)
    }
}

#[tokio::test]
async fn missing_runtime_catalog_does_not_mask_missing_hosted_storage() {
    let state = ApiState::hosted_with_control_store(
        ProfileService::disabled(),
        MemoryControlRecordStore::default(),
    )
    .unwrap();
    let app = router(state);

    let liveness = app.clone().oneshot(get("/v1/health")).await.unwrap();
    assert_eq!(liveness.status(), StatusCode::OK);

    let readiness = app.oneshot(get("/v1/readyz")).await.unwrap();
    assert_eq!(readiness.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(readiness.into_body(), usize::MAX)
        .await
        .unwrap();
    let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem["code"], "object_storage_not_configured");
}

#[tokio::test]
async fn local_readiness_accepts_local_memory_adapters() {
    let app = router(ApiState::local(ProfileService::disabled()));
    let readiness = app.oneshot(get("/v1/readyz")).await.unwrap();
    assert_eq!(readiness.status(), StatusCode::OK);
    assert_eq!(
        readiness
            .headers()
            .get("x-buglogin-persistence-mode")
            .and_then(|value| value.to_str().ok()),
        Some("memory")
    );
    assert_eq!(
        readiness
            .headers()
            .get("x-buglogin-runtime-catalog")
            .and_then(|value| value.to_str().ok()),
        Some("unavailable")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn stalled_readiness_probe_does_not_block_health_or_queue_other_probes() {
    let store = Arc::new(GatedReadinessStore::new());
    let state = ApiState::local(ProfileService::disabled())
        .with_concurrent_remote_chunks(Box::new(store.as_ref().clone()));
    let app = router(state);
    let readiness_app = app.clone();
    let readiness =
        tokio::spawn(async move { readiness_app.oneshot(get("/v1/readyz")).await.unwrap() });

    tokio::time::timeout(Duration::from_secs(5), async {
        while !store.entered.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("readiness probe should reach the object-store adapter");

    let health = tokio::time::timeout(
        Duration::from_millis(200),
        app.clone().oneshot(get("/v1/health")),
    )
    .await;
    let second_readiness = app.clone().oneshot(get("/v1/readyz")).await.unwrap();

    store.release_readiness();
    let first_readiness = tokio::time::timeout(Duration::from_secs(5), readiness)
        .await
        .expect("readiness request should finish after the dependency resumes")
        .unwrap();

    assert_eq!(health.unwrap().unwrap().status(), StatusCode::OK);
    assert_eq!(second_readiness.status(), StatusCode::SERVICE_UNAVAILABLE);
    let busy_body = axum::body::to_bytes(second_readiness.into_body(), usize::MAX)
        .await
        .unwrap();
    let busy_problem: serde_json::Value = serde_json::from_slice(&busy_body).unwrap();
    assert_eq!(busy_problem["code"], "readiness_busy");
    assert_eq!(first_readiness.status(), StatusCode::OK);
}
