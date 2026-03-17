use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

/// Read the CPU timestamp counter and return a millisecond timestamp.
///
/// TSC frequency is estimated at 3 GHz (3_000_000 ticks/ms).  Replace with
/// a calibrated value once the ACPI/HPET timer subsystem is available.
#[inline]
fn get_timestamp_ms() -> u64 {
    let tsc: u64;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            "shl rdx, 32",
            "or rax, rdx",
            out("rax") tsc,
            out("rdx") _,
            options(nomem, nostack, preserves_flags),
        );
    }
    tsc / 3_000_000
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UserProfile {
    Owner,
    Standard,
    Restricted,
    Guest,
    Child,
    Work,
}

#[derive(Clone, Copy)]
pub struct UserAccount {
    pub uid: u32,
    pub profile: UserProfile,
    pub name_hash: u64,
    pub pin_hash: u64,
    pub created: u64,
    pub last_login: u64,
    pub storage_quota_mb: u32,
    pub storage_used_mb: u32,
    pub app_count: u16,
    pub active: bool,
}

pub struct MultiUserManager {
    users: Vec<UserAccount>,
    current_user: u32,
    max_users: u8,
    guest_enabled: bool,
}

impl MultiUserManager {
    fn new() -> Self {
        let mut mgr = Self {
            users: Vec::new(),
            current_user: 0,
            max_users: 8,
            guest_enabled: false,
        };

        // Create default owner account (UID 1000)
        let owner = UserAccount {
            uid: 1000,
            profile: UserProfile::Owner,
            name_hash: 0x4f574e4552, // "OWNER" hash
            pin_hash: 0,
            created: 0,
            last_login: 0,
            storage_quota_mb: 0, // Unlimited for owner
            storage_used_mb: 0,
            app_count: 0,
            active: true,
        };
        mgr.users.push(owner);
        mgr.current_user = 1000;

        mgr
    }

    pub fn add_user(
        &mut self,
        profile: UserProfile,
        name_hash: u64,
        pin_hash: u64,
        storage_quota_mb: u32,
    ) -> Result<u32, &'static str> {
        if self.users.len() >= self.max_users as usize {
            return Err("User limit reached");
        }

        // Check for duplicate name_hash
        if self.users.iter().any(|u| u.name_hash == name_hash) {
            return Err("User name already exists");
        }

        let uid = 1000 + self.users.len() as u32;

        let user = UserAccount {
            uid,
            profile,
            name_hash,
            pin_hash,
            created: get_timestamp_ms(),
            last_login: 0,
            storage_quota_mb,
            storage_used_mb: 0,
            app_count: 0,
            active: true,
        };

