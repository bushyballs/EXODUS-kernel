use crate::sync::Mutex;
use alloc::vec;
/// OS Customization Engine — AI-driven system configuration
///
/// Lets users customize the ENTIRE Hoags OS through natural
/// conversation with the AI. Instead of digging through settings
/// menus, just tell the AI what you want:
///
///   "Make the screen brighter"
///   "Turn on dark mode"
///   "Disable Bluetooth"
///   "Set the font size to 18"
///   "I want a quieter notification sound"
///   "Lock down network access"
///
/// The AI interprets intent, maps it to system settings, and
/// applies changes — with confirmation for sensitive operations.
///
/// Features:
///   - 30+ built-in system settings across 18 domains
///   - Intent parsing from hashed user input
///   - Undo stack for reverting changes
///   - Auto-confirm for safe domains (user-configurable)
///   - Sensitive setting protection (security, privacy, network)
///   - Full history tracking for audit trail
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

use super::transformer::{q16_from_int, Q16};

// ── Domain & Action Enums ───────────────────────────────────────

/// Every configurable domain in the OS
#[derive(Clone, Copy, PartialEq)]
pub enum CustomizeDomain {
    Display,
    Audio,
    Network,
    Security,
    Privacy,
    Power,
    Storage,
    Input,
    Accessibility,
    Apps,
    Notifications,
    Theme,
    Wallpaper,
    Fonts,
    Language,
    Shortcuts,
    Automation,
    AI,
}

/// What the user wants to do
#[derive(Clone, Copy, PartialEq)]
pub enum CustomizeAction {
    Set,
    Toggle,
    Increase,
    Decrease,
    Reset,
    Enable,
    Disable,
    Configure,
    Install,
    Remove,
}

// ── Core Data Structures ────────────────────────────────────────

/// A parsed customization request from user intent
#[derive(Clone, Copy)]
pub struct CustomizeRequest {
    pub domain: CustomizeDomain,
    pub action: CustomizeAction,
    pub target_hash: u64,
    pub value: Q16,
    pub string_value_hash: u64,
    pub confirmed: bool,
    pub timestamp: u64,
}

/// Result of applying a customization
#[derive(Clone, Copy)]
pub struct CustomizeResult {
    pub success: bool,
    pub domain: CustomizeDomain,
    pub description_hash: u64,
    pub previous_value: Q16,
    pub new_value: Q16,
    pub requires_reboot: bool,
}

/// A single system setting that can be modified
#[derive(Clone, Copy)]
pub struct SystemSetting {
    pub id: u32,
    pub domain: CustomizeDomain,
    pub name_hash: u64,
    pub current_value: Q16,
    pub min_value: Q16,
    pub max_value: Q16,
    pub is_sensitive: bool,
}

/// History of all customizations for undo/audit
#[derive(Clone)]
pub struct CustomizeHistory {
    pub entries: Vec<(u64, CustomizeRequest, CustomizeResult)>,
}

/// The main OS customization engine
#[derive(Clone)]
pub struct OsCustomizer {
    pub settings: Vec<SystemSetting>,
    pub history: CustomizeHistory,
    pub pending_requests: Vec<CustomizeRequest>,
    pub auto_confirm_domains: Vec<CustomizeDomain>,
    pub total_customizations: u64,
    pub undo_stack_depth: u32,
}

// ── Well-Known Setting Name Hashes ──────────────────────────────
//
// Simple FNV-1a-style constants so we can identify settings by
// name without heap-allocated strings.

