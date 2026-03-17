use super::schema::PrefSchema;
use super::store::int_to_q16;
use crate::sync::Mutex;
/// Built-in system preferences for Genesis
///
/// Provides default values and schemas for all core system preferences:
///   - Display: brightness, resolution, refresh rate, theme, DPI, night mode
///   - Sound: master volume, notification volume, media volume, output device
///   - Network: DNS, proxy, metered connections, Wi-Fi preferences
///   - Privacy: location access, camera access, telemetry, ad tracking
///   - Security: lock timeout, biometric auth, app install sources, encryption
///   - General: locale, timezone, hostname, auto-update
///
/// All numeric values use plain i32 or Q16 fixed-point. No floats.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// System preference category identifiers
pub const CAT_DISPLAY: &str = "display";
pub const CAT_SOUND: &str = "sound";
pub const CAT_NETWORK: &str = "network";
pub const CAT_PRIVACY: &str = "privacy";
pub const CAT_SECURITY: &str = "security";
pub const CAT_GENERAL: &str = "general";
pub const CAT_POWER: &str = "power";
pub const CAT_INPUT: &str = "input";
pub const CAT_ACCESSIBILITY: &str = "accessibility";

/// Build all display preference schemas
fn display_schemas() -> Vec<PrefSchema> {
    vec![
        PrefSchema::int("display.brightness", 80, "Screen brightness percentage")
            .with_range(0, 100)
            .with_category(CAT_DISPLAY),
        PrefSchema::boolean(
            "display.auto_brightness",
            true,
            "Automatic brightness adjustment",
        )
        .with_category(CAT_DISPLAY),
        PrefSchema::int("display.width", 1920, "Horizontal resolution in pixels")
            .with_range(640, 7680)
            .with_category(CAT_DISPLAY)
            .restart_required(),
        PrefSchema::int("display.height", 1080, "Vertical resolution in pixels")
            .with_range(480, 4320)
            .with_category(CAT_DISPLAY)
            .restart_required(),
        PrefSchema::int("display.refresh_rate", 60, "Display refresh rate in Hz")
            .with_range(30, 360)
            .with_category(CAT_DISPLAY),
        PrefSchema::string("display.theme", "dark", "UI theme")
            .with_allowed(&["dark", "light", "auto", "high_contrast"])
            .with_category(CAT_DISPLAY),
        PrefSchema::int("display.dpi", 96, "Display DPI scaling")
            .with_range(72, 384)
            .with_category(CAT_DISPLAY)
            .restart_required(),
        PrefSchema::boolean(
            "display.night_mode",
            false,
            "Night mode (blue light filter)",
        )
        .with_category(CAT_DISPLAY),
        PrefSchema::int(
            "display.night_mode_start",
            2200,
            "Night mode start time (HHMM)",
        )
        .with_range(0, 2359)
        .with_category(CAT_DISPLAY),
        PrefSchema::int("display.night_mode_end", 600, "Night mode end time (HHMM)")
            .with_range(0, 2359)
            .with_category(CAT_DISPLAY),
        PrefSchema::q16(
            "display.night_warmth",
            int_to_q16(50),
            "Night mode color warmth",
        )
        .with_range(int_to_q16(0), int_to_q16(100))
        .with_category(CAT_DISPLAY),
        PrefSchema::int("display.font_size", 14, "Default font size in points")
            .with_range(8, 48)
            .with_category(CAT_DISPLAY),
        PrefSchema::boolean("display.vsync", true, "Vertical sync enabled")
            .with_category(CAT_DISPLAY),
        PrefSchema::string("display.color_profile", "srgb", "Color profile")
            .with_allowed(&["srgb", "p3", "adobe_rgb", "native"])
            .with_category(CAT_DISPLAY),
    ]
}

