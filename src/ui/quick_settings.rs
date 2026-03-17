/// Quick settings for Genesis — swipe-down toggles panel
///
/// Provides quick access to system toggles: WiFi, Bluetooth,
/// airplane mode, brightness, do-not-disturb, flashlight, etc.
///
/// Inspired by: Android Quick Settings, iOS Control Center. All code is original.
use crate::sync::Mutex;
use alloc::vec::Vec;

/// Quick setting tile state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileState {
    Active,
    Inactive,
    Unavailable,
}

/// Quick setting tile
pub struct QsTile {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    pub state: TileState,
}

/// Quick settings panel
pub struct QuickSettings {
    pub visible: bool,
    pub expanded: bool, // full panel vs. compact
    pub tiles: Vec<QsTile>,
    /// Brightness level (0-255)
    pub brightness: u8,
    /// Auto-brightness
    pub auto_brightness: bool,
}

impl QuickSettings {
    const fn new() -> Self {
        QuickSettings {
            visible: false,
            expanded: false,
            tiles: Vec::new(),
            brightness: 200,
            auto_brightness: true,
        }
    }

    fn setup_default_tiles(&mut self) {
        self.tiles = alloc::vec![
            QsTile {
                id: "wifi",
                label: "Wi-Fi",
                icon: "wifi",
                state: TileState::Active
            },
            QsTile {
                id: "bluetooth",
                label: "Bluetooth",
                icon: "bluetooth",
                state: TileState::Inactive
            },
            QsTile {
                id: "airplane",
                label: "Airplane",
                icon: "airplane",
                state: TileState::Inactive
            },
            QsTile {
                id: "dnd",
                label: "Do Not Disturb",
                icon: "dnd",
                state: TileState::Inactive
            },
            QsTile {
                id: "flashlight",
                label: "Flashlight",
                icon: "flashlight",
                state: TileState::Inactive
            },
            QsTile {
                id: "rotation",
                label: "Auto-rotate",
                icon: "rotation",
                state: TileState::Active
            },
            QsTile {
                id: "battery_saver",
                label: "Battery Saver",
                icon: "battery_saver",
                state: TileState::Inactive
            },
            QsTile {
                id: "location",
                label: "Location",
                icon: "location",
                state: TileState::Active
            },
            QsTile {
                id: "hotspot",
                label: "Hotspot",
                icon: "hotspot",
                state: TileState::Inactive
            },
            QsTile {
                id: "dark_mode",
                label: "Dark Mode",
                icon: "dark_mode",
                state: TileState::Active
            },
            QsTile {
                id: "screen_cast",
                label: "Cast",
                icon: "cast",
                state: TileState::Inactive
            },
            QsTile {
                id: "vpn",
                label: "VPN",
                icon: "vpn",
                state: TileState::Inactive
            },
        ];
    }

    /// Toggle a tile
    pub fn toggle(&mut self, id: &str) {
        if let Some(tile) = self.tiles.iter_mut().find(|t| t.id == id) {
            tile.state = match tile.state {
                TileState::Active => TileState::Inactive,
                TileState::Inactive => TileState::Active,
                TileState::Unavailable => TileState::Unavailable,
            };
        }
    }

    /// Set brightness
    pub fn set_brightness(&mut self, level: u8) {
        self.brightness = level;
        self.auto_brightness = false;
    }

    /// Show/hide panel
    pub fn show(&mut self) {
        self.visible = true;
    }
    pub fn hide(&mut self) {
        self.visible = false;
    }
    pub fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }

    /// Get tile state
    pub fn tile_state(&self, id: &str) -> TileState {
        self.tiles
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.state)
            .unwrap_or(TileState::Unavailable)
    }
}

static QUICK_SETTINGS: Mutex<QuickSettings> = Mutex::new(QuickSettings::new());

pub fn init() {
    QUICK_SETTINGS.lock().setup_default_tiles();
    crate::serial_println!("  [quick-settings] Quick settings initialized");
}

pub fn toggle(id: &str) {
    QUICK_SETTINGS.lock().toggle(id);
}
pub fn show() {
    QUICK_SETTINGS.lock().show();
}
pub fn hide() {
    QUICK_SETTINGS.lock().hide();
}
pub fn set_brightness(level: u8) {
    QUICK_SETTINGS.lock().set_brightness(level);
}