const HASH_BRIGHTNESS: u64 = 0x00A1_B2C3_D4E5_0001;
const HASH_CONTRAST: u64 = 0x00A1_B2C3_D4E5_0002;
const HASH_RESOLUTION_SCALE: u64 = 0x00A1_B2C3_D4E5_0003;
const HASH_REFRESH_RATE: u64 = 0x00A1_B2C3_D4E5_0004;
const HASH_NIGHT_MODE: u64 = 0x00A1_B2C3_D4E5_0005;
const HASH_VOLUME: u64 = 0x00A1_B2C3_D4E5_0006;
const HASH_MUTE: u64 = 0x00A1_B2C3_D4E5_0007;
const HASH_BASS_BOOST: u64 = 0x00A1_B2C3_D4E5_0008;
const HASH_WIFI_ENABLED: u64 = 0x00A1_B2C3_D4E5_0009;
const HASH_BLUETOOTH_ON: u64 = 0x00A1_B2C3_D4E5_000A;
const HASH_AIRPLANE_MODE: u64 = 0x00A1_B2C3_D4E5_000B;
const HASH_VPN_ENABLED: u64 = 0x00A1_B2C3_D4E5_000C;
const HASH_FIREWALL: u64 = 0x00A1_B2C3_D4E5_000D;
const HASH_LOCKSCREEN_TIMEOUT: u64 = 0x00A1_B2C3_D4E5_000E;
const HASH_ENCRYPTION: u64 = 0x00A1_B2C3_D4E5_000F;
const HASH_TELEMETRY: u64 = 0x00A1_B2C3_D4E5_0010;
const HASH_LOCATION_SERVICES: u64 = 0x00A1_B2C3_D4E5_0011;
const HASH_CAMERA_ACCESS: u64 = 0x00A1_B2C3_D4E5_0012;
const HASH_SCREEN_TIMEOUT: u64 = 0x00A1_B2C3_D4E5_0013;
const HASH_SLEEP_AFTER: u64 = 0x00A1_B2C3_D4E5_0014;
const HASH_PERFORMANCE_MODE: u64 = 0x00A1_B2C3_D4E5_0015;
const HASH_DARK_MODE: u64 = 0x00A1_B2C3_D4E5_0016;
const HASH_ACCENT_COLOR: u64 = 0x00A1_B2C3_D4E5_0017;
const HASH_TRANSPARENCY: u64 = 0x00A1_B2C3_D4E5_0018;
const HASH_FONT_SIZE: u64 = 0x00A1_B2C3_D4E5_0019;
const HASH_FONT_WEIGHT: u64 = 0x00A1_B2C3_D4E5_001A;
const HASH_DND_ENABLED: u64 = 0x00A1_B2C3_D4E5_001B;
const HASH_NOTIFICATION_SOUND: u64 = 0x00A1_B2C3_D4E5_001C;
const HASH_BADGE_COUNT: u64 = 0x00A1_B2C3_D4E5_001D;
const HASH_CURSOR_SIZE: u64 = 0x00A1_B2C3_D4E5_001E;
const HASH_KEY_REPEAT_RATE: u64 = 0x00A1_B2C3_D4E5_001F;
const HASH_SCROLL_SPEED: u64 = 0x00A1_B2C3_D4E5_0020;
const HASH_HIGH_CONTRAST: u64 = 0x00A1_B2C3_D4E5_0021;
const HASH_SCREEN_READER: u64 = 0x00A1_B2C3_D4E5_0022;
const HASH_AI_RESPONSE_STYLE: u64 = 0x00A1_B2C3_D4E5_0023;
const HASH_AI_VERBOSITY: u64 = 0x00A1_B2C3_D4E5_0024;
const HASH_REDUCE_MOTION: u64 = 0x00A1_B2C3_D4E5_0025;

// ── Intent Mapping Hashes ───────────────────────────────────────
//
// Hashes representing common user intents that the AI recognizes.