/// Build all sound preference schemas
fn sound_schemas() -> Vec<PrefSchema> {
    vec![
        PrefSchema::int("sound.master_volume", 80, "Master volume percentage")
            .with_range(0, 100)
            .with_category(CAT_SOUND),
        PrefSchema::int("sound.media_volume", 70, "Media playback volume")
            .with_range(0, 100)
            .with_category(CAT_SOUND),
        PrefSchema::int("sound.notification_volume", 60, "Notification sound volume")
            .with_range(0, 100)
            .with_category(CAT_SOUND),
        PrefSchema::int("sound.alarm_volume", 90, "Alarm volume")
            .with_range(0, 100)
            .with_category(CAT_SOUND),
        PrefSchema::int("sound.call_volume", 75, "Phone/VOIP call volume")
            .with_range(0, 100)
            .with_category(CAT_SOUND),
        PrefSchema::boolean("sound.mute", false, "System-wide mute").with_category(CAT_SOUND),
        PrefSchema::boolean("sound.haptic_feedback", true, "Haptic vibration feedback")
            .with_category(CAT_SOUND),
        PrefSchema::string("sound.output_device", "auto", "Active audio output device")
            .with_allowed(&["auto", "speaker", "headphones", "bluetooth", "hdmi", "usb"])
            .with_category(CAT_SOUND),
        PrefSchema::string("sound.input_device", "auto", "Active audio input device")
            .with_allowed(&[
                "auto",
                "built_in_mic",
                "headset_mic",
                "usb_mic",
                "bluetooth_mic",
            ])
            .with_category(CAT_SOUND),
        PrefSchema::int("sound.sample_rate", 48000, "Audio sample rate in Hz")
            .with_range(8000, 192000)
            .with_category(CAT_SOUND)
            .restart_required(),
        PrefSchema::boolean("sound.do_not_disturb", false, "Do not disturb mode")
            .with_category(CAT_SOUND),
        PrefSchema::boolean("sound.spatial_audio", false, "Spatial audio processing")
            .with_category(CAT_SOUND),
    ]
}

/// Build all network preference schemas
fn network_schemas() -> Vec<PrefSchema> {
    vec![
        PrefSchema::string("network.hostname", "hoags-os", "System hostname")
            .with_category(CAT_NETWORK)
            .restart_required(),
        PrefSchema::list(
            "network.dns_servers",
            vec![String::from("1.1.1.1"), String::from("8.8.8.8")],
            "DNS server addresses",
        )
        .with_max_list_len(8)
        .with_category(CAT_NETWORK),
        PrefSchema::boolean("network.wifi_enabled", true, "Wi-Fi radio enabled")
            .with_category(CAT_NETWORK),
        PrefSchema::boolean("network.bluetooth_enabled", true, "Bluetooth radio enabled")
            .with_category(CAT_NETWORK),
        PrefSchema::boolean(
            "network.airplane_mode",
            false,
            "Airplane mode (all radios off)",
        )
        .with_category(CAT_NETWORK),
        PrefSchema::boolean(
            "network.metered_warning",
            true,
            "Warn on metered connections",
        )
        .with_category(CAT_NETWORK),
        PrefSchema::boolean(
            "network.auto_connect_wifi",
            true,
            "Auto-connect to known networks",
        )
        .with_category(CAT_NETWORK),
        PrefSchema::boolean("network.firewall_enabled", true, "System firewall active")
            .with_category(CAT_NETWORK),
        PrefSchema::string("network.proxy_mode", "none", "Proxy configuration mode")
            .with_allowed(&["none", "manual", "auto", "system"])
            .with_category(CAT_NETWORK),
        PrefSchema::boolean(
            "network.vpn_always_on",
            false,
            "VPN always-on (kill switch)",
        )
        .with_category(CAT_NETWORK),
        PrefSchema::boolean("network.ipv6_enabled", true, "IPv6 protocol support")
            .with_category(CAT_NETWORK),
    ]
}

