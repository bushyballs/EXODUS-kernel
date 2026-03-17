use crate::sync::Mutex;
/// System clipboard manager
///
/// Part of the AIOS UI layer.
use alloc::string::String;
use alloc::vec::Vec;

/// Clipboard content type
#[derive(Debug, Clone)]
pub enum ClipContent {
    Text(String),
    Image(Vec<u8>),
    Files(Vec<String>),
}

/// Manages the system clipboard with history
pub struct ClipboardManager {
    pub current: Option<ClipContent>,
    pub history: Vec<ClipContent>,
    pub max_history: usize,
}

impl ClipboardManager {
    pub fn new(max_history: usize) -> Self {
        ClipboardManager {
            current: None,
            history: Vec::new(),
            max_history,
        }
    }

    /// Copy content to the clipboard.
    ///
    /// The previous content (if any) is pushed into the history ring.
    pub fn copy(&mut self, content: ClipContent) {
        // Move current content to history
        if let Some(prev) = self.current.take() {
            self.history.push(prev);
            // Trim history to max size
            while self.history.len() > self.max_history {
                self.history.remove(0);
            }
        }

        let desc = match &content {
            ClipContent::Text(t) => {
                let preview_len = if t.len() > 32 { 32 } else { t.len() };
                let mut s = String::from("text(");
                s.push_str(&t[..preview_len]);
                if t.len() > 32 {
                    s.push_str("...");
                }
                s.push(')');
                s
            }
            ClipContent::Image(data) => {
                let mut s = String::from("image(");
                // Simple length description
                let len = data.len();
                // Manual integer to string for no_std
                let mut buf = [0u8; 20];
                let mut n = len;
                let mut i = 0;
                if n == 0 {
                    buf[0] = b'0';
                    i = 1;
                } else {
                    while n > 0 {
                        buf[i] = b'0' + (n % 10) as u8;
                        n /= 10;
                        i += 1;
                    }
                    // Reverse
                    buf[..i].reverse();
                }
                if let Ok(num_str) = core::str::from_utf8(&buf[..i]) {
                    s.push_str(num_str);
                }
                s.push_str(" bytes)");
                s
            }
            ClipContent::Files(paths) => {
                let mut s = String::from("files(");
                let mut buf = [0u8; 20];
                let mut n = paths.len();
                let mut i = 0;
                if n == 0 {
                    buf[0] = b'0';
                    i = 1;
                } else {
                    while n > 0 {
                        buf[i] = b'0' + (n % 10) as u8;
                        n /= 10;
                        i += 1;
                    }
                    buf[..i].reverse();
                }
                if let Ok(num_str) = core::str::from_utf8(&buf[..i]) {
                    s.push_str(num_str);
                }
                s.push(')');
                s
            }
        };

        crate::serial_println!("  [clipboard] copy: {}", desc);
        self.current = Some(content);
    }

    /// Paste (read) the current clipboard content.
    pub fn paste(&self) -> Option<&ClipContent> {
        self.current.as_ref()
    }

    /// Get a reference to a history item by index (0 = most recent).
    pub fn history_item(&self, index: usize) -> Option<&ClipContent> {
        if index < self.history.len() {
            // History is stored oldest-first; reverse index for most-recent-first
            let rev = self.history.len() - 1 - index;
            self.history.get(rev)
        } else {
            None
        }
    }

    /// Number of items in the clipboard history.
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Clear the clipboard and all history.
    pub fn clear(&mut self) {
        self.current = None;
        self.history.clear();
        crate::serial_println!("  [clipboard] cleared");
    }
}

static CLIPBOARD: Mutex<Option<ClipboardManager>> = Mutex::new(None);

/// Default history depth
const DEFAULT_MAX_HISTORY: usize = 25;

pub fn init() {
    *CLIPBOARD.lock() = Some(ClipboardManager::new(DEFAULT_MAX_HISTORY));
    crate::serial_println!(
        "  [clipboard] Clipboard manager initialized (history={})",
        DEFAULT_MAX_HISTORY
    );
}

/// Copy content to the global clipboard.
pub fn copy(content: ClipContent) {
    if let Some(ref mut mgr) = *CLIPBOARD.lock() {
        mgr.copy(content);
    }
}

/// Paste from the global clipboard.
pub fn paste() -> Option<ClipContent> {
    match CLIPBOARD.lock().as_ref() {
        Some(mgr) => mgr.paste().cloned(),
        None => None,
    }
}
