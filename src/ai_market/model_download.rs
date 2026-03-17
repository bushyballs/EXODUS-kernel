/// AI Model Download Manager for Genesis
///
/// Manages the full lifecycle of model downloads: queueing, bandwidth
/// management, pause/resume, checksum verification, and installation.
/// Supports concurrent downloads with per-task progress tracking.
///
/// All sizes and speeds are tracked in bytes. No floating-point arithmetic;
/// fixed-point Q16 is used for progress percentages.
///
/// Original implementation for Hoags OS.
use crate::sync::Mutex;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ── Q16 helpers ────────────────────────────────────────────────────────────

pub type Q16 = i32;

const Q16_ONE: Q16 = 65536;

fn q16_from_int(v: i32) -> Q16 {
    v << 16
}

fn q16_div(a: Q16, b: Q16) -> Q16 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << 16) / (b as i64)) as i32
}

fn q16_mul(a: Q16, b: Q16) -> Q16 {
    ((a as i64 * b as i64) >> 16) as i32
}

// ── Enums ──────────────────────────────────────────────────────────────────

/// Status of a download task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadStatus {
    Queued,
    Downloading,
    Paused,
    Verifying,
    Installing,
    Complete,
    Error,
}

/// Error codes for download failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadError {
    NetworkUnavailable,
    InsufficientStorage,
    ChecksumMismatch,
    Timeout,
    Cancelled,
    InstallFailed,
    CorruptData,
    PermissionDenied,
}

/// Download priority level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DownloadPriority {
    Low,
    Normal,
    High,
    Critical,
}

// ── Core structs ───────────────────────────────────────────────────────────

/// A single download task tracking a model being fetched.
#[derive(Clone)]
pub struct DownloadTask {
    pub id: u32,
    pub model_id: u32,
    pub url_hash: u64,
    pub total_bytes: u64,
    pub downloaded_bytes: u64,
    pub status: DownloadStatus,
    pub checksum: u64,
    pub started: u64,
    pub speed_bps: u32,
    pub priority: DownloadPriority,
    pub retry_count: u8,
    pub max_retries: u8,
    pub error: Option<DownloadError>,
    pub chunks_complete: u32,
    pub chunks_total: u32,
    pub last_activity: u64,
}

/// Summary of a completed or in-progress download.
#[derive(Clone)]
pub struct DownloadProgress {
    pub task_id: u32,
    pub model_id: u32,
    pub percent: Q16,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub speed_bps: u32,
    pub status: DownloadStatus,
    pub eta_seconds: u64,
}

/// Bandwidth allocation configuration.
#[derive(Clone)]
pub struct BandwidthConfig {
    pub max_concurrent: u8,
    pub max_bps: u32,
    pub per_task_max_bps: u32,
    pub throttle_on_battery: bool,
}

/// Install manifest for a downloaded model.
#[derive(Clone)]
pub struct InstallManifest {
    pub model_id: u32,
    pub install_path_hash: u64,
    pub size_bytes: u64,
    pub checksum: u64,
    pub installed_at: u64,
    pub version: u32,
}

// ── Global state ───────────────────────────────────────────────────────────

static DOWNLOAD_MANAGER: Mutex<Option<DownloadManager>> = Mutex::new(None);

struct DownloadManager {
    tasks: Vec<DownloadTask>,
    installed: Vec<InstallManifest>,
    bandwidth: BandwidthConfig,
    next_id: u32,
    total_downloaded: u64,
    active_count: u8,
}

impl DownloadManager {
    fn new() -> Self {
        DownloadManager {
            tasks: Vec::new(),
            installed: Vec::new(),
            bandwidth: BandwidthConfig {
                max_concurrent: 3,
                max_bps: 0, // unlimited
                per_task_max_bps: 0,
                throttle_on_battery: true,
            },
            next_id: 1,
            total_downloaded: 0,
            active_count: 0,
        }
    }

    fn count_active(&self) -> u8 {
        self.tasks
            .iter()
            .filter(|t| t.status == DownloadStatus::Downloading)
            .count() as u8
    }