/// Build all privacy preference schemas
fn privacy_schemas() -> Vec<PrefSchema> {
    vec![
        PrefSchema::boolean("privacy.location_enabled", false, "Global location access")
            .with_category(CAT_PRIVACY),
        PrefSchema::string(
            "privacy.location_accuracy",
            "approximate",
            "Location precision level",
        )
        .with_allowed(&["exact", "approximate", "city", "disabled"])
        .with_category(CAT_PRIVACY),
        PrefSchema::boolean("privacy.camera_enabled", true, "Global camera access")
            .with_category(CAT_PRIVACY),
        PrefSchema::boolean(
            "privacy.microphone_enabled",
            true,
            "Global microphone access",
        )
        .with_category(CAT_PRIVACY),
        PrefSchema::boolean(
            "privacy.telemetry_enabled",
            false,
            "Anonymous usage telemetry",
        )
        .with_category(CAT_PRIVACY),
        PrefSchema::boolean(
            "privacy.crash_reports",
            true,
            "Automatic crash report submission",
        )
        .with_category(CAT_PRIVACY),
        PrefSchema::boolean("privacy.ad_tracking", false, "Advertising ID tracking")
            .with_category(CAT_PRIVACY),
        PrefSchema::boolean(
            "privacy.clipboard_access_notify",
            true,
            "Notify on clipboard access",
        )
        .with_category(CAT_PRIVACY),
        PrefSchema::boolean("privacy.contacts_access", false, "Global contacts access")
            .with_category(CAT_PRIVACY),
        PrefSchema::boolean("privacy.calendar_access", false, "Global calendar access")
            .with_category(CAT_PRIVACY),
        PrefSchema::string(
            "privacy.sensor_access",
            "ask",
            "Sensor access default policy",
        )
        .with_allowed(&["allow", "ask", "deny"])
        .with_category(CAT_PRIVACY),
        PrefSchema::boolean(
            "privacy.screen_capture_notify",
            true,
            "Notify on screen capture",
        )
        .with_category(CAT_PRIVACY),
    ]
}

/// Build all security preference schemas
fn security_schemas() -> Vec<PrefSchema> {
    vec![
        PrefSchema::int("security.lock_timeout", 300, "Auto-lock timeout in seconds")
            .with_range(0, 3600)
            .with_category(CAT_SECURITY),
        PrefSchema::string("security.lock_method", "pin", "Screen lock method")
            .with_allowed(&["none", "pin", "password", "pattern", "biometric"])
            .with_category(CAT_SECURITY),
        PrefSchema::boolean(
            "security.biometric_enabled",
            true,
            "Biometric authentication",
        )
        .with_category(CAT_SECURITY),
        PrefSchema::boolean("security.fingerprint_unlock", true, "Fingerprint unlock")
            .with_category(CAT_SECURITY),
        PrefSchema::boolean("security.face_unlock", false, "Face recognition unlock")
            .with_category(CAT_SECURITY),
        PrefSchema::string(
            "security.install_source",
            "store_only",
            "App installation source policy",
        )
        .with_allowed(&["store_only", "store_and_verified", "any"])
        .with_category(CAT_SECURITY),
        PrefSchema::boolean("security.encryption_enabled", true, "Full-disk encryption")
            .with_category(CAT_SECURITY)
            .restart_required(),
        PrefSchema::boolean(
            "security.secure_boot",
            true,
            "Secure boot chain verification",
        )
        .with_category(CAT_SECURITY)
        .restart_required()
        .hide(),
        PrefSchema::boolean("security.usb_debugging", false, "USB debugging mode")
            .with_category(CAT_SECURITY),
        PrefSchema::boolean(
            "security.show_passwords",
            false,
            "Show password characters briefly",
        )
        .with_category(CAT_SECURITY),
        PrefSchema::int(
            "security.failed_attempts_wipe",
            10,
            "Wipe after N failed unlock attempts",
        )
        .with_range(3, 50)
        .with_category(CAT_SECURITY),
        PrefSchema::boolean("security.auto_update", true, "Automatic security updates")
            .with_category(CAT_SECURITY),
    ]
}

