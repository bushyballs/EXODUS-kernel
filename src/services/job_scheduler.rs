/// Job scheduler for Genesis
///
/// Deferred task execution, constraints (network, charging, idle),
/// periodic jobs, expedited jobs, and backoff policies.
///
/// Inspired by: Android JobScheduler/WorkManager, iOS BGTaskScheduler. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Job state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
    Rescheduled,
}

/// Network constraint
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkType {
    None,
    Any,
    Unmetered,
    NotRoaming,
}

/// Backoff policy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackoffPolicy {
    Linear,
    Exponential,
}

/// Job constraints
pub struct JobConstraints {
    pub require_network: NetworkType,
    pub require_charging: bool,
    pub require_idle: bool,
    pub require_storage_not_low: bool,
    pub require_battery_not_low: bool,
}

impl JobConstraints {
    pub fn none() -> Self {
        JobConstraints {
            require_network: NetworkType::None,
            require_charging: false,
            require_idle: false,
            require_storage_not_low: false,
            require_battery_not_low: false,
        }
    }
}

/// A scheduled job
pub struct Job {
    pub id: u32,
    pub app_id: String,
    pub tag: String,
    pub state: JobState,
    pub constraints: JobConstraints,
    pub periodic_ms: Option<u64>,
    pub flex_ms: Option<u64>,
    pub initial_delay_ms: u64,
    pub backoff_policy: BackoffPolicy,
    pub backoff_delay_ms: u64,
    pub max_retries: u32,
    pub retry_count: u32,
    pub scheduled_at: u64,
    pub started_at: Option<u64>,
    pub expedited: bool,
}

/// Job scheduler
pub struct JobScheduler {
    pub jobs: Vec<Job>,
    pub next_id: u32,
    pub max_concurrent: usize,
    pub running_count: usize,
}

impl JobScheduler {
    const fn new() -> Self {
        JobScheduler {
            jobs: Vec::new(),
            next_id: 1,
            max_concurrent: 4,
            running_count: 0,
        }
    }

    pub fn schedule(
        &mut self,
        app_id: &str,
        tag: &str,
        constraints: JobConstraints,
        periodic: Option<u64>,
        expedited: bool,
    ) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.jobs.push(Job {
            id,
            app_id: String::from(app_id),
            tag: String::from(tag),
            state: JobState::Pending,
            constraints,
            periodic_ms: periodic,
            flex_ms: None,
            initial_delay_ms: 0,
            backoff_policy: BackoffPolicy::Exponential,
            backoff_delay_ms: 30000,
            max_retries: 3,
            retry_count: 0,
            scheduled_at: crate::time::clock::unix_time(),
            started_at: None,
            expedited,
        });
        id
    }

    pub fn cancel(&mut self, id: u32) -> bool {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            job.state = JobState::Cancelled;
            true
        } else {
            false
        }
    }

    pub fn cancel_all_for_app(&mut self, app_id: &str) {
        for job in &mut self.jobs {
            if job.app_id == app_id && job.state == JobState::Pending {
                job.state = JobState::Cancelled;
            }
        }
    }

    pub fn tick(&mut self) {
        // Start pending jobs that meet constraints
        let now = crate::time::clock::unix_time();

        for job in &mut self.jobs {
            if job.state != JobState::Pending {
                continue;
            }
            if self.running_count >= self.max_concurrent {
                break;
            }

            // Check if enough time has passed
            let delay = job.initial_delay_ms / 1000;
            if now < job.scheduled_at + delay {
                continue;
            }

            // In real implementation: check constraints (network, battery, etc.)
            job.state = JobState::Running;
            job.started_at = Some(now);
            self.running_count = self.running_count.saturating_add(1);
        }
    }

    pub fn complete_job(&mut self, id: u32, success: bool) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            if success {
                job.state = JobState::Completed;
                // Reschedule if periodic
                if let Some(period) = job.periodic_ms {
                    job.state = JobState::Pending;
                    job.scheduled_at = crate::time::clock::unix_time() + period / 1000;
                    job.started_at = None;
                }
            } else {
                job.retry_count = job.retry_count.saturating_add(1);
                if job.retry_count >= job.max_retries {
                    job.state = JobState::Failed;
                } else {
                    job.state = JobState::Rescheduled;
                    let backoff = match job.backoff_policy {
                        BackoffPolicy::Linear => job.backoff_delay_ms * job.retry_count as u64,
                        BackoffPolicy::Exponential => job.backoff_delay_ms * (1 << job.retry_count),
                    };
                    job.scheduled_at = crate::time::clock::unix_time() + backoff / 1000;
                    job.state = JobState::Pending;
                }
            }
            if self.running_count > 0 {
                self.running_count -= 1;
            }
        }
    }

    pub fn pending_count(&self) -> usize {
        self.jobs
            .iter()
            .filter(|j| j.state == JobState::Pending)
            .count()
    }
}

static SCHEDULER: Mutex<JobScheduler> = Mutex::new(JobScheduler::new());

pub fn init() {
    crate::serial_println!("  [services] Job scheduler initialized");
}

pub fn tick() {
    SCHEDULER.lock().tick();
}
