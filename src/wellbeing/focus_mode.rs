use crate::sync::Mutex;
/// Focus mode for Genesis
///
/// Do Not Disturb profiles, scheduled focus,
/// allowed contacts/apps, auto-reply, status sharing.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum FocusProfile {
    DoNotDisturb,
    Work,
    Personal,
    Sleep,
    Driving,
    Exercise,
    Custom,
}

struct FocusRule {
    profile: FocusProfile,
    active: bool,
    allowed_apps: [u32; 10],
    allowed_app_count: usize,
    allowed_contacts: [u32; 10],
    allowed_contact_count: usize,
    auto_reply_enabled: bool,
    schedule_start_hour: u8,
    schedule_start_min: u8,
    schedule_end_hour: u8,
    schedule_end_min: u8,
    days_bitmask: u8, // bit per day (Mon=0..Sun=6)
    suppress_notifications: bool,
    dim_lock_screen: bool,
    silence_calls: bool,
    allow_repeat_callers: bool,
}

struct FocusEngine {
    rules: Vec<FocusRule>,
    active_profile: Option<FocusProfile>,
    active_since: u64,
    total_focus_minutes: u64,
    sessions_count: u32,
}

static FOCUS: Mutex<Option<FocusEngine>> = Mutex::new(None);

impl FocusEngine {
    fn new() -> Self {
        let mut engine = FocusEngine {
            rules: Vec::new(),
            active_profile: None,
            active_since: 0,
            total_focus_minutes: 0,
            sessions_count: 0,
        };
        // Default DND profile
        engine.rules.push(FocusRule {
            profile: FocusProfile::DoNotDisturb,
            active: false,
            allowed_apps: [0; 10],
            allowed_app_count: 0,
            allowed_contacts: [0; 10],
            allowed_contact_count: 0,
            auto_reply_enabled: false,
            schedule_start_hour: 0,
            schedule_start_min: 0,
            schedule_end_hour: 0,
            schedule_end_min: 0,
            days_bitmask: 0,
            suppress_notifications: true,
            dim_lock_screen: true,
            silence_calls: true,
            allow_repeat_callers: true,
        });
        // Sleep profile
        engine.rules.push(FocusRule {
            profile: FocusProfile::Sleep,
            active: false,
            allowed_apps: [0; 10],
            allowed_app_count: 0,
            allowed_contacts: [0; 10],
            allowed_contact_count: 0,
            auto_reply_enabled: true,
            schedule_start_hour: 22,
            schedule_start_min: 0,
            schedule_end_hour: 7,
            schedule_end_min: 0,
            days_bitmask: 0x7F, // every day
            suppress_notifications: true,
            dim_lock_screen: true,
            silence_calls: true,
            allow_repeat_callers: true,
        });
        engine
    }

    fn activate(&mut self, profile: FocusProfile, timestamp: u64) {
        self.active_profile = Some(profile);
        self.active_since = timestamp;
        self.sessions_count = self.sessions_count.saturating_add(1);
        if let Some(rule) = self.rules.iter_mut().find(|r| r.profile == profile) {
            rule.active = true;
        }
    }

    fn deactivate(&mut self, timestamp: u64) {
        if let Some(profile) = self.active_profile {
            let duration = timestamp.saturating_sub(self.active_since) / 60;
            self.total_focus_minutes += duration;
            if let Some(rule) = self.rules.iter_mut().find(|r| r.profile == profile) {
                rule.active = false;
            }
        }
        self.active_profile = None;
    }

    fn should_suppress_notification(&self, app_id: u32) -> bool {
        if let Some(profile) = self.active_profile {
            if let Some(rule) = self.rules.iter().find(|r| r.profile == profile) {
                if !rule.suppress_notifications {
                    return false;
                }
                // Check if app is in allowed list
                for i in 0..rule.allowed_app_count {
                    if rule.allowed_apps[i] == app_id {
                        return false;
                    }
                }
                return true;
            }
        }
        false
    }

    fn should_silence_call(&self, contact_id: u32) -> bool {
        if let Some(profile) = self.active_profile {
            if let Some(rule) = self.rules.iter().find(|r| r.profile == profile) {
                if !rule.silence_calls {
                    return false;
                }
                for i in 0..rule.allowed_contact_count {
                    if rule.allowed_contacts[i] == contact_id {
                        return false;
                    }
                }
                return true;
            }
        }
        false
    }
}

pub fn init() {
    let mut f = FOCUS.lock();
    *f = Some(FocusEngine::new());
    serial_println!("    Wellbeing: focus mode (DND, work, sleep profiles) ready");
}
