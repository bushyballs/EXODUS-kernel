/// Settings app for Genesis — system configuration
///
/// Hierarchical settings with categories, preferences, and search.
/// Persisted to flash/disk. Provides API for apps to read system prefs.
///
/// Inspired by: Android Settings, GNOME Settings. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Setting value types
#[derive(Clone)]
pub enum SettingValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Choice(String, Vec<String>), // (selected, options)
}

/// A single setting entry
pub struct Setting {
    pub key: String,
    pub title: String,
    pub summary: String,
    pub value: SettingValue,
    pub category: String,
    /// Whether this requires a restart
    pub requires_restart: bool,
    /// Permission needed to modify
    pub protected: bool,
}

/// Settings store
pub struct SettingsStore {
    settings: BTreeMap<String, Setting>,
    /// Listeners (key -> callback count)
    listeners: BTreeMap<String, u32>,
}

impl SettingsStore {
    const fn new() -> Self {
        SettingsStore {
            settings: BTreeMap::new(),
            listeners: BTreeMap::new(),
        }
    }

    fn setup_defaults(&mut self) {
        // Display
        self.set_def(
            "display.brightness",
            "Brightness",
            "display",
            SettingValue::Int(200),
        );
        self.set_def(
            "display.auto_brightness",
            "Auto-brightness",
            "display",
            SettingValue::Bool(true),
        );
        self.set_def(
            "display.dark_mode",
            "Dark mode",
            "display",
            SettingValue::Bool(true),
        );
        self.set_def(
            "display.font_size",
            "Font size",
            "display",
            SettingValue::Int(14),
        );
        self.set_def(
            "display.screen_timeout",
            "Screen timeout (seconds)",
            "display",
            SettingValue::Int(60),
        );
        self.set_def(
            "display.resolution",
            "Resolution",
            "display",
            SettingValue::Choice(
                String::from("1920x1080"),
                alloc::vec![
                    String::from("1280x720"),
                    String::from("1920x1080"),
                    String::from("2560x1440")
                ],
            ),
        );

        // Sound
        self.set_def(
            "sound.volume_media",
            "Media volume",
            "sound",
            SettingValue::Int(80),
        );
        self.set_def(
            "sound.volume_ring",
            "Ring volume",
            "sound",
            SettingValue::Int(100),
        );
        self.set_def(
            "sound.volume_alarm",
            "Alarm volume",
            "sound",
            SettingValue::Int(100),
        );
        self.set_def(
            "sound.volume_system",
            "System volume",
            "sound",
            SettingValue::Int(50),
        );
        self.set_def(
            "sound.vibrate",
            "Vibrate",
            "sound",
            SettingValue::Bool(true),
        );

        // Network
        self.set_def(
            "network.wifi_enabled",
            "Wi-Fi",
            "network",
            SettingValue::Bool(true),
        );
        self.set_def(
            "network.bluetooth_enabled",
            "Bluetooth",
            "network",
            SettingValue::Bool(false),
        );
        self.set_def(
            "network.airplane_mode",
            "Airplane mode",
            "network",
            SettingValue::Bool(false),
        );
        self.set_def(
            "network.hostname",
            "Device name",
            "network",
            SettingValue::Text(String::from("genesis")),
        );

        // Security
        self.set_def(
            "security.lock_method",
            "Lock method",
            "security",
            SettingValue::Choice(
                String::from("none"),
                alloc::vec![
                    String::from("none"),
                    String::from("pin"),
                    String::from("password"),
                    String::from("pattern")
                ],
            ),
        );
        self.set_def(
            "security.auto_lock",
            "Auto-lock (seconds)",
            "security",
            SettingValue::Int(60),
        );
        self.set_def(
            "security.show_lockscreen_notifs",
            "Show notifications on lock screen",
            "security",
            SettingValue::Bool(true),
        );

        // System
        self.set_def(
            "system.language",
            "Language",
            "system",
            SettingValue::Text(String::from("en-US")),
        );
        self.set_def(
            "system.timezone",
            "Timezone",
            "system",
            SettingValue::Text(String::from("UTC")),
        );
        self.set_def(
            "system.24h_clock",
            "24-hour clock",
            "system",
            SettingValue::Bool(false),
        );
        self.set_def(
            "system.developer_mode",
            "Developer mode",
            "system",
            SettingValue::Bool(false),
        );

        // Developer
        self.set_def(
            "dev.usb_debugging",
            "USB debugging",
            "developer",
            SettingValue::Bool(false),
        );
        self.set_def(
            "dev.show_fps",
            "Show FPS overlay",
            "developer",
            SettingValue::Bool(false),
        );
        self.set_def(
            "dev.show_layout_bounds",
            "Show layout bounds",
            "developer",
            SettingValue::Bool(false),
        );
        self.set_def(
            "dev.animation_scale",
            "Animation scale",
            "developer",
            SettingValue::Float(1.0),
        );
    }

