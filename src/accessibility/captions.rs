/// Live captions for Genesis
///
/// Real-time speech-to-text captions, caption display styling,
/// caption history, and notification caption support.
///
/// Inspired by: Android Live Caption, iOS Live Captions. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Caption display position
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptionPosition {
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Caption style
pub struct CaptionStyle {
    pub font_size: u32,
    pub text_color: u32, // ARGB
    pub bg_color: u32,   // ARGB
    pub window_color: u32,
    pub edge_type: EdgeType,
    pub bold: bool,
    pub italic: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeType {
    None,
    Outline,
    DropShadow,
    Raised,
    Depressed,
}

impl CaptionStyle {
    pub fn default_style() -> Self {
        CaptionStyle {
            font_size: 18,
            text_color: 0xFFFFFFFF,
            bg_color: 0xCC000000,
            window_color: 0x00000000,
            edge_type: EdgeType::None,
            bold: false,
            italic: false,
        }
    }
}

/// A caption entry
pub struct Caption {
    pub text: String,
    pub timestamp: u64,
    pub speaker: Option<String>,
    pub confidence: f32,
    pub is_final: bool,
}

/// Caption manager
pub struct CaptionManager {
    pub enabled: bool,
    pub position: CaptionPosition,
    pub style: CaptionStyle,
    pub captions: Vec<Caption>,
    pub max_history: usize,
    pub current_partial: String,
    pub show_sound_labels: bool, // [laughter], [music], etc.
    pub language: String,
}

impl CaptionManager {
    const fn new() -> Self {
        CaptionManager {
            enabled: false,
            position: CaptionPosition::Bottom,
            style: CaptionStyle {
                font_size: 18,
                text_color: 0xFFFFFFFF,
                bg_color: 0xCC000000,
                window_color: 0x00000000,
                edge_type: EdgeType::None,
                bold: false,
                italic: false,
            },
            captions: Vec::new(),
            max_history: 100,
            current_partial: String::new(),
            show_sound_labels: true,
            language: String::new(),
        }
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }

    pub fn disable(&mut self) {
        self.enabled = false;
        self.current_partial.clear();
    }

    pub fn add_partial(&mut self, text: &str) {
        if !self.enabled {
            return;
        }
        self.current_partial = String::from(text);
    }

    pub fn finalize(&mut self, text: &str, confidence: f32) {
        if !self.enabled {
            return;
        }
        self.current_partial.clear();

        if self.captions.len() >= self.max_history {
            self.captions.remove(0);
        }

        self.captions.push(Caption {
            text: String::from(text),
            timestamp: crate::time::clock::unix_time(),
            speaker: None,
            confidence,
            is_final: true,
        });
    }

    pub fn add_sound_label(&mut self, label: &str) {
        if !self.enabled || !self.show_sound_labels {
            return;
        }
        let text = alloc::format!("[{}]", label);
        self.finalize(&text, 1.0);
    }

    pub fn recent_captions(&self, count: usize) -> &[Caption] {
        let start = if self.captions.len() > count {
            self.captions.len() - count
        } else {
            0
        };
        &self.captions[start..]
    }

    pub fn clear_history(&mut self) {
        self.captions.clear();
        self.current_partial.clear();
    }
}

static CAPTIONS: Mutex<CaptionManager> = Mutex::new(CaptionManager::new());

pub fn init() {
    crate::serial_println!("  [a11y] Live captions initialized");
}

pub fn enable() {
    CAPTIONS.lock().enable();
}
pub fn disable() {
    CAPTIONS.lock().disable();
}
