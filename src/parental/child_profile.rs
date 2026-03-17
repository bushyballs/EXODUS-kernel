use crate::sync::Mutex;
/// Child profile management for Genesis
///
/// Age-restricted profiles, app allowlists,
/// bedtime enforcement, location boundaries.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum AgeGroup {
    Toddler, // 0-4
    Child,   // 5-8
    PreTeen, // 9-12
    Teen,    // 13-17
}

struct ChildProfile {
    id: u32,
    name: [u8; 24],
    name_len: usize,
    age: u8,
    age_group: AgeGroup,
    allowed_apps: Vec<u32>,
    blocked_apps: Vec<u32>,
    daily_screen_limit_min: u32,
    bedtime_hour: u8,
    bedtime_min: u8,
    wakeup_hour: u8,
    wakeup_min: u8,
    location_tracking: bool,
    safe_zones: Vec<SafeZone>,
    can_install_apps: bool,
    can_make_purchases: bool,
    web_filtering: bool,
}

struct SafeZone {
    lat_x1000: i32,
    lon_x1000: i32,
    radius_m: u32,
    name: [u8; 24],
    name_len: usize,
}

struct ChildProfileEngine {
    profiles: Vec<ChildProfile>,
    next_id: u32,
    parent_pin_hash: u64,
}

static CHILD_PROFILES: Mutex<Option<ChildProfileEngine>> = Mutex::new(None);

impl ChildProfileEngine {
    fn new() -> Self {
        ChildProfileEngine {
            profiles: Vec::new(),
            next_id: 1,
            parent_pin_hash: 0,
        }
    }

    fn create_profile(&mut self, name: &[u8], age: u8) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let age_group = match age {
            0..=4 => AgeGroup::Toddler,
            5..=8 => AgeGroup::Child,
            9..=12 => AgeGroup::PreTeen,
            _ => AgeGroup::Teen,
        };
        let mut n = [0u8; 24];
        let nlen = name.len().min(24);
        n[..nlen].copy_from_slice(&name[..nlen]);
        let limit = match age_group {
            AgeGroup::Toddler => 60,
            AgeGroup::Child => 120,
            AgeGroup::PreTeen => 180,
            AgeGroup::Teen => 240,
        };
        self.profiles.push(ChildProfile {
            id,
            name: n,
            name_len: nlen,
            age,
            age_group,
            allowed_apps: Vec::new(),
            blocked_apps: Vec::new(),
            daily_screen_limit_min: limit,
            bedtime_hour: 20,
            bedtime_min: 0,
            wakeup_hour: 7,
            wakeup_min: 0,
            location_tracking: true,
            safe_zones: Vec::new(),
            can_install_apps: age >= 13,
            can_make_purchases: false,
            web_filtering: true,
        });
        id
    }

    fn is_app_allowed(&self, profile_id: u32, app_id: u32) -> bool {
        if let Some(p) = self.profiles.iter().find(|p| p.id == profile_id) {
            if p.blocked_apps.contains(&app_id) {
                return false;
            }
            if !p.allowed_apps.is_empty() {
                return p.allowed_apps.contains(&app_id);
            }
            true
        } else {
            true
        }
    }

    fn is_bedtime(&self, profile_id: u32, hour: u8, min: u8) -> bool {
        if let Some(p) = self.profiles.iter().find(|p| p.id == profile_id) {
            let current = hour as u16 * 60 + min as u16;
            let bed = p.bedtime_hour as u16 * 60 + p.bedtime_min as u16;
            let wake = p.wakeup_hour as u16 * 60 + p.wakeup_min as u16;
            if bed > wake {
                current >= bed || current < wake
            } else {
                current >= bed && current < wake
            }
        } else {
            false
        }
    }

    fn is_in_safe_zone(&self, profile_id: u32, lat: i32, lon: i32) -> bool {
        if let Some(p) = self.profiles.iter().find(|p| p.id == profile_id) {
            for zone in &p.safe_zones {
                let dlat = (lat - zone.lat_x1000).abs() as u32;
                let dlon = (lon - zone.lon_x1000).abs() as u32;
                // Rough distance check (not precise, but functional)
                let dist_approx = dlat + dlon; // Manhattan distance in x1000 degrees
                                               // 1 degree ~= 111km, so x1000 = 111m
                if dist_approx * 111 < zone.radius_m {
                    return true;
                }
            }
            p.safe_zones.is_empty() // if no zones defined, everywhere is safe
        } else {
            true
        }
    }
}

pub fn init() {
    let mut p = CHILD_PROFILES.lock();
    *p = Some(ChildProfileEngine::new());
    serial_println!("    Parental: child profiles (age groups, limits, safe zones) ready");
}