        self.users.push(user);
        Ok(uid)
    }

    pub fn remove_user(&mut self, uid: u32) -> Result<(), &'static str> {
        if uid == 1000 {
            return Err("Cannot remove owner account");
        }

        let idx = self
            .users
            .iter()
            .position(|u| u.uid == uid)
            .ok_or("User not found")?;

        self.users.remove(idx);

        // If we removed current user, switch to owner
        if self.current_user == uid {
            self.current_user = 1000;
        }

        Ok(())
    }

    pub fn switch_user(&mut self, uid: u32, pin_hash: u64) -> Result<(), &'static str> {
        let user = self
            .users
            .iter_mut()
            .find(|u| u.uid == uid)
            .ok_or("User not found")?;

        if !user.active {
            return Err("User account disabled");
        }

        // Guest profile doesn't require PIN
        if user.profile != UserProfile::Guest && user.pin_hash != pin_hash {
            return Err("Invalid PIN");
        }

        self.current_user = uid;
        user.last_login = get_timestamp_ms();

        Ok(())
    }

    pub fn get_current(&self) -> u32 {
        self.current_user
    }

    pub fn set_quota(&mut self, uid: u32, quota_mb: u32) -> Result<(), &'static str> {
        let user = self
            .users
            .iter_mut()
            .find(|u| u.uid == uid)
            .ok_or("User not found")?;

        if user.profile == UserProfile::Owner {
            return Err("Cannot set quota for owner");
        }

        user.storage_quota_mb = quota_mb;
        Ok(())
    }

    pub fn check_quota(&self, uid: u32) -> Result<bool, &'static str> {
        let user = self
            .users
            .iter()
            .find(|u| u.uid == uid)
            .ok_or("User not found")?;

        // Owner has unlimited quota
        if user.profile == UserProfile::Owner || user.storage_quota_mb == 0 {
            return Ok(true);
        }

        Ok(user.storage_used_mb < user.storage_quota_mb)
    }

    pub fn enable_guest(&mut self, enabled: bool) {
        self.guest_enabled = enabled;
    }

    pub fn get_user_list(&self) -> Vec<(u32, UserProfile, u64, bool)> {
        self.users
            .iter()
            .map(|u| (u.uid, u.profile, u.name_hash, u.active))
            .collect()
    }

    pub fn get_user(&self, uid: u32) -> Option<UserAccount> {
        self.users.iter().find(|u| u.uid == uid).copied()
    }

    pub fn user_count(&self) -> usize {
        self.users.len()
    }

    pub fn active_user_count(&self) -> usize {
        self.users.iter().filter(|u| u.active).count()
    }
}

static MULTI_USER: Mutex<Option<MultiUserManager>> = Mutex::new(None);

pub fn init() {
    let mut mgr = MULTI_USER.lock();
    *mgr = Some(MultiUserManager::new());
    serial_println!("[MULTI_USER] Multi-user system initialized (owner: UID 1000)");
}

pub fn add_user(
    profile: UserProfile,
    name_hash: u64,
    pin_hash: u64,
    storage_quota_mb: u32,
) -> Result<u32, &'static str> {
    let mut mgr = MULTI_USER.lock();
    mgr.as_mut()
        .ok_or("Multi-user manager not initialized")?
        .add_user(profile, name_hash, pin_hash, storage_quota_mb)
}

pub fn remove_user(uid: u32) -> Result<(), &'static str> {
    let mut mgr = MULTI_USER.lock();
    mgr.as_mut()
        .ok_or("Multi-user manager not initialized")?
        .remove_user(uid)
}

pub fn switch_user(uid: u32, pin_hash: u64) -> Result<(), &'static str> {
    let mut mgr = MULTI_USER.lock();
    mgr.as_mut()
        .ok_or("Multi-user manager not initialized")?
        .switch_user(uid, pin_hash)
}

pub fn get_current_user() -> u32 {
    let mgr = MULTI_USER.lock();
    match mgr.as_ref() {
        Some(manager) => manager.get_current(),
        None => 0,
    }
}

pub fn set_user_quota(uid: u32, quota_mb: u32) -> Result<(), &'static str> {
    let mut mgr = MULTI_USER.lock();
    mgr.as_mut()
        .ok_or("Multi-user manager not initialized")?
        .set_quota(uid, quota_mb)
}

pub fn check_user_quota(uid: u32) -> Result<bool, &'static str> {
    let mgr = MULTI_USER.lock();
    mgr.as_ref()
        .ok_or("Multi-user manager not initialized")?
        .check_quota(uid)
}

pub fn enable_guest_account(enabled: bool) {
    let mut mgr = MULTI_USER.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.enable_guest(enabled);
    }
}

pub fn list_users() -> Vec<(u32, UserProfile, u64, bool)> {
    let mgr = MULTI_USER.lock();
    match mgr.as_ref() {
        Some(manager) => manager.get_user_list(),
        None => Vec::new(),
    }
}

pub fn get_user_account(uid: u32) -> Option<UserAccount> {
    let mgr = MULTI_USER.lock();
    mgr.as_ref().and_then(|manager| manager.get_user(uid))
}
