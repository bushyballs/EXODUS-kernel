/// Device policy controller for Genesis
///
/// System-wide policy enforcement, restrictions,
/// compliance rules, and admin capabilities.
///
/// Inspired by: Android DevicePolicyManager, iOS Configuration Profiles. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Device restriction
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Restriction {
    DisableCamera,
    DisableScreenCapture,
    DisableUsb,
    DisableBluetooth,
    DisableWifi,
    DisableNfc,
    DisableLocationSharing,
    DisableAppInstall,
    DisableFactoryReset,
    DisableModifyAccounts,
    DisableMountPhysical,
    DisableOutgoingCalls,
    DisableSms,
    DisableShareLocation,
    RequireEncryption,
    RequirePasswordChange,
    EnforceBackup,
}

/// Password quality
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordQuality {
    Unspecified,
    Biometric,
    Something,
    Numeric,
    NumericComplex,
    Alphabetic,
    Alphanumeric,
    Complex,
}

/// Device admin
pub struct DeviceAdmin {
    pub package_id: String,
    pub is_device_owner: bool,
    pub is_profile_owner: bool,
}

/// Device policy state
pub struct DevicePolicy {
    pub restrictions: BTreeMap<u8, bool>, // Restriction as u8 -> enabled
    pub admins: Vec<DeviceAdmin>,
    pub password_quality: PasswordQuality,
    pub password_min_length: u8,
    pub password_max_failed: u8,
    pub password_expiry_days: u32,
    pub max_inactivity_lock: u32, // seconds
    pub keyguard_disabled_features: u32,
    pub permitted_apps: Option<Vec<String>>, // None = all allowed
    pub blocked_apps: Vec<String>,
    pub vpn_always_on: Option<String>, // package ID
}

impl DevicePolicy {
    const fn new() -> Self {
        DevicePolicy {
            restrictions: BTreeMap::new(),
            admins: Vec::new(),
            password_quality: PasswordQuality::Unspecified,
            password_min_length: 4,
            password_max_failed: 10,
            password_expiry_days: 0,
            max_inactivity_lock: 300,
            keyguard_disabled_features: 0,
            permitted_apps: None,
            blocked_apps: Vec::new(),
            vpn_always_on: None,
        }
    }

    pub fn set_restriction(&mut self, restriction: Restriction, enabled: bool) {
        self.restrictions.insert(restriction as u8, enabled);
    }

    pub fn is_restricted(&self, restriction: Restriction) -> bool {
        self.restrictions
            .get(&(restriction as u8))
            .copied()
            .unwrap_or(false)
    }

    pub fn add_admin(&mut self, package_id: &str, device_owner: bool) {
        self.admins.push(DeviceAdmin {
            package_id: String::from(package_id),
            is_device_owner: device_owner,
            is_profile_owner: !device_owner,
        });
    }

    pub fn remove_admin(&mut self, package_id: &str) -> bool {
        let len = self.admins.len();
        self.admins.retain(|a| a.package_id != package_id);
        self.admins.len() < len
    }

    pub fn is_app_allowed(&self, app_id: &str) -> bool {
        if self.blocked_apps.iter().any(|a| a == app_id) {
            return false;
        }
        match &self.permitted_apps {
            Some(list) => list.iter().any(|a| a == app_id),
            None => true,
        }
    }

    pub fn block_app(&mut self, app_id: &str) {
        if !self.blocked_apps.iter().any(|a| a == app_id) {
            self.blocked_apps.push(String::from(app_id));
        }
    }

    pub fn set_always_on_vpn(&mut self, package_id: &str) {
        self.vpn_always_on = Some(String::from(package_id));
    }

    pub fn has_device_owner(&self) -> bool {
        self.admins.iter().any(|a| a.is_device_owner)
    }
}

static POLICY: Mutex<DevicePolicy> = Mutex::new(DevicePolicy::new());

pub fn init() {
    crate::serial_println!("  [enterprise] Device policy controller initialized");
}

pub fn is_restricted(r: Restriction) -> bool {
    POLICY.lock().is_restricted(r)
}
