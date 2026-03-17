/// Backup manager application for Genesis OS
///
/// Scheduled and on-demand backups with incremental and full modes,
/// restore points, integrity verification, and optional encryption.
/// Backup manifests track file hashes for deduplication. All sizes
/// and progress use Q16 fixed-point. No floating point used.
///
/// Inspired by: Timeshift, BorgBackup, rsync, Time Machine. All code is original.

use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 helpers
// ---------------------------------------------------------------------------

/// 1.0 in Q16
const Q16_ONE: i32 = 65536;

/// Q16 division: (a << 16) / b
fn q16_div(a: i32, b: i32) -> Option<i32> {
    if b == 0 {
        return None;
    }
    Some((((a as i64) << 16) / (b as i64)) as i32)
}

/// Q16 percentage: (part * 100) / total  in Q16
fn q16_percent(part: i32, total: i32) -> i32 {
    match q16_div(part, total) {
        Some(ratio) => ((ratio as i64 * 100 * Q16_ONE as i64) >> 16) as i32,
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum backup jobs
const MAX_JOBS: usize = 128;
/// Maximum restore points
const MAX_RESTORE_POINTS: usize = 1024;
/// Maximum files in a single backup manifest
const MAX_MANIFEST_FILES: usize = 100_000;
/// Maximum scheduled tasks
const MAX_SCHEDULES: usize = 64;
/// Maximum exclusion patterns
const MAX_EXCLUSIONS: usize = 256;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Backup type
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BackupType {
    Full,
    Incremental,
    Differential,
    Mirror,
}

/// Backup status
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BackupStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
    Verifying,
}

/// Schedule frequency
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScheduleFreq {
    Hourly,
    Daily,
    Weekly,
    Monthly,
    Custom,
}

/// Encryption algorithm
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EncryptionAlgo {
    None,
    Aes128,
    Aes256,
    ChaCha20,
}

/// Compression mode
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Compression {
    None,
    Lz4,
    Zstd,
    Deflate,
}

/// Verification result
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VerifyResult {
    Valid,
    Corrupted,
    Missing,
    Mismatch,
    NotChecked,
}

/// Backup operation result
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BackupResult {
    Success,
    NotFound,
    AlreadyExists,
    LimitReached,
    InProgress,
    InvalidConfig,
    StorageFull,
    VerifyFailed,
    DecryptFailed,
    IoError,
}

/// A file entry in a backup manifest
#[derive(Debug, Clone, Copy)]
pub struct ManifestEntry {
    pub path_hash: u64,
    pub content_hash: u64,
    pub size_bytes: u64,
    pub modified: u64,
    pub permissions: u32,
    pub is_dir: bool,
}

/// A restore point (snapshot of a completed backup)
#[derive(Debug, Clone)]
pub struct RestorePoint {
    pub id: u64,
    pub job_id: u64,
    pub label_hash: u64,
    pub backup_type: BackupType,
    pub created: u64,
    pub total_size: u64,
    pub file_count: u32,
    pub manifest: Vec<ManifestEntry>,
    pub encrypted: bool,
    pub encryption: EncryptionAlgo,
    pub compression: Compression,
    pub verify_status: VerifyResult,
    pub parent_id: u64,
}

/// A backup job definition
#[derive(Debug, Clone)]
pub struct BackupJob {
    pub id: u64,
    pub name_hash: u64,
    pub source_hash: u64,
    pub dest_hash: u64,
    pub backup_type: BackupType,
    pub status: BackupStatus,
    pub encryption: EncryptionAlgo,
    pub key_hash: u64,
    pub compression: Compression,
    pub created: u64,
    pub last_run: u64,
    pub next_run: u64,
    pub total_bytes: u64,
    pub bytes_done: u64,
    pub files_total: u32,
    pub files_done: u32,
    pub error_count: u32,
    pub run_count: u32,
    pub exclusions: Vec<u64>,
}

/// A backup schedule
#[derive(Debug, Clone)]
pub struct BackupSchedule {
    pub id: u64,
    pub job_id: u64,
    pub frequency: ScheduleFreq,
    pub hour: u8,
    pub minute: u8,
    pub day_of_week: u8,
    pub day_of_month: u8,
    pub enabled: bool,
    pub retain_count: u32,
    pub last_triggered: u64,
}

/// Backup manager state
struct BackupMgrState {
    jobs: Vec<BackupJob>,
    restore_points: Vec<RestorePoint>,
    schedules: Vec<BackupSchedule>,
    global_exclusions: Vec<u64>,
    next_job_id: u64,
    next_point_id: u64,
    next_schedule_id: u64,
    timestamp: u64,
    total_backup_bytes: u64,
    total_backups_completed: u32,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static BACKUP_MGR: Mutex<Option<BackupMgrState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn next_timestamp(state: &mut BackupMgrState) -> u64 {
    state.timestamp += 1;
    state.timestamp
}

fn default_state() -> BackupMgrState {
    BackupMgrState {
        jobs: Vec::new(),
        restore_points: Vec::new(),
        schedules: Vec::new(),
        global_exclusions: Vec::new(),
        next_job_id: 1,
        next_point_id: 1,
        next_schedule_id: 1,
        timestamp: 0,
        total_backup_bytes: 0,
        total_backups_completed: 0,
    }
}

fn job_progress_q16(job: &BackupJob) -> i32 {
    if job.total_bytes == 0 {
        return 0;
    }
    q16_percent(job.bytes_done as i32, job.total_bytes as i32)
}

// ---------------------------------------------------------------------------
// Public API -- Job management
// ---------------------------------------------------------------------------

/// Create a new backup job
pub fn create_job(
    name_hash: u64,
    source_hash: u64,
    dest_hash: u64,
    backup_type: BackupType,
    encryption: EncryptionAlgo,
    key_hash: u64,
    compression: Compression,
) -> Result<u64, BackupResult> {
    let mut guard = BACKUP_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return Err(BackupResult::IoError),
    };
    if state.jobs.len() >= MAX_JOBS {
        return Err(BackupResult::LimitReached);
    }
    let now = next_timestamp(state);
    let id = state.next_job_id;
    state.next_job_id += 1;
    state.jobs.push(BackupJob {
        id,
        name_hash,
        source_hash,
        dest_hash,
        backup_type,
        status: BackupStatus::Pending,
        encryption,
        key_hash,
        compression,
        created: now,
        last_run: 0,
        next_run: 0,
        total_bytes: 0,
        bytes_done: 0,
        files_total: 0,
        files_done: 0,
        error_count: 0,
        run_count: 0,
        exclusions: Vec::new(),
    });
    Ok(id)
}

/// Delete a backup job (must not be running)
pub fn delete_job(job_id: u64) -> BackupResult {
    let mut guard = BACKUP_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return BackupResult::IoError,
    };
    if let Some(job) = state.jobs.iter().find(|j| j.id == job_id) {
        if job.status == BackupStatus::Running {
            return BackupResult::InProgress;
        }
    } else {
        return BackupResult::NotFound;
    }
    state.jobs.retain(|j| j.id != job_id);
    state.schedules.retain(|s| s.job_id != job_id);
    BackupResult::Success
}

