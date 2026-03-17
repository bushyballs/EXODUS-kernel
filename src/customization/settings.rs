/// settings.rs — user customisation settings for Genesis.
///
/// Provides:
/// - `LayoutMode` enum: `Default`, `Compact`, `Expanded`, `Minimal`.
/// - `CustomSettings` struct with `font_size`, `accent_color`, and
///   `layout_mode` fields.
/// - `save_settings()` — persist settings to a static shadow buffer
///   (stub: logs via serial; real impl would write to NVRAM / disk).
/// - `load_settings()` — restore from the shadow buffer.
/// - `apply_settings()` — push the active settings to dependent
///   subsystems (stub: logs the active configuration).
///
/// The module purposely avoids heap allocation: all state lives in
/// `static mut` variables.
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// LayoutMode
// ---------------------------------------------------------------------------

/// High-level desktop layout strategy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayoutMode {
    /// Standard spacing (default).
    Default,
    /// Reduced spacing for small displays.
    Compact,
    /// Increased spacing / larger touch targets.
    Expanded,
    /// Hide non-essential UI chrome.
    Minimal,
}

// ---------------------------------------------------------------------------
// CustomSettings
// ---------------------------------------------------------------------------

/// User-facing customisation settings.
#[derive(Clone, Copy, Debug)]
pub struct CustomSettings {
    /// UI font size in points (typically 10-24).
    pub font_size: u8,

    /// Accent colour as packed ARGB (0xAARRGGBB).
    pub accent_color: u32,

    /// Desktop layout mode.
    pub layout_mode: LayoutMode,

    /// Whether the dark theme is enabled.
    pub dark_theme: bool,

    /// UI animation speed multiplier in Q8 fixed-point
    /// (0x100 = 1.0x, 0x80 = 0.5x, 0x200 = 2.0x).
    pub animation_speed_q8: u16,

    /// Whether reduced motion (accessibility) is requested.
    pub reduced_motion: bool,

    /// Display scaling factor in percent (100 = 1:1, 150 = 150%).
    pub display_scale_pct: u8,
}

impl CustomSettings {
    /// Compile-time default settings.
    pub const fn default() -> Self {
        Self {
            font_size: 14,
            accent_color: 0xFF_F5_9E_0B, // amber #f59e0b
            layout_mode: LayoutMode::Default,
            dark_theme: true,
            animation_speed_q8: 0x100, // 1.0x
            reduced_motion: false,
            display_scale_pct: 100,
        }
    }

    /// Return `true` if the values are within sane bounds.
    pub fn is_valid(&self) -> bool {
        self.font_size >= 8
            && self.font_size <= 36
            && self.display_scale_pct >= 50
            && self.display_scale_pct <= 200
    }

    /// Clamp all fields to their valid ranges in-place.
    pub fn sanitise(&mut self) {
        self.font_size = self.font_size.clamp(8, 36);
        self.display_scale_pct = self.display_scale_pct.clamp(50, 200);
        if self.animation_speed_q8 == 0 {
            self.animation_speed_q8 = 0x100;
        }
    }
}

// ---------------------------------------------------------------------------
// Static state
// ---------------------------------------------------------------------------

/// Active (in-use) settings.
static mut ACTIVE: CustomSettings = CustomSettings::default();

/// Persisted shadow copy (written by `save_settings`, read by `load_settings`).
static mut SAVED: CustomSettings = CustomSettings::default();

/// Whether `save_settings` has been called at least once.
static mut HAS_SAVED: bool = false;

// ---------------------------------------------------------------------------
// save_settings
// ---------------------------------------------------------------------------

/// Persist the current active settings to the shadow buffer.
///
/// Stub: copies `ACTIVE` into `SAVED` and logs a confirmation.  A real
/// implementation would serialise to NVRAM, EFI variables, or a config file.
pub fn save_settings() {
    unsafe {
        SAVED = ACTIVE;
        HAS_SAVED = true;
    }
    serial_println!(
        "[settings] save_settings  font_size={}  accent=0x{:08X}  layout={:?}  dark={}  scale={}%",
        unsafe { ACTIVE.font_size },
        unsafe { ACTIVE.accent_color },
        unsafe { ACTIVE.layout_mode },
        unsafe { ACTIVE.dark_theme },
        unsafe { ACTIVE.display_scale_pct },
    );
}

