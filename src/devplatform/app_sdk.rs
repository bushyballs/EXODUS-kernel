/// App development SDK for Genesis
///
/// Provides the core SDK that third-party developers use to build
/// applications for Hoags OS. Handles app manifests, permission
/// requests, system info queries, intent messaging, and activity
/// lifecycle management.
///
/// Apps declare capabilities in an AppManifest and interact with
/// the OS through the SdkApi interface. All communication is
/// mediated by the permission system.
///
/// Original implementation for Hoags OS. No external crates.
use crate::sync::Mutex;
use alloc::vec;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of permissions an app can request
const MAX_PERMISSIONS_PER_APP: usize = 32;

/// Maximum number of registered apps
const MAX_REGISTERED_APPS: usize = 256;

/// Maximum number of pending intents in the queue
const MAX_INTENT_QUEUE: usize = 128;

/// Current SDK version (major << 16 | minor << 8 | patch)
const SDK_VERSION: u32 = 0x00010000;

/// Minimum supported OS version for this SDK
const MIN_OS_VERSION: u32 = 0x00010000;

/// App state: not yet initialized
const APP_STATE_UNINIT: u8 = 0;

/// App state: registered and ready
const APP_STATE_REGISTERED: u8 = 1;

/// App state: running in foreground
const APP_STATE_FOREGROUND: u8 = 2;

/// App state: running in background
const APP_STATE_BACKGROUND: u8 = 3;

/// App state: paused
const APP_STATE_PAUSED: u8 = 4;

/// App state: stopped
const APP_STATE_STOPPED: u8 = 5;

// ---------------------------------------------------------------------------
// Permission enum
// ---------------------------------------------------------------------------

/// Permissions that an app can request from the OS
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// Access to outbound network / internet
    Internet,
    /// Read/write app-scoped storage
    Storage,
    /// Access the device camera
    Camera,
    /// Access the microphone for recording
    Microphone,
    /// Access GPS / location services
    Location,
    /// Read the user's contacts database
    Contacts,
    /// Make or manage phone calls
    Phone,
    /// Use Bluetooth hardware
    Bluetooth,
    /// Post user-visible notifications
    Notifications,
    /// Modify system-level settings
    SystemSettings,
    /// Run tasks while app is in background
    BackgroundRun,
    /// Elevated administrative privileges
    Admin,
}

impl Permission {
    /// Return a numeric identifier for this permission
    pub fn id(&self) -> u8 {
        match self {
            Permission::Internet => 0,
            Permission::Storage => 1,
            Permission::Camera => 2,
            Permission::Microphone => 3,
            Permission::Location => 4,
            Permission::Contacts => 5,
            Permission::Phone => 6,
            Permission::Bluetooth => 7,
            Permission::Notifications => 8,
            Permission::SystemSettings => 9,
            Permission::BackgroundRun => 10,
            Permission::Admin => 11,
        }
    }

    /// Reconstruct a permission from its numeric id
    pub fn from_id(id: u8) -> Option<Permission> {
        match id {
            0 => Some(Permission::Internet),
            1 => Some(Permission::Storage),
            2 => Some(Permission::Camera),
            3 => Some(Permission::Microphone),
            4 => Some(Permission::Location),
            5 => Some(Permission::Contacts),
            6 => Some(Permission::Phone),
            7 => Some(Permission::Bluetooth),
            8 => Some(Permission::Notifications),
            9 => Some(Permission::SystemSettings),
            10 => Some(Permission::BackgroundRun),
            11 => Some(Permission::Admin),
            _ => None,
        }
    }

    /// Whether the permission is considered dangerous and requires explicit consent
    pub fn is_dangerous(&self) -> bool {
        match self {
            Permission::Camera
            | Permission::Microphone
            | Permission::Location
            | Permission::Contacts
            | Permission::Phone
            | Permission::SystemSettings
            | Permission::Admin => true,
            _ => false,
        }
    }

    /// Return all known permission variants
    pub fn all() -> Vec<Permission> {
        vec![
            Permission::Internet,
            Permission::Storage,
            Permission::Camera,
            Permission::Microphone,
            Permission::Location,
            Permission::Contacts,
            Permission::Phone,
            Permission::Bluetooth,
            Permission::Notifications,
            Permission::SystemSettings,
            Permission::BackgroundRun,
            Permission::Admin,
        ]
    }
}

// ---------------------------------------------------------------------------
// AppManifest
// ---------------------------------------------------------------------------

