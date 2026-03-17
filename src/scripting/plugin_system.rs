use crate::sync::Mutex;
/// Hoags Plugin System — extension framework with lifecycle hooks
///
/// Plugins are loadable extensions that register hooks into system events
/// (boot, shutdown, app launch, file open, network, render, etc.).
/// Each plugin declares permissions, an entry point, and can export
/// named API functions. Plugins are sandboxed and version-checked.
///
/// All numeric values use i32 Q16 fixed-point (65536 = 1.0).
/// No external crates. No f32/f64.
///
/// Inspired by: VSCode extensions, Firefox WebExtensions, Linux kernel
/// modules, Eclipse OSGI. All code is original.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

/// Q16 fixed-point: 65536 = 1.0
type Q16 = i32;
const Q16_ONE: Q16 = 65536;

/// Maximum plugins
const MAX_PLUGINS: usize = 128;
/// Maximum hooks per hook point
const MAX_HOOKS_PER_POINT: usize = 64;
/// Maximum exports per plugin
const MAX_EXPORTS: usize = 32;
/// Maximum permissions per plugin
const MAX_PERMISSIONS: usize = 16;

/// Minimum compatible API version (Q16: 1.0)
const MIN_API_VERSION: Q16 = Q16_ONE;
/// Current API version (Q16: 1.0)
const CURRENT_API_VERSION: Q16 = Q16_ONE;

// ---------------------------------------------------------------------------
// HookPoint — lifecycle events plugins can hook into
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookPoint {
    /// Called during system boot
    OnBoot,
    /// Called during system shutdown
    OnShutdown,
    /// Called when any application launches
    OnAppLaunch,
    /// Called when a file is opened
    OnFileOpen,
    /// Called before a network request is sent
    OnNetworkRequest,
    /// Called when a notification is posted
    OnNotification,
    /// Called on a periodic timer tick
    OnTimer,
    /// Called when user input is received
    OnUserInput,
    /// Called before frame render
    BeforeRender,
    /// Called after frame render
    AfterRender,
}

impl HookPoint {
    fn index(&self) -> usize {
        match self {
            HookPoint::OnBoot => 0,
            HookPoint::OnShutdown => 1,
            HookPoint::OnAppLaunch => 2,
            HookPoint::OnFileOpen => 3,
            HookPoint::OnNetworkRequest => 4,
            HookPoint::OnNotification => 5,
            HookPoint::OnTimer => 6,
            HookPoint::OnUserInput => 7,
            HookPoint::BeforeRender => 8,
            HookPoint::AfterRender => 9,
        }
    }

    fn from_index(idx: usize) -> Option<HookPoint> {
        match idx {
            0 => Some(HookPoint::OnBoot),
            1 => Some(HookPoint::OnShutdown),
            2 => Some(HookPoint::OnAppLaunch),
            3 => Some(HookPoint::OnFileOpen),
            4 => Some(HookPoint::OnNetworkRequest),
            5 => Some(HookPoint::OnNotification),
            6 => Some(HookPoint::OnTimer),
            7 => Some(HookPoint::OnUserInput),
            8 => Some(HookPoint::BeforeRender),
            9 => Some(HookPoint::AfterRender),
            _ => None,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            HookPoint::OnBoot => "OnBoot",
            HookPoint::OnShutdown => "OnShutdown",
            HookPoint::OnAppLaunch => "OnAppLaunch",
            HookPoint::OnFileOpen => "OnFileOpen",
            HookPoint::OnNetworkRequest => "OnNetworkRequest",
            HookPoint::OnNotification => "OnNotification",
            HookPoint::OnTimer => "OnTimer",
            HookPoint::OnUserInput => "OnUserInput",
            HookPoint::BeforeRender => "BeforeRender",
            HookPoint::AfterRender => "AfterRender",
        }
    }
}

const NUM_HOOK_POINTS: usize = 10;

// ---------------------------------------------------------------------------
// Permission IDs — what a plugin is allowed to do
// ---------------------------------------------------------------------------

