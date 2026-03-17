use crate::sync::Mutex;
/// Cron — timed task execution for Genesis
///
/// Runs scheduled commands at specified intervals.
/// Supports per-user crontabs and system-wide schedules.
///
/// Crontab format (simplified):
///   interval_secs  command
///   @reboot        command  (run once at boot)
///   @hourly        command  (every 3600s)
///   @daily         command  (every 86400s)
///
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

/// A cron job entry
#[derive(Debug, Clone)]
pub struct CronJob {
    pub id: u32,
    /// Interval in seconds (0 = one-shot / @reboot)
    pub interval_secs: u64,
    /// Command to execute
    pub command: String,
    /// User who owns this job
    pub uid: u32,
    /// Last execution time (uptime_secs)
    pub last_run: u64,
    /// Whether this job has run (for one-shot jobs)
    pub has_run: bool,
    /// Whether this is a @reboot job
    pub at_reboot: bool,
    /// Whether this job is enabled
    pub enabled: bool,
}

/// Cron daemon state
pub struct CronDaemon {
    pub jobs: Vec<CronJob>,
    pub next_id: u32,
    pub last_check: u64,
}

impl CronDaemon {
    pub const fn new() -> Self {
        CronDaemon {
            jobs: Vec::new(),
            next_id: 1,
            last_check: 0,
        }
    }

    /// Add a periodic job
    pub fn add_periodic(&mut self, interval_secs: u64, command: &str, uid: u32) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.jobs.push(CronJob {
            id,
            interval_secs,
            command: String::from(command),
            uid,
            last_run: 0,
            has_run: false,
            at_reboot: false,
            enabled: true,
        });
        id
    }

    /// Add a @reboot job (runs once)
    pub fn add_reboot(&mut self, command: &str, uid: u32) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.jobs.push(CronJob {
            id,
            interval_secs: 0,
            command: String::from(command),
            uid,
            last_run: 0,
            has_run: false,
            at_reboot: true,
            enabled: true,
        });
        id
    }

    /// Remove a job by ID
    pub fn remove(&mut self, id: u32) -> bool {
        let before = self.jobs.len();
        self.jobs.retain(|j| j.id != id);
        self.jobs.len() < before
    }

    /// Check and return jobs that are due
    pub fn check_due(&mut self, now_secs: u64) -> Vec<CronJob> {
        let mut due = Vec::new();
        for job in &mut self.jobs {
            if !job.enabled {
                continue;
            }

            if job.at_reboot && !job.has_run {
                job.has_run = true;
                job.last_run = now_secs;
                due.push(job.clone());
            } else if !job.at_reboot && job.interval_secs > 0 {
                if now_secs >= job.last_run + job.interval_secs {
                    job.last_run = now_secs;
                    due.push(job.clone());
                }
            }
        }
        self.last_check = now_secs;
        due
    }

    /// List all jobs
    pub fn list(&self) -> &[CronJob] {
        &self.jobs
    }

    /// Format jobs for display
    pub fn format_table(&self) -> String {
        let mut out = String::from("ID  INTERVAL     USER  ENABLED  COMMAND\n");
        for job in &self.jobs {
            let interval = if job.at_reboot {
                String::from("@reboot")
            } else if job.interval_secs == 3600 {
                String::from("@hourly")
            } else if job.interval_secs == 86400 {
                String::from("@daily")
            } else {
                alloc::format!("{}s", job.interval_secs)
            };
            out.push_str(&alloc::format!(
                "{:<3} {:<12} {:<5} {:<8} {}\n",
                job.id,
                interval,
                job.uid,
                if job.enabled { "yes" } else { "no" },
                job.command
            ));
        }
        out
    }
}

static CRON: Mutex<CronDaemon> = Mutex::new(CronDaemon::new());

/// Initialize cron with default system jobs
pub fn init() {
    let mut cron = CRON.lock();

    // System maintenance — sync filesystem cache every 5 minutes
    cron.add_periodic(300, "sync", 0);

    // Log rotation — every hour
    cron.add_periodic(3600, "logrotate", 0);

    serial_println!("  Cron: daemon ready ({} jobs)", cron.jobs.len());
}

/// Add a cron job
pub fn add(interval_secs: u64, command: &str, uid: u32) -> u32 {
    CRON.lock().add_periodic(interval_secs, command, uid)
}

/// Remove a cron job
pub fn remove(id: u32) -> bool {
    CRON.lock().remove(id)
}

/// Check for due jobs (call periodically from main loop)
pub fn tick() -> Vec<CronJob> {
    let now = crate::time::clock::uptime_secs();
    CRON.lock().check_due(now)
}

/// List all cron jobs
pub fn list() -> String {
    CRON.lock().format_table()
}