const INTENT_BRIGHTER: u64 = 0x00CC_AA00_0000_0001;
const INTENT_DIMMER: u64 = 0x00CC_AA00_0000_0002;
const INTENT_LOUDER: u64 = 0x00CC_AA00_0000_0003;
const INTENT_QUIETER: u64 = 0x00CC_AA00_0000_0004;
const INTENT_DARK_MODE_ON: u64 = 0x00CC_AA00_0000_0005;
const INTENT_DARK_MODE_OFF: u64 = 0x00CC_AA00_0000_0006;
const INTENT_WIFI_ON: u64 = 0x00CC_AA00_0000_0007;
const INTENT_WIFI_OFF: u64 = 0x00CC_AA00_0000_0008;
const INTENT_BLUETOOTH_ON: u64 = 0x00CC_AA00_0000_0009;
const INTENT_BLUETOOTH_OFF: u64 = 0x00CC_AA00_0000_000A;
const INTENT_DND_ON: u64 = 0x00CC_AA00_0000_000B;
const INTENT_DND_OFF: u64 = 0x00CC_AA00_0000_000C;
const INTENT_MUTE: u64 = 0x00CC_AA00_0000_000D;
const INTENT_UNMUTE: u64 = 0x00CC_AA00_0000_000E;
const INTENT_BIGGER_TEXT: u64 = 0x00CC_AA00_0000_000F;
const INTENT_SMALLER_TEXT: u64 = 0x00CC_AA00_0000_0010;
const INTENT_LOCK_DOWN: u64 = 0x00CC_AA00_0000_0011;
const INTENT_AIRPLANE_ON: u64 = 0x00CC_AA00_0000_0012;
const INTENT_AIRPLANE_OFF: u64 = 0x00CC_AA00_0000_0013;
const INTENT_NIGHT_MODE_ON: u64 = 0x00CC_AA00_0000_0014;
const INTENT_NIGHT_MODE_OFF: u64 = 0x00CC_AA00_0000_0015;

// ── Adjustment Step Size ────────────────────────────────────────
//
// When the user says "brighter" or "louder" we bump by this amount.

const STEP_SMALL: Q16 = 0x0000_2800; // ~0.15 in Q16 (10 / 65536 * 1000 ~ 152)
const STEP_MEDIUM: Q16 = 0x0000_5000; // ~0.31 in Q16

// ── Global State ────────────────────────────────────────────────

static CUSTOMIZER: Mutex<Option<OsCustomizer>> = Mutex::new(None);

// ── Helper: build a SystemSetting ───────────────────────────────

fn make_setting(
    id: u32,
    domain: CustomizeDomain,
    name_hash: u64,
    current: i32,
    min: i32,
    max: i32,
    sensitive: bool,
) -> SystemSetting {
    SystemSetting {
        id,
        domain,
        name_hash,
        current_value: q16_from_int(current),
        min_value: q16_from_int(min),
        max_value: q16_from_int(max),
        is_sensitive: sensitive,
    }
}

// ── OsCustomizer Implementation ─────────────────────────────────

