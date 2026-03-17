/// install_manager.rs — app package registry and installation manager.
///
/// Provides:
/// - `AppEntry` struct: name, version, size, installed flag.
/// - A static app registry of up to 64 entries.
/// - `list_apps()` — return a reference to the registry slice.
/// - `install_app(name)` — mark an app as installed (stub).
/// - `uninstall_app(name)` — mark an app as uninstalled (stub).
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of apps in the registry.
pub const APP_REGISTRY_SIZE: usize = 64;

/// Maximum length of an app name (bytes).
pub const APP_NAME_LEN: usize = 48;

/// Maximum length of a version string such as "1.2.3" (bytes).
pub const VERSION_LEN: usize = 16;

// ---------------------------------------------------------------------------
// AppEntry
// ---------------------------------------------------------------------------

/// A single application record in the package registry.
#[derive(Clone, Copy)]
pub struct AppEntry {
    /// Application name (e.g. "genesis-notes"), null-padded.
    pub name: [u8; APP_NAME_LEN],
    pub name_len: usize,

    /// Version string (e.g. "2.0.1"), null-padded.
    pub version: [u8; VERSION_LEN],
    pub version_len: usize,

    /// Installed package size in bytes.
    pub size: u64,

    /// Whether the application is currently installed.
    pub installed: bool,

    /// Whether this registry slot is occupied.
    pub valid: bool,
}

impl AppEntry {
    /// Construct an empty / unused slot.
    pub const fn empty() -> Self {
        Self {
            name: [0u8; APP_NAME_LEN],
            name_len: 0,
            version: [0u8; VERSION_LEN],
            version_len: 0,
            size: 0,
            installed: false,
            valid: false,
        }
    }

    /// Return the app name as a `&str`.
    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("")
    }

    /// Return the version string as a `&str`.
    pub fn version_str(&self) -> &str {
        core::str::from_utf8(&self.version[..self.version_len]).unwrap_or("")
    }
}

// ---------------------------------------------------------------------------
// Static registry
// ---------------------------------------------------------------------------

static mut APP_REGISTRY: [AppEntry; APP_REGISTRY_SIZE] = [AppEntry::empty(); APP_REGISTRY_SIZE];

/// Number of valid entries in the registry (installed or not).
static mut REGISTRY_LEN: usize = 0;

// ---------------------------------------------------------------------------
// register_app (internal helper)
// ---------------------------------------------------------------------------

/// Register a new app in the registry without installing it.
///
/// Used during system initialisation to populate the catalogue.
/// Returns the slot index, or `None` if the registry is full or the
/// name already exists.
pub fn register_app(name: &str, version: &str, size: u64) -> Option<usize> {
    if name.len() > APP_NAME_LEN || version.len() > VERSION_LEN {
        return None;
    }
    // Prevent duplicate names.
    unsafe {
        for entry in APP_REGISTRY.iter() {
            if entry.valid && &entry.name[..entry.name_len] == name.as_bytes() {
                return None;
            }
        }
        for (idx, slot) in APP_REGISTRY.iter_mut().enumerate() {
            if !slot.valid {
                let nlen = name.len();
                slot.name[..nlen].copy_from_slice(name.as_bytes());
                slot.name_len = nlen;

                let vlen = version.len();
                slot.version[..vlen].copy_from_slice(version.as_bytes());
                slot.version_len = vlen;

                slot.size = size;
                slot.installed = false;
                slot.valid = true;
                REGISTRY_LEN = REGISTRY_LEN.saturating_add(1);
                return Some(idx);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// list_apps
// ---------------------------------------------------------------------------

/// Return a reference to the entire app registry slice.
///
/// Callers should iterate and filter on `entry.valid` to obtain only
/// active entries.
pub fn list_apps() -> &'static [AppEntry] {
    unsafe { &APP_REGISTRY }
}

/// Return the number of valid (registered) apps.
pub fn app_count() -> usize {
    unsafe { REGISTRY_LEN }
}

/// Return the number of installed apps.
pub fn installed_count() -> usize {
    let mut count = 0usize;
    unsafe {
        for entry in APP_REGISTRY.iter() {
            if entry.valid && entry.installed {
                count += 1;
            }
        }
    }
    count
}

// ---------------------------------------------------------------------------
// install_app
// ---------------------------------------------------------------------------

/// Mark the app named `name` as installed.
///
/// In a real implementation this would:
///   1. Download / verify the package from the repository.
///   2. Extract files into the apps filesystem.
///   3. Run the installer hook.
///
/// Here it is a stub that updates the `installed` flag and logs.
///
/// Returns `true` if the app was found and not already installed.
pub fn install_app(name: &str) -> bool {
    let needle = name.as_bytes();
    unsafe {
        for entry in APP_REGISTRY.iter_mut() {
            if entry.valid && &entry.name[..entry.name_len] == needle {
                if entry.installed {
                    serial_println!(
                        "[install_manager] install_app '{}': already installed",
                        name
                    );
                    return false;
                }
                entry.installed = true;
                serial_println!(
                    "[install_manager] install_app '{}' v{}  size={} bytes  [stub: OK]",
                    name,
                    core::str::from_utf8(&entry.version[..entry.version_len]).unwrap_or("?"),
                    entry.size
                );
                return true;
            }
        }
    }
    serial_println!(
        "[install_manager] install_app '{}': not found in registry",
        name
    );
    false
}

// ---------------------------------------------------------------------------
// uninstall_app
// ---------------------------------------------------------------------------

/// Mark the app named `name` as uninstalled.
///
/// Stub: clears the `installed` flag and logs.  A real implementation
/// would remove the app's files and run the uninstaller hook.
///
/// Returns `true` if the app was found and was installed.
pub fn uninstall_app(name: &str) -> bool {
    let needle = name.as_bytes();
    unsafe {
        for entry in APP_REGISTRY.iter_mut() {
            if entry.valid && &entry.name[..entry.name_len] == needle {
                if !entry.installed {
                    serial_println!("[install_manager] uninstall_app '{}': not installed", name);
                    return false;
                }
                entry.installed = false;
                serial_println!("[install_manager] uninstall_app '{}' [stub: OK]", name);
                return true;
            }
        }
    }
    serial_println!(
        "[install_manager] uninstall_app '{}': not found in registry",
        name
    );
    false
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    // Pre-populate a small catalogue of built-in Genesis apps.
    let _ = register_app("genesis-files", "1.0.0", 2_048_000);
    let _ = register_app("genesis-browser", "1.0.0", 8_192_000);
    let _ = register_app("genesis-notes", "1.0.0", 1_024_000);
    let _ = register_app("genesis-camera", "1.0.0", 4_096_000);
    let _ = register_app("genesis-maps", "1.0.0", 16_777_216);
    let _ = register_app("genesis-calendar", "1.0.0", 1_536_000);
    let _ = register_app("genesis-contacts", "1.0.0", 768_000);
    let _ = register_app("genesis-messages", "1.0.0", 2_048_000);
    let _ = register_app("genesis-settings", "1.0.0", 512_000);
    let _ = register_app("genesis-terminal", "1.0.0", 1_024_000);

    serial_println!(
        "[install_manager] app registry ready ({} apps registered)",
        unsafe { REGISTRY_LEN }
    );
}