/// Application manifest describing an app's identity and requirements
#[derive(Debug, Clone)]
pub struct AppManifest {
    /// Hash of the application name
    pub name_hash: u64,
    /// Semantic version packed as (major << 16 | minor << 8 | patch)
    pub version: u32,
    /// Permissions the app requests at install time
    pub permissions: Vec<Permission>,
    /// Hash of the entry point symbol / function name
    pub entry_point_hash: u64,
    /// Hash of the app icon resource
    pub icon_hash: u64,
    /// Minimum OS version required to run this app
    pub min_os_version: u32,
}

impl AppManifest {
    /// Create a new manifest with the given name hash and version
    pub fn new(name_hash: u64, version: u32) -> Self {
        Self {
            name_hash,
            version,
            permissions: Vec::new(),
            entry_point_hash: 0,
            icon_hash: 0,
            min_os_version: MIN_OS_VERSION,
        }
    }

    /// Add a permission to the manifest
    pub fn add_permission(&mut self, perm: Permission) {
        if self.permissions.len() < MAX_PERMISSIONS_PER_APP && !self.permissions.contains(&perm) {
            self.permissions.push(perm);
        }
    }

    /// Check whether this manifest requests a specific permission
    pub fn has_permission(&self, perm: Permission) -> bool {
        self.permissions.contains(&perm)
    }