impl OsCustomizer {
    /// Create a new customizer pre-populated with default settings
    pub fn new() -> Self {
        let settings = vec![
            // ── Display ──
            make_setting(
                1,
                CustomizeDomain::Display,
                HASH_BRIGHTNESS,
                70,
                0,
                100,
                false,
            ),
            make_setting(
                2,
                CustomizeDomain::Display,
                HASH_CONTRAST,
                50,
                0,
                100,
                false,
            ),
            make_setting(
                3,
                CustomizeDomain::Display,
                HASH_RESOLUTION_SCALE,
                100,
                50,
                200,
                false,
            ),
            make_setting(
                4,
                CustomizeDomain::Display,
                HASH_REFRESH_RATE,
                60,
                30,
                144,
                false,
            ),
            make_setting(5, CustomizeDomain::Display, HASH_NIGHT_MODE, 0, 0, 1, false),
            // ── Audio ──
            make_setting(6, CustomizeDomain::Audio, HASH_VOLUME, 50, 0, 100, false),
            make_setting(7, CustomizeDomain::Audio, HASH_MUTE, 0, 0, 1, false),
            make_setting(8, CustomizeDomain::Audio, HASH_BASS_BOOST, 0, 0, 1, false),
            // ── Network ──
            make_setting(
                9,
                CustomizeDomain::Network,
                HASH_WIFI_ENABLED,
                1,
                0,
                1,
                false,
            ),
            make_setting(
                10,
                CustomizeDomain::Network,
                HASH_BLUETOOTH_ON,
                1,
                0,
                1,
                false,
            ),
            make_setting(
                11,
                CustomizeDomain::Network,
                HASH_AIRPLANE_MODE,
                0,
                0,
                1,
                false,
            ),
            make_setting(
                12,
                CustomizeDomain::Network,
                HASH_VPN_ENABLED,
                0,
                0,
                1,
                true,
            ),
            // ── Security ──
            make_setting(13, CustomizeDomain::Security, HASH_FIREWALL, 1, 0, 1, true),
            make_setting(
                14,
                CustomizeDomain::Security,
                HASH_LOCKSCREEN_TIMEOUT,
                5,
                1,
                60,
                true,
            ),
            make_setting(
                15,
                CustomizeDomain::Security,
                HASH_ENCRYPTION,
                1,
                0,
                1,
                true,
            ),
            // ── Privacy ──
            make_setting(16, CustomizeDomain::Privacy, HASH_TELEMETRY, 0, 0, 1, true),
            make_setting(
                17,
                CustomizeDomain::Privacy,
                HASH_LOCATION_SERVICES,
                0,
                0,
                1,
                true,
            ),
            make_setting(
                18,
                CustomizeDomain::Privacy,
                HASH_CAMERA_ACCESS,
                1,
                0,
                1,
                true,
            ),
            // ── Power ──
            make_setting(
                19,
                CustomizeDomain::Power,
                HASH_SCREEN_TIMEOUT,
                30,
                10,
                600,
                false,
            ),
            make_setting(
                20,
                CustomizeDomain::Power,
                HASH_SLEEP_AFTER,
                300,
                60,
                3600,
                false,
            ),
            make_setting(
                21,
                CustomizeDomain::Power,
                HASH_PERFORMANCE_MODE,
                1,
                0,
                2,
                false,
            ),
            // ── Theme ──
            make_setting(22, CustomizeDomain::Theme, HASH_DARK_MODE, 1, 0, 1, false),
            make_setting(
                23,
                CustomizeDomain::Theme,
                HASH_ACCENT_COLOR,
                0,
                0,
                255,
                false,
            ),
            make_setting(
                24,
                CustomizeDomain::Theme,
                HASH_TRANSPARENCY,
                80,
                0,
                100,
                false,
            ),
            // ── Fonts ──
            make_setting(25, CustomizeDomain::Fonts, HASH_FONT_SIZE, 14, 8, 48, false),
            make_setting(
                26,
                CustomizeDomain::Fonts,
                HASH_FONT_WEIGHT,
                400,
                100,
                900,
                false,
            ),
            // ── Notifications ──
            make_setting(
                27,
                CustomizeDomain::Notifications,
                HASH_DND_ENABLED,
                0,
                0,
                1,
                false,
            ),
            make_setting(
                28,
                CustomizeDomain::Notifications,
                HASH_NOTIFICATION_SOUND,
                1,
                0,
                1,
                false,
            ),
            make_setting(
                29,
                CustomizeDomain::Notifications,
                HASH_BADGE_COUNT,
                1,
                0,
                1,
                false,
            ),
            // ── Input ──
            make_setting(30, CustomizeDomain::Input, HASH_CURSOR_SIZE, 1, 1, 5, false),
            make_setting(
                31,
                CustomizeDomain::Input,
                HASH_KEY_REPEAT_RATE,
                30,
                5,
                100,
                false,
            ),
            make_setting(
                32,
                CustomizeDomain::Input,
                HASH_SCROLL_SPEED,
                3,
                1,
                10,
                false,
            ),
            // ── Accessibility ──
            make_setting(
                33,
                CustomizeDomain::Accessibility,
                HASH_HIGH_CONTRAST,
                0,
                0,
                1,
                false,
            ),
            make_setting(
                34,
                CustomizeDomain::Accessibility,
                HASH_SCREEN_READER,
                0,
                0,
                1,
                false,
            ),
            make_setting(
                35,
                CustomizeDomain::Accessibility,
                HASH_REDUCE_MOTION,
                0,
                0,
                1,
                false,
            ),
            // ── AI ──
            make_setting(
                36,
                CustomizeDomain::AI,
                HASH_AI_RESPONSE_STYLE,
                1,
                0,
                3,
                false,
            ),
            make_setting(37, CustomizeDomain::AI, HASH_AI_VERBOSITY, 2, 0, 4, false),
        ];

        OsCustomizer {
            settings,
            history: CustomizeHistory {
                entries: Vec::new(),
            },
            pending_requests: Vec::new(),
            auto_confirm_domains: vec![
                CustomizeDomain::Display,
                CustomizeDomain::Audio,
                CustomizeDomain::Theme,
                CustomizeDomain::Fonts,
                CustomizeDomain::Notifications,
            ],
            total_customizations: 0,
            undo_stack_depth: 64,
        }
    }