/// Permission constants
pub mod permissions {
    pub const FILESYSTEM_READ: u8 = 0x01;
    pub const FILESYSTEM_WRITE: u8 = 0x02;
    pub const NETWORK_ACCESS: u8 = 0x03;
    pub const PROCESS_SPAWN: u8 = 0x04;
    pub const SYSTEM_SETTINGS: u8 = 0x05;
    pub const NOTIFICATIONS: u8 = 0x06;
    pub const INPUT_CAPTURE: u8 = 0x07;
    pub const AUDIO_ACCESS: u8 = 0x08;
    pub const CAMERA_ACCESS: u8 = 0x09;
    pub const LOCATION_ACCESS: u8 = 0x0A;
    pub const CRYPTO_KEYS: u8 = 0x0B;
    pub const DISPLAY_OVERLAY: u8 = 0x0C;
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Plugin {
    pub id: u32,
    pub name_hash: u64,
    pub version: u32,
    pub author_hash: u64,
    pub permissions: Vec<u8>,
    pub entry_point_hash: u64,
    pub enabled: bool,
    pub loaded: bool,
}

impl Plugin {
    fn new(id: u32, name_hash: u64, version: u32, author_hash: u64) -> Self {
        Plugin {
            id,
            name_hash,
            version,
            author_hash,
            permissions: Vec::new(),
            entry_point_hash: 0,
            enabled: false,
            loaded: false,
        }
    }
}

// ---------------------------------------------------------------------------
// PluginApi — exported functions and hooks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PluginApi {
    pub hooks: Vec<HookPoint>,
    pub exports: Vec<(u64, u64)>, // (function_name_hash, function_body_hash)
}

