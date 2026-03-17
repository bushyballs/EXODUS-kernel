use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
/// Hoags Backup/Restore — full device backup and recovery for Genesis
///
/// Features:
///   - Full, incremental, selective, app-data, settings, media, contacts backups
///   - Status tracking: pending, running, paused, complete, failed, verifying
///   - Progress monitoring with items total/done
///   - Scheduled automatic backups with configurable intervals
///   - Backup verification via integrity checksums
///   - Size estimation before starting
///   - Encrypted backup output with XOR-derived key
///
/// All progress fractions use Q16 fixed-point (i32, 1.0 = 65536).
/// No floating-point. No external crates. All code is original.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers (1.0 = 65536)
// ---------------------------------------------------------------------------

const Q16_ONE: i32 = 65536;

fn q16_from_int(v: i32) -> i32 {
    v * Q16_ONE
}

fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    ((a as i64 * Q16_ONE as i64) / b as i64) as i32
}

fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) / Q16_ONE as i64) as i32
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Type of backup to create
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupType {
    /// Complete system backup (all data)
    Full,
    /// Only changes since last backup
    Incremental,
    /// User-selected files and directories
    Selective,
    /// Application data only
    AppData,
    /// System and user settings
    Settings,
    /// Media files (photos, videos, audio)
    Media,
    /// Contact records
    Contacts,
}

/// Current status of a backup job
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupStatus {
    /// Queued but not started
    Pending,
    /// Currently in progress
    Running,
    /// Temporarily paused by user
    Paused,
    /// Successfully finished
    Complete,
    /// Terminated due to error
    Failed,
    /// Checking integrity after completion
    Verifying,
}

// ---------------------------------------------------------------------------
// Backup job
// ---------------------------------------------------------------------------

/// Represents a single backup or restore operation
#[derive(Debug, Clone)]
pub struct BackupJob {
    /// Unique job identifier
    pub id: u64,
    /// What kind of backup
    pub backup_type: BackupType,
    /// Current job status
    pub status: BackupStatus,
    /// Total size in bytes
    pub size_bytes: u64,
    /// Total items (files/records) to process
    pub items_total: u32,
    /// Items processed so far
    pub items_done: u32,
    /// Timestamp when the job started
    pub started: u64,
    /// Timestamp when the job completed (0 if not done)
    pub completed: u64,
    /// Hash of the destination path/device
    pub destination_hash: u64,
}

/// Scheduled backup configuration
#[derive(Debug, Clone)]
struct ScheduledBackup {
    /// Backup type to run
    backup_type: BackupType,
    /// Destination hash
    destination_hash: u64,
    /// Interval in seconds between runs
    interval_secs: u64,
    /// Timestamp of the last run (0 = never)
    last_run: u64,
    /// Whether this schedule is enabled
    enabled: bool,
}

// ---------------------------------------------------------------------------
// Backup manager state
// ---------------------------------------------------------------------------

struct BackupManager {
    /// All backup jobs (history + active)
    jobs: Vec<BackupJob>,
    /// Scheduled backup configurations
    schedules: Vec<ScheduledBackup>,
    /// Next job ID
    next_id: u64,
    /// Encryption key hash for backup encryption
    encryption_key_hash: u64,
    /// Maximum number of stored backup jobs to retain
    max_history: usize,
}

impl BackupManager {
    const fn new() -> Self {
        BackupManager {
            jobs: Vec::new(),
            schedules: Vec::new(),
            next_id: 1,
            encryption_key_hash: 0xABCDEF0123456789,
            max_history: 64,
        }
    }
}

static BACKUP_MGR: Mutex<Option<BackupManager>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Simple checksum for verification
// ---------------------------------------------------------------------------

fn compute_checksum(data: &[u8]) -> u64 {
    let mut h: u64 = 0xCBF29CE484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001B3);
    }
    h
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new backup job and return its ID
pub fn create_backup(
    backup_type: BackupType,
    destination_hash: u64,
    estimated_items: u32,
    current_time: u64,
) -> u64 {
    let mut guard = BACKUP_MGR.lock();
    if let Some(ref mut mgr) = *guard {
        let id = mgr.next_id;
        mgr.next_id += 1;

        let job = BackupJob {
            id,
            backup_type,
            status: BackupStatus::Running,
            size_bytes: 0,
            items_total: estimated_items,
            items_done: 0,
            started: current_time,
            completed: 0,
            destination_hash,
        };

        mgr.jobs.push(job);
        // Prune old history if needed
        while mgr.jobs.len() > mgr.max_history {
            mgr.jobs.remove(0);
        }

        serial_println!("  Backup: created job {} ({:?})", id, backup_type);
        id
    } else {
        0
    }
}