    // ── Intent Parsing ──────────────────────────────────────────

    /// Map a hashed user intent to a concrete customization request.
    /// Returns `None` if the intent is not recognized.
    pub fn parse_intent(&self, intent_hash: u64) -> Option<CustomizeRequest> {
        let timestamp = self.total_customizations; // monotonic stand-in

        match intent_hash {
            INTENT_BRIGHTER => Some(CustomizeRequest {
                domain: CustomizeDomain::Display,
                action: CustomizeAction::Increase,
                target_hash: HASH_BRIGHTNESS,
                value: STEP_MEDIUM,
                string_value_hash: 0,
                confirmed: false,
                timestamp,
            }),
            INTENT_DIMMER => Some(CustomizeRequest {
                domain: CustomizeDomain::Display,
                action: CustomizeAction::Decrease,
                target_hash: HASH_BRIGHTNESS,
                value: STEP_MEDIUM,
                string_value_hash: 0,
                confirmed: false,
                timestamp,
            }),
            INTENT_LOUDER => Some(CustomizeRequest {
                domain: CustomizeDomain::Audio,
                action: CustomizeAction::Increase,
                target_hash: HASH_VOLUME,
                value: STEP_MEDIUM,
                string_value_hash: 0,
                confirmed: false,
                timestamp,
            }),
            INTENT_QUIETER => Some(CustomizeRequest {
                domain: CustomizeDomain::Audio,
                action: CustomizeAction::Decrease,
                target_hash: HASH_VOLUME,
                value: STEP_MEDIUM,
                string_value_hash: 0,
                confirmed: false,
                timestamp,
            }),
            INTENT_DARK_MODE_ON => Some(CustomizeRequest {
                domain: CustomizeDomain::Theme,
                action: CustomizeAction::Enable,
                target_hash: HASH_DARK_MODE,
                value: q16_from_int(1),
                string_value_hash: 0,
                confirmed: false,
                timestamp,
            }),
            INTENT_DARK_MODE_OFF => Some(CustomizeRequest {
                domain: CustomizeDomain::Theme,
                action: CustomizeAction::Disable,
                target_hash: HASH_DARK_MODE,
                value: q16_from_int(0),
                string_value_hash: 0,
                confirmed: false,
                timestamp,
            }),
            INTENT_WIFI_ON => Some(CustomizeRequest {
                domain: CustomizeDomain::Network,
                action: CustomizeAction::Enable,
                target_hash: HASH_WIFI_ENABLED,
                value: q16_from_int(1),
                string_value_hash: 0,
                confirmed: false,
                timestamp,
            }),
            INTENT_WIFI_OFF => Some(CustomizeRequest {
                domain: CustomizeDomain::Network,
                action: CustomizeAction::Disable,
                target_hash: HASH_WIFI_ENABLED,
                value: q16_from_int(0),
                string_value_hash: 0,
                confirmed: false,
                timestamp,
            }),
            INTENT_BLUETOOTH_ON => Some(CustomizeRequest {
                domain: CustomizeDomain::Network,
                action: CustomizeAction::Enable,
                target_hash: HASH_BLUETOOTH_ON,
                value: q16_from_int(1),
                string_value_hash: 0,
                confirmed: false,
                timestamp,
            }),
            INTENT_BLUETOOTH_OFF => Some(CustomizeRequest {
                domain: CustomizeDomain::Network,
                action: CustomizeAction::Disable,
                target_hash: HASH_BLUETOOTH_ON,
                value: q16_from_int(0),
                string_value_hash: 0,
                confirmed: false,
                timestamp,
            }),
            INTENT_DND_ON => Some(CustomizeRequest {
                domain: CustomizeDomain::Notifications,
                action: CustomizeAction::Enable,
                target_hash: HASH_DND_ENABLED,
                value: q16_from_int(1),
                string_value_hash: 0,
                confirmed: false,
                timestamp,
            }),
            INTENT_DND_OFF => Some(CustomizeRequest {
                domain: CustomizeDomain::Notifications,
                action: CustomizeAction::Disable,
                target_hash: HASH_DND_ENABLED,
                value: q16_from_int(0),
                string_value_hash: 0,
                confirmed: false,
                timestamp,
            }),
            INTENT_MUTE => Some(CustomizeRequest {
                domain: CustomizeDomain::Audio,
                action: CustomizeAction::Enable,
                target_hash: HASH_MUTE,
                value: q16_from_int(1),
                string_value_hash: 0,
                confirmed: false,
                timestamp,
            }),
            INTENT_UNMUTE => Some(CustomizeRequest {
                domain: CustomizeDomain::Audio,
                action: CustomizeAction::Disable,
                target_hash: HASH_MUTE,
                value: q16_from_int(0),
                string_value_hash: 0,
                confirmed: false,
                timestamp,
            }),
            INTENT_BIGGER_TEXT => Some(CustomizeRequest {
                domain: CustomizeDomain::Fonts,
                action: CustomizeAction::Increase,
                target_hash: HASH_FONT_SIZE,
                value: q16_from_int(2),
                string_value_hash: 0,
                confirmed: false,
                timestamp,
            }),
            INTENT_SMALLER_TEXT => Some(CustomizeRequest {
                domain: CustomizeDomain::Fonts,
                action: CustomizeAction::Decrease,
                target_hash: HASH_FONT_SIZE,
                value: q16_from_int(2),
                string_value_hash: 0,
                confirmed: false,
                timestamp,
            }),
            INTENT_LOCK_DOWN => Some(CustomizeRequest {
                domain: CustomizeDomain::Security,
                action: CustomizeAction::Configure,
                target_hash: HASH_FIREWALL,
                value: q16_from_int(1),
                string_value_hash: 0,
                confirmed: false,
                timestamp,
            }),
            INTENT_AIRPLANE_ON => Some(CustomizeRequest {
                domain: CustomizeDomain::Network,
                action: CustomizeAction::Enable,
                target_hash: HASH_AIRPLANE_MODE,
                value: q16_from_int(1),
                string_value_hash: 0,
                confirmed: false,
                timestamp,
            }),
            INTENT_AIRPLANE_OFF => Some(CustomizeRequest {
                domain: CustomizeDomain::Network,
                action: CustomizeAction::Disable,
                target_hash: HASH_AIRPLANE_MODE,
                value: q16_from_int(0),
                string_value_hash: 0,
                confirmed: false,
                timestamp,
            }),
            INTENT_NIGHT_MODE_ON => Some(CustomizeRequest {
                domain: CustomizeDomain::Display,
                action: CustomizeAction::Enable,
                target_hash: HASH_NIGHT_MODE,
                value: q16_from_int(1),
                string_value_hash: 0,
                confirmed: false,
                timestamp,
            }),
            INTENT_NIGHT_MODE_OFF => Some(CustomizeRequest {
                domain: CustomizeDomain::Display,
                action: CustomizeAction::Disable,
                target_hash: HASH_NIGHT_MODE,
                value: q16_from_int(0),
                string_value_hash: 0,
                confirmed: false,
                timestamp,
            }),
            _ => None,
        }
    }

