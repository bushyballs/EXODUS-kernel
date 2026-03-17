/// Multi-monitor support for Genesis
///
/// Provides: display enumeration, arrangement configuration,
/// mirroring mode, extend mode, per-display DPI scaling,
/// and cross-monitor coordinate mapping.
///
/// Uses Q16 fixed-point math throughout (no floats).
///
/// Inspired by: Windows display settings, Xrandr, Wayland output protocol.
/// All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Q16 fixed-point constant: 1.0
const Q16_ONE: i32 = 65536;

/// Q16 multiply
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Q16 divide
fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << 16) / b as i64) as i32
}

/// Q16 from integer
fn q16_from_int(x: i32) -> i32 {
    x << 16
}

/// Display connector type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorConnector {
    HDMI,
    DisplayPort,
    VGA,
    DVI,
    EDP, // Embedded DisplayPort (laptop panels)
    UsbC,
    Thunderbolt,
    Virtual,
    Wireless,
}

/// Display mode for a monitor
#[derive(Debug, Clone)]
pub struct MonitorMode {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
    pub preferred: bool,
}

impl MonitorMode {
    pub fn new(w: u32, h: u32, hz: u32, preferred: bool) -> Self {
        MonitorMode {
            width: w,
            height: h,
            refresh_hz: hz,
            preferred,
        }
    }

    /// Get pixel count
    pub fn pixel_count(&self) -> u64 {
        self.width as u64 * self.height as u64
    }
}

/// DPI scaling level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DpiScale {
    Scale100,    // 1x (96 DPI)
    Scale125,    // 1.25x (120 DPI)
    Scale150,    // 1.5x (144 DPI)
    Scale175,    // 1.75x (168 DPI)
    Scale200,    // 2x (192 DPI)
    Scale250,    // 2.5x (240 DPI)
    Scale300,    // 3x (288 DPI)
    Custom(i32), // Q16 custom scale factor
}

impl DpiScale {
    /// Get scale factor as Q16
    pub fn factor(&self) -> i32 {
        match self {
            DpiScale::Scale100 => Q16_ONE,
            DpiScale::Scale125 => Q16_ONE + Q16_ONE / 4,
            DpiScale::Scale150 => Q16_ONE + Q16_ONE / 2,
            DpiScale::Scale175 => Q16_ONE + Q16_ONE * 3 / 4,
            DpiScale::Scale200 => Q16_ONE * 2,
            DpiScale::Scale250 => Q16_ONE * 5 / 2,
            DpiScale::Scale300 => Q16_ONE * 3,
            DpiScale::Custom(f) => *f,
        }
    }

    /// Get DPI value
    pub fn dpi(&self) -> u32 {
        let factor = self.factor();
        ((96i64 * factor as i64) >> 16) as u32
    }

    /// Scale a pixel dimension by this DPI
    pub fn scale_px(&self, px: u32) -> u32 {
        let scaled = q16_mul(q16_from_int(px as i32), self.factor());
        (scaled >> 16) as u32
    }

    /// Inverse-scale a pixel dimension (screen coords to logical coords)
    pub fn unscale_px(&self, px: u32) -> u32 {
        let factor = self.factor();
        if factor == 0 {
            return px;
        }
        let unscaled = ((px as i64) << 16) / factor as i64;
        unscaled as u32
    }
}

/// Monitor rotation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    Normal,
    Left,     // 90 degrees counter-clockwise
    Right,    // 90 degrees clockwise
    Inverted, // 180 degrees
}

impl Rotation {
    /// Get effective dimensions after rotation
    pub fn effective_size(&self, w: u32, h: u32) -> (u32, u32) {
        match self {
            Rotation::Normal | Rotation::Inverted => (w, h),
            Rotation::Left | Rotation::Right => (h, w),
        }
    }
}

/// A single physical display monitor
pub struct Monitor {
    pub id: u32,
    pub name: String,
    pub connector: MonitorConnector,
    pub connected: bool,
    pub enabled: bool,
    pub primary: bool,

    // Current mode
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,

    // Available modes
    pub modes: Vec<MonitorMode>,

    // Position in virtual desktop space
    pub x: i32,
    pub y: i32,

    // DPI and scaling
    pub dpi_scale: DpiScale,
    pub physical_width_mm: u32,
    pub physical_height_mm: u32,

    // Rotation
    pub rotation: Rotation,

