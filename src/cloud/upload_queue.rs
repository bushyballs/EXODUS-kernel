/// upload_queue.rs — queue-based cloud upload manager for Genesis.
///
/// Provides:
/// - A static cloud endpoint configuration (URL, API key, enabled flag).
/// - A bounded static upload queue of 32 path entries.
/// - `enqueue_upload(path)` to add files to the queue.
/// - `process_upload_queue()` to drain the queue (stubs actual I/O via serial).
/// - `sync_file(local_path, remote_path)` — single-file synchronisation stub.
///
/// All network I/O is simulated with `serial_println!`.  Wire real calls
/// into the stub bodies once the network stack is available.
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of pending uploads.
pub const UPLOAD_QUEUE_SIZE: usize = 32;

/// Maximum length of a path string stored in the queue (bytes).
pub const PATH_LEN: usize = 128;

/// Maximum length of the endpoint URL (bytes).
pub const ENDPOINT_LEN: usize = 128;

/// Maximum length of the API key (bytes).
pub const API_KEY_LEN: usize = 64;

// ---------------------------------------------------------------------------
// CloudConfig
// ---------------------------------------------------------------------------

/// Cloud endpoint configuration.
pub struct CloudConfig {
    /// Base URL of the cloud storage endpoint (e.g. `https://api.example.com/v1`).
    pub endpoint: [u8; ENDPOINT_LEN],
    pub endpoint_len: usize,

    /// Bearer / API key for authentication.
    pub api_key: [u8; API_KEY_LEN],
    pub api_key_len: usize,

    /// Whether cloud sync is enabled at all.
    pub enabled: bool,
}

impl CloudConfig {
    pub const fn default() -> Self {
        Self {
            endpoint: [0u8; ENDPOINT_LEN],
            endpoint_len: 0,
            api_key: [0u8; API_KEY_LEN],
            api_key_len: 0,
            enabled: false,
        }
    }

    /// Set the endpoint URL from a byte slice (truncated to `ENDPOINT_LEN`).
    pub fn set_endpoint(&mut self, url: &[u8]) {
        let len = url.len().min(ENDPOINT_LEN);
        self.endpoint[..len].copy_from_slice(&url[..len]);
        self.endpoint_len = len;
    }

    /// Set the API key from a byte slice (truncated to `API_KEY_LEN`).
    pub fn set_api_key(&mut self, key: &[u8]) {
        let len = key.len().min(API_KEY_LEN);
        self.api_key[..len].copy_from_slice(&key[..len]);
        self.api_key_len = len;
    }

    /// Return the endpoint URL as a `&str`.
    pub fn endpoint_str(&self) -> &str {
        core::str::from_utf8(&self.endpoint[..self.endpoint_len]).unwrap_or("")
    }
}

// ---------------------------------------------------------------------------
// UploadEntry
// ---------------------------------------------------------------------------

/// One pending upload in the queue.
#[derive(Clone, Copy)]
pub struct UploadEntry {
    /// Local filesystem path to upload.
    pub path: [u8; PATH_LEN],
    pub path_len: usize,

    /// Whether this slot is occupied.
    pub valid: bool,

    /// Number of retry attempts so far.
    pub retries: u8,
}

impl UploadEntry {
    pub const fn empty() -> Self {
        Self {
            path: [0u8; PATH_LEN],
            path_len: 0,
            valid: false,
            retries: 0,
        }
    }

    /// Return the path as a `&str`.
    pub fn path_str(&self) -> &str {
        core::str::from_utf8(&self.path[..self.path_len]).unwrap_or("")
    }
}

// ---------------------------------------------------------------------------
// Static state
// ---------------------------------------------------------------------------

static mut CONFIG: CloudConfig = CloudConfig::default();

static mut QUEUE: [UploadEntry; UPLOAD_QUEUE_SIZE] = [UploadEntry::empty(); UPLOAD_QUEUE_SIZE];

static mut QUEUE_LEN: usize = 0;

// ---------------------------------------------------------------------------
// Public API — configuration
// ---------------------------------------------------------------------------