/// Build all general preference schemas
fn general_schemas() -> Vec<PrefSchema> {
    vec![
        PrefSchema::string("general.locale", "en_US", "System locale")
            .with_allowed(&[
                "en_US", "en_GB", "es_ES", "fr_FR", "de_DE", "ja_JP", "zh_CN", "ko_KR", "pt_BR",
                "ar_SA",
            ])
            .with_category(CAT_GENERAL)
            .restart_required(),
        PrefSchema::string("general.timezone", "America/Los_Angeles", "System timezone")
            .with_category(CAT_GENERAL),
        PrefSchema::string("general.date_format", "yyyy-mm-dd", "Date display format")
            .with_allowed(&["yyyy-mm-dd", "mm/dd/yyyy", "dd/mm/yyyy", "dd.mm.yyyy"])
            .with_category(CAT_GENERAL),
        PrefSchema::boolean("general.24h_clock", false, "24-hour time format")
            .with_category(CAT_GENERAL),
        PrefSchema::string(
            "general.update_channel",
            "stable",
            "Software update channel",
        )
        .with_allowed(&["stable", "beta", "dev", "canary"])
        .with_category(CAT_GENERAL),
        PrefSchema::boolean("general.auto_update", true, "Automatic OS updates")
            .with_category(CAT_GENERAL),
    ]
}

/// Build all power preference schemas
fn power_schemas() -> Vec<PrefSchema> {
    vec![
        PrefSchema::int("power.screen_timeout", 300, "Screen off timeout in seconds")
            .with_range(15, 3600)
            .with_category(CAT_POWER),
        PrefSchema::int(
            "power.sleep_timeout",
            600,
            "System sleep timeout in seconds",
        )
        .with_range(60, 7200)
        .with_category(CAT_POWER),
        PrefSchema::boolean("power.lid_close_sleep", true, "Sleep on lid close")
            .with_category(CAT_POWER),
        PrefSchema::string("power.profile", "balanced", "Power management profile")
            .with_allowed(&["performance", "balanced", "power_saver", "ultra_saver"])
            .with_category(CAT_POWER),
        PrefSchema::boolean(
            "power.battery_saver_auto",
            true,
            "Auto battery saver at low charge",
        )
        .with_category(CAT_POWER),
        PrefSchema::int(
            "power.battery_saver_threshold",
            20,
            "Battery saver activation percentage",
        )
        .with_range(5, 50)
        .with_category(CAT_POWER),
    ]
}

/// Build all input preference schemas
fn input_schemas() -> Vec<PrefSchema> {
    vec![
        PrefSchema::string("input.keyboard_layout", "qwerty_us", "Keyboard layout")
            .with_allowed(&[
                "qwerty_us",
                "qwerty_uk",
                "dvorak",
                "colemak",
                "azerty",
                "qwertz",
            ])
            .with_category(CAT_INPUT),
        PrefSchema::int("input.key_repeat_delay", 500, "Key repeat delay in ms")
            .with_range(100, 2000)
            .with_category(CAT_INPUT),
        PrefSchema::int(
            "input.key_repeat_rate",
            30,
            "Key repeat rate (keys per second)",
        )
        .with_range(1, 100)
        .with_category(CAT_INPUT),
        PrefSchema::q16(
            "input.mouse_sensitivity",
            int_to_q16(50),
            "Mouse pointer sensitivity",
        )
        .with_range(int_to_q16(1), int_to_q16(100))
        .with_category(CAT_INPUT),
        PrefSchema::boolean(
            "input.natural_scroll",
            false,
            "Natural (inverted) scrolling",
        )
        .with_category(CAT_INPUT),
        PrefSchema::boolean("input.tap_to_click", true, "Trackpad tap-to-click")
            .with_category(CAT_INPUT),
        PrefSchema::q16(
            "input.scroll_speed",
            int_to_q16(3),
            "Scroll speed multiplier",
        )
        .with_range(int_to_q16(1), int_to_q16(10))
        .with_category(CAT_INPUT),
    ]
}