    // Color profile id (from color_mgr)
    pub color_profile_id: u32,

    // Brightness (Q16, 0..Q16_ONE)
    pub brightness: i32,
}

impl Monitor {
    pub fn new(id: u32, name: &str, connector: MonitorConnector) -> Self {
        let mut modes = Vec::new();
        modes.push(MonitorMode::new(1920, 1080, 60, true));
        modes.push(MonitorMode::new(1280, 720, 60, false));
        modes.push(MonitorMode::new(1024, 768, 60, false));

        Monitor {
            id,
            name: String::from(name),
            connector,
            connected: true,
            enabled: true,
            primary: false,
            width: 1920,
            height: 1080,
            refresh_hz: 60,
            modes,
            x: 0,
            y: 0,
            dpi_scale: DpiScale::Scale100,
            physical_width_mm: 530,
            physical_height_mm: 300,
            rotation: Rotation::Normal,
            color_profile_id: 0,
            brightness: Q16_ONE,
        }
    }

    /// Set the display mode
    pub fn set_mode(&mut self, width: u32, height: u32, refresh: u32) -> bool {
        let found = self
            .modes
            .iter()
            .any(|m| m.width == width && m.height == height && m.refresh_hz == refresh);
        if found {
            self.width = width;
            self.height = height;
            self.refresh_hz = refresh;
            true
        } else {
            false
        }
    }

    /// Get the effective (rotated) dimensions
    pub fn effective_size(&self) -> (u32, u32) {
        self.rotation.effective_size(self.width, self.height)
    }

    /// Get the logical (DPI-scaled) dimensions
    pub fn logical_size(&self) -> (u32, u32) {
        let (ew, eh) = self.effective_size();
        (self.dpi_scale.unscale_px(ew), self.dpi_scale.unscale_px(eh))
    }

    /// Get the bounding rect in virtual desktop space
    pub fn bounds(&self) -> (i32, i32, u32, u32) {
        let (w, h) = self.effective_size();
        (self.x, self.y, w, h)
    }

    /// Check if a virtual desktop coordinate falls on this monitor
    pub fn contains(&self, vx: i32, vy: i32) -> bool {
        let (w, h) = self.effective_size();
        vx >= self.x && vy >= self.y && vx < self.x + w as i32 && vy < self.y + h as i32
    }

    /// Convert virtual desktop coords to local monitor coords
    pub fn to_local(&self, vx: i32, vy: i32) -> (i32, i32) {
        (vx - self.x, vy - self.y)
    }

    /// Convert local monitor coords to virtual desktop coords
    pub fn to_virtual(&self, lx: i32, ly: i32) -> (i32, i32) {
        (lx + self.x, ly + self.y)
    }

    /// Compute physical DPI from panel size and resolution
    pub fn physical_dpi(&self) -> u32 {
        if self.physical_width_mm == 0 || self.physical_height_mm == 0 {
            return 96;
        }
        // DPI = pixels / inches; 1 inch = 25.4mm
        // Horizontal DPI = width_px * 254 / (width_mm * 10)
        let hdpi = (self.width as u64 * 254) / (self.physical_width_mm as u64 * 10);
        hdpi as u32
    }

    /// Auto-detect appropriate DPI scale from physical DPI
    pub fn auto_dpi_scale(&mut self) {
        let pdpi = self.physical_dpi();
        self.dpi_scale = if pdpi < 110 {
            DpiScale::Scale100
        } else if pdpi < 135 {
            DpiScale::Scale125
        } else if pdpi < 165 {
            DpiScale::Scale150
        } else if pdpi < 210 {
            DpiScale::Scale200
        } else if pdpi < 270 {
            DpiScale::Scale250
        } else {
            DpiScale::Scale300
        };
    }
}

/// Multi-monitor arrangement mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrangementMode {
    Extend,     // Each monitor is a separate region of the virtual desktop
    Mirror,     // All monitors show the same content
    SingleOnly, // Only one monitor is active
}

/// Multi-monitor manager
pub struct MultiMonitorManager {
    pub monitors: Vec<Monitor>,
    pub arrangement: ArrangementMode,
    pub mirror_source: u32, // Primary monitor id for mirroring
    pub next_id: u32,
    pub virtual_width: u32,  // Total virtual desktop width
    pub virtual_height: u32, // Total virtual desktop height
}