/// Start a restore operation from an existing backup
pub fn restore_backup(backup_id: u64, current_time: u64) -> bool {
    let mut guard = BACKUP_MGR.lock();
    if let Some(ref mut mgr) = *guard {
        // Find the completed backup to restore from
        let source = mgr
            .jobs
            .iter()
            .find(|j| j.id == backup_id && matches!(j.status, BackupStatus::Complete));

        if let Some(src) = source {
            let restore_id = mgr.next_id;
            mgr.next_id += 1;

            let restore_job = BackupJob {
                id: restore_id,
                backup_type: src.backup_type,
                status: BackupStatus::Running,
                size_bytes: src.size_bytes,
                items_total: src.items_total,
                items_done: 0,
                started: current_time,
                completed: 0,
                destination_hash: src.destination_hash,
            };

            mgr.jobs.push(restore_job);
            serial_println!(
                "  Backup: restore started from backup {}, new job {}",
                backup_id,
                restore_id
            );
            true
        } else {
            serial_println!(
                "  Backup: cannot restore, backup {} not found or incomplete",
                backup_id
            );
            false
        }
    } else {
        false
    }
}

/// Schedule an automatic backup
pub fn schedule_backup(backup_type: BackupType, destination_hash: u64, interval_secs: u64) {
    let mut guard = BACKUP_MGR.lock();
    if let Some(ref mut mgr) = *guard {
        // Replace existing schedule for the same type, or add new
        if let Some(sched) = mgr.schedules.iter_mut().find(|s| {
            matches!(
                (&s.backup_type, &backup_type),
                (BackupType::Full, BackupType::Full)
                    | (BackupType::Incremental, BackupType::Incremental)
                    | (BackupType::Selective, BackupType::Selective)
                    | (BackupType::AppData, BackupType::AppData)
                    | (BackupType::Settings, BackupType::Settings)
                    | (BackupType::Media, BackupType::Media)
                    | (BackupType::Contacts, BackupType::Contacts)
            )
        }) {
            sched.destination_hash = destination_hash;
            sched.interval_secs = interval_secs;
            sched.enabled = true;
        } else {
            mgr.schedules.push(ScheduledBackup {
                backup_type,
                destination_hash,
                interval_secs,
                last_run: 0,
                enabled: true,
            });
        }
        serial_println!(
            "  Backup: scheduled {:?} every {} seconds",
            backup_type,
            interval_secs
        );
    }
}

/// List all backup jobs (history)
pub fn list_backups() -> Vec<BackupJob> {
    let guard = BACKUP_MGR.lock();
    if let Some(ref mgr) = *guard {
        mgr.jobs.clone()
    } else {
        Vec::new()
    }
}

/// Delete a backup job from history by ID
pub fn delete_backup(job_id: u64) -> bool {
    let mut guard = BACKUP_MGR.lock();
    if let Some(ref mut mgr) = *guard {
        let before = mgr.jobs.len();
        mgr.jobs.retain(|j| j.id != job_id);
        let removed = mgr.jobs.len() < before;
        if removed {
            serial_println!("  Backup: deleted job {}", job_id);
        }
        removed
    } else {
        false
    }
}

/// Verify a completed backup's integrity
pub fn verify_backup(job_id: u64, data: &[u8], expected_checksum: u64) -> bool {
    let mut guard = BACKUP_MGR.lock();
    if let Some(ref mut mgr) = *guard {
        if let Some(job) = mgr.jobs.iter_mut().find(|j| j.id == job_id) {
            job.status = BackupStatus::Verifying;
            let actual = compute_checksum(data);
            if actual == expected_checksum {
                job.status = BackupStatus::Complete;
                serial_println!("  Backup: job {} verified OK", job_id);
                return true;
            } else {
                job.status = BackupStatus::Failed;
                serial_println!(
                    "  Backup: job {} verification FAILED (expected {:016X}, got {:016X})",
                    job_id,
                    expected_checksum,
                    actual
                );
                return false;
            }
        }
    }
    false
}