/// Add an exclusion pattern to a job
pub fn add_exclusion(job_id: u64, pattern_hash: u64) -> BackupResult {
    let mut guard = BACKUP_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return BackupResult::IoError,
    };
    let job = match state.jobs.iter_mut().find(|j| j.id == job_id) {
        Some(j) => j,
        None => return BackupResult::NotFound,
    };
    if job.exclusions.contains(&pattern_hash) {
        return BackupResult::AlreadyExists;
    }
    job.exclusions.push(pattern_hash);
    BackupResult::Success
}

/// Add a global exclusion pattern
pub fn add_global_exclusion(pattern_hash: u64) -> BackupResult {
    let mut guard = BACKUP_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return BackupResult::IoError,
    };
    if state.global_exclusions.len() >= MAX_EXCLUSIONS {
        return BackupResult::LimitReached;
    }
    if state.global_exclusions.contains(&pattern_hash) {
        return BackupResult::AlreadyExists;
    }
    state.global_exclusions.push(pattern_hash);
    BackupResult::Success
}

/// List all backup jobs
pub fn list_jobs() -> Vec<BackupJob> {
    let guard = BACKUP_MGR.lock();
    match guard.as_ref() {
        Some(state) => state.jobs.clone(),
        None => Vec::new(),
    }
}

