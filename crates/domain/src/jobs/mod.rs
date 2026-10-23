use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobId(String);

impl JobId {
    pub fn new(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-')
            })
        {
            return Err("invalid_job_id");
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobStatus {
    Queued,
    Running,
    Cancelling,
    Cancelled,
    Completed,
    Partial,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JobProgress {
    pub total: usize,
    pub completed: usize,
}

impl JobProgress {
    pub fn new(total: usize) -> Self {
        Self {
            total,
            completed: 0,
        }
    }
    pub fn fraction(self) -> f64 {
        if self.total == 0 {
            1.0
        } else {
            self.completed as f64 / self.total as f64
        }
    }
    pub fn advance(&mut self) {
        self.completed = self.completed.saturating_add(1).min(self.total);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobSnapshot {
    pub id: JobId,
    pub status: JobStatus,
    pub progress: JobProgress,
}

#[derive(Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