/// Build all accessibility preference schemas
fn accessibility_schemas() -> Vec<PrefSchema> {
    vec![
        PrefSchema::boolean(
            "accessibility.screen_reader",
            false,
            "Screen reader enabled",
        )
        .with_category(CAT_ACCESSIBILITY),
        PrefSchema::boolean("accessibility.magnification", false, "Screen magnification")
            .with_category(CAT_ACCESSIBILITY),
        PrefSchema::int(
            "accessibility.magnification_level",
            2,
            "Magnification zoom level",
        )
        .with_range(1, 16)
        .with_category(CAT_ACCESSIBILITY),
        PrefSchema::string(
            "accessibility.color_correction",
            "none",
            "Color correction mode",
        )
        .with_allowed(&[
            "none",
            "protanopia",
            "deuteranopia",
            "tritanopia",
            "grayscale",
        ])
        .with_category(CAT_ACCESSIBILITY),
        PrefSchema::boolean("accessibility.captions", false, "Live captions enabled")
            .with_category(CAT_ACCESSIBILITY),
        PrefSchema::int("accessibility.caption_font_size", 16, "Caption font size")
            .with_range(10, 48)
            .with_category(CAT_ACCESSIBILITY),
        PrefSchema::boolean("accessibility.high_contrast", false, "High contrast mode")
            .with_category(CAT_ACCESSIBILITY),
        PrefSchema::boolean(
            "accessibility.reduce_motion",
            false,
            "Reduce motion/animation",
        )
        .with_category(CAT_ACCESSIBILITY),
    ]
}

/// All system preference schemas combined
pub fn all_schemas() -> Vec<PrefSchema> {
    let mut all = Vec::new();
    all.extend(display_schemas());
    all.extend(sound_schemas());
    all.extend(network_schemas());
    all.extend(privacy_schemas());
    all.extend(security_schemas());
    all.extend(general_schemas());
    all.extend(power_schemas());
    all.extend(input_schemas());
    all.extend(accessibility_schemas());
    all
}

/// System preferences state
pub struct SystemPrefs {
    /// Whether system defaults have been applied to the store
    pub defaults_applied: bool,
    /// Number of system preference schemas registered
    pub schema_count: u32,
    /// Categories available
    pub categories: Vec<String>,
}

impl SystemPrefs {
    pub fn new() -> Self {
        Self {
            defaults_applied: false,
            schema_count: 0,
            categories: vec![
                String::from(CAT_DISPLAY),
                String::from(CAT_SOUND),
                String::from(CAT_NETWORK),
                String::from(CAT_PRIVACY),
                String::from(CAT_SECURITY),
                String::from(CAT_GENERAL),
                String::from(CAT_POWER),
                String::from(CAT_INPUT),
                String::from(CAT_ACCESSIBILITY),
            ],
        }
    }

    /// Apply all system default values to the preference store
    pub fn apply_defaults(&mut self) {
        let schemas = all_schemas();

        // Register schemas in the schema registry
        if let Some(registry) = super::schema::get_registry().lock().as_mut() {
            let count = registry.register_batch(schemas.clone());
            self.schema_count = count;
        }

        // Register namespaces in the store
        if let Some(store) = super::store::get_store().lock().as_mut() {
            store.register_namespace("display", "Display settings", false);
            store.register_namespace("sound", "Audio settings", false);
            store.register_namespace("network", "Network configuration", false);
            store.register_namespace("privacy", "Privacy controls", false);
            store.register_namespace("security", "Security settings", false);
            store.register_namespace("general", "General system settings", false);
            store.register_namespace("power", "Power management", false);
            store.register_namespace("input", "Input devices", false);
            store.register_namespace("accessibility", "Accessibility features", false);

            // Apply default values
            for schema in &schemas {
                store.set(schema.key.as_str(), schema.default.clone(), false);
            }
        }

        self.defaults_applied = true;
        serial_println!(
            "[SYSPREFS] Applied {} system defaults across {} categories",
            self.schema_count,
            self.categories.len()
        );
    }

    /// List all preference categories
    pub fn list_categories(&self) -> &[String] {
        &self.categories
    }
}

static SYSTEM_PREFS: Mutex<Option<SystemPrefs>> = Mutex::new(None);

/// Initialize system preferences with all defaults
pub fn init() {
    let mut prefs = SystemPrefs::new();
    prefs.apply_defaults();

    let mut lock = SYSTEM_PREFS.lock();
    *lock = Some(prefs);
    serial_println!("[SYSPREFS] System preferences initialized");
}

/// Get a reference to the global system preferences
pub fn get_system_prefs() -> &'static Mutex<Option<SystemPrefs>> {
    &SYSTEM_PREFS
}
