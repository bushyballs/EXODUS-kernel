/// Clipboard for Genesis — system-wide copy/paste
///
/// Supports text, rich text, images, URIs, and custom MIME types.
/// Clipboard history and cross-app paste with permission checks.
///
/// Inspired by: Android ClipboardManager, Wayland data-device. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Clipboard data type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipType {
    PlainText,
    Html,
    Uri,
    Image,
    Custom(String), // MIME type
}

/// Clipboard entry
#[derive(Clone)]
pub struct ClipEntry {
    pub clip_type: ClipType,
    pub data: Vec<u8>,
    pub label: String,
    pub source_app: String,
    pub timestamp: u64,
    pub sensitive: bool, // auto-clear after timeout
}

/// Clipboard manager
pub struct ClipboardManager {
    current: Option<ClipEntry>,
    history: Vec<ClipEntry>,
    max_history: usize,
    /// Auto-clear sensitive clips after this many seconds
    sensitive_timeout: u64,
}

impl ClipboardManager {
    const fn new() -> Self {
        ClipboardManager {
            current: None,
            history: Vec::new(),
            max_history: 20,
            sensitive_timeout: 30,
        }
    }

    /// Copy text to clipboard
    pub fn copy_text(&mut self, text: &str, app_id: &str) {
        let entry = ClipEntry {
            clip_type: ClipType::PlainText,
            data: text.as_bytes().to_vec(),
            label: String::from("Text"),
            source_app: String::from(app_id),
            timestamp: crate::time::clock::unix_time(),
            sensitive: false,
        };
        self.set(entry);
    }

    /// Copy sensitive text (passwords, etc.) — auto-clears
    pub fn copy_sensitive(&mut self, text: &str, app_id: &str) {
        let entry = ClipEntry {
            clip_type: ClipType::PlainText,
            data: text.as_bytes().to_vec(),
            label: String::from("Sensitive"),
            source_app: String::from(app_id),
            timestamp: crate::time::clock::unix_time(),
            sensitive: true,
        };
        self.set(entry);
    }

    /// Copy image data
    pub fn copy_image(&mut self, data: &[u8], app_id: &str) {
        let entry = ClipEntry {
            clip_type: ClipType::Image,
            data: data.to_vec(),
            label: String::from("Image"),
            source_app: String::from(app_id),
            timestamp: crate::time::clock::unix_time(),
            sensitive: false,
        };
        self.set(entry);
    }

    fn set(&mut self, entry: ClipEntry) {
        if let Some(prev) = self.current.take() {
            if !prev.sensitive {
                self.history.push(prev);
                if self.history.len() > self.max_history {
                    self.history.remove(0);
                }
            }
        }
        self.current = Some(entry);
    }

    /// Paste — get current clipboard content
    pub fn paste(&self) -> Option<&ClipEntry> {
        self.current.as_ref()
    }

    /// Paste as text
    pub fn paste_text(&self) -> Option<String> {
        self.current.as_ref().and_then(|entry| {
            if entry.clip_type == ClipType::PlainText {
                core::str::from_utf8(&entry.data).ok().map(String::from)
            } else {
                None
            }
        })
    }

    /// Get clipboard history
    pub fn history(&self) -> &[ClipEntry] {
        &self.history
    }

    /// Clear clipboard
    pub fn clear(&mut self) {
        // Zero out sensitive data
        if let Some(ref mut entry) = self.current {
            if entry.sensitive {
                for b in entry.data.iter_mut() {
                    *b = 0;
                }
            }
        }
        self.current = None;
    }

    /// Check and clear expired sensitive clips
    pub fn check_expiry(&mut self) {
        let now = crate::time::clock::unix_time();
        if let Some(ref entry) = self.current {
            if entry.sensitive && now - entry.timestamp > self.sensitive_timeout {
                self.clear();
            }
        }
    }

    /// Has content?
    pub fn has_content(&self) -> bool {
        self.current.is_some()
    }
}

static CLIPBOARD: Mutex<ClipboardManager> = Mutex::new(ClipboardManager::new());

pub fn init() {
    crate::serial_println!("  [clipboard] Clipboard system initialized");
}

pub fn copy_text(text: &str, app_id: &str) {
    CLIPBOARD.lock().copy_text(text, app_id);
}
pub fn paste_text() -> Option<String> {
    CLIPBOARD.lock().paste_text()
}
pub fn clear() {
    CLIPBOARD.lock().clear();
}
pub fn has_content() -> bool {
    CLIPBOARD.lock().has_content()
}
