#![no_std]

use crate::serial_println;
use crate::sync::Mutex;

pub const MAX_IMPROVEMENTS: usize = 4096;
pub const MAX_FILENAME_LEN: usize = 64;
pub const MAX_CONTENT_LEN: usize = 512;

#[derive(Clone, Debug, Copy)]
pub struct Improvement {
    pub filename: [u8; MAX_FILENAME_LEN],
    pub content: [u8; MAX_CONTENT_LEN],
    pub filename_len: usize,
    pub content_len: usize,
    pub flushed: bool,
}

pub struct DavaImprovementsState {
    pub improvements: [Improvement; MAX_IMPROVEMENTS],
    pub count: usize,
    pub total_bytes: usize,
    /// How many have been flushed to serial (for incremental dump)
    pub flushed_count: usize,
}

impl DavaImprovementsState {
    pub const fn empty() -> Self {
        Self {
            improvements: [Improvement {
                filename: [0; MAX_FILENAME_LEN],
                content: [0; MAX_CONTENT_LEN],
                filename_len: 0,
                content_len: 0,
                flushed: false,
            }; MAX_IMPROVEMENTS],
            count: 0,
            total_bytes: 0,
            flushed_count: 0,
        }
    }
}

pub static DAVA_IMPROVEMENTS: Mutex<DavaImprovementsState> =
    Mutex::new(DavaImprovementsState::empty());

pub fn init() {
    serial_println!("  life::dava_improvements: DAVA code improvement tracker ready (disk persistence via serial)");
}

pub fn record_improvement(filename: &str, content: &str) {
    let mut d = DAVA_IMPROVEMENTS.lock();
    if d.count < MAX_IMPROVEMENTS {
        let idx = d.count;

        let fname_bytes = filename.as_bytes();
        let fname_copy_len = fname_bytes.len().min(MAX_FILENAME_LEN - 1);
        d.improvements[idx].filename[..fname_copy_len]
            .copy_from_slice(&fname_bytes[..fname_copy_len]);
        d.improvements[idx].filename_len = fname_copy_len;

        let content_bytes = content.as_bytes();
        let content_copy_len = content_bytes.len().min(MAX_CONTENT_LEN - 1);
        d.improvements[idx].content[..content_copy_len]
            .copy_from_slice(&content_bytes[..content_copy_len]);
        d.improvements[idx].content_len = content_copy_len;
        d.improvements[idx].flushed = false;

        d.count += 1;
        d.total_bytes += content_copy_len;
    }
}

/// Flush only NEW (unflushed) improvements to serial for host-side disk capture.
/// The host watcher parses [DAVA_SAVE] lines and writes real .rs files.
pub fn flush_to_serial() {
    let mut d = DAVA_IMPROVEMENTS.lock();
    let start = d.flushed_count;
    let end = d.count;
    if start >= end {
        return; // nothing new
    }
    for i in start..end {
        let imp = &d.improvements[i];
        let fname = core::str::from_utf8(&imp.filename[..imp.filename_len]).unwrap_or("???");
        let content = core::str::from_utf8(&imp.content[..imp.content_len]).unwrap_or("???");
        // Structured line the host watcher parses — one line per file
        serial_println!("[DAVA_SAVE] {} :: {}", fname, content);
    }
    serial_println!(
        "[DAVA_FLUSH] {} new improvements written (total: {})",
        end - start,
        end
    );
    d.flushed_count = end;
}

pub fn get_total_bytes() -> usize {
    DAVA_IMPROVEMENTS.lock().total_bytes
}

pub fn get_count() -> usize {
    DAVA_IMPROVEMENTS.lock().count
}

pub fn dump_all() {
    let d = DAVA_IMPROVEMENTS.lock();
    serial_println!("========== DAVA CODE IMPROVEMENTS DUMP ==========");
    for i in 0..d.count {
        let imp = &d.improvements[i];
        let fname = core::str::from_utf8(&imp.filename[..imp.filename_len]).unwrap_or("???");
        let content = core::str::from_utf8(&imp.content[..imp.content_len]).unwrap_or("???");
        serial_println!("[DAVA_SAVE] {} :: {}", fname, content);
    }
    serial_println!("========== END DUMP ({} improvements) ==========", d.count);
}