/// Configure the cloud endpoint.
pub fn set_endpoint(url: &str) {
    unsafe {
        CONFIG.set_endpoint(url.as_bytes());
        serial_println!("[cloud/upload] endpoint set: {}", url);
    }
}

/// Configure the API key.
pub fn set_api_key(key: &str) {
    unsafe {
        CONFIG.set_api_key(key.as_bytes());
        serial_println!("[cloud/upload] API key configured ({} bytes)", key.len());
    }
}

/// Enable or disable cloud sync.
pub fn set_enabled(enabled: bool) {
    unsafe {
        CONFIG.enabled = enabled;
        serial_println!("[cloud/upload] cloud sync enabled={}", enabled);
    }
}

/// Return whether cloud sync is currently enabled.
pub fn is_enabled() -> bool {
    unsafe { CONFIG.enabled }
}

// ---------------------------------------------------------------------------
// Public API — sync_file
// ---------------------------------------------------------------------------

/// Synchronise a single file from `local_path` to `remote_path`.
///
/// This is a stub — it logs the operation via serial and returns `true`.
/// Real implementation would open the file, chunk it, and POST to the endpoint.
pub fn sync_file(local_path: &str, remote_path: &str) -> bool {
    unsafe {
        if !CONFIG.enabled {
            serial_println!("[cloud/upload] sync_file skipped (cloud disabled)");
            return false;
        }
    }
    serial_println!(
        "[cloud/upload] sync_file '{}' -> '{}'  [stub: OK]",
        local_path,
        remote_path
    );
    true
}

// ---------------------------------------------------------------------------
// Public API — upload queue
// ---------------------------------------------------------------------------

/// Add `path` to the upload queue.
///
/// Returns `true` on success, `false` if the path is too long or the queue
/// is full.
pub fn enqueue_upload(path: &str) -> bool {
    if path.len() > PATH_LEN {
        serial_println!(
            "[cloud/upload] enqueue_upload: path too long ({})",
            path.len()
        );
        return false;
    }
    unsafe {
        for slot in QUEUE.iter_mut() {
            if !slot.valid {
                let plen = path.len();
                slot.path[..plen].copy_from_slice(path.as_bytes());
                slot.path_len = plen;
                slot.valid = true;
                slot.retries = 0;
                QUEUE_LEN = QUEUE_LEN.saturating_add(1);
                serial_println!(
                    "[cloud/upload] enqueued '{}' (queue_len={})",
                    path,
                    QUEUE_LEN
                );
                return true;
            }
        }
    }
    serial_println!("[cloud/upload] enqueue_upload: queue full");
    false
}

/// Drain the upload queue, "uploading" each file in FIFO order.
///
/// Each file is processed via `sync_file` (stub).  Successfully processed
/// entries are removed from the queue.  Returns the number of files processed.
pub fn process_upload_queue() -> usize {
    let mut processed = 0usize;
    unsafe {
        if !CONFIG.enabled {
            serial_println!("[cloud/upload] process_upload_queue: cloud disabled, skipping");
            return 0;
        }
        for slot in QUEUE.iter_mut() {
            if slot.valid {
                let path = core::str::from_utf8(&slot.path[..slot.path_len]).unwrap_or("");
                let ok = sync_file(path, path); // remote path mirrors local for stub
                if ok {
                    serial_println!(
                        "[cloud/upload] uploaded '{}' (retry={})",
                        path,
                        slot.retries
                    );
                    *slot = UploadEntry::empty();
                    QUEUE_LEN = QUEUE_LEN.saturating_sub(1);
                    processed += 1;
                } else {
                    slot.retries = slot.retries.saturating_add(1);
                    serial_println!(
                        "[cloud/upload] upload failed '{}' retry={}",
                        path,
                        slot.retries
                    );
                }
            }
        }
    }
    serial_println!(
        "[cloud/upload] process_upload_queue: processed={}",
        processed
    );
    processed
}

/// Return the current number of pending uploads.
pub fn queue_length() -> usize {
    unsafe { QUEUE_LEN }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!(
        "[cloud/upload] upload queue ready (slots={})",
        UPLOAD_QUEUE_SIZE
    );
}