    fn promote_queued(&mut self) {
        let active = self.count_active();
        if active >= self.bandwidth.max_concurrent {
            return;
        }

        let slots = self.bandwidth.max_concurrent - active;
        let mut promoted = 0u8;

        // Sort queued tasks by priority (highest first)
        let mut queued_ids: Vec<(u32, DownloadPriority)> = self
            .tasks
            .iter()
            .filter(|t| t.status == DownloadStatus::Queued)
            .map(|t| (t.id, t.priority))
            .collect();

        queued_ids.sort_by(|a, b| b.1.cmp(&a.1));

        for (id, _) in queued_ids {
            if promoted >= slots {
                break;
            }
            if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
                task.status = DownloadStatus::Downloading;
                promoted += 1;
            }
        }

        self.active_count = self.count_active();
    }
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Initialize the download manager subsystem.
pub fn init() {
    let mut mgr = DOWNLOAD_MANAGER.lock();
    *mgr = Some(DownloadManager::new());
    serial_println!("    AI Market download manager initialized");
}

/// Start a new download for a model. Returns the task ID.
/// If the maximum number of concurrent downloads is reached, the task is queued.
pub fn start_download(
    model_id: u32,
    url_hash: u64,
    total_bytes: u64,
    checksum: u64,
    priority: DownloadPriority,
) -> u32 {
    let mut guard = DOWNLOAD_MANAGER.lock();
    let mgr = guard.as_mut().expect("download manager not initialized");

    let id = mgr.next_id;
    mgr.next_id = mgr.next_id.saturating_add(1);

    let active = mgr.count_active();
    let initial_status = if active < mgr.bandwidth.max_concurrent {
        DownloadStatus::Downloading
    } else {
        DownloadStatus::Queued
    };

    // Calculate chunk count (1 MB chunks)
    let chunk_size: u64 = 1024 * 1024;
    let chunks_total = if total_bytes == 0 {
        1
    } else {
        ((total_bytes + chunk_size - 1) / chunk_size) as u32
    };

    let task = DownloadTask {
        id,
        model_id,
        url_hash,
        total_bytes,
        downloaded_bytes: 0,
        status: initial_status,
        checksum,
        started: 0,
        speed_bps: 0,
        priority,
        retry_count: 0,
        max_retries: 3,
        error: None,
        chunks_complete: 0,
        chunks_total,
        last_activity: 0,
    };
    mgr.tasks.push(task);

    mgr.active_count = mgr.count_active();
    id
}

/// Pause an active download. Data already fetched is retained.
pub fn pause(task_id: u32) -> bool {
    let mut guard = DOWNLOAD_MANAGER.lock();
    let mgr = guard.as_mut().expect("download manager not initialized");

    if let Some(task) = mgr.tasks.iter_mut().find(|t| t.id == task_id) {
        if task.status == DownloadStatus::Downloading {
            task.status = DownloadStatus::Paused;
            task.speed_bps = 0;
            mgr.active_count = mgr.count_active();
            mgr.promote_queued();
            return true;
        }
    }
    false
}

/// Resume a paused download.
pub fn resume(task_id: u32) -> bool {
    let mut guard = DOWNLOAD_MANAGER.lock();
    let mgr = guard.as_mut().expect("download manager not initialized");

    let max_concurrent = mgr.bandwidth.max_concurrent;
    let active = mgr.count_active();
    if let Some(task) = mgr.tasks.iter_mut().find(|t| t.id == task_id) {
        if task.status == DownloadStatus::Paused {
            if active < max_concurrent {
                task.status = DownloadStatus::Downloading;
            } else {
                task.status = DownloadStatus::Queued;
            }
            return true;
        }
    }
    mgr.active_count = mgr.count_active();
    false
}

