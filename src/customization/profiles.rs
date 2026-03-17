use crate::sync::Mutex;
/// User customization profiles for Genesis
///
/// Save, load, and switch between profiles (work, gaming, media).
/// Auto-switch rules based on time-of-day, connected peripherals,
/// or running applications. Profile inheritance and layering.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum ProfileMode {
    Default,
    Work,
    Gaming,
    Media,
    Presentation,
    Reading,
    Custom,
}

#[derive(Clone, Copy, PartialEq)]
pub enum AutoSwitchTrigger {
    TimeOfDay,
    AppLaunched,
    PeripheralConnected,
    NetworkChanged,
    BatteryLevel,
    Manual,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ProfilePriority {
    Low,
    Normal,
    High,
    Override,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SettingCategory {
    Display,
    Audio,
    Input,
    Network,
    Power,
    Notifications,
    Shortcuts,
    Layout,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ProfileState {
    Inactive,
    Active,
    Transitioning,
    Locked,
}

// ---------------------------------------------------------------------------
// Q16 helpers
// ---------------------------------------------------------------------------

const Q16_SHIFT: i32 = 16;
const Q16_ONE: i32 = 1 << Q16_SHIFT;

fn q16_from_int(v: i32) -> i32 {
    v << Q16_SHIFT
}

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct ProfileSetting {
    category: SettingCategory,
    key_hash: u64,
    value_i32: i32,
    value_bool: bool,
    enabled: bool,
}

#[derive(Clone, Copy)]
struct AutoSwitchRule {
    id: u32,
    trigger: AutoSwitchTrigger,
    profile_id: u32,
    param_a: u32, // trigger-specific (e.g., hour for TimeOfDay, app_id for AppLaunched)
    param_b: u32, // trigger-specific (e.g., end hour, peripheral type)
    priority: ProfilePriority,
    enabled: bool,
    trigger_count: u32,
}

#[derive(Clone, Copy)]
struct DisplayOverride {
    brightness_q16: i32, // 0..Q16_ONE
    night_mode: bool,
    night_warmth_q16: i32,
    refresh_rate: u16,
    hdr_enabled: bool,
    resolution_w: u16,
    resolution_h: u16,
}

#[derive(Clone, Copy)]
struct AudioOverride {
    master_volume_q16: i32,
    media_volume_q16: i32,
    notification_volume_q16: i32,
    do_not_disturb: bool,
    output_device_id: u32,
    spatial_audio: bool,
    eq_preset: u8,
}

#[derive(Clone, Copy)]
struct PowerOverride {
    performance_mode: u8, // 0=power_save, 1=balanced, 2=performance
    screen_timeout_sec: u16,
    sleep_timeout_min: u16,
    cpu_governor: u8, // 0=conservative, 1=ondemand, 2=performance
    gpu_boost: bool,
    wifi_power_save: bool,
}

#[derive(Clone, Copy)]
struct NotificationOverride {
    enabled: bool,
    silent_mode: bool,
    priority_only: bool,
    badge_enabled: bool,
    popup_enabled: bool,
    sound_enabled: bool,
    vibration_enabled: bool,
}

struct Profile {
    id: u32,
    mode: ProfileMode,
    name_hash: u64,
    state: ProfileState,
    parent_id: u32, // 0 = no parent (inheritance)
    settings: Vec<ProfileSetting>,
    display: DisplayOverride,
    audio: AudioOverride,
    power: PowerOverride,
    notifications: NotificationOverride,
    creation_time: u64,
    last_activated: u64,
    activation_count: u32,
    locked: bool,
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

struct ProfileManager {
    profiles: Vec<Profile>,
    rules: Vec<AutoSwitchRule>,
    active_profile_id: u32,
    previous_profile_id: u32,
    next_profile_id: u32,
    next_rule_id: u32,
    auto_switch_enabled: bool,
    transition_duration_ms: u32,
    current_hour: u8,
    current_battery: u8,
}

static PROFILES: Mutex<Option<ProfileManager>> = Mutex::new(None);

impl ProfileManager {
    fn new() -> Self {
        ProfileManager {
            profiles: Vec::new(),
            rules: Vec::new(),
            active_profile_id: 0,
            previous_profile_id: 0,
            next_profile_id: 1,
            next_rule_id: 1,
            auto_switch_enabled: true,
            transition_duration_ms: 300,
            current_hour: 12,
            current_battery: 100,
        }
    }

    fn create_profile(&mut self, mode: ProfileMode, name_hash: u64, timestamp: u64) -> u32 {
        if self.profiles.len() >= 32 {
            return 0;
        }

        let id = self.next_profile_id;
        self.next_profile_id = self.next_profile_id.saturating_add(1);

        let display = DisplayOverride {
            brightness_q16: Q16_ONE >> 1,
            night_mode: false,
            night_warmth_q16: q16_from_int(50),
            refresh_rate: 60,
            hdr_enabled: false,
            resolution_w: 1920,
            resolution_h: 1080,
        };

        let audio = AudioOverride {
            master_volume_q16: Q16_ONE >> 1,
            media_volume_q16: Q16_ONE >> 1,
            notification_volume_q16: Q16_ONE >> 2,
            do_not_disturb: false,
            output_device_id: 0,
            spatial_audio: false,
            eq_preset: 0,
        };

        let power = PowerOverride {
            performance_mode: 1,
            screen_timeout_sec: 300,
            sleep_timeout_min: 15,
            cpu_governor: 1,
            gpu_boost: false,
            wifi_power_save: true,
        };

        let notifications = NotificationOverride {
            enabled: true,
            silent_mode: false,
            priority_only: false,
            badge_enabled: true,
            popup_enabled: true,
            sound_enabled: true,
            vibration_enabled: true,
        };

        // Apply mode-specific defaults
        let (display, audio, power, notifications) =
            Self::apply_mode_defaults(mode, display, audio, power, notifications);

        let profile = Profile {
            id,
            mode,
            name_hash,
            state: ProfileState::Inactive,
            parent_id: 0,
            settings: Vec::new(),
            display,
            audio,
            power,
            notifications,
            creation_time: timestamp,
            last_activated: 0,
            activation_count: 0,
            locked: false,
        };
        self.profiles.push(profile);
        id
    }

    fn apply_mode_defaults(
        mode: ProfileMode,
        mut display: DisplayOverride,
        mut audio: AudioOverride,
        mut power: PowerOverride,
        mut notif: NotificationOverride,
    ) -> (
        DisplayOverride,
        AudioOverride,
        PowerOverride,
        NotificationOverride,
    ) {
        match mode {
            ProfileMode::Work => {
                display.brightness_q16 = (Q16_ONE * 3) >> 2;
                display.night_mode = false;
                audio.do_not_disturb = false;
                notif.priority_only = true;
                power.performance_mode = 1;
                power.screen_timeout_sec = 600;
            }
            ProfileMode::Gaming => {
                display.refresh_rate = 144;
                display.hdr_enabled = true;
                display.brightness_q16 = Q16_ONE;
                audio.spatial_audio = true;
                audio.master_volume_q16 = (Q16_ONE * 3) >> 2;
                power.performance_mode = 2;
                power.cpu_governor = 2;
                power.gpu_boost = true;
                power.wifi_power_save = false;
                notif.enabled = false;
            }
            ProfileMode::Media => {
                display.brightness_q16 = (Q16_ONE * 3) >> 2;
                display.hdr_enabled = true;
                audio.spatial_audio = true;
                audio.master_volume_q16 = (Q16_ONE * 3) >> 2;
                audio.eq_preset = 2;
                notif.silent_mode = true;
                power.screen_timeout_sec = 0;
            }
            ProfileMode::Presentation => {
                display.brightness_q16 = Q16_ONE;
                display.refresh_rate = 60;
                audio.do_not_disturb = true;
                notif.enabled = false;
                power.screen_timeout_sec = 0;
                power.sleep_timeout_min = 0;
            }
            ProfileMode::Reading => {
                display.night_mode = true;
                display.night_warmth_q16 = q16_from_int(80);
                display.brightness_q16 = Q16_ONE >> 2;
                audio.do_not_disturb = true;
                notif.silent_mode = true;
                power.performance_mode = 0;
                power.screen_timeout_sec = 900;
            }
            _ => {}
        }
        (display, audio, power, notif)
    }

    fn delete_profile(&mut self, profile_id: u32) -> bool {
        if profile_id == self.active_profile_id {
            return false;
        }

        // Remove auto-switch rules pointing to this profile
        self.rules.retain(|r| r.profile_id != profile_id);

        let len_before = self.profiles.len();
        self.profiles.retain(|p| p.id != profile_id);
        self.profiles.len() < len_before
    }

    fn activate_profile(&mut self, profile_id: u32, timestamp: u64) -> bool {
        // Deactivate current
        if let Some(current) = self
            .profiles
            .iter_mut()
            .find(|p| p.id == self.active_profile_id)
        {
            current.state = ProfileState::Inactive;
        }

        if let Some(profile) = self.profiles.iter_mut().find(|p| p.id == profile_id) {
            if profile.locked {
                return false;
            }
            self.previous_profile_id = self.active_profile_id;
            self.active_profile_id = profile_id;
            profile.state = ProfileState::Active;
            profile.last_activated = timestamp;
            profile.activation_count = profile.activation_count.saturating_add(1);
            return true;
        }
        false
    }

    fn switch_to_previous(&mut self, timestamp: u64) -> bool {
        if self.previous_profile_id == 0 {
            return false;
        }
        let prev = self.previous_profile_id;
        self.activate_profile(prev, timestamp)
    }

    fn set_profile_setting(
        &mut self,
        profile_id: u32,
        category: SettingCategory,
        key_hash: u64,
        value: i32,
    ) -> bool {
        if let Some(profile) = self.profiles.iter_mut().find(|p| p.id == profile_id) {
            if profile.locked {
                return false;
            }
            // Update existing or add new
            if let Some(s) = profile
                .settings
                .iter_mut()
                .find(|s| s.category == category && s.key_hash == key_hash)
            {
                s.value_i32 = value;
                return true;
            }
            if profile.settings.len() >= 128 {
                return false;
            }
            profile.settings.push(ProfileSetting {
                category,
                key_hash,
                value_i32: value,
                value_bool: value != 0,
                enabled: true,
            });
            return true;
        }
        false
    }

    fn set_parent(&mut self, profile_id: u32, parent_id: u32) -> bool {
        // Prevent circular inheritance
        if profile_id == parent_id {
            return false;
        }
        let parent_exists = self.profiles.iter().any(|p| p.id == parent_id);
        if !parent_exists && parent_id != 0 {
            return false;
        }

        if let Some(profile) = self.profiles.iter_mut().find(|p| p.id == profile_id) {
            profile.parent_id = parent_id;
            return true;
        }
        false
    }

    fn lock_profile(&mut self, profile_id: u32, locked: bool) -> bool {
        if let Some(profile) = self.profiles.iter_mut().find(|p| p.id == profile_id) {
            profile.locked = locked;
            profile.state = if locked {
                ProfileState::Locked
            } else {
                ProfileState::Inactive
            };
            return true;
        }
        false
    }

    fn add_rule(
        &mut self,
        trigger: AutoSwitchTrigger,
        profile_id: u32,
        param_a: u32,
        param_b: u32,
        priority: ProfilePriority,
    ) -> u32 {
        if self.rules.len() >= 64 {
            return 0;
        }
        if !self.profiles.iter().any(|p| p.id == profile_id) {
            return 0;
        }

        let id = self.next_rule_id;
        self.next_rule_id = self.next_rule_id.saturating_add(1);

        self.rules.push(AutoSwitchRule {
            id,
            trigger,
            profile_id,
            param_a,
            param_b,
            priority,
            enabled: true,
            trigger_count: 0,
        });
        id
    }

    fn remove_rule(&mut self, rule_id: u32) -> bool {
        let len_before = self.rules.len();
        self.rules.retain(|r| r.id != rule_id);
        self.rules.len() < len_before
    }

    fn evaluate_rules(&mut self, timestamp: u64) -> Option<u32> {
        if !self.auto_switch_enabled {
            return None;
        }

        let mut best_profile: Option<u32> = None;
        let mut best_priority = 0u8;

        for rule in &mut self.rules {
            if !rule.enabled {
                continue;
            }

            let matches = match rule.trigger {
                AutoSwitchTrigger::TimeOfDay => {
                    let start = rule.param_a as u8;
                    let end = rule.param_b as u8;
                    if start <= end {
                        self.current_hour >= start && self.current_hour < end
                    } else {
                        self.current_hour >= start || self.current_hour < end
                    }
                }
                AutoSwitchTrigger::BatteryLevel => self.current_battery <= rule.param_a as u8,
                AutoSwitchTrigger::AppLaunched => false, // evaluated externally
                AutoSwitchTrigger::PeripheralConnected => false,
                AutoSwitchTrigger::NetworkChanged => false,
                AutoSwitchTrigger::Manual => false,
            };

            if matches {
                let prio = match rule.priority {
                    ProfilePriority::Low => 1,
                    ProfilePriority::Normal => 2,
                    ProfilePriority::High => 3,
                    ProfilePriority::Override => 4,
                };

                if prio > best_priority {
                    best_priority = prio;
                    best_profile = Some(rule.profile_id);
                    rule.trigger_count = rule.trigger_count.saturating_add(1);
                }
            }
        }

        if let Some(pid) = best_profile {
            if pid != self.active_profile_id {
                self.activate_profile(pid, timestamp);
                return Some(pid);
            }
        }
        None
    }

    fn on_app_launched(&mut self, app_id: u32, timestamp: u64) -> Option<u32> {
        if !self.auto_switch_enabled {
            return None;
        }

        for rule in &mut self.rules {
            if !rule.enabled {
                continue;
            }
            if rule.trigger != AutoSwitchTrigger::AppLaunched {
                continue;
            }
            if rule.param_a == app_id {
                rule.trigger_count = rule.trigger_count.saturating_add(1);
                let pid = rule.profile_id;
                if pid != self.active_profile_id {
                    self.activate_profile(pid, timestamp);
                    return Some(pid);
                }
            }
        }
        None
    }

    fn on_peripheral_connected(&mut self, peripheral_type: u32, timestamp: u64) -> Option<u32> {
        if !self.auto_switch_enabled {
            return None;
        }

        for rule in &mut self.rules {
            if !rule.enabled {
                continue;
            }
            if rule.trigger != AutoSwitchTrigger::PeripheralConnected {
                continue;
            }
            if rule.param_a == peripheral_type {
                rule.trigger_count = rule.trigger_count.saturating_add(1);
                let pid = rule.profile_id;
                if pid != self.active_profile_id {
                    self.activate_profile(pid, timestamp);
                    return Some(pid);
                }
            }
        }
        None
    }

    fn update_time(&mut self, hour: u8) {
        self.current_hour = hour;
    }

    fn update_battery(&mut self, level: u8) {
        self.current_battery = level;
    }

    fn profile_count(&self) -> usize {
        self.profiles.len()
    }

    fn rule_count(&self) -> usize {
        self.rules.len()
    }

    fn setup_defaults(&mut self, timestamp: u64) {
        let default_id =
            self.create_profile(ProfileMode::Default, 0xDEFA_0000_0000_0001, timestamp);
        let work_id = self.create_profile(ProfileMode::Work, 0xDEFA_0000_0000_0002, timestamp);
        let _gaming_id = self.create_profile(ProfileMode::Gaming, 0xDEFA_0000_0000_0003, timestamp);
        let _media_id = self.create_profile(ProfileMode::Media, 0xDEFA_0000_0000_0004, timestamp);

        self.activate_profile(default_id, timestamp);

        // Work mode from 9:00 to 17:00
        self.add_rule(
            AutoSwitchTrigger::TimeOfDay,
            work_id,
            9,
            17,
            ProfilePriority::Normal,
        );
        // Gaming mode when battery above threshold and game launched (external trigger)
        // Low battery -> power save default
        self.add_rule(
            AutoSwitchTrigger::BatteryLevel,
            default_id,
            20,
            0,
            ProfilePriority::High,
        );
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    let mut mgr = ProfileManager::new();
    mgr.setup_defaults(0);

    let mut guard = PROFILES.lock();
    *guard = Some(mgr);
    serial_println!(
        "    Profiles: work/gaming/media modes ready ({} profiles, {} rules)",
        4,
        2
    );
}

pub fn create_profile(mode: ProfileMode, name_hash: u64, timestamp: u64) -> u32 {
    let mut guard = PROFILES.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.create_profile(mode, name_hash, timestamp);
    }
    0
}

pub fn activate_profile(profile_id: u32, timestamp: u64) -> bool {
    let mut guard = PROFILES.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.activate_profile(profile_id, timestamp);
    }
    false
}

pub fn switch_to_previous(timestamp: u64) -> bool {
    let mut guard = PROFILES.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.switch_to_previous(timestamp);
    }
    false
}

pub fn add_auto_switch_rule(
    trigger: AutoSwitchTrigger,
    profile_id: u32,
    param_a: u32,
    param_b: u32,
    priority: ProfilePriority,
) -> u32 {
    let mut guard = PROFILES.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.add_rule(trigger, profile_id, param_a, param_b, priority);
    }
    0
}

pub fn evaluate_auto_switch(timestamp: u64) -> Option<u32> {
    let mut guard = PROFILES.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.evaluate_rules(timestamp);
    }
    None
}

pub fn on_app_launched(app_id: u32, timestamp: u64) -> Option<u32> {
    let mut guard = PROFILES.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.on_app_launched(app_id, timestamp);
    }
    None
}

pub fn on_peripheral_connected(peripheral_type: u32, timestamp: u64) -> Option<u32> {
    let mut guard = PROFILES.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.on_peripheral_connected(peripheral_type, timestamp);
    }
    None
}