/// Get progress of a backup job as Q16 fraction (0 = 0%, 65536 = 100%)
pub fn get_progress(job_id: u64) -> i32 {
    let guard = BACKUP_MGR.lock();
    if let Some(ref mgr) = *guard {
        if let Some(job) = mgr.jobs.iter().find(|j| j.id == job_id) {
            if job.items_total == 0 {
                return 0;
            }
            return q16_div(job.items_done as i32, job.items_total as i32);
        }
    }
    0
}

/// Estimate the size in bytes for a given backup type
/// Returns estimated size based on item count and average item size heuristics
pub fn estimate_size(backup_type: BackupType, item_count: u32) -> u64 {
    let avg_item_bytes: u64 = match backup_type {
        BackupType::Full => 32768,       // 32 KB average per item
        BackupType::Incremental => 8192, // 8 KB average (only changes)
        BackupType::Selective => 16384,  // 16 KB average
        BackupType::AppData => 65536,    // 64 KB average per app
        BackupType::Settings => 512,     // 512 bytes average per setting
        BackupType::Media => 2097152,    // 2 MB average per media file
        BackupType::Contacts => 1024,    // 1 KB average per contact
    };
    let estimated = avg_item_bytes * item_count as u64;
    serial_println!(
        "  Backup: estimated size for {:?} ({} items): {} bytes",
        backup_type,
        item_count,
        estimated
    );
    estimated
}

/// Encrypt backup data with the manager's encryption key
/// Returns encrypted bytes with a prepended checksum
pub fn encrypt_backup(data: &[u8]) -> Vec<u8> {
    let guard = BACKUP_MGR.lock();
    if let Some(ref mgr) = *guard {
        let checksum = compute_checksum(data);
        let key_bytes = mgr.encryption_key_hash.to_le_bytes();

        let mut output = Vec::with_capacity(8 + data.len());
        // Prepend checksum for later verification
        output.extend_from_slice(&checksum.to_le_bytes());
        // XOR encrypt with rotating key
        for (i, &b) in data.iter().enumerate() {
            output.push(b ^ key_bytes[i % 8]);
        }
        serial_println!("  Backup: encrypted {} bytes", data.len());
        output
    } else {
        Vec::new()
    }
}

/// Update progress on an active job (called by the backup engine)
pub fn update_progress(job_id: u64, items_done: u32, bytes_written: u64, current_time: u64) {
    let mut guard = BACKUP_MGR.lock();
    if let Some(ref mut mgr) = *guard {
        if let Some(job) = mgr.jobs.iter_mut().find(|j| j.id == job_id) {
            job.items_done = items_done;
            job.size_bytes = bytes_written;
            if items_done >= job.items_total {
                job.status = BackupStatus::Complete;
                job.completed = current_time;
                serial_println!(
                    "  Backup: job {} complete ({} bytes)",
                    job_id,
                    bytes_written
                );
            }
        }
    }
}

/// Pause a running backup job
pub fn pause_backup(job_id: u64) -> bool {
    let mut guard = BACKUP_MGR.lock();
    if let Some(ref mut mgr) = *guard {
        if let Some(job) = mgr.jobs.iter_mut().find(|j| j.id == job_id) {
            if matches!(job.status, BackupStatus::Running) {
                job.status = BackupStatus::Paused;
                serial_println!("  Backup: job {} paused", job_id);
                return true;
            }
        }
    }
    false
}

/// Resume a paused backup job
pub fn resume_backup(job_id: u64) -> bool {
    let mut guard = BACKUP_MGR.lock();
    if let Some(ref mut mgr) = *guard {
        if let Some(job) = mgr.jobs.iter_mut().find(|j| j.id == job_id) {
            if matches!(job.status, BackupStatus::Paused) {
                job.status = BackupStatus::Running;
                serial_println!("  Backup: job {} resumed", job_id);
                return true;
            }
        }
    }
    false
}

/// Check if any scheduled backups need to run, returns list of types due
pub fn check_schedules(current_time: u64) -> Vec<BackupType> {
    let guard = BACKUP_MGR.lock();
    if let Some(ref mgr) = *guard {
        let mut due = Vec::new();
        for sched in &mgr.schedules {
            if sched.enabled && (current_time - sched.last_run) >= sched.interval_secs {
                due.push(sched.backup_type);
            }
        }
        due
    } else {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the backup/restore subsystem
pub fn init() {
    let mut guard = BACKUP_MGR.lock();
    *guard = Some(BackupManager::new());
    serial_println!("  Backup: manager initialized");
}