    /// Validate the manifest for correctness
    pub fn validate(&self) -> bool {
        if self.name_hash == 0 {
            return false;
        }
        if self.entry_point_hash == 0 {
            return false;
        }
        if self.permissions.len() > MAX_PERMISSIONS_PER_APP {
            return false;
        }
        if self.min_os_version > SDK_VERSION {
            return false;
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Intent — inter-app messaging
// ---------------------------------------------------------------------------

/// An intent for inter-app communication
#[derive(Debug, Clone)]
pub struct Intent {
    /// Hash of the action (e.g. "VIEW", "SHARE", "EDIT")
    pub action_hash: u64,
    /// Target app name hash (0 = broadcast)
    pub target_hash: u64,
    /// Payload data (opaque bytes, interpreted by receiver)
    pub data: Vec<u8>,
    /// Source app id
    pub source_app_id: u32,
    /// Timestamp (kernel ticks)
    pub timestamp: u64,
}

// ---------------------------------------------------------------------------
// RegisteredApp — runtime tracking of an installed SDK app
// ---------------------------------------------------------------------------

/// Runtime state for a registered application
#[derive(Debug, Clone)]
struct RegisteredApp {
    /// Unique app id assigned at registration
    app_id: u32,
    /// The manifest provided at registration
    manifest: AppManifest,
    /// Bitmask of granted permissions (bit = Permission::id())
    granted_permissions: u32,
    /// Current lifecycle state
    state: u8,
    /// Activity stack depth (how many screens deep)
    activity_depth: u32,
}

// ---------------------------------------------------------------------------
// SystemInfo
// ---------------------------------------------------------------------------

/// System information returned to apps
#[derive(Debug, Clone)]
pub struct SystemInfo {
    /// OS version (major << 16 | minor << 8 | patch)
    pub os_version: u32,
    /// SDK version
    pub sdk_version: u32,
    /// Total RAM in KB
    pub total_ram_kb: u32,
    /// Available RAM in KB
    pub available_ram_kb: u32,
    /// Number of CPU cores
    pub cpu_cores: u32,
    /// Screen width in pixels
    pub screen_width: u32,
    /// Screen height in pixels
    pub screen_height: u32,
    /// Whether network is available
    pub network_available: bool,
    /// Whether GPS is available
    pub gps_available: bool,
    /// Whether camera hardware is present
    pub camera_available: bool,
    /// Battery level (0-100 as Q16 fixed-point, 100 << 16)
    pub battery_q16: i32,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Global registry of all SDK-registered applications
static SDK_REGISTRY: Mutex<Option<SdkState>> = Mutex::new(None);

/// Internal SDK state
struct SdkState {
    /// All registered apps
    apps: Vec<RegisteredApp>,
    /// Next app id to assign
    next_app_id: u32,
    /// Pending intent queue
    intent_queue: Vec<Intent>,
    /// Whether the SDK subsystem is initialized
    initialized: bool,
}

impl SdkState {
    fn new() -> Self {
        Self {
            apps: Vec::new(),
            next_app_id: 1,
            intent_queue: Vec::new(),
            initialized: true,
        }
    }
}

// ---------------------------------------------------------------------------
// SdkApi — the public API surface
// ---------------------------------------------------------------------------

/// The SDK API that app developers use to interact with the OS
pub struct SdkApi;

impl SdkApi {
    /// Register a new application with the OS
    ///
    /// Returns the assigned app_id on success, or 0 on failure.
    pub fn register_app(manifest: AppManifest) -> u32 {
        if !manifest.validate() {
            serial_println!("[sdk] register_app: invalid manifest");
            return 0;
        }

        let mut guard = SDK_REGISTRY.lock();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => return 0,
        };

        if state.apps.len() >= MAX_REGISTERED_APPS {
            serial_println!("[sdk] register_app: registry full");
            return 0;
        }

        // Check for duplicate name hash
        for app in &state.apps {
            if app.manifest.name_hash == manifest.name_hash {
                serial_println!("[sdk] register_app: duplicate name_hash");
                return 0;
            }
        }

        let app_id = state.next_app_id;
        state.next_app_id = state.next_app_id.saturating_add(1);

        // Auto-grant non-dangerous permissions
        let mut granted: u32 = 0;
        for perm in &manifest.permissions {
            if !perm.is_dangerous() {
                granted |= 1 << perm.id();
            }
        }

        let registered = RegisteredApp {
            app_id,
            manifest,
            granted_permissions: granted,
            state: APP_STATE_REGISTERED,
            activity_depth: 0,
        };

        state.apps.push(registered);
        serial_println!("[sdk] registered app id={}", app_id);
        app_id
    }

    /// Request a runtime permission for an app
    ///
    /// Returns true if the permission is granted.
    /// Non-dangerous permissions are auto-granted.
    /// Dangerous permissions require user consent (simulated as granted here).
    pub fn request_permission(app_id: u32, perm: Permission) -> bool {
        let mut guard = SDK_REGISTRY.lock();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => return false,
        };

        let app = match state.apps.iter_mut().find(|a| a.app_id == app_id) {
            Some(a) => a,
            None => return false,
        };

        // Must be declared in manifest
        if !app.manifest.has_permission(perm) {
            serial_println!(
                "[sdk] permission {} not in manifest for app {}",
                perm.id(),
                app_id
            );
            return false;
        }

        // Already granted?
        let bit = 1u32 << perm.id();
        if app.granted_permissions & bit != 0 {
            return true;
        }

        // For dangerous permissions, in a real OS we'd show a dialog.
        // Here we simulate consent being granted.
        if perm.is_dangerous() {
            serial_println!(
                "[sdk] granting dangerous permission {} to app {}",
                perm.id(),
                app_id
            );
        }

        app.granted_permissions |= bit;
        true
    }

    /// Check whether a permission is currently granted for an app
    pub fn check_permission(app_id: u32, perm: Permission) -> bool {
        let guard = SDK_REGISTRY.lock();
        let state = match guard.as_ref() {
            Some(s) => s,
            None => return false,
        };

        match state.apps.iter().find(|a| a.app_id == app_id) {
            Some(app) => app.granted_permissions & (1 << perm.id()) != 0,
            None => false,
        }
    }

    /// Retrieve system information
    pub fn get_system_info() -> SystemInfo {
        SystemInfo {
            os_version: 0x00010000,
            sdk_version: SDK_VERSION,
            total_ram_kb: 262144,     // 256 MB
            available_ram_kb: 131072, // 128 MB free
            cpu_cores: 4,
            screen_width: 1920,
            screen_height: 1080,
            network_available: true,
            gps_available: true,
            camera_available: true,
            battery_q16: 85 << 16, // 85%
        }
    }

    /// Send an intent from one app to another (or broadcast)
    ///
    /// Returns true if the intent was queued successfully.
    pub fn send_intent(app_id: u32, action_hash: u64, target_hash: u64, data: Vec<u8>) -> bool {
        let mut guard = SDK_REGISTRY.lock();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => return false,
        };

        // Verify sender exists
        if !state.apps.iter().any(|a| a.app_id == app_id) {
            return false;
        }

        if state.intent_queue.len() >= MAX_INTENT_QUEUE {
            serial_println!("[sdk] intent queue full");
            return false;
        }

        let intent = Intent {
            action_hash,
            target_hash,
            data,
            source_app_id: app_id,
            timestamp: 0, // filled by kernel time if available
        };

        state.intent_queue.push(intent);
        serial_println!(
            "[sdk] intent queued from app {} action=0x{:016X}",
            app_id,
            action_hash
        );
        true
    }

    /// Receive the next pending intent for a given app
    ///
    /// Returns None if no intents are pending for this app.
    pub fn receive_intent(app_id: u32) -> Option<Intent> {
        let mut guard = SDK_REGISTRY.lock();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => return None,
        };

        // Find the first intent targeted at this app (by name_hash) or broadcast
        let app_name_hash = match state.apps.iter().find(|a| a.app_id == app_id) {
            Some(app) => app.manifest.name_hash,
            None => return None,
        };

        let idx = state
            .intent_queue
            .iter()
            .position(|i| i.target_hash == app_name_hash || i.target_hash == 0);

        match idx {
            Some(i) => Some(state.intent_queue.remove(i)),
            None => None,
        }
    }

