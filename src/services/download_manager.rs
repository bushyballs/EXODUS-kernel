/// Download manager for Genesis
///
/// Background downloads, pause/resume, progress tracking,
/// multi-part downloads, and download queue management.
///
/// Inspired by: Android DownloadManager, iOS NSURLSession. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Download state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadState {
    Queued,
    Running,
    Paused,
    Complete,
    Failed,
    Cancelled,
}

/// Download priority
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadPriority {
    Low,
    Normal,
    High,
}

/// A download request
pub struct Download {
    pub id: u32,
    pub url: String,
    pub destination: String,
    pub file_name: String,
    pub state: DownloadState,
    pub priority: DownloadPriority,
    pub total_bytes: u64,
    pub downloaded_bytes: u64,
    pub speed_bps: u64,
    pub mime_type: String,
    pub created_at: u64,
    pub allow_metered: bool,
    pub allow_roaming: bool,
    pub require_charging: bool,
    pub retry_count: u32,
    pub max_retries: u32,
}

impl Download {
    pub fn progress_percent(&self) -> u8 {
        if self.total_bytes == 0 {
            return 0;
        }
        ((self.downloaded_bytes * 100) / self.total_bytes) as u8
    }

    pub fn eta_seconds(&self) -> Option<u64> {
        if self.speed_bps == 0 {
            return None;
        }
        let remaining = self.total_bytes.saturating_sub(self.downloaded_bytes);
        Some(remaining / self.speed_bps)
    }
}

/// Download manager
pub struct DownloadManager {
    pub downloads: Vec<Download>,
    pub next_id: u32,
    pub max_concurrent: usize,
    pub active_count: usize,
    pub total_downloaded: u64,
}

impl DownloadManager {
    const fn new() -> Self {
        DownloadManager {
            downloads: Vec::new(),
            next_id: 1,
            max_concurrent: 3,
            active_count: 0,
            total_downloaded: 0,
        }
    }

    pub fn enqueue(&mut self, url: &str, dest: &str, file_name: &str) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.downloads.push(Download {
            id,
            url: String::from(url),
            destination: String::from(dest),
            file_name: String::from(file_name),
            state: DownloadState::Queued,
            priority: DownloadPriority::Normal,
            total_bytes: 0,
            downloaded_bytes: 0,
            speed_bps: 0,
            mime_type: String::new(),
            created_at: crate::time::clock::unix_time(),
            allow_metered: true,
            allow_roaming: false,
            require_charging: false,
            retry_count: 0,
            max_retries: 3,
        });
        id
    }

    pub fn pause(&mut self, id: u32) -> bool {
        if let Some(dl) = self.downloads.iter_mut().find(|d| d.id == id) {
            if dl.state == DownloadState::Running {
                dl.state = DownloadState::Paused;
                if self.active_count > 0 {
                    self.active_count -= 1;
                }
                return true;
            }
        }
        false
    }

    pub fn resume(&mut self, id: u32) -> bool {
        if let Some(dl) = self.downloads.iter_mut().find(|d| d.id == id) {
            if dl.state == DownloadState::Paused {
                dl.state = DownloadState::Queued;
                return true;
            }
        }
        false
    }

    pub fn cancel(&mut self, id: u32) -> bool {
        if let Some(dl) = self.downloads.iter_mut().find(|d| d.id == id) {
            dl.state = DownloadState::Cancelled;
            if self.active_count > 0 {
                self.active_count -= 1;
            }
            true
        } else {
            false
        }
    }

    pub fn update_progress(&mut self, id: u32, bytes: u64, total: u64, speed: u64) {
        if let Some(dl) = self.downloads.iter_mut().find(|d| d.id == id) {
            dl.downloaded_bytes = bytes;
            dl.total_bytes = total;
            dl.speed_bps = speed;
            if bytes >= total && total > 0 {
                dl.state = DownloadState::Complete;
                self.total_downloaded += total;
                if self.active_count > 0 {
                    self.active_count -= 1;
                }
            }
        }
    }

    pub fn tick(&mut self) {
        // Start queued downloads up to max concurrent
        for dl in &mut self.downloads {
            if self.active_count >= self.max_concurrent {
                break;
            }
            if dl.state == DownloadState::Queued {
                dl.state = DownloadState::Running;
                self.active_count = self.active_count.saturating_add(1);
            }
        }
    }

    pub fn active_downloads(&self) -> Vec<&Download> {
        self.downloads
            .iter()
            .filter(|d| d.state == DownloadState::Running)
            .collect()
    }
}

static DOWNLOADS: Mutex<DownloadManager> = Mutex::new(DownloadManager::new());

pub fn init() {
    crate::serial_println!("  [services] Download manager initialized");
}

pub fn tick() {
    DOWNLOADS.lock().tick();
}