// ---------------------------------------------------------------------------
// load_settings
// ---------------------------------------------------------------------------

/// Restore the active settings from the shadow buffer.
///
/// If `save_settings` has never been called the active settings remain at
/// their compile-time defaults.  Returns `true` if saved data was found.
pub fn load_settings() -> bool {
    unsafe {
        if HAS_SAVED {
            ACTIVE = SAVED;
            ACTIVE.sanitise();
            serial_println!("[settings] load_settings: restored from shadow copy");
            true
        } else {
            serial_println!("[settings] load_settings: no saved data; using defaults");
            false
        }
    }
}

// ---------------------------------------------------------------------------
// apply_settings
// ---------------------------------------------------------------------------

/// Push the active settings to dependent subsystems.
///
/// Stub: logs the full active configuration.  In a full implementation
/// this would call into `desktop_layout`, font rendering, the compositor,
/// and the theme engine.
pub fn apply_settings() {
    let s = unsafe { ACTIVE };

    serial_println!("[settings] apply_settings ─────────────────────────");
    serial_println!("  font_size         = {}", s.font_size);
    serial_println!("  accent_color      = 0x{:08X}", s.accent_color);
    serial_println!("  layout_mode       = {:?}", s.layout_mode);
    serial_println!("  dark_theme        = {}", s.dark_theme);
    serial_println!(
        "  animation_speed   = {}x (Q8=0x{:04X})",
        s.animation_speed_q8 >> 8,
        s.animation_speed_q8
    );
    serial_println!("  reduced_motion    = {}", s.reduced_motion);
    serial_println!("  display_scale     = {}%", s.display_scale_pct);
    serial_println!("[settings] ─────────────────────────────────────────");

    // TODO: wire calls to subsystems:
    //   desktop_layout::set_scale(s.display_scale_pct);
    //   font_engine::set_size(s.font_size);
    //   theme::set_dark(s.dark_theme);
    //   theme::set_accent(s.accent_color);
    //   animation::set_speed_q8(s.animation_speed_q8);
    //   animation::set_reduced_motion(s.reduced_motion);
}

// ---------------------------------------------------------------------------
// Accessors / mutators
// ---------------------------------------------------------------------------

/// Return a copy of the active settings.
pub fn get_settings() -> CustomSettings {
    unsafe { ACTIVE }
}

/// Replace the active settings entirely.
///
/// The new settings are sanitised before being applied.  Call
/// `apply_settings()` afterwards to push the changes to subsystems.
pub fn set_settings(mut new: CustomSettings) {
    new.sanitise();
    unsafe {
        ACTIVE = new;
    }
    serial_println!("[settings] set_settings: active settings replaced");
}

/// Update only the font size.
pub fn set_font_size(size: u8) {
    let size = size.clamp(8, 36);
    unsafe {
        ACTIVE.font_size = size;
    }
    serial_println!("[settings] set_font_size: {}", size);
}

/// Update only the accent colour.
pub fn set_accent_color(argb: u32) {
    unsafe {
        ACTIVE.accent_color = argb;
    }
    serial_println!("[settings] set_accent_color: 0x{:08X}", argb);
}

/// Update only the layout mode.
pub fn set_layout_mode(mode: LayoutMode) {
    unsafe {
        ACTIVE.layout_mode = mode;
    }
    serial_println!("[settings] set_layout_mode: {:?}", mode);
}

/// Toggle dark theme.
pub fn set_dark_theme(enabled: bool) {
    unsafe {
        ACTIVE.dark_theme = enabled;
    }
    serial_println!("[settings] set_dark_theme: {}", enabled);
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    // On init: attempt to load persisted settings; if none exist, use defaults
    // and immediately save so the shadow copy is populated.
    if !load_settings() {
        save_settings();
    }
    apply_settings();
    serial_println!("[settings] customisation settings module ready");
}