impl MultiMonitorManager {
    const fn new() -> Self {
        MultiMonitorManager {
            monitors: Vec::new(),
            arrangement: ArrangementMode::Extend,
            mirror_source: 0,
            next_id: 1,
            virtual_width: 0,
            virtual_height: 0,
        }
    }

    /// Add a monitor
    pub fn add_monitor(&mut self, name: &str, connector: MonitorConnector) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut mon = Monitor::new(id, name, connector);
        if self.monitors.is_empty() {
            mon.primary = true;
            self.mirror_source = id;
        }
        self.monitors.push(mon);
        self.recalculate_layout();
        id
    }

    /// Remove a monitor
    pub fn remove_monitor(&mut self, id: u32) {
        self.monitors.retain(|m| m.id != id);
        // Reassign primary if needed
        if !self.monitors.iter().any(|m| m.primary) {
            if let Some(first) = self.monitors.first_mut() {
                first.primary = true;
                self.mirror_source = first.id;
            }
        }
        self.recalculate_layout();
    }

    /// Set a monitor as primary
    pub fn set_primary(&mut self, id: u32) {
        for m in &mut self.monitors {
            m.primary = m.id == id;
        }
        self.mirror_source = id;
    }

    /// Set arrangement mode
    pub fn set_arrangement(&mut self, mode: ArrangementMode) {
        self.arrangement = mode;
        self.recalculate_layout();
    }

    /// Recalculate monitor positions for the current arrangement
    pub fn recalculate_layout(&mut self) {
        match self.arrangement {
            ArrangementMode::Extend => {
                // Place monitors side by side, left to right
                let mut x_offset: i32 = 0;
                let mut max_h: u32 = 0;
                for mon in &mut self.monitors {
                    if !mon.enabled {
                        continue;
                    }
                    let (w, h) = mon.effective_size();
                    mon.x = x_offset;
                    mon.y = 0;
                    x_offset += w as i32;
                    if h > max_h {
                        max_h = h;
                    }
                }
                self.virtual_width = x_offset as u32;
                self.virtual_height = max_h;
            }
            ArrangementMode::Mirror => {
                // All monitors at origin (0, 0)
                let mut max_w: u32 = 0;
                let mut max_h: u32 = 0;
                for mon in &mut self.monitors {
                    mon.x = 0;
                    mon.y = 0;
                    let (w, h) = mon.effective_size();
                    if w > max_w {
                        max_w = w;
                    }
                    if h > max_h {
                        max_h = h;
                    }
                }
                self.virtual_width = max_w;
                self.virtual_height = max_h;
            }
            ArrangementMode::SingleOnly => {
                for mon in &mut self.monitors {
                    if mon.primary {
                        mon.x = 0;
                        mon.y = 0;
                        mon.enabled = true;
                        let (w, h) = mon.effective_size();
                        self.virtual_width = w;
                        self.virtual_height = h;
                    } else {
                        mon.enabled = false;
                    }
                }
            }
        }
    }

    /// Arrange monitors vertically instead of horizontally
    pub fn arrange_vertical(&mut self) {
        if self.arrangement != ArrangementMode::Extend {
            return;
        }
        let mut y_offset: i32 = 0;
        let mut max_w: u32 = 0;
        for mon in &mut self.monitors {
            if !mon.enabled {
                continue;
            }
            let (w, h) = mon.effective_size();
            mon.x = 0;
            mon.y = y_offset;
            y_offset += h as i32;
            if w > max_w {
                max_w = w;
            }
        }
        self.virtual_width = max_w;
        self.virtual_height = y_offset as u32;
    }

    /// Set custom position for a monitor
    pub fn set_position(&mut self, id: u32, x: i32, y: i32) {
        if let Some(mon) = self.monitors.iter_mut().find(|m| m.id == id) {
            mon.x = x;
            mon.y = y;
        }
        self.update_virtual_bounds();
    }

    /// Recalculate virtual desktop bounds from current positions
    fn update_virtual_bounds(&mut self) {
        let mut max_right: i32 = 0;
        let mut max_bottom: i32 = 0;
        for mon in &self.monitors {
            if !mon.enabled {
                continue;
            }
            let (w, h) = mon.effective_size();
            let right = mon.x + w as i32;
            let bottom = mon.y + h as i32;
            if right > max_right {
                max_right = right;
            }
            if bottom > max_bottom {
                max_bottom = bottom;
            }
        }
        self.virtual_width = max_right as u32;
        self.virtual_height = max_bottom as u32;
    }

    /// Find which monitor contains a virtual desktop coordinate
    pub fn monitor_at(&self, vx: i32, vy: i32) -> Option<u32> {
        for mon in &self.monitors {
            if mon.enabled && mon.contains(vx, vy) {
                return Some(mon.id);
            }
        }
        None
    }

    /// Get the primary monitor id
    pub fn primary_id(&self) -> Option<u32> {
        self.monitors.iter().find(|m| m.primary).map(|m| m.id)
    }

    /// Get monitor count (connected and enabled)
    pub fn active_count(&self) -> usize {
        self.monitors
            .iter()
            .filter(|m| m.connected && m.enabled)
            .count()
    }

    /// List all monitors with their info
    pub fn list_monitors(&self) -> Vec<(u32, String, u32, u32, i32, i32, bool)> {
        self.monitors
            .iter()
            .filter(|m| m.connected)
            .map(|m| {
                let (w, h) = m.effective_size();
                (m.id, m.name.clone(), w, h, m.x, m.y, m.primary)
            })
            .collect()
    }

    /// Set DPI scale for a specific monitor
    pub fn set_dpi(&mut self, id: u32, scale: DpiScale) {
        if let Some(mon) = self.monitors.iter_mut().find(|m| m.id == id) {
            mon.dpi_scale = scale;
        }
    }

    /// Set rotation for a specific monitor
    pub fn set_rotation(&mut self, id: u32, rotation: Rotation) {
        if let Some(mon) = self.monitors.iter_mut().find(|m| m.id == id) {
            mon.rotation = rotation;
        }
        self.recalculate_layout();
    }

    /// Set mode for a specific monitor
    pub fn set_mode(&mut self, id: u32, w: u32, h: u32, hz: u32) -> bool {
        let result = if let Some(mon) = self.monitors.iter_mut().find(|m| m.id == id) {
            mon.set_mode(w, h, hz)
        } else {
            false
        };
        if result {
            self.recalculate_layout();
        }
        result
    }

    /// Clamp a cursor position to the virtual desktop bounds
    pub fn clamp_cursor(&self, x: i32, y: i32) -> (i32, i32) {
        let cx = if x < 0 {
            0
        } else if x >= self.virtual_width as i32 {
            self.virtual_width as i32 - 1
        } else {
            x
        };
        let cy = if y < 0 {
            0
        } else if y >= self.virtual_height as i32 {
            self.virtual_height as i32 - 1
        } else {
            y
        };
        (cx, cy)
    }
}

