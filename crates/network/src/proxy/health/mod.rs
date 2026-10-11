use super::model::ProxyRow;
use buglogin_domain::jobs::{CancellationToken, JobId, JobProgress, JobSnapshot, JobStatus};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProxyHealth {
    Healthy { latency_ms: u64 },
    Unhealthy { code: String },
}

pub trait ProxyChecker: Send + Sync {
    fn check(&self, proxy: &ProxyRow) -> ProxyHealth;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProxyHealthResult {
    pub index: usize,
    pub health: ProxyHealth,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProxyHealthReport {
    pub job: JobSnapshot,
    pub results: Vec<ProxyHealthResult>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProxyHealthProgress {
    pub completed: usize,
    pub total: usize,
    pub result: ProxyHealthResult,
}

pub fn check_proxies<C, F>(
    job_id: JobId,
    proxies: Vec<ProxyRow>,
    concurrency: usize,
    checker: &C,
    cancellation: CancellationToken,
    mut on_progress: F,
) -> ProxyHealthReport
where
    C: ProxyChecker,
    F: FnMut(ProxyHealthProgress),
{
    let total = proxies.len();
    let mut progress = JobProgress::new(total);
    let mut status = JobStatus::Running;
    if total == 0 {
        return ProxyHealthReport {
            job: JobSnapshot {
                id: job_id,
                status: JobStatus::Completed,
                progress,
            },
            results: Vec::new(),
        };
    }
    if concurrency == 0 {
        return ProxyHealthReport {
            job: JobSnapshot {
                id: job_id,
                status: JobStatus::Failed,
                progress,
            },
            results: Vec::new(),
        };
    }
    if cancellation.is_cancelled() {
        return ProxyHealthReport {
            job: JobSnapshot {
                id: job_id,
                status: JobStatus::Cancelled,
                progress,
            },
            results: Vec::new(),
        };
    }

    let next = AtomicUsize::new(0);
    let (sender, receiver) = mpsc::channel();
    let checker = Arc::new(checker);
    let worker_count = concurrency.min(total);
    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            let sender = sender.clone();
            let checker = Arc::clone(&checker);
            let cancellation = cancellation.clone();
            let next = &next;
            let proxies = &proxies;
            scope.spawn(move || {
                loop {
                    if cancellation.is_cancelled() {
                        break;
                    }
                    let index = next.fetch_add(1, Ordering::AcqRel);
                    if index >= total {
                        break;
                    }
                    let result = ProxyHealthResult {
                        index,
                        health: checker.check(&proxies[index]),
                    };
                    if sender.send(result).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        let mut results = Vec::with_capacity(total);
        while let Ok(result) = receiver.recv() {
            progress.advance();
            on_progress(ProxyHealthProgress {
                completed: progress.completed,
                total,
                result: result.clone(),
            });
            results.push(result);
        }
        results.sort_by_key(|result| result.index);
        if progress.completed == total {
            status = if results
                .iter()
                .any(|result| matches!(result.health, ProxyHealth::Unhealthy { .. }))
            {
                JobStatus::Partial
            } else {
                JobStatus::Completed
            };
        } else if cancellation.is_cancelled() {
            status = JobStatus::Cancelled;
        } else {
            status = JobStatus::Failed;
        }
        ProxyHealthReport {
            job: JobSnapshot {
                id: job_id,
                status,
                progress,
            },
            results,
        }
    })
}

pub fn check_proxies_without_progress<C: ProxyChecker>(
    job_id: JobId,
    proxies: Vec<ProxyRow>,
    concurrency: usize,
    checker: &C,
    cancellation: CancellationToken,
) -> ProxyHealthReport {
    check_proxies(job_id, proxies, concurrency, checker, cancellation, |_| {})
}
