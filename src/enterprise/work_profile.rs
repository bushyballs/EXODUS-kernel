/// Work profile for Genesis
///
/// Separate work/personal containers, cross-profile data isolation,
/// work mode toggle, and profile-specific settings.
///
/// Inspired by: Android Work Profile, Samsung Knox. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Profile type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileType {
    Personal,
    Work,
    Guest,
    Restricted,
}

/// A user profile
pub struct UserProfile {
    pub id: u32,
    pub name: String,
    pub profile_type: ProfileType,
    pub active: bool,
    pub created_at: u64,
    pub storage_path: String,
    pub apps: Vec<String>,
    pub data_encrypted: bool,
}

/// Cross-profile policy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossProfilePolicy {
    AllowAll,
    AllowContacts,
    AllowCalendar,
    DenyAll,
}

/// Work profile manager
pub struct WorkProfileManager {
    pub profiles: Vec<UserProfile>,
    pub next_id: u32,
    pub active_profile: u32,
    pub cross_profile_policy: CrossProfilePolicy,
    pub work_mode_enabled: bool,
    pub work_challenge_required: bool,
}

impl WorkProfileManager {
    const fn new() -> Self {
        WorkProfileManager {
            profiles: Vec::new(),
            next_id: 1,
            active_profile: 0,
            cross_profile_policy: CrossProfilePolicy::AllowContacts,
            work_mode_enabled: true,
            work_challenge_required: false,
        }
    }

    pub fn create_profile(&mut self, name: &str, ptype: ProfileType) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let path = alloc::format!("/data/profiles/{}", id);
        self.profiles.push(UserProfile {
            id,
            name: String::from(name),
            profile_type: ptype,
            active: ptype == ProfileType::Personal,
            created_at: crate::time::clock::unix_time(),
            storage_path: path,
            apps: Vec::new(),
            data_encrypted: true,
        });
        if self.active_profile == 0 {
            self.active_profile = id;
        }
        id
    }

    pub fn switch_profile(&mut self, id: u32) -> bool {
        if let Some(profile) = self.profiles.iter().find(|p| p.id == id) {
            if !profile.active {
                return false;
            }
            self.active_profile = id;
            true
        } else {
            false
        }
    }

    pub fn toggle_work_mode(&mut self) {
        self.work_mode_enabled = !self.work_mode_enabled;
        // Suspend/resume all work profile apps
        for profile in &mut self.profiles {
            if profile.profile_type == ProfileType::Work {
                profile.active = self.work_mode_enabled;
            }
        }
    }

    pub fn add_app_to_profile(&mut self, profile_id: u32, app_id: &str) {
        if let Some(profile) = self.profiles.iter_mut().find(|p| p.id == profile_id) {
            profile.apps.push(String::from(app_id));
        }
    }

    pub fn get_current(&self) -> Option<&UserProfile> {
        self.profiles.iter().find(|p| p.id == self.active_profile)
    }

    pub fn remove_profile(&mut self, id: u32) -> bool {
        let len = self.profiles.len();
        self.profiles.retain(|p| p.id != id);
        self.profiles.len() < len
    }

    pub fn is_cross_profile_allowed(&self, data_type: &str) -> bool {
        match self.cross_profile_policy {
            CrossProfilePolicy::AllowAll => true,
            CrossProfilePolicy::AllowContacts => data_type == "contacts",
            CrossProfilePolicy::AllowCalendar => data_type == "calendar",
            CrossProfilePolicy::DenyAll => false,
        }
    }
}

static PROFILES: Mutex<WorkProfileManager> = Mutex::new(WorkProfileManager::new());

pub fn init() {
    let mut mgr = PROFILES.lock();
    mgr.create_profile("Owner", ProfileType::Personal);
    crate::serial_println!("  [enterprise] Work profile system initialized");
}