/// Get job progress as Q16 percentage
pub fn get_progress(job_id: u64) -> Option<i32> {
    let guard = BACKUP_MGR.lock();
    let state = guard.as_ref()?;
    let job = state.jobs.iter().find(|j| j.id == job_id)?;
    Some(job_progress_q16(job))
}

// ---------------------------------------------------------------------------
// Public API -- Backup execution
// ---------------------------------------------------------------------------

/// Start a backup job
pub fn start_backup(job_id: u64, total_bytes: u64, file_count: u32) -> BackupResult {
    let mut guard = BACKUP_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return BackupResult::IoError,
    };
    let now = next_timestamp(state);
    let job = match state.jobs.iter_mut().find(|j| j.id == job_id) {
        Some(j) => j,
        None => return BackupResult::NotFound,
    };
    if job.status == BackupStatus::Running {
        return BackupResult::InProgress;
    }
    job.status = BackupStatus::Running;
    job.total_bytes = total_bytes;
    job.bytes_done = 0;
    job.files_total = file_count;
    job.files_done = 0;
    job.error_count = 0;
    job.last_run = now;
    job.run_count += 1;
    BackupResult::Success
}

/// Report progress on a running backup
pub fn report_progress(job_id: u64, bytes_done: u64, files_done: u32, errors: u32) -> BackupResult {
    let mut guard = BACKUP_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return BackupResult::IoError,
    };
    let job = match state.jobs.iter_mut().find(|j| j.id == job_id) {
        Some(j) => j,
        None => return BackupResult::NotFound,
    };
    if job.status != BackupStatus::Running {
        return BackupResult::InvalidConfig;
    }
    job.bytes_done = bytes_done;
    job.files_done = files_done;
    job.error_count = errors;
    BackupResult::Success
}

/// Complete a backup and create a restore point
pub fn complete_backup(job_id: u64, manifest: Vec<ManifestEntry>) -> Result<u64, BackupResult> {
    let mut guard = BACKUP_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return Err(BackupResult::IoError),
    };
    if state.restore_points.len() >= MAX_RESTORE_POINTS {
        return Err(BackupResult::LimitReached);
    }
    let now = next_timestamp(state);
    let job = match state.jobs.iter_mut().find(|j| j.id == job_id) {
        Some(j) => j,
        None => return Err(BackupResult::NotFound),
    };
    if job.status != BackupStatus::Running {
        return Err(BackupResult::InvalidConfig);
    }
    job.status = BackupStatus::Completed;
    job.bytes_done = job.total_bytes;
    job.files_done = job.files_total;

    let point_id = state.next_point_id;
    state.next_point_id += 1;
    let file_count = manifest.len() as u32;
    let total_size: u64 = manifest.iter().map(|e| e.size_bytes).sum();

    // Find previous restore point for this job (for incremental chain)
    let parent_id = state
        .restore_points
        .iter()
        .rev()
        .find(|rp| rp.job_id == job_id)
        .map(|rp| rp.id)
        .unwrap_or(0);

    state.restore_points.push(RestorePoint {
        id: point_id,
        job_id,
        label_hash: job.name_hash,
        backup_type: job.backup_type,
        created: now,
        total_size,
        file_count,
        manifest,
        encrypted: job.encryption != EncryptionAlgo::None,
        encryption: job.encryption,
        compression: job.compression,
        verify_status: VerifyResult::NotChecked,
        parent_id,
    });

    state.total_backup_bytes += total_size;
    state.total_backups_completed += 1;

    Ok(point_id)
}

/// Cancel a running backup
pub fn cancel_backup(job_id: u64) -> BackupResult {
    let mut guard = BACKUP_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return BackupResult::IoError,
    };
    let job = match state.jobs.iter_mut().find(|j| j.id == job_id) {
        Some(j) => j,
        None => return BackupResult::NotFound,
    };
    if job.status != BackupStatus::Running {
        return BackupResult::InvalidConfig;
    }
    job.status = BackupStatus::Cancelled;
    BackupResult::Success
}

// ---------------------------------------------------------------------------
// Public API -- Restore points
// ---------------------------------------------------------------------------

/// List all restore points
pub fn list_restore_points() -> Vec<RestorePoint> {
    let guard = BACKUP_MGR.lock();
    match guard.as_ref() {
        Some(state) => state.restore_points.clone(),
        None => Vec::new(),
    }
}

