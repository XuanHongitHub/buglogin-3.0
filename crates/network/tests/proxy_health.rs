use buglogin_domain::jobs::{CancellationToken, JobId, JobStatus};
use buglogin_network::proxy::health::{
    ProxyChecker, ProxyHealth, check_proxies, check_proxies_without_progress,
};
use buglogin_network::proxy::{ProxyProtocol, ProxyRow, parse_lines};
use std::sync::atomic::{AtomicUsize, Ordering};

fn proxy(port: u16) -> ProxyRow {
    ProxyRow {
        protocol: ProxyProtocol::Http,
        host: "127.0.0.1".to_owned(),
        port,
        username: None,
        password: None,
    }
}

fn job_id(value: &str) -> JobId {
    JobId::new(value).unwrap()
}

struct TrackingChecker {
    active: AtomicUsize,
    maximum: AtomicUsize,
    calls: AtomicUsize,
}

impl TrackingChecker {
    fn new() -> Self {
        Self {
            active: AtomicUsize::new(0),
            maximum: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
        }
    }
}

impl ProxyChecker for TrackingChecker {
    fn check(&self, proxy: &ProxyRow) -> ProxyHealth {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.maximum.fetch_max(active, Ordering::SeqCst);
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::thread::yield_now();
        self.active.fetch_sub(1, Ordering::SeqCst);
        if proxy.port % 2 == 0 {
            ProxyHealth::Healthy {
                latency_ms: u64::from(proxy.port),
            }
        } else {
            ProxyHealth::Unhealthy {
                code: "connection_failed".to_owned(),
            }
        }
    }
}

#[test]
fn batch_is_bounded_reports_progress_and_keeps_partial_results() {
    let checker = TrackingChecker::new();
    let mut progress = Vec::new();
    let report = check_proxies(
        job_id("job_proxy_health"),
        (8000..8012).map(proxy).collect(),
        3,
        &checker,
        CancellationToken::default(),
        |event| progress.push((event.completed, event.total, event.result.index)),
    );

    assert_eq!(report.job.status, JobStatus::Partial);
    assert_eq!(report.job.progress.completed, 12);
    assert_eq!(report.job.progress.total, 12);
    assert_eq!(report.results.len(), 12);
    assert!(checker.maximum.load(Ordering::SeqCst) <= 3);
    assert_eq!(checker.calls.load(Ordering::SeqCst), 12);
    assert_eq!(progress.len(), 12);
    assert_eq!(
        progress.last().map(|event| (event.0, event.1)),
        Some((12, 12))
    );
    assert_eq!(
        report
            .results
            .iter()
            .map(|result| result.index)
            .collect::<Vec<_>>(),
        (0..12).collect::<Vec<_>>()
    );
}

struct CancellingChecker {
    cancellation: CancellationToken,
    calls: AtomicUsize,
}

impl ProxyChecker for CancellingChecker {
    fn check(&self, proxy: &ProxyRow) -> ProxyHealth {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.cancellation.cancel();
        ProxyHealth::Healthy {
            latency_ms: u64::from(proxy.port),
        }
    }
}

#[test]
fn cancellation_stops_new_work_and_returns_completed_items() {
    let cancellation = CancellationToken::default();
    let checker = CancellingChecker {
        cancellation: cancellation.clone(),
        calls: AtomicUsize::new(0),
    };
    let report = check_proxies_without_progress(
        job_id("job_proxy_cancel"),
        (9000..9100).map(proxy).collect(),
        4,
        &checker,
        cancellation,
    );

    assert_eq!(report.job.status, JobStatus::Cancelled);
    assert!(!report.results.is_empty());
    assert!(report.results.len() <= 4);
    assert_eq!(report.job.progress.completed, report.results.len());
    assert_eq!(checker.calls.load(Ordering::SeqCst), report.results.len());
}

#[test]
fn zero_concurrency_fails_without_dropping_input_or_running_checker() {
    let checker = TrackingChecker::new();
    let report = check_proxies_without_progress(
        job_id("job_proxy_invalid_bound"),
        vec![proxy(8080)],
        0,
        &checker,
        CancellationToken::default(),
    );

    assert_eq!(report.job.status, JobStatus::Failed);
    assert_eq!(report.job.progress.total, 1);
    assert_eq!(report.job.progress.completed, 0);
    assert!(report.results.is_empty());
    assert_eq!(checker.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn parser_never_invokes_or_waits_for_health_checker() {
    let checker = TrackingChecker::new();
    let parsed = parse_lines("http://127.0.0.1:8080\ninvalid");

    assert_eq!(parsed.rows.len(), 1);
    assert_eq!(parsed.errors.len(), 1);
    assert_eq!(checker.calls.load(Ordering::SeqCst), 0);
}