    // ── Setting Lookup ──────────────────────────────────────────

    /// Find a setting by its name hash
    pub fn get_setting(&self, name: u64) -> Option<&SystemSetting> {
        self.settings.iter().find(|s| s.name_hash == name)
    }

    /// Find a setting's index by name hash
    fn setting_index(&self, name: u64) -> Option<usize> {
        self.settings.iter().position(|s| s.name_hash == name)
    }

    // ── Mutators ────────────────────────────────────────────────

    /// Set a setting to an exact Q16 value. Returns false if not found
    /// or the value is out of range.
    pub fn set_setting(&mut self, name: u64, value: Q16) -> bool {
        if let Some(idx) = self.setting_index(name) {
            let setting = &self.settings[idx];
            if value < setting.min_value || value > setting.max_value {
                return false;
            }
            self.settings[idx].current_value = value;
            true
        } else {
            false
        }
    }

    /// Toggle a boolean (0/1) setting. Returns false if not found
    /// or the setting is not boolean-ranged.
    pub fn toggle_setting(&mut self, name: u64) -> bool {
        if let Some(idx) = self.setting_index(name) {
            let setting = &self.settings[idx];
            // Only toggle settings whose range is 0..1
            if setting.max_value != q16_from_int(1) || setting.min_value != q16_from_int(0) {
                return false;
            }
            let cur = setting.current_value;
            self.settings[idx].current_value = if cur == q16_from_int(0) {
                q16_from_int(1)
            } else {
                q16_from_int(0)
            };
            true
        } else {
            false
        }
    }

