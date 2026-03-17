use crate::sync::Mutex;
/// Clipboard manager for Genesis
///
/// Provides system-wide copy/paste between windows and applications.
/// Supports text and binary data. Multiple clipboard formats can
/// coexist (text, rich text, image).
///
/// Inspired by: X11 CLIPBOARD/PRIMARY selections, Wayland clipboard,
/// Windows clipboard, macOS pasteboard. All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

/// Clipboard data format
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipFormat {
    /// Plain text (UTF-8)
    Text,
    /// Rich text (with ANSI formatting)
    RichText,
    /// Raw binary data
    Binary,
    /// Image data (width, height, ARGB pixels)
    Image,
    /// File path(s)
    FilePaths,
}

/// A single clipboard entry
#[derive(Debug, Clone)]
pub struct ClipEntry {
    pub format: ClipFormat,
    pub data: Vec<u8>,
    /// Source window ID
    pub source_window: u32,
    /// Timestamp (tick count)
    pub timestamp: u64,
}

/// Clipboard state
pub struct Clipboard {
    /// Primary clipboard (Ctrl+C / Ctrl+V)
    pub primary: Option<ClipEntry>,
    /// Selection clipboard (X11-style: auto-copy on mouse select)
    pub selection: Option<ClipEntry>,
    /// Clipboard history (last N entries)
    pub history: Vec<ClipEntry>,
    /// Maximum history entries
    pub max_history: usize,
}

impl Clipboard {
    pub const fn new() -> Self {
        Clipboard {
            primary: None,
            selection: None,
            history: Vec::new(),
            max_history: 50,
        }
    }

    /// Copy text to the primary clipboard
    pub fn copy_text(&mut self, text: &str, source_window: u32) {
        let entry = ClipEntry {
            format: ClipFormat::Text,
            data: text.as_bytes().to_vec(),
            source_window,
            timestamp: crate::time::clock::uptime_secs(),
        };

        // Add to history
        self.history.push(entry.clone());
        while self.history.len() > self.max_history {
            self.history.remove(0);
        }

        self.primary = Some(entry);
    }

    /// Paste text from the primary clipboard
    pub fn paste_text(&self) -> Option<String> {
        self.primary.as_ref().and_then(|entry| {
            if entry.format == ClipFormat::Text {
                String::from_utf8(entry.data.clone()).ok()
            } else {
                None
            }
        })
    }

    /// Copy binary data
    pub fn copy_binary(&mut self, data: &[u8], format: ClipFormat, source_window: u32) {
        let entry = ClipEntry {
            format,
            data: data.to_vec(),
            source_window,
            timestamp: crate::time::clock::uptime_secs(),
        };

        self.history.push(entry.clone());
        while self.history.len() > self.max_history {
            self.history.remove(0);
        }

        self.primary = Some(entry);
    }

    /// Set the selection clipboard (mouse selection)
    pub fn set_selection(&mut self, text: &str) {
        self.selection = Some(ClipEntry {
            format: ClipFormat::Text,
            data: text.as_bytes().to_vec(),
            source_window: 0,
            timestamp: crate::time::clock::uptime_secs(),
        });
    }

    /// Get the selection text
    pub fn get_selection(&self) -> Option<String> {
        self.selection
            .as_ref()
            .and_then(|entry| String::from_utf8(entry.data.clone()).ok())
    }

    /// Get clipboard history
    pub fn get_history(&self) -> &[ClipEntry] {
        &self.history
    }

    /// Clear the clipboard
    pub fn clear(&mut self) {
        self.primary = None;
        self.selection = None;
    }

    /// Clear history
    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    /// Has content?
    pub fn has_content(&self) -> bool {
        self.primary.is_some()
    }
}

/// Global clipboard instance
pub static CLIPBOARD: Mutex<Clipboard> = Mutex::new(Clipboard::new());

/// Initialize clipboard
pub fn init() {
    serial_println!("  Clipboard: system clipboard ready");
}

/// Copy text to clipboard
pub fn copy(text: &str) {
    CLIPBOARD.lock().copy_text(text, 0);
}

/// Paste text from clipboard
pub fn paste() -> Option<String> {
    CLIPBOARD.lock().paste_text()
}

/// Set selection text
pub fn set_selection(text: &str) {
    CLIPBOARD.lock().set_selection(text);
}

/// Get selection text
pub fn get_selection() -> Option<String> {
    CLIPBOARD.lock().get_selection()
}
