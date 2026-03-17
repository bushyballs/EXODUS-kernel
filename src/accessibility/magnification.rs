/// Screen magnification for Genesis
///
/// Zoom levels, pan navigation, magnification window,
/// triple-tap activation, and smooth zoom transitions.
///
/// Inspired by: Android Magnification, iOS Zoom. All code is original.
use crate::sync::Mutex;

/// Magnification mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MagMode {
    Off,
    FullScreen,
    Window,
}

/// Magnifier state
pub struct Magnifier {
    pub mode: MagMode,
    pub zoom_level: u32, // 100 = 1x, 200 = 2x, etc.
    pub min_zoom: u32,
    pub max_zoom: u32,
    pub center_x: i32,
    pub center_y: i32,
    pub window_width: u32,
    pub window_height: u32,
    pub follow_focus: bool,
    pub smooth_zoom: bool,
    pub invert_colors: bool,
}

impl Magnifier {
    const fn new() -> Self {
        Magnifier {
            mode: MagMode::Off,
            zoom_level: 200,
            min_zoom: 100,
            max_zoom: 800,
            center_x: 0,
            center_y: 0,
            window_width: 400,
            window_height: 300,
            follow_focus: true,
            smooth_zoom: true,
            invert_colors: false,
        }
    }

    pub fn enable(&mut self, mode: MagMode) {
        self.mode = mode;
    }

    pub fn disable(&mut self) {
        self.mode = MagMode::Off;
    }

    pub fn zoom_in(&mut self) {
        if self.zoom_level < self.max_zoom {
            self.zoom_level += 25;
        }
    }

    pub fn zoom_out(&mut self) {
        if self.zoom_level > self.min_zoom {
            self.zoom_level -= 25;
        }
    }

    pub fn set_zoom(&mut self, level: u32) {
        self.zoom_level = level.clamp(self.min_zoom, self.max_zoom);
    }

    pub fn pan(&mut self, dx: i32, dy: i32) {
        self.center_x += dx;
        self.center_y += dy;
    }

    pub fn follow_point(&mut self, x: i32, y: i32) {
        if self.follow_focus {
            self.center_x = x;
            self.center_y = y;
        }
    }

    pub fn is_active(&self) -> bool {
        self.mode != MagMode::Off
    }

    /// Get the visible viewport in screen coordinates.
    ///
    /// Uses integer-only arithmetic (no f64/f32) to stay `#![no_std]`
    /// compatible on targets without a floating-point ABI.
    ///
    /// `zoom_level` is a fixed-point percentage: 100 = 1x, 200 = 2x, etc.
    /// The viewport dimensions are computed as:
    ///     vw = screen_w * 100 / zoom_level
    ///     vh = screen_h * 100 / zoom_level
    pub fn viewport(&self, screen_w: u32, screen_h: u32) -> (i32, i32, u32, u32) {
        // Guard against zero zoom (should never happen in practice).
        let zoom = if self.zoom_level == 0 {
            100
        } else {
            self.zoom_level
        };
        // Compute viewport size using integer division (100 = 1x scale).
        let vw = screen_w.saturating_mul(100) / zoom;
        let vh = screen_h.saturating_mul(100) / zoom;
        // Centre the viewport on (center_x, center_y).
        let vx = self.center_x - (vw as i32 / 2);
        let vy = self.center_y - (vh as i32 / 2);
        (vx, vy, vw, vh)
    }
}

static MAGNIFIER: Mutex<Magnifier> = Mutex::new(Magnifier::new());

pub fn init() {
    crate::serial_println!("  [a11y] Magnification engine initialized");
}

pub fn enable(mode: MagMode) {
    MAGNIFIER.lock().enable(mode);
}
pub fn disable() {
    MAGNIFIER.lock().disable();
}
pub fn zoom_in() {
    MAGNIFIER.lock().zoom_in();
}
pub fn zoom_out() {
    MAGNIFIER.lock().zoom_out();
}