    /// Clamp a Q16 value to [min, max]
    fn clamp(val: Q16, min: Q16, max: Q16) -> Q16 {
        if val < min {
            min
        } else if val > max {
            max
        } else {
            val
        }
    }

    // ── Apply a Request ─────────────────────────────────────────

    /// Apply a customization request and return the result.
    pub fn apply(&mut self, req: &CustomizeRequest) -> CustomizeResult {
        let idx = match self.setting_index(req.target_hash) {
            Some(i) => i,
            None => {
                return CustomizeResult {
                    success: false,
                    domain: req.domain,
                    description_hash: req.target_hash,
                    previous_value: q16_from_int(0),
                    new_value: q16_from_int(0),
                    requires_reboot: false,
                };
            }
        };

        let previous_value = self.settings[idx].current_value;
        let min = self.settings[idx].min_value;
        let max = self.settings[idx].max_value;

        let new_value = match req.action {
            CustomizeAction::Set => Self::clamp(req.value, min, max),
            CustomizeAction::Enable => q16_from_int(1),
            CustomizeAction::Disable => q16_from_int(0),
            CustomizeAction::Toggle => {
                if previous_value == q16_from_int(0) {
                    q16_from_int(1)
                } else {
                    q16_from_int(0)
                }
            }
            CustomizeAction::Increase => {
                let stepped = previous_value + req.value;
                Self::clamp(stepped, min, max)
            }
            CustomizeAction::Decrease => {
                let stepped = previous_value - req.value;
                Self::clamp(stepped, min, max)
            }
            CustomizeAction::Reset => {
                // Reset to midpoint of range
                let mid = (min >> 1) + (max >> 1);
                mid
            }
            CustomizeAction::Configure => Self::clamp(req.value, min, max),
            CustomizeAction::Install | CustomizeAction::Remove => {
                // These actions don't change numeric settings directly
                return CustomizeResult {
                    success: false,
                    domain: req.domain,
                    description_hash: req.target_hash,
                    previous_value,
                    new_value: previous_value,
                    requires_reboot: false,
                };
            }
        };

        self.settings[idx].current_value = new_value;
        self.total_customizations = self.total_customizations.saturating_add(1);

        // Determine if a reboot is needed (security & encryption changes)
        let requires_reboot = matches!(req.domain, CustomizeDomain::Security)
            && (req.target_hash == HASH_ENCRYPTION || req.target_hash == HASH_FIREWALL);

        let result = CustomizeResult {
            success: true,
            domain: req.domain,
            description_hash: req.target_hash,
            previous_value,
            new_value,
            requires_reboot,
        };

        // Record in history for undo (trim to stack depth)
        self.history.entries.push((req.timestamp, *req, result));
        if self.history.entries.len() > self.undo_stack_depth as usize {
            self.history.entries.remove(0);
        }

        result
    }

    // ── Undo ────────────────────────────────────────────────────

    /// Undo the most recent customization. Returns the reversal result,
    /// or `None` if there is nothing to undo.
    pub fn undo_last(&mut self) -> Option<CustomizeResult> {
        let entry = self.history.entries.pop()?;
        let (_ts, original_req, original_result) = entry;

        // Restore the previous value
        if let Some(idx) = self.setting_index(original_req.target_hash) {
            let before_undo = self.settings[idx].current_value;
            self.settings[idx].current_value = original_result.previous_value;

            Some(CustomizeResult {
                success: true,
                domain: original_req.domain,
                description_hash: original_req.target_hash,
                previous_value: before_undo,
                new_value: original_result.previous_value,
                requires_reboot: original_result.requires_reboot,
            })
        } else {
            None
        }
    }

    // ── Domain Listing ──────────────────────────────────────────

    /// Get all settings belonging to a specific domain.
    pub fn list_domain(&self, domain: CustomizeDomain) -> Vec<&SystemSetting> {
        self.settings
            .iter()
            .filter(|s| s.domain == domain)
            .collect()
    }