/// Get a specific restore point
pub fn get_restore_point(point_id: u64) -> Option<RestorePoint> {
    let guard = BACKUP_MGR.lock();
    let state = guard.as_ref()?;
    state.restore_points.iter().find(|rp| rp.id == point_id).cloned()
}

/// Delete a restore point
pub fn delete_restore_point(point_id: u64) -> BackupResult {
    let mut guard = BACKUP_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return BackupResult::IoError,
    };
    let before = state.restore_points.len();
    state.restore_points.retain(|rp| rp.id != point_id);
    if state.restore_points.len() < before {
        BackupResult::Success
    } else {
        BackupResult::NotFound
    }
}

/// Verify a restore point's integrity
pub fn verify_restore_point(point_id: u64) -> BackupResult {
    let mut guard = BACKUP_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return BackupResult::IoError,
    };
    let rp = match state.restore_points.iter_mut().find(|rp| rp.id == point_id) {
        Some(rp) => rp,
        None => return BackupResult::NotFound,
    };
    // Simulate verification by checking manifest consistency
    let mut valid = true;
    for entry in rp.manifest.iter() {
        if entry.content_hash == 0 && entry.size_bytes > 0 && !entry.is_dir {
            valid = false;
            break;
        }
    }
    rp.verify_status = if valid {
        VerifyResult::Valid
    } else {
        VerifyResult::Corrupted
    };
    if valid {
        BackupResult::Success
    } else {
        BackupResult::VerifyFailed
    }
}

// ---------------------------------------------------------------------------
// Public API -- Scheduling
// ---------------------------------------------------------------------------

/// Create a backup schedule
pub fn create_schedule(
    job_id: u64,
    frequency: ScheduleFreq,
    hour: u8,
    minute: u8,
    retain_count: u32,
) -> Result<u64, BackupResult> {
    let mut guard = BACKUP_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return Err(BackupResult::IoError),
    };
    if state.schedules.len() >= MAX_SCHEDULES {
        return Err(BackupResult::LimitReached);
    }
    if !state.jobs.iter().any(|j| j.id == job_id) {
        return Err(BackupResult::NotFound);
    }
    let id = state.next_schedule_id;
    state.next_schedule_id += 1;
    state.schedules.push(BackupSchedule {
        id,
        job_id,
        frequency,
        hour,
        minute,
        day_of_week: 0,
        day_of_month: 1,
        enabled: true,
        retain_count,
        last_triggered: 0,
    });
    Ok(id)
}

/// Enable or disable a schedule
pub fn toggle_schedule(schedule_id: u64, enabled: bool) -> BackupResult {
    let mut guard = BACKUP_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return BackupResult::IoError,
    };
    if let Some(sched) = state.schedules.iter_mut().find(|s| s.id == schedule_id) {
        sched.enabled = enabled;
        BackupResult::Success
    } else {
        BackupResult::NotFound
    }
}

/// Delete a schedule
pub fn delete_schedule(schedule_id: u64) -> BackupResult {
    let mut guard = BACKUP_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return BackupResult::IoError,
    };
    let before = state.schedules.len();
    state.schedules.retain(|s| s.id != schedule_id);
    if state.schedules.len() < before {
        BackupResult::Success
    } else {
        BackupResult::NotFound
    }
}

/// List all schedules
pub fn list_schedules() -> Vec<BackupSchedule> {
    let guard = BACKUP_MGR.lock();
    match guard.as_ref() {
        Some(state) => state.schedules.clone(),
        None => Vec::new(),
    }
}

/// Get total backup statistics
pub fn stats() -> (u64, u32) {
    let guard = BACKUP_MGR.lock();
    match guard.as_ref() {
        Some(state) => (state.total_backup_bytes, state.total_backups_completed),
        None => (0, 0),
    }
}

/// Get job count
pub fn job_count() -> usize {
    let guard = BACKUP_MGR.lock();
    match guard.as_ref() {
        Some(state) => state.jobs.len(),
        None => 0,
    }
}

/// Get restore point count
pub fn restore_point_count() -> usize {
    let guard = BACKUP_MGR.lock();
    match guard.as_ref() {
        Some(state) => state.restore_points.len(),
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the backup manager subsystem
pub fn init() {
    let mut guard = BACKUP_MGR.lock();
    *guard = Some(default_state());
    serial_println!("    Backup manager ready");
}