/// Cancel a download and discard any partial data.
pub fn cancel(task_id: u32) -> bool {
    let mut guard = DOWNLOAD_MANAGER.lock();
    let mgr = guard.as_mut().expect("download manager not initialized");

    let idx = mgr.tasks.iter().position(|t| t.id == task_id);
    if let Some(i) = idx {
        let was_active = mgr.tasks[i].status == DownloadStatus::Downloading;
        mgr.tasks.remove(i);
        if was_active {
            mgr.active_count = mgr.count_active();
            mgr.promote_queued();
        }
        true
    } else {
        false
    }
}

/// Verify the checksum of a completed download.
/// Uses a simple XOR-rotate hash for demonstration.
pub fn verify_checksum(task_id: u32) -> bool {
    let mut guard = DOWNLOAD_MANAGER.lock();
    let mgr = guard.as_mut().expect("download manager not initialized");

    if let Some(task) = mgr.tasks.iter_mut().find(|t| t.id == task_id) {
        if task.downloaded_bytes < task.total_bytes {
            return false;
        }

        task.status = DownloadStatus::Verifying;

        // Simulate checksum verification
        // In a real kernel, we would hash the downloaded file bytes
        let computed_hash = compute_download_hash(task.total_bytes, task.url_hash);
        let valid = computed_hash == task.checksum;

        if !valid {
            task.status = DownloadStatus::Error;
            task.error = Some(DownloadError::ChecksumMismatch);
        }

        valid
    } else {
        false
    }
}

/// Install a verified model into the local model store.
pub fn install_model(task_id: u32, install_path_hash: u64) -> bool {
    let mut guard = DOWNLOAD_MANAGER.lock();
    let mgr = guard.as_mut().expect("download manager not initialized");

    if let Some(task) = mgr.tasks.iter_mut().find(|t| t.id == task_id) {
        if task.status != DownloadStatus::Verifying && task.status != DownloadStatus::Downloading {
            // Must be verified or at least downloaded
            if task.downloaded_bytes < task.total_bytes {
                return false;
            }
        }

        task.status = DownloadStatus::Installing;

        // Create install manifest
        let manifest = InstallManifest {
            model_id: task.model_id,
            install_path_hash,
            size_bytes: task.total_bytes,
            checksum: task.checksum,
            installed_at: 0, // kernel timestamp
            version: 1,
        };

        task.status = DownloadStatus::Complete;
        mgr.installed.push(manifest);
        mgr.total_downloaded += task.total_bytes;

        // Promote any queued downloads
        mgr.active_count = mgr.count_active();
        mgr.promote_queued();

        true
    } else {
        false
    }
}

/// Get progress information for a specific download task.
pub fn get_progress(task_id: u32) -> Option<DownloadProgress> {
    let guard = DOWNLOAD_MANAGER.lock();
    let mgr = guard.as_ref().expect("download manager not initialized");

    mgr.tasks.iter().find(|t| t.id == task_id).map(|task| {
        let percent = if task.total_bytes == 0 {
            Q16_ONE
        } else {
            q16_div(
                q16_from_int(task.downloaded_bytes as i32),
                q16_from_int(task.total_bytes as i32),
            )
        };

        let eta_seconds = if task.speed_bps == 0 {
            0
        } else {
            let remaining = task.total_bytes.saturating_sub(task.downloaded_bytes);
            remaining / task.speed_bps as u64
        };

        DownloadProgress {
            task_id: task.id,
            model_id: task.model_id,
            percent,
            downloaded_bytes: task.downloaded_bytes,
            total_bytes: task.total_bytes,
            speed_bps: task.speed_bps,
            status: task.status,
            eta_seconds,
        }
    })
}

/// Get the current download speed for a task (bytes per second).
pub fn get_speed(task_id: u32) -> u32 {
    let guard = DOWNLOAD_MANAGER.lock();
    let mgr = guard.as_ref().expect("download manager not initialized");

    mgr.tasks
        .iter()
        .find(|t| t.id == task_id)
        .map(|t| t.speed_bps)
        .unwrap_or(0)
}

