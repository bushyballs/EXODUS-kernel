use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
/// Hoags Safe Mode — restricted boot modes for Genesis OS
///
/// Features:
///   - Minimal safe mode: core kernel + essential drivers only
///   - Diagnostic mode: all drivers loaded, logging maximized, no user services
///   - Network safe mode: minimal boot with networking for remote diagnostics
///   - Driver whitelist/blacklist management for safe boot
///   - Service filtering (only critical services in safe mode)
///   - Boot mode persistence across reboots
///   - Diagnostic log collection for post-boot analysis
///   - Automatic safe mode entry after repeated boot failures
///
/// All values use Q16 fixed-point (i32, 1.0 = 65536) where applicable.
/// No floating-point. No external crates. All code is original.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers (1.0 = 65536)
// ---------------------------------------------------------------------------

const Q16_ONE: i32 = 65536;

fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    (((a as i64) * (Q16_ONE as i64)) / (b as i64)) as i32
}

fn q16_from_int(v: i32) -> i32 {
    v * Q16_ONE
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Available boot modes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootMode {
    /// Normal boot with all drivers and services
    Normal,
    /// Minimal safe mode: essential kernel + core drivers only
    SafeMinimal,
    /// Safe mode with networking enabled for remote repair
    SafeNetwork,
    /// Full diagnostic mode: max logging, no user services
    Diagnostic,
    /// Recovery console: text-only, repair tools available
    RecoveryConsole,
}

/// State of the safe mode boot process
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafeBootState {
    /// Not in safe mode
    Inactive,
    /// Selecting boot mode (boot menu displayed)
    ModeSelection,
    /// Loading essential drivers
    LoadingDrivers,
    /// Starting critical services
    StartingServices,
    /// Safe mode is active and ready
    Active,
    /// Transitioning back to normal mode
    ExitingToNormal,
}

/// Category for a driver or service in the safe mode filter
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentClass {
    /// Essential — always loaded in every mode
    Essential,
    /// Core — loaded in safe mode and above
    Core,
    /// Standard — loaded in diagnostic and normal modes
    Standard,
    /// Optional — only loaded in normal mode
    Optional,
    /// Blacklisted — never loaded (user override)
    Blacklisted,
}

/// Severity level for diagnostic log entries
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagLogLevel {
    Trace,
    Debug,
    Info,
    Warning,
    Error,
    Critical,
}

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// A driver entry in the safe mode filter list
#[derive(Debug, Clone)]
pub struct DriverEntry {
    /// Hash of the driver name
    pub name_hash: u64,
    /// Classification for safe mode filtering
    pub class: ComponentClass,
    /// Whether this driver loaded successfully on last boot
    pub last_load_ok: bool,
    /// Number of consecutive load failures
    pub failure_count: u8,
    /// Maximum allowed failures before auto-blacklist
    pub max_failures: u8,
}

/// A service entry in the safe mode filter list
#[derive(Debug, Clone)]
pub struct ServiceEntry {
    /// Hash of the service name
    pub name_hash: u64,
    /// Classification for safe mode filtering
    pub class: ComponentClass,
    /// Whether this service is currently running
    pub running: bool,
    /// Number of consecutive crash restarts
    pub crash_count: u8,
    /// Maximum crash restarts before disabling
    pub max_crashes: u8,
}

/// A diagnostic log entry captured during safe mode
#[derive(Debug, Clone)]
pub struct DiagLogEntry {
    /// Timestamp (ticks since boot)
    pub timestamp: u64,
    /// Log level
    pub level: DiagLogLevel,
    /// Hash of the source module name
    pub source_hash: u64,
    /// Hash of the log message
    pub message_hash: u64,
    /// Associated numeric value (context-dependent)
    pub value: u64,
}

/// Boot failure record for auto-safe-mode detection
#[derive(Debug, Clone, Copy)]
struct BootFailureRecord {
    /// Timestamp of the failure
    timestamp: u64,
    /// Hash identifying the failure cause
    cause_hash: u64,
    /// Which boot stage the failure occurred in
    stage: u8,
}

// ---------------------------------------------------------------------------
// Safe mode manager state
// ---------------------------------------------------------------------------

struct SafeModeManager {
    /// Current boot mode
    current_mode: BootMode,
    /// Current boot state
    state: SafeBootState,
    /// Driver filter list
    drivers: Vec<DriverEntry>,
    /// Service filter list
    services: Vec<ServiceEntry>,
    /// Diagnostic log buffer
    diag_log: Vec<DiagLogEntry>,
    /// Boot failure history
    boot_failures: Vec<BootFailureRecord>,
    /// Maximum diagnostic log entries
    max_log_entries: usize,
    /// Number of consecutive boot failures before auto-safe-mode
    auto_safe_threshold: u8,
    /// Current consecutive failure count
    consecutive_failures: u8,
    /// Whether safe mode was triggered automatically
    auto_triggered: bool,
    /// Boot progress (Q16)
    boot_progress_q16: i32,
    /// Requested mode for next reboot
    next_boot_mode: BootMode,
}

impl SafeModeManager {
    const fn new() -> Self {
        SafeModeManager {
            current_mode: BootMode::Normal,
            state: SafeBootState::Inactive,
            drivers: Vec::new(),
            services: Vec::new(),
            diag_log: Vec::new(),
            boot_failures: Vec::new(),
            max_log_entries: 256,
            auto_safe_threshold: 3,
            consecutive_failures: 0,
            auto_triggered: false,
            boot_progress_q16: 0,
            next_boot_mode: BootMode::Normal,
        }
    }
}

static SAFE_MODE: Mutex<Option<SafeModeManager>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Default driver/service hash constants
// ---------------------------------------------------------------------------

const DRV_PCI_BUS: u64 = 0xAA11BB22CC330001;
const DRV_STORAGE: u64 = 0xAA11BB22CC330002;
const DRV_DISPLAY_BASIC: u64 = 0xAA11BB22CC330003;
const DRV_KEYBOARD: u64 = 0xAA11BB22CC330004;
const DRV_SERIAL: u64 = 0xAA11BB22CC330005;
const DRV_NETWORK: u64 = 0xAA11BB22CC330006;
const DRV_USB_HCI: u64 = 0xAA11BB22CC330007;
const DRV_AUDIO: u64 = 0xAA11BB22CC330008;
const DRV_GPU_ACCEL: u64 = 0xAA11BB22CC330009;
const DRV_BLUETOOTH: u64 = 0xAA11BB22CC33000A;
const DRV_CAMERA: u64 = 0xAA11BB22CC33000B;
const DRV_SENSOR_HUB: u64 = 0xAA11BB22CC33000C;

const SVC_INIT: u64 = 0xDD44EE55FF660001;
const SVC_SYSLOG: u64 = 0xDD44EE55FF660002;
const SVC_FILESYSTEM: u64 = 0xDD44EE55FF660003;
const SVC_NETWORK_MGR: u64 = 0xDD44EE55FF660004;
const SVC_DISPLAY_SRV: u64 = 0xDD44EE55FF660005;
const SVC_SECURITY: u64 = 0xDD44EE55FF660006;
const SVC_USER_SESSION: u64 = 0xDD44EE55FF660007;
const SVC_SCHEDULER: u64 = 0xDD44EE55FF660008;
const SVC_BLUETOOTH: u64 = 0xDD44EE55FF660009;
const SVC_MEDIA: u64 = 0xDD44EE55FF66000A;
const SVC_LOCATION: u64 = 0xDD44EE55FF66000B;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Create the default driver list with safe-mode classifications
fn create_default_drivers() -> Vec<DriverEntry> {
    let entries = vec![
        (DRV_PCI_BUS, ComponentClass::Essential),
        (DRV_STORAGE, ComponentClass::Essential),
        (DRV_DISPLAY_BASIC, ComponentClass::Essential),
        (DRV_KEYBOARD, ComponentClass::Essential),
        (DRV_SERIAL, ComponentClass::Essential),
        (DRV_NETWORK, ComponentClass::Core),
        (DRV_USB_HCI, ComponentClass::Core),
        (DRV_AUDIO, ComponentClass::Standard),
        (DRV_GPU_ACCEL, ComponentClass::Standard),
        (DRV_BLUETOOTH, ComponentClass::Optional),
        (DRV_CAMERA, ComponentClass::Optional),
        (DRV_SENSOR_HUB, ComponentClass::Optional),
    ];

    entries
        .into_iter()
        .map(|(hash, class)| DriverEntry {
            name_hash: hash,
            class,
            last_load_ok: true,
            failure_count: 0,
            max_failures: 3,
        })
        .collect()
}

/// Create the default service list with safe-mode classifications
fn create_default_services() -> Vec<ServiceEntry> {
    let entries = vec![
        (SVC_INIT, ComponentClass::Essential),
        (SVC_SYSLOG, ComponentClass::Essential),
        (SVC_FILESYSTEM, ComponentClass::Essential),
        (SVC_SECURITY, ComponentClass::Essential),
        (SVC_SCHEDULER, ComponentClass::Core),
        (SVC_NETWORK_MGR, ComponentClass::Core),
        (SVC_DISPLAY_SRV, ComponentClass::Standard),
        (SVC_USER_SESSION, ComponentClass::Standard),
        (SVC_BLUETOOTH, ComponentClass::Optional),
        (SVC_MEDIA, ComponentClass::Optional),
        (SVC_LOCATION, ComponentClass::Optional),
    ];

    entries
        .into_iter()
        .map(|(hash, class)| ServiceEntry {
            name_hash: hash,
            class,
            running: false,
            crash_count: 0,
            max_crashes: 5,
        })
        .collect()
}

/// Determine which component classes are allowed for a given boot mode
fn allowed_classes(mode: BootMode) -> Vec<ComponentClass> {
    match mode {
        BootMode::SafeMinimal => vec![ComponentClass::Essential],
        BootMode::SafeNetwork => vec![ComponentClass::Essential, ComponentClass::Core],
        BootMode::Diagnostic => vec![
            ComponentClass::Essential,
            ComponentClass::Core,
            ComponentClass::Standard,
        ],
        BootMode::RecoveryConsole => vec![ComponentClass::Essential],
        BootMode::Normal => vec![
            ComponentClass::Essential,
            ComponentClass::Core,
            ComponentClass::Standard,
            ComponentClass::Optional,
        ],
    }
}

/// Check if a component class is allowed in the given mode
fn is_class_allowed(class: ComponentClass, mode: BootMode) -> bool {
    if matches!(class, ComponentClass::Blacklisted) {
        return false;
    }
    let allowed = allowed_classes(mode);
    allowed.iter().any(|&c| c as u8 == class as u8)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Enter a specific boot mode, loading only allowed drivers and services
pub fn enter_boot_mode(mode: BootMode, timestamp: u64) -> bool {
    let mut guard = SAFE_MODE.lock();
    if let Some(ref mut mgr) = *guard {
        serial_println!("  SafeMode: entering {:?} mode...", mode);

        mgr.current_mode = mode;
        mgr.state = SafeBootState::LoadingDrivers;
        mgr.boot_progress_q16 = 0;

        // Phase 1: Load allowed drivers
        let total_drivers = mgr.drivers.len() as i32;
        let mut loaded_count: i32 = 0;

        for driver in mgr.drivers.iter_mut() {
            if is_class_allowed(driver.class, mode) {
                // Simulate loading the driver
                if driver.failure_count < driver.max_failures {
                    driver.last_load_ok = true;
                    loaded_count += 1;
                } else {
                    driver.last_load_ok = false;
                    driver.class = ComponentClass::Blacklisted;
                    serial_println!(
                        "  SafeMode: auto-blacklisted driver {:016X} (too many failures)",
                        driver.name_hash
                    );
                }
            }
        }

        if total_drivers > 0 {
            mgr.boot_progress_q16 = q16_div(loaded_count, total_drivers);
        }

        serial_println!(
            "  SafeMode: loaded {}/{} drivers",
            loaded_count,
            total_drivers
        );

        // Phase 2: Start allowed services
        mgr.state = SafeBootState::StartingServices;
        let total_services = mgr.services.len() as i32;
        let mut started_count: i32 = 0;

        for service in mgr.services.iter_mut() {
            if is_class_allowed(service.class, mode) {
                if service.crash_count < service.max_crashes {
                    service.running = true;
                    started_count += 1;
                } else {
                    service.running = false;
                    serial_println!(
                        "  SafeMode: disabled service {:016X} (too many crashes)",
                        service.name_hash
                    );
                }
            } else {
                service.running = false;
            }
        }

        serial_println!(
            "  SafeMode: started {}/{} services",
            started_count,
            total_services
        );

        // Log the mode entry
        mgr.diag_log.push(DiagLogEntry {
            timestamp,
            level: DiagLogLevel::Info,
            source_hash: 0xBADC0FFEE0DDF00D,
            message_hash: mode as u64,
            value: loaded_count as u64,
        });

        // Trim log
        while mgr.diag_log.len() > mgr.max_log_entries {
            mgr.diag_log.remove(0);
        }

        mgr.state = SafeBootState::Active;
        mgr.boot_progress_q16 = Q16_ONE;

        serial_println!("  SafeMode: {:?} mode active", mode);
        true
    } else {
        false
    }
}

/// Exit safe mode and schedule normal boot for next reboot
pub fn exit_safe_mode() -> bool {
    let mut guard = SAFE_MODE.lock();
    if let Some(ref mut mgr) = *guard {
        if matches!(mgr.current_mode, BootMode::Normal) {
            serial_println!("  SafeMode: already in normal mode");
            return false;
        }

        mgr.state = SafeBootState::ExitingToNormal;
        mgr.next_boot_mode = BootMode::Normal;
        mgr.auto_triggered = false;
        mgr.consecutive_failures = 0;

        serial_println!("  SafeMode: scheduled normal boot for next reboot");
        true
    } else {
        false
    }
}

/// Record a boot failure, potentially triggering auto-safe-mode
pub fn record_boot_failure(timestamp: u64, cause_hash: u64, stage: u8) {
    let mut guard = SAFE_MODE.lock();
    if let Some(ref mut mgr) = *guard {
        mgr.consecutive_failures += 1;
        mgr.boot_failures.push(BootFailureRecord {
            timestamp,
            cause_hash,
            stage,
        });

        // Keep only last 16 failures
        while mgr.boot_failures.len() > 16 {
            mgr.boot_failures.remove(0);
        }

        serial_println!(
            "  SafeMode: boot failure recorded ({}/{} before auto-safe)",
            mgr.consecutive_failures,
            mgr.auto_safe_threshold
        );

        if mgr.consecutive_failures >= mgr.auto_safe_threshold {
            mgr.next_boot_mode = BootMode::SafeMinimal;
            mgr.auto_triggered = true;
            serial_println!("  SafeMode: auto-safe-mode TRIGGERED (next boot will be SafeMinimal)");
        }
    }
}

/// Mark the current boot as successful, resetting failure counters
pub fn mark_boot_success() {
    let mut guard = SAFE_MODE.lock();
    if let Some(ref mut mgr) = *guard {
        mgr.consecutive_failures = 0;
        mgr.auto_triggered = false;
        serial_println!("  SafeMode: boot success recorded, failure counter reset");
    }
}

/// Add a diagnostic log entry
pub fn log_diagnostic(
    timestamp: u64,
    level: DiagLogLevel,
    source_hash: u64,
    message_hash: u64,
    value: u64,
) {
    let mut guard = SAFE_MODE.lock();
    if let Some(ref mut mgr) = *guard {
        mgr.diag_log.push(DiagLogEntry {
            timestamp,
            level,
            source_hash,
            message_hash,
            value,
        });
        while mgr.diag_log.len() > mgr.max_log_entries {
            mgr.diag_log.remove(0);
        }
    }
}

/// Get all diagnostic log entries
pub fn get_diagnostic_log() -> Vec<DiagLogEntry> {
    let guard = SAFE_MODE.lock();
    if let Some(ref mgr) = *guard {
        mgr.diag_log.clone()
    } else {
        Vec::new()
    }
}

/// Get diagnostic log entries filtered by level
pub fn get_log_by_level(min_level: DiagLogLevel) -> Vec<DiagLogEntry> {
    let guard = SAFE_MODE.lock();
    if let Some(ref mgr) = *guard {
        let min_ord = min_level as u8;
        mgr.diag_log
            .iter()
            .filter(|e| (e.level as u8) >= min_ord)
            .cloned()
            .collect()
    } else {
        Vec::new()
    }
}

/// Get the current boot mode
pub fn current_mode() -> BootMode {
    let guard = SAFE_MODE.lock();
    if let Some(ref mgr) = *guard {
        mgr.current_mode
    } else {
        BootMode::Normal
    }
}

/// Get the current safe boot state
pub fn current_state() -> SafeBootState {
    let guard = SAFE_MODE.lock();
    if let Some(ref mgr) = *guard {
        mgr.state
    } else {
        SafeBootState::Inactive
    }
}

/// Get the next scheduled boot mode
pub fn next_boot_mode() -> BootMode {
    let guard = SAFE_MODE.lock();
    if let Some(ref mgr) = *guard {
        mgr.next_boot_mode
    } else {
        BootMode::Normal
    }
}

/// Check if safe mode was auto-triggered by boot failures
pub fn is_auto_triggered() -> bool {
    let guard = SAFE_MODE.lock();
    if let Some(ref mgr) = *guard {
        mgr.auto_triggered
    } else {
        false
    }
}

/// Blacklist a driver by name hash (prevents loading in all modes)
pub fn blacklist_driver(name_hash: u64) -> bool {
    let mut guard = SAFE_MODE.lock();
    if let Some(ref mut mgr) = *guard {
        if let Some(drv) = mgr.drivers.iter_mut().find(|d| d.name_hash == name_hash) {
            drv.class = ComponentClass::Blacklisted;
            serial_println!("  SafeMode: blacklisted driver {:016X}", name_hash);
            return true;
        }
    }
    false
}

/// Restore a blacklisted driver to its default class
pub fn unblacklist_driver(name_hash: u64, restore_class: ComponentClass) -> bool {
    let mut guard = SAFE_MODE.lock();
    if let Some(ref mut mgr) = *guard {
        if let Some(drv) = mgr.drivers.iter_mut().find(|d| d.name_hash == name_hash) {
            if matches!(drv.class, ComponentClass::Blacklisted) {
                drv.class = restore_class;
                drv.failure_count = 0;
                serial_println!("  SafeMode: unblacklisted driver {:016X}", name_hash);
                return true;
            }
        }
    }
    false
}

/// Get the list of loaded drivers in the current mode
pub fn get_loaded_drivers() -> Vec<DriverEntry> {
    let guard = SAFE_MODE.lock();
    if let Some(ref mgr) = *guard {
        mgr.drivers
            .iter()
            .filter(|d| d.last_load_ok && is_class_allowed(d.class, mgr.current_mode))
            .cloned()
            .collect()
    } else {
        Vec::new()
    }
}

/// Get the list of running services in the current mode
pub fn get_running_services() -> Vec<ServiceEntry> {
    let guard = SAFE_MODE.lock();
    if let Some(ref mgr) = *guard {
        mgr.services.iter().filter(|s| s.running).cloned().collect()
    } else {
        Vec::new()
    }
}

/// Set the auto-safe-mode failure threshold
pub fn set_auto_safe_threshold(threshold: u8) {
    let mut guard = SAFE_MODE.lock();
    if let Some(ref mut mgr) = *guard {
        mgr.auto_safe_threshold = threshold;
        serial_println!("  SafeMode: auto-safe threshold set to {}", threshold);
    }
}

/// Get boot progress as Q16 fraction
pub fn boot_progress() -> i32 {
    let guard = SAFE_MODE.lock();
    if let Some(ref mgr) = *guard {
        mgr.boot_progress_q16
    } else {
        0
    }
}

/// Clear all diagnostic logs
pub fn clear_diagnostic_log() {
    let mut guard = SAFE_MODE.lock();
    if let Some(ref mut mgr) = *guard {
        mgr.diag_log.clear();
        serial_println!("  SafeMode: diagnostic log cleared");
    }
}

/// Get the count of consecutive boot failures
pub fn consecutive_failure_count() -> u8 {
    let guard = SAFE_MODE.lock();
    if let Some(ref mgr) = *guard {
        mgr.consecutive_failures
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the safe mode manager
pub fn init() {
    let mut guard = SAFE_MODE.lock();
    let mut mgr = SafeModeManager::new();
    mgr.drivers = create_default_drivers();
    mgr.services = create_default_services();
    *guard = Some(mgr);
    serial_println!(
        "  SafeMode: manager initialized (threshold={}, {} drivers, {} services)",
        3,
        12,
        11
    );
}
