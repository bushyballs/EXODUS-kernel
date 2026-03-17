/// Permissions system for Genesis — app capability management
///
/// Each app requests permissions at install time or runtime.
/// Users can grant/deny/revoke. Permissions are capability-based
/// and fine-grained (e.g., camera, location, contacts, network).
///
/// Inspired by: Android runtime permissions, iOS entitlements. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Permission identifiers
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Permission {
    // Device access
    Camera,
    Microphone,
    Location,
    LocationBackground,
    Sensors,
    Bluetooth,
    Nfc,

    // Data access
    Contacts,
    Calendar,
    Storage,
    MediaLibrary,
    CallLog,

    // Communication
    Phone,
    Sms,
    Internet,
    LocalNetwork,

    // System
    Notifications,
    SystemAlert,
    InstallPackages,
    DeviceAdmin,
    Accessibility,
    BackgroundExecution,
    BootCompleted,

    // Dangerous
    RootAccess,
    KernelModule,
    RawDisk,
}

/// Permission state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionState {
    /// Not yet requested
    NotRequested,
    /// User granted
    Granted,
    /// User denied
    Denied,
    /// User denied + "don't ask again"
    PermanentlyDenied,
    /// System policy prevents granting
    Restricted,
}

/// Permission group (for UI display)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionGroup {
    DeviceHardware,
    PersonalData,
    Communication,
    System,
    Dangerous,
}

impl Permission {
    pub fn group(&self) -> PermissionGroup {
        match self {
            Permission::Camera
            | Permission::Microphone
            | Permission::Sensors
            | Permission::Bluetooth
            | Permission::Nfc => PermissionGroup::DeviceHardware,

            Permission::Contacts
            | Permission::Calendar
            | Permission::Storage
            | Permission::MediaLibrary
            | Permission::CallLog
            | Permission::Location
            | Permission::LocationBackground => PermissionGroup::PersonalData,

            Permission::Phone
            | Permission::Sms
            | Permission::Internet
            | Permission::LocalNetwork => PermissionGroup::Communication,

            Permission::Notifications
            | Permission::SystemAlert
            | Permission::InstallPackages
            | Permission::Accessibility
            | Permission::BackgroundExecution
            | Permission::BootCompleted
            | Permission::DeviceAdmin => PermissionGroup::System,

            Permission::RootAccess | Permission::KernelModule | Permission::RawDisk => {
                PermissionGroup::Dangerous
            }
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Permission::Camera => "Take photos and videos",
            Permission::Microphone => "Record audio",
            Permission::Location => "Access approximate location",
            Permission::LocationBackground => "Access location in background",
            Permission::Sensors => "Access body sensors",
            Permission::Bluetooth => "Connect to Bluetooth devices",
            Permission::Nfc => "Use NFC",
            Permission::Contacts => "Access your contacts",
            Permission::Calendar => "Access your calendar",
            Permission::Storage => "Access files and media",
            Permission::MediaLibrary => "Access photos and videos",
            Permission::CallLog => "Access call history",
            Permission::Phone => "Make and manage phone calls",
            Permission::Sms => "Send and view SMS messages",
            Permission::Internet => "Access the internet",
            Permission::LocalNetwork => "Access devices on local network",
            Permission::Notifications => "Send notifications",
            Permission::SystemAlert => "Display over other apps",
            Permission::InstallPackages => "Install applications",
            Permission::DeviceAdmin => "Administer the device",
            Permission::Accessibility => "Control accessibility features",
            Permission::BackgroundExecution => "Run in the background",
            Permission::BootCompleted => "Run at startup",
            Permission::RootAccess => "Full system access (dangerous)",
            Permission::KernelModule => "Load kernel modules (dangerous)",
            Permission::RawDisk => "Direct disk access (dangerous)",
        }
    }

    pub fn is_dangerous(&self) -> bool {
        matches!(self.group(), PermissionGroup::Dangerous)
    }
}

/// Per-app permission store
pub struct AppPermissions {
    pub app_id: String,
    pub permissions: BTreeMap<u8, PermissionState>,
}

/// Permission manager
pub struct PermissionManager {
    apps: Vec<AppPermissions>,
    /// System-wide permission policy overrides
    policy: BTreeMap<u8, PermissionState>,
}

impl PermissionManager {
    const fn new() -> Self {
        PermissionManager {
            apps: Vec::new(),
            policy: BTreeMap::new(),
        }
    }

    fn perm_key(perm: Permission) -> u8 {
        perm as u8
    }

    /// Check if an app has a permission
    pub fn check(&self, app_id: &str, perm: Permission) -> PermissionState {
        let key = Self::perm_key(perm);

        // Check system policy first
        if let Some(&state) = self.policy.get(&key) {
            return state;
        }

        // Check app permissions
        if let Some(app) = self.apps.iter().find(|a| a.app_id == app_id) {
            if let Some(&state) = app.permissions.get(&key) {
                return state;
            }
        }

        PermissionState::NotRequested
    }

    /// Grant a permission to an app
    pub fn grant(&mut self, app_id: &str, perm: Permission) {
        let key = Self::perm_key(perm);
        let app = self.get_or_create_app(app_id);
        app.permissions.insert(key, PermissionState::Granted);
    }

    /// Deny a permission
    pub fn deny(&mut self, app_id: &str, perm: Permission, permanent: bool) {
        let key = Self::perm_key(perm);
        let state = if permanent {
            PermissionState::PermanentlyDenied
        } else {
            PermissionState::Denied
        };
        let app = self.get_or_create_app(app_id);
        app.permissions.insert(key, state);
    }

    /// Revoke a previously granted permission
    pub fn revoke(&mut self, app_id: &str, perm: Permission) {
        let key = Self::perm_key(perm);
        let app = self.get_or_create_app(app_id);
        app.permissions.insert(key, PermissionState::Denied);
    }

    /// Set system-wide policy
    pub fn set_policy(&mut self, perm: Permission, state: PermissionState) {
        self.policy.insert(Self::perm_key(perm), state);
    }

    /// List all permissions for an app
    pub fn list_app_permissions(&self, _app_id: &str) -> Vec<(Permission, PermissionState)> {
        // This is simplified — would normally iterate all Permission variants
        Vec::new()
    }

    fn get_or_create_app(&mut self, app_id: &str) -> &mut AppPermissions {
        if !self.apps.iter().any(|a| a.app_id == app_id) {
            self.apps.push(AppPermissions {
                app_id: String::from(app_id),
                permissions: BTreeMap::new(),
            });
        }
        self.apps.iter_mut().find(|a| a.app_id == app_id).unwrap()
    }
}

static PERM_MANAGER: Mutex<PermissionManager> = Mutex::new(PermissionManager::new());

pub fn init() {
    crate::serial_println!("  [permissions] Permission system initialized");
}

pub fn check(app_id: &str, perm: Permission) -> PermissionState {
    PERM_MANAGER.lock().check(app_id, perm)
}

pub fn grant(app_id: &str, perm: Permission) {
    PERM_MANAGER.lock().grant(app_id, perm);
}

pub fn deny(app_id: &str, perm: Permission) {
    PERM_MANAGER.lock().deny(app_id, perm, false);
}