    /// Start a new activity (push onto the activity stack)
    ///
    /// Returns the new activity depth.
    pub fn start_activity(app_id: u32) -> u32 {
        let mut guard = SDK_REGISTRY.lock();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => return 0,
        };

        let app = match state.apps.iter_mut().find(|a| a.app_id == app_id) {
            Some(a) => a,
            None => return 0,
        };

        app.activity_depth = app.activity_depth.saturating_add(1);
        app.state = APP_STATE_FOREGROUND;
        serial_println!(
            "[sdk] app {} start_activity depth={}",
            app_id,
            app.activity_depth
        );
        app.activity_depth
    }

    /// Finish the current activity (pop from the activity stack)
    ///
    /// If the stack is empty after finishing, the app moves to STOPPED state.
    /// Returns the remaining activity depth.
    pub fn finish(app_id: u32) -> u32 {
        let mut guard = SDK_REGISTRY.lock();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => return 0,
        };

        let app = match state.apps.iter_mut().find(|a| a.app_id == app_id) {
            Some(a) => a,
            None => return 0,
        };

        if app.activity_depth > 0 {
            app.activity_depth = app.activity_depth.saturating_sub(1);
        }

        if app.activity_depth == 0 {
            app.state = APP_STATE_STOPPED;
            serial_println!("[sdk] app {} finished (stopped)", app_id);
        } else {
            serial_println!("[sdk] app {} finish depth={}", app_id, app.activity_depth);
        }

        app.activity_depth
    }

    /// Get the current state of an app
    pub fn get_app_state(app_id: u32) -> u8 {
        let guard = SDK_REGISTRY.lock();
        let state = match guard.as_ref() {
            Some(s) => s,
            None => return APP_STATE_UNINIT,
        };

        match state.apps.iter().find(|a| a.app_id == app_id) {
            Some(app) => app.state,
            None => APP_STATE_UNINIT,
        }
    }

    /// Move an app to the background
    pub fn move_to_background(app_id: u32) -> bool {
        let mut guard = SDK_REGISTRY.lock();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => return false,
        };

        let app = match state.apps.iter_mut().find(|a| a.app_id == app_id) {
            Some(a) => a,
            None => return false,
        };

        if app.state == APP_STATE_FOREGROUND {
            app.state = APP_STATE_BACKGROUND;
            serial_println!("[sdk] app {} moved to background", app_id);
            true
        } else {
            false
        }
    }

    /// Move an app to the foreground
    pub fn move_to_foreground(app_id: u32) -> bool {
        let mut guard = SDK_REGISTRY.lock();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => return false,
        };

        let app = match state.apps.iter_mut().find(|a| a.app_id == app_id) {
            Some(a) => a,
            None => return false,
        };

        if app.state == APP_STATE_BACKGROUND || app.state == APP_STATE_PAUSED {
            app.state = APP_STATE_FOREGROUND;
            serial_println!("[sdk] app {} moved to foreground", app_id);
            true
        } else {
            false
        }
    }

    /// Get the total number of registered apps
    pub fn registered_app_count() -> usize {
        let guard = SDK_REGISTRY.lock();
        match guard.as_ref() {
            Some(s) => s.apps.len(),
            None => 0,
        }
    }

    /// Unregister an app, removing it from the registry
    pub fn unregister_app(app_id: u32) -> bool {
        let mut guard = SDK_REGISTRY.lock();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => return false,
        };

        let before = state.apps.len();
        state.apps.retain(|a| a.app_id != app_id);
        let removed = state.apps.len() < before;
        if removed {
            // Also remove any pending intents from this app
            state.intent_queue.retain(|i| i.source_app_id != app_id);
            serial_println!("[sdk] unregistered app {}", app_id);
        }
        removed
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the app SDK subsystem
pub fn init() {
    let mut guard = SDK_REGISTRY.lock();
    *guard = Some(SdkState::new());
    serial_println!("[sdk] app SDK initialized (version 0x{:08X})", SDK_VERSION);
}