static MONITORS: Mutex<MultiMonitorManager> = Mutex::new(MultiMonitorManager::new());

/// Initialize multi-monitor support
pub fn init() {
    let mut mgr = MONITORS.lock();

    // Register default virtual display
    let id = mgr.add_monitor("Virtual-1", MonitorConnector::Virtual);

    serial_println!(
        "    [multi-monitor] Multi-monitor support initialized (display {}, extend/mirror/per-DPI)",
        id
    );
}

/// Add a monitor
pub fn add_monitor(name: &str, connector: MonitorConnector) -> u32 {
    MONITORS.lock().add_monitor(name, connector)
}

/// Remove a monitor
pub fn remove_monitor(id: u32) {
    MONITORS.lock().remove_monitor(id);
}

/// Set arrangement mode
pub fn set_arrangement(mode: ArrangementMode) {
    MONITORS.lock().set_arrangement(mode);
}

/// Set primary monitor
pub fn set_primary(id: u32) {
    MONITORS.lock().set_primary(id);
}

/// Set DPI scale for a monitor
pub fn set_dpi(id: u32, scale: DpiScale) {
    MONITORS.lock().set_dpi(id, scale);
}

/// Set rotation for a monitor
pub fn set_rotation(id: u32, rotation: Rotation) {
    MONITORS.lock().set_rotation(id, rotation);
}

/// Find which monitor contains coordinates
pub fn monitor_at(x: i32, y: i32) -> Option<u32> {
    MONITORS.lock().monitor_at(x, y)
}

/// Get active monitor count
pub fn active_count() -> usize {
    MONITORS.lock().active_count()
}

/// Get virtual desktop size
pub fn virtual_size() -> (u32, u32) {
    let mgr = MONITORS.lock();
    (mgr.virtual_width, mgr.virtual_height)
}
