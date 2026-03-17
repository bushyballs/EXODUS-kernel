/// Status bar for Genesis — top-of-screen system info
///
/// Shows: time, battery, signal, wifi, notifications, and system icons.
/// Tapping opens notification shade; swiping down opens quick settings.
///
/// Inspired by: Android status bar, iOS status bar. All code is original.
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Status bar icon type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusIcon {
    Battery(u8),  // percentage
    Wifi(u8),     // signal strength 0-4
    Cellular(u8), // signal bars 0-5
    Bluetooth,
    Airplane,
    Alarm,
    Vibrate,
    Mute,
    Location,
    Hotspot,
    Vpn,
    Nfc,
    DoNotDisturb,
    BatterySaver,
    Usb,
    Headphones,
    Cast,
}

/// Notification indicator
pub struct NotifIndicator {
    pub app_id: String,
    pub icon_name: String,
    pub count: u8,
}

/// Status bar state
pub struct StatusBar {
    pub visible: bool,
    pub height: u16,
    /// System icons (right side)
    pub system_icons: Vec<StatusIcon>,
    /// Notification icons (left side)
    pub notif_icons: Vec<NotifIndicator>,
    /// Clock format
    pub clock_24h: bool,
    /// Battery percentage visibility
    pub show_battery_pct: bool,
    /// Background color (ARGB)
    pub bg_color: u32,
    /// Text color
    pub text_color: u32,
    /// Whether status bar is in light mode (dark icons)
    pub light_mode: bool,
}

impl StatusBar {
    const fn new() -> Self {
        StatusBar {
            visible: true,
            height: 24,
            system_icons: Vec::new(),
            notif_icons: Vec::new(),
            clock_24h: false,
            show_battery_pct: true,
            bg_color: 0xFF000000,   // black
            text_color: 0xFFFFFFFF, // white
            light_mode: false,
        }
    }

    /// Render the status bar to a pixel buffer
    pub fn render(&self, framebuf: &mut [u32], screen_width: u32) {
        if !self.visible {
            return;
        }

        let h = self.height as u32;
        // Fill background
        for y in 0..h {
            for x in 0..screen_width {
                let idx = (y * screen_width + x) as usize;
                if idx < framebuf.len() {
                    framebuf[idx] = self.bg_color;
                }
            }
        }

        // Render clock (center) — simplified text rendering
        let _time = crate::time::clock::unix_time();
        // Would render actual clock digits here

        // Render battery icon (right side)
        // Would render battery level indicator here
    }

    /// Get the time string
    pub fn time_string(&self) -> String {
        let secs = crate::time::clock::uptime_secs();
        let hours = (secs / 3600) % 24;
        let mins = (secs / 60) % 60;
        if self.clock_24h {
            format!("{:02}:{:02}", hours, mins)
        } else {
            let (h, ampm) = if hours == 0 {
                (12, "AM")
            } else if hours < 12 {
                (hours, "AM")
            } else if hours == 12 {
                (12, "PM")
            } else {
                (hours - 12, "PM")
            };
            format!("{}:{:02} {}", h, mins, ampm)
        }
    }

    /// Update system icons based on current state
    pub fn update_icons(&mut self) {
        self.system_icons.clear();

        // Battery
        self.system_icons.push(StatusIcon::Battery(85));

        // WiFi (if connected)
        self.system_icons.push(StatusIcon::Wifi(3));
    }

    /// Set transparent/translucent mode
    pub fn set_translucent(&mut self, translucent: bool) {
        if translucent {
            self.bg_color = 0x80000000; // 50% transparent black
        } else {
            self.bg_color = 0xFF000000;
        }
    }

    /// Toggle light/dark icons
    pub fn set_light_mode(&mut self, light: bool) {
        self.light_mode = light;
        self.text_color = if light { 0xFF000000 } else { 0xFFFFFFFF };
    }
}

static STATUS_BAR: Mutex<StatusBar> = Mutex::new(StatusBar::new());

pub fn init() {
    STATUS_BAR.lock().update_icons();
    crate::serial_println!("  [status-bar] Status bar initialized");
}

pub fn render(framebuf: &mut [u32], width: u32) {
    STATUS_BAR.lock().render(framebuf, width);
}

pub fn time_string() -> String {
    STATUS_BAR.lock().time_string()
}
pub fn set_visible(v: bool) {
    STATUS_BAR.lock().visible = v;
}