    // ── Auto-Confirm Management ─────────────────────────────────

    /// Add a domain to the auto-confirm list. Changes in auto-confirm
    /// domains do not require user confirmation.
    pub fn auto_confirm(&mut self, domain: CustomizeDomain) {
        if !self.auto_confirm_domains.iter().any(|d| *d == domain) {
            self.auto_confirm_domains.push(domain);
        }
    }

    /// Check if a request requires explicit user confirmation.
    /// Sensitive settings always need confirmation regardless of
    /// the auto-confirm list.
    pub fn needs_confirmation(&self, req: &CustomizeRequest) -> bool {
        // Sensitive settings always need confirmation
        if let Some(setting) = self.get_setting(req.target_hash) {
            if setting.is_sensitive {
                return true;
            }
        }

        // If the domain is in auto-confirm, no confirmation needed
        if self.auto_confirm_domains.iter().any(|d| *d == req.domain) {
            return false;
        }

        // Everything else needs confirmation
        true
    }

    // ── Statistics ───────────────────────────────────────────────

    /// How many customizations are recorded in the undo history.
    pub fn get_history_count(&self) -> u32 {
        self.history.entries.len() as u32
    }

    /// Total number of settings registered.
    pub fn setting_count(&self) -> u32 {
        self.settings.len() as u32
    }

    /// Total number of pending requests awaiting confirmation.
    pub fn pending_count(&self) -> u32 {
        self.pending_requests.len() as u32
    }

    /// Process all pending requests that do not need confirmation.
    /// Returns the number of requests applied.
    pub fn flush_auto_confirmed(&mut self) -> u32 {
        let mut applied = 0u32;

        // Drain pending requests that are safe to auto-confirm
        let mut i = 0;
        while i < self.pending_requests.len() {
            let req = self.pending_requests[i];
            if !self.needs_confirmation(&req) {
                self.pending_requests.remove(i);
                let mut confirmed = req;
                confirmed.confirmed = true;
                self.apply(&confirmed);
                applied += 1;
            } else {
                i += 1;
            }
        }

        applied
    }

    /// Queue a request for later processing. If the domain is
    /// auto-confirmed and the setting is not sensitive, apply
    /// it immediately.
    pub fn submit(&mut self, req: CustomizeRequest) -> Option<CustomizeResult> {
        if !self.needs_confirmation(&req) {
            let mut confirmed = req;
            confirmed.confirmed = true;
            Some(self.apply(&confirmed))
        } else {
            self.pending_requests.push(req);
            None
        }
    }

    /// Confirm and apply a pending request at the given index.
    pub fn confirm_pending(&mut self, index: usize) -> Option<CustomizeResult> {
        if index >= self.pending_requests.len() {
            return None;
        }
        let mut req = self.pending_requests.remove(index);
        req.confirmed = true;
        Some(self.apply(&req))
    }

    /// Reject and discard a pending request at the given index.
    pub fn reject_pending(&mut self, index: usize) -> bool {
        if index >= self.pending_requests.len() {
            return false;
        }
        self.pending_requests.remove(index);
        true
    }

    /// Compute a summary score for how customized the system is
    /// relative to defaults. Returns a Q16 value from 0 (stock)
    /// to q16_from_int(100) (heavily customized).
    pub fn customization_score(&self) -> Q16 {
        if self.total_customizations == 0 {
            return q16_from_int(0);
        }
        // Simple heuristic: min(total_changes * 3, 100)
        let raw = self.total_customizations as i32 * 3;
        let capped = if raw > 100 { 100 } else { raw };
        q16_from_int(capped)
    }
}

// ── Module Initialization ───────────────────────────────────────

/// Initialize the OS customization engine with default settings.
pub fn init() {
    let customizer = OsCustomizer::new();
    let count = customizer.setting_count();
    let auto_count = customizer.auto_confirm_domains.len() as u32;

    let mut lock = CUSTOMIZER.lock();
    *lock = Some(customizer);

    serial_println!(
        "  OS customizer initialized ({} settings, {} auto-confirm domains)",
        count,
        auto_count
    );
}