/// Clean up partial downloads that are in an error state.
/// Returns the number of tasks cleaned up.
pub fn cleanup_partial() -> u32 {
    let mut guard = DOWNLOAD_MANAGER.lock();
    let mgr = guard.as_mut().expect("download manager not initialized");

    let before = mgr.tasks.len();
    mgr.tasks.retain(|t| t.status != DownloadStatus::Error);
    let removed = before - mgr.tasks.len();

    mgr.active_count = mgr.count_active();
    removed as u32
}

/// Simulate receiving bytes for a download task (used by the network layer).
pub fn receive_bytes(task_id: u32, bytes: u64, current_speed: u32) -> bool {
    let mut guard = DOWNLOAD_MANAGER.lock();
    let mgr = guard.as_mut().expect("download manager not initialized");

    if let Some(task) = mgr.tasks.iter_mut().find(|t| t.id == task_id) {
        if task.status != DownloadStatus::Downloading {
            return false;
        }

        task.downloaded_bytes = (task.downloaded_bytes + bytes).min(task.total_bytes);
        task.speed_bps = current_speed;
        task.last_activity = task.last_activity.saturating_add(1);

        // Update chunk progress
        let chunk_size: u64 = 1024 * 1024;
        if chunk_size > 0 {
            task.chunks_complete = (task.downloaded_bytes / chunk_size) as u32;
        }

        true
    } else {
        false
    }
}

/// Set bandwidth configuration.
pub fn set_bandwidth(config: BandwidthConfig) {
    let mut guard = DOWNLOAD_MANAGER.lock();
    let mgr = guard.as_mut().expect("download manager not initialized");
    mgr.bandwidth = config;
}

/// Get all active and queued download tasks.
pub fn get_all_tasks() -> Vec<DownloadTask> {
    let guard = DOWNLOAD_MANAGER.lock();
    let mgr = guard.as_ref().expect("download manager not initialized");
    mgr.tasks.clone()
}

/// Get list of all installed models.
pub fn get_installed() -> Vec<InstallManifest> {
    let guard = DOWNLOAD_MANAGER.lock();
    let mgr = guard.as_ref().expect("download manager not initialized");
    mgr.installed.clone()
}

/// Check if a model is already installed.
pub fn is_installed(model_id: u32) -> bool {
    let guard = DOWNLOAD_MANAGER.lock();
    let mgr = guard.as_ref().expect("download manager not initialized");
    mgr.installed.iter().any(|m| m.model_id == model_id)
}

/// Get total bytes downloaded across all completed tasks.
pub fn total_downloaded() -> u64 {
    let guard = DOWNLOAD_MANAGER.lock();
    let mgr = guard.as_ref().expect("download manager not initialized");
    mgr.total_downloaded
}

/// Retry a failed download task.
pub fn retry(task_id: u32) -> bool {
    let mut guard = DOWNLOAD_MANAGER.lock();
    let mgr = guard.as_mut().expect("download manager not initialized");

    let max_concurrent = mgr.bandwidth.max_concurrent;
    let active = mgr.count_active();
    let mut found = false;
    for task in mgr.tasks.iter_mut() {
        if task.id == task_id {
            if task.status != DownloadStatus::Error {
                return false;
            }
            if task.retry_count >= task.max_retries {
                return false;
            }
            task.retry_count = task.retry_count.saturating_add(1);
            task.error = None;
            task.downloaded_bytes = 0;
            task.chunks_complete = 0;
            task.speed_bps = 0;
            if active < max_concurrent {
                task.status = DownloadStatus::Downloading;
            } else {
                task.status = DownloadStatus::Queued;
            }
            found = true;
            break;
        }
    }
    if !found {
        return false;
    }
    mgr.active_count = mgr.count_active();
    true
}

// ── Internal helpers ───────────────────────────────────────────────────────

/// Compute a simple hash from download metadata for checksum verification.
fn compute_download_hash(size: u64, url_hash: u64) -> u64 {
    let mut h = size ^ url_hash;
    h = h.wrapping_mul(0x517CC1B727220A95);
    h ^= h >> 32;
    h = h.wrapping_mul(0x6C62272E07BB0142);
    h ^= h >> 28;
    h
}
