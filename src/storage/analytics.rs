/// Storage analytics for Genesis
///
/// Disk usage analysis, app size tracking, cleanup suggestions,
/// I/O performance metrics, and storage trends.
///
/// Inspired by: Android Storage Settings, DiskUsage. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Storage category
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StorageCategory {
    System,
    Apps,
    Images,
    Videos,
    Audio,
    Documents,
    Downloads,
    Cache,
    Trash,
    Other,
}

/// Usage entry per category
pub struct CategoryUsage {
    pub category: StorageCategory,
    pub bytes: u64,
    pub file_count: u64,
}

/// Per-app storage usage
pub struct AppUsage {
    pub app_id: String,
    pub app_size: u64,
    pub data_size: u64,
    pub cache_size: u64,
    pub total: u64,
}

/// I/O performance sample
pub struct IoSample {
    pub timestamp: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub read_ops: u32,
    pub write_ops: u32,
    pub latency_us: u32,
}

/// Storage analytics
pub struct StorageAnalytics {
    pub category_usage: Vec<CategoryUsage>,
    pub app_usage: Vec<AppUsage>,
    pub io_samples: Vec<IoSample>,
    pub max_io_samples: usize,
    pub total_reads: u64,
    pub total_writes: u64,
    pub total_read_bytes: u64,
    pub total_write_bytes: u64,
}

impl StorageAnalytics {
    const fn new() -> Self {
        StorageAnalytics {
            category_usage: Vec::new(),
            app_usage: Vec::new(),
            io_samples: Vec::new(),
            max_io_samples: 1000,
            total_reads: 0,
            total_writes: 0,
            total_read_bytes: 0,
            total_write_bytes: 0,
        }
    }

    pub fn record_io(&mut self, read_bytes: u64, write_bytes: u64, read_ops: u32, write_ops: u32) {
        self.total_reads += read_ops as u64;
        self.total_writes += write_ops as u64;
        self.total_read_bytes += read_bytes;
        self.total_write_bytes += write_bytes;

        if self.io_samples.len() >= self.max_io_samples {
            self.io_samples.remove(0);
        }
        self.io_samples.push(IoSample {
            timestamp: crate::time::clock::unix_time(),
            read_bytes,
            write_bytes,
            read_ops,
            write_ops,
            latency_us: 0,
        });
    }

    pub fn update_category(&mut self, category: StorageCategory, bytes: u64, files: u64) {
        if let Some(entry) = self
            .category_usage
            .iter_mut()
            .find(|c| c.category == category)
        {
            entry.bytes = bytes;
            entry.file_count = files;
        } else {
            self.category_usage.push(CategoryUsage {
                category,
                bytes,
                file_count: files,
            });
        }
    }

    pub fn update_app_usage(
        &mut self,
        app_id: &str,
        app_size: u64,
        data_size: u64,
        cache_size: u64,
    ) {
        let total = app_size + data_size + cache_size;
        if let Some(entry) = self.app_usage.iter_mut().find(|a| a.app_id == app_id) {
            entry.app_size = app_size;
            entry.data_size = data_size;
            entry.cache_size = cache_size;
            entry.total = total;
        } else {
            self.app_usage.push(AppUsage {
                app_id: String::from(app_id),
                app_size,
                data_size,
                cache_size,
                total,
            });
        }
    }

    pub fn total_used(&self) -> u64 {
        self.category_usage.iter().map(|c| c.bytes).sum()
    }

    pub fn largest_apps(&self, count: usize) -> Vec<&AppUsage> {
        let mut sorted: Vec<&AppUsage> = self.app_usage.iter().collect();
        sorted.sort_by(|a, b| b.total.cmp(&a.total));
        sorted.truncate(count);
        sorted
    }

    pub fn clearable_cache(&self) -> u64 {
        self.app_usage.iter().map(|a| a.cache_size).sum()
    }

    pub fn io_throughput(&self) -> (u64, u64) {
        // Average reads/writes per second from recent samples
        if self.io_samples.len() < 2 {
            return (0, 0);
        }
        let first = &self.io_samples[0];
        let last = match self.io_samples.last() {
            Some(s) => s,
            None => return (0, 0),
        };
        let duration = last.timestamp.saturating_sub(first.timestamp);
        if duration == 0 {
            return (0, 0);
        }
        let total_r: u64 = self.io_samples.iter().map(|s| s.read_bytes).sum();
        let total_w: u64 = self.io_samples.iter().map(|s| s.write_bytes).sum();
        (total_r / duration, total_w / duration)
    }
}

static ANALYTICS: Mutex<StorageAnalytics> = Mutex::new(StorageAnalytics::new());

pub fn init() {
    crate::serial_println!("  [storage] Storage analytics initialized");
}

pub fn record_io(read_bytes: u64, write_bytes: u64, read_ops: u32, write_ops: u32) {
    ANALYTICS
        .lock()
        .record_io(read_bytes, write_bytes, read_ops, write_ops);
}