    fn set_def(&mut self, key: &str, title: &str, category: &str, value: SettingValue) {
        self.settings.insert(
            String::from(key),
            Setting {
                key: String::from(key),
                title: String::from(title),
                summary: String::new(),
                value,
                category: String::from(category),
                requires_restart: false,
                protected: false,
            },
        );
    }

    /// Get a setting value
    pub fn get(&self, key: &str) -> Option<&SettingValue> {
        self.settings.get(key).map(|s| &s.value)
    }

    /// Get bool setting
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        match self.get(key) {
            Some(SettingValue::Bool(v)) => Some(*v),
            _ => None,
        }
    }

    /// Get int setting
    pub fn get_int(&self, key: &str) -> Option<i64> {
        match self.get(key) {
            Some(SettingValue::Int(v)) => Some(*v),
            _ => None,
        }
    }

    /// Get text setting
    pub fn get_text(&self, key: &str) -> Option<&str> {
        match self.get(key) {
            Some(SettingValue::Text(v)) => Some(v.as_str()),
            _ => None,
        }
    }

    /// Set a value
    pub fn set(&mut self, key: &str, value: SettingValue) -> bool {
        if let Some(setting) = self.settings.get_mut(key) {
            setting.value = value;
            true
        } else {
            false
        }
    }

    /// Set bool
    pub fn set_bool(&mut self, key: &str, val: bool) -> bool {
        self.set(key, SettingValue::Bool(val))
    }

    /// Set int
    pub fn set_int(&mut self, key: &str, val: i64) -> bool {
        self.set(key, SettingValue::Int(val))
    }

    /// List settings in a category
    pub fn list_category(&self, category: &str) -> Vec<(&str, &str)> {
        self.settings
            .values()
            .filter(|s| s.category == category)
            .map(|s| (s.key.as_str(), s.title.as_str()))
            .collect()
    }

    /// Search settings
    pub fn search(&self, query: &str) -> Vec<&str> {
        let q = query.to_lowercase();
        self.settings
            .values()
            .filter(|s| s.title.to_lowercase().contains(&q) || s.key.contains(&q))
            .map(|s| s.key.as_str())
            .collect()
    }

    /// Get all categories
    pub fn categories(&self) -> Vec<String> {
        let mut cats: Vec<String> = self.settings.values().map(|s| s.category.clone()).collect();
        cats.sort();
        cats.dedup();
        cats
    }
}

static SETTINGS: Mutex<SettingsStore> = Mutex::new(SettingsStore::new());

pub fn init() {
    SETTINGS.lock().setup_defaults();
    crate::serial_println!(
        "  [settings] Settings initialized ({} entries)",
        SETTINGS.lock().settings.len()
    );
}

pub fn get_bool(key: &str) -> Option<bool> {
    SETTINGS.lock().get_bool(key)
}
pub fn get_int(key: &str) -> Option<i64> {
    SETTINGS.lock().get_int(key)
}
pub fn set_bool(key: &str, val: bool) -> bool {
    SETTINGS.lock().set_bool(key, val)
}
pub fn set_int(key: &str, val: i64) -> bool {
    SETTINGS.lock().set_int(key, val)
}
