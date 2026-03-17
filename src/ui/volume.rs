use crate::sync::Mutex;
/// Volume control UI overlay
///
/// Part of the Genesis System UI. Renders the on-screen
/// volume slider when hardware volume keys are pressed.
use alloc::vec::Vec;

/// Audio stream type being controlled
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioStream {
    Media,
    Notification,
    Alarm,
    System,
}

/// Maximum volume level (0-100 range)
const MAX_VOLUME: u8 = 100;

/// Default volume step for key presses
const DEFAULT_STEP: u8 = 5;

/// How long the overlay stays visible after the last interaction (ms)
const OVERLAY_TIMEOUT_MS: u32 = 2000;

pub struct VolumeOverlay {
    pub stream: AudioStream,
    pub level: u8,
    pub muted: bool,
    pub visible: bool,
    timeout_remaining_ms: u32,
}

impl VolumeOverlay {
    pub fn new() -> Self {
        VolumeOverlay {
            stream: AudioStream::Media,
            level: 50,
            muted: false,
            visible: false,
            timeout_remaining_ms: 0,
        }
    }

    /// Adjust volume by a signed delta.
    ///
    /// Clamps to 0..=MAX_VOLUME. Shows the overlay and resets the auto-hide timer.
    pub fn adjust(&mut self, delta: i8) {
        let new_level = (self.level as i16 + delta as i16)
            .max(0)
            .min(MAX_VOLUME as i16) as u8;
        self.level = new_level;

        // Un-mute if adjusting while muted
        if self.muted && delta > 0 {
            self.muted = false;
        }

        self.visible = true;
        self.timeout_remaining_ms = OVERLAY_TIMEOUT_MS;

        crate::serial_println!(
            "  [volume] {:?} = {}{}",
            self.stream,
            self.level,
            if self.muted { " (muted)" } else { "" }
        );
    }

    /// Adjust volume by the default step size.
    pub fn volume_up(&mut self) {
        self.adjust(DEFAULT_STEP as i8);
    }

    /// Decrease volume by the default step size.
    pub fn volume_down(&mut self) {
        self.adjust(-(DEFAULT_STEP as i8));
    }

    /// Toggle mute state.
    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        self.visible = true;
        self.timeout_remaining_ms = OVERLAY_TIMEOUT_MS;

        crate::serial_println!(
            "  [volume] {:?} {}",
            self.stream,
            if self.muted { "MUTED" } else { "UNMUTED" }
        );
    }

    /// Switch which audio stream is being controlled.
    pub fn set_stream(&mut self, stream: AudioStream) {
        self.stream = stream;
        crate::serial_println!("  [volume] active stream: {:?}", stream);
    }

    /// Set volume to an absolute level.
    pub fn set_level(&mut self, level: u8) {
        self.level = level.min(MAX_VOLUME);
    }

    /// Tick the auto-hide timer. Returns true if the overlay is still visible.
    pub fn tick(&mut self, dt_ms: u32) -> bool {
        if self.visible {
            self.timeout_remaining_ms = self.timeout_remaining_ms.saturating_sub(dt_ms);
            if self.timeout_remaining_ms == 0 {
                self.visible = false;
                crate::serial_println!("  [volume] overlay auto-hidden");
            }
        }
        self.visible
    }

    /// Check if the overlay is currently visible.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Get the current volume as a fraction (0.0 to 1.0).
    pub fn level_fraction(&self) -> f32 {
        self.level as f32 / MAX_VOLUME as f32
    }
}

static VOLUME_OVERLAY: Mutex<Option<VolumeOverlay>> = Mutex::new(None);

pub fn init() {
    *VOLUME_OVERLAY.lock() = Some(VolumeOverlay::new());
    crate::serial_println!("  [volume] Volume overlay initialized");
}

/// Adjust global volume by delta.
pub fn adjust(delta: i8) {
    if let Some(ref mut overlay) = *VOLUME_OVERLAY.lock() {
        overlay.adjust(delta);
    }
}

/// Toggle global mute.
pub fn toggle_mute() {
    if let Some(ref mut overlay) = *VOLUME_OVERLAY.lock() {
        overlay.toggle_mute();
    }
}

/// Tick the global volume overlay timer.
pub fn tick(dt_ms: u32) {
    if let Some(ref mut overlay) = *VOLUME_OVERLAY.lock() {
        overlay.tick(dt_ms);
    }
}