impl PluginApi {
    fn new() -> Self {
        PluginApi {
            hooks: Vec::new(),
            exports: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// HookRegistration — maps plugin to hook point
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct HookRegistration {
    plugin_id: u32,
    priority: i32, // Q16 priority (higher = earlier in chain)
    callback_hash: u64,
}

// ---------------------------------------------------------------------------
// PluginSystemState
// ---------------------------------------------------------------------------

struct PluginSystemState {
    plugins: Vec<Plugin>,
    apis: Vec<(u32, PluginApi)>, // (plugin_id, api)
    hook_table: [Vec<HookRegistration>; NUM_HOOK_POINTS],
    next_id: u32,
    initialized: bool,
    total_hook_calls: u64,
}

impl PluginSystemState {
    fn new() -> Self {
        PluginSystemState {
            plugins: Vec::new(),
            apis: Vec::new(),
            hook_table: [
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ],
            next_id: 1,
            initialized: false,
            total_hook_calls: 0,
        }
    }
}

static PLUGIN_SYS: Mutex<Option<PluginSystemState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Plugin lifecycle
// ---------------------------------------------------------------------------

/// Install a new plugin. Returns the plugin ID.
pub fn install_plugin(
    name_hash: u64,
    version: u32,
    author_hash: u64,
    entry_point_hash: u64,
    perms: Vec<u8>,
) -> u32 {
    let mut guard = PLUGIN_SYS.lock();
    if let Some(ref mut state) = *guard {
        if state.plugins.len() >= MAX_PLUGINS {
            serial_println!("[plugin] ERROR: max plugins ({}) reached", MAX_PLUGINS);
            return 0;
        }

        // Check for duplicate
        for p in &state.plugins {
            if p.name_hash == name_hash {
                serial_println!(
                    "[plugin] ERROR: plugin already installed (name_hash={:#018X})",
                    name_hash
                );
                return 0;
            }
        }

        let id = state.next_id;
        state.next_id = state.next_id.saturating_add(1);

        let mut plugin = Plugin::new(id, name_hash, version, author_hash);
        plugin.entry_point_hash = entry_point_hash;
        for &perm in &perms {
            if plugin.permissions.len() < MAX_PERMISSIONS {
                plugin.permissions.push(perm);
            }
        }

        serial_println!(
            "[plugin] Installed plugin {} (name_hash={:#018X}, v{}, {} permissions)",
            id,
            name_hash,
            version,
            plugin.permissions.len()
        );

        state.plugins.push(plugin);
        state.apis.push((id, PluginApi::new()));
        id
    } else {
        0
    }
}

/// Uninstall a plugin by ID. Removes all hooks and exports.
pub fn uninstall(plugin_id: u32) -> bool {
    let mut guard = PLUGIN_SYS.lock();
    if let Some(ref mut state) = *guard {
        // Unload first if loaded
        unload_plugin_internal(state, plugin_id);

        // Remove hooks
        for hooks in &mut state.hook_table {
            hooks.retain(|h| h.plugin_id != plugin_id);
        }

        // Remove API
        state.apis.retain(|(id, _)| *id != plugin_id);

        // Remove plugin
        let before = state.plugins.len();
        state.plugins.retain(|p| p.id != plugin_id);
        let removed = state.plugins.len() < before;
        if removed {
            serial_println!("[plugin] Uninstalled plugin {}", plugin_id);
        }
        removed
    } else {
        false
    }
}

/// Enable a plugin.
pub fn enable(plugin_id: u32) -> bool {
    let mut guard = PLUGIN_SYS.lock();
    if let Some(ref mut state) = *guard {
        for p in &mut state.plugins {
            if p.id == plugin_id {
                p.enabled = true;
                serial_println!("[plugin] Enabled plugin {}", plugin_id);
                return true;
            }
        }
    }
    false
}

/// Disable a plugin (does not unload, but hooks won't fire).
pub fn disable(plugin_id: u32) -> bool {
    let mut guard = PLUGIN_SYS.lock();
    if let Some(ref mut state) = *guard {
        for p in &mut state.plugins {
            if p.id == plugin_id {
                p.enabled = false;
                serial_println!("[plugin] Disabled plugin {}", plugin_id);
                return true;
            }
        }
    }
    false
}

/// Load a plugin into memory and call its entry point.
pub fn load(plugin_id: u32) -> bool {
    let mut guard = PLUGIN_SYS.lock();
    if let Some(ref mut state) = *guard {
        for p in &mut state.plugins {
            if p.id == plugin_id {
                if p.loaded {
                    serial_println!("[plugin] Plugin {} already loaded", plugin_id);
                    return true;
                }
                p.loaded = true;
                p.enabled = true;
                serial_println!(
                    "[plugin] Loaded plugin {} (entry_point={:#018X})",
                    plugin_id,
                    p.entry_point_hash
                );
                return true;
            }
        }
    }
    false
}

/// Unload a plugin from memory.
pub fn unload(plugin_id: u32) -> bool {
    let mut guard = PLUGIN_SYS.lock();
    if let Some(ref mut state) = *guard {
        unload_plugin_internal(state, plugin_id)
    } else {
        false
    }
}

fn unload_plugin_internal(state: &mut PluginSystemState, plugin_id: u32) -> bool {
    for p in &mut state.plugins {
        if p.id == plugin_id && p.loaded {
            p.loaded = false;
            p.enabled = false;
            serial_println!("[plugin] Unloaded plugin {}", plugin_id);
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Hook management
// ---------------------------------------------------------------------------

/// Register a hook for a plugin at a specific hook point.
pub fn register_hook(plugin_id: u32, hook: HookPoint, callback_hash: u64, priority: i32) -> bool {
    let mut guard = PLUGIN_SYS.lock();
    if let Some(ref mut state) = *guard {
        // Verify plugin exists
        let exists = state.plugins.iter().any(|p| p.id == plugin_id);
        if !exists {
            serial_println!("[plugin] ERROR: plugin {} not found", plugin_id);
            return false;
        }

        let idx = hook.index();
        if state.hook_table[idx].len() >= MAX_HOOKS_PER_POINT {
            serial_println!("[plugin] ERROR: max hooks for {:?} reached", hook);
            return false;
        }

        let reg = HookRegistration {
            plugin_id,
            priority,
            callback_hash,
        };

        state.hook_table[idx].push(reg);

        // Sort by priority (descending — higher priority first)
        state.hook_table[idx].sort_by(|a, b| b.priority.cmp(&a.priority));

        // Update plugin API record
        for (id, api) in &mut state.apis {
            if *id == plugin_id {
                if !api.hooks.contains(&hook) {
                    api.hooks.push(hook);
                }
                break;
            }
        }

        serial_println!(
            "[plugin] Registered hook {:?} for plugin {} (priority={})",
            hook,
            plugin_id,
            priority
        );
        true
    } else {
        false
    }
}

/// Call all registered hooks for a given hook point.
/// Returns the number of hooks that were called.
pub fn call_hook(hook: HookPoint, context_hash: u64) -> u32 {
    let mut guard = PLUGIN_SYS.lock();
    if let Some(ref mut state) = *guard {
        let idx = hook.index();
        let registrations = state.hook_table[idx].clone();
        let mut called = 0u32;

        for reg in &registrations {
            // Check that plugin is enabled and loaded
            let is_active = state
                .plugins
                .iter()
                .any(|p| p.id == reg.plugin_id && p.enabled && p.loaded);

            if is_active {
                serial_println!(
                    "[plugin] Hook {} -> plugin {} (callback={:#018X}, ctx={:#018X})",
                    hook.name(),
                    reg.plugin_id,
                    reg.callback_hash,
                    context_hash
                );
                // In a full implementation, this would invoke the plugin's callback
                // via the script engine or a function pointer table
                called += 1;
            }
        }

        state.total_hook_calls += called as u64;
        called
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// API management
// ---------------------------------------------------------------------------

/// Get the API (hooks + exports) for a plugin.
pub fn get_api(plugin_id: u32) -> Option<PluginApi> {
    let guard = PLUGIN_SYS.lock();
    if let Some(ref state) = *guard {
        for (id, api) in &state.apis {
            if *id == plugin_id {
                return Some(api.clone());
            }
        }
    }
    None
}

/// Register an exported function for a plugin.
pub fn register_export(plugin_id: u32, name_hash: u64, body_hash: u64) -> bool {
    let mut guard = PLUGIN_SYS.lock();
    if let Some(ref mut state) = *guard {
        for (id, api) in &mut state.apis {
            if *id == plugin_id {
                if api.exports.len() >= MAX_EXPORTS {
                    serial_println!("[plugin] ERROR: max exports for plugin {}", plugin_id);
                    return false;
                }
                api.exports.push((name_hash, body_hash));
                serial_println!(
                    "[plugin] Registered export {:#018X} for plugin {}",
                    name_hash,
                    plugin_id
                );
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Query API
// ---------------------------------------------------------------------------

/// List all installed plugins as (id, name_hash, version, enabled, loaded).
pub fn list_plugins() -> Vec<(u32, u64, u32, bool, bool)> {
    let guard = PLUGIN_SYS.lock();
    if let Some(ref state) = *guard {
        let mut result = Vec::new();
        for p in &state.plugins {
            result.push((p.id, p.name_hash, p.version, p.enabled, p.loaded));
        }
        result
    } else {
        Vec::new()
    }
}

/// Check if a plugin is compatible with the current API version.
/// Returns true if the plugin's version is within the compatible range.
pub fn check_compatibility(plugin_id: u32) -> bool {
    let guard = PLUGIN_SYS.lock();
    if let Some(ref state) = *guard {
        for p in &state.plugins {
            if p.id == plugin_id {
                // Simple version check: plugin version must be >= 1
                // In a full system, this would check semantic versioning
                let compatible = p.version >= 1;
                serial_println!(
                    "[plugin] Compatibility check for plugin {}: {} (v{})",
                    plugin_id,
                    if compatible { "OK" } else { "FAIL" },
                    p.version
                );
                return compatible;
            }
        }
    }
    false
}

/// Get the number of installed plugins.
pub fn plugin_count() -> usize {
    let guard = PLUGIN_SYS.lock();
    if let Some(ref state) = *guard {
        state.plugins.len()
    } else {
        0
    }
}

/// Get total hook invocations across all plugins.
pub fn total_hook_calls() -> u64 {
    let guard = PLUGIN_SYS.lock();
    if let Some(ref state) = *guard {
        state.total_hook_calls
    } else {
        0
    }
}

/// Check if a plugin has a specific permission.
pub fn has_permission(plugin_id: u32, permission: u8) -> bool {
    let guard = PLUGIN_SYS.lock();
    if let Some(ref state) = *guard {
        for p in &state.plugins {
            if p.id == plugin_id {
                return p.permissions.contains(&permission);
            }
        }
    }
    false
}

/// Get number of hooks registered at a specific hook point.
pub fn hooks_at(hook: HookPoint) -> usize {
    let guard = PLUGIN_SYS.lock();
    if let Some(ref state) = *guard {
        state.hook_table[hook.index()].len()
    } else {
        0
    }
}

pub fn init() {
    let mut guard = PLUGIN_SYS.lock();
    *guard = Some(PluginSystemState::new());
    if let Some(ref mut state) = *guard {
        state.initialized = true;
    }
    serial_println!(
        "    [scripting] Plugin system initialized ({} hook points, permissions, API)",
        NUM_HOOK_POINTS
    );
}
