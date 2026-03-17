/// Device emulator for Genesis developer testing
///
/// Provides a virtual device environment where developers can test
/// their apps without physical hardware. Supports multiple device
/// profiles (phone, tablet, watch, TV, auto, embedded), configurable
/// hardware (screen, RAM, CPU, sensors), and runtime controls
/// (start, stop, pause, resume, install app, simulate sensors).
///
/// Original implementation for Hoags OS. No external crates.
use crate::sync::Mutex;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of emulator instances
const MAX_EMULATORS: usize = 8;

/// Maximum number of installed apps per emulator
const MAX_INSTALLED_APPS: usize = 64;

/// Maximum number of log entries per emulator
const MAX_LOG_ENTRIES: usize = 1024;

/// Maximum number of sensor data points
const MAX_SENSOR_POINTS: usize = 256;

/// Maximum number of screenshots stored
const MAX_SCREENSHOTS: usize = 16;

/// Default phone screen width
const DEFAULT_PHONE_WIDTH: u32 = 1080;

/// Default phone screen height
const DEFAULT_PHONE_HEIGHT: u32 = 2400;

/// Default tablet screen width
const DEFAULT_TABLET_WIDTH: u32 = 2560;

/// Default tablet screen height
const DEFAULT_TABLET_HEIGHT: u32 = 1600;

/// Default watch screen width
const DEFAULT_WATCH_WIDTH: u32 = 454;

/// Default watch screen height
const DEFAULT_WATCH_HEIGHT: u32 = 454;

/// Default TV screen width
const DEFAULT_TV_WIDTH: u32 = 3840;

/// Default TV screen height
const DEFAULT_TV_HEIGHT: u32 = 2160;

/// Default auto display width
const DEFAULT_AUTO_WIDTH: u32 = 1920;

/// Default auto display height
const DEFAULT_AUTO_HEIGHT: u32 = 720;

/// Default embedded display width
const DEFAULT_EMBEDDED_WIDTH: u32 = 320;

/// Default embedded display height
const DEFAULT_EMBEDDED_HEIGHT: u32 = 240;

// ---------------------------------------------------------------------------
// DeviceProfile
// ---------------------------------------------------------------------------

/// Pre-defined device profiles for common form factors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceProfile {
    /// Smartphone form factor
    Phone,
    /// Tablet form factor
    Tablet,
    /// Smartwatch form factor
    Watch,
    /// Television / large screen
    TV,
    /// Automotive infotainment
    Auto,
    /// Embedded / IoT device
    Embedded,
}

impl DeviceProfile {
    /// Get a default EmulatorConfig for this profile
    pub fn default_config(&self) -> EmulatorConfig {
        match self {
            DeviceProfile::Phone => EmulatorConfig {
                screen_width: DEFAULT_PHONE_WIDTH,
                screen_height: DEFAULT_PHONE_HEIGHT,
                ram_mb: 8192,
                cpu_cores: 8,
                has_gps: true,
                has_camera: true,
                has_nfc: true,
                api_level: 1,
            },
            DeviceProfile::Tablet => EmulatorConfig {
                screen_width: DEFAULT_TABLET_WIDTH,
                screen_height: DEFAULT_TABLET_HEIGHT,
                ram_mb: 12288,
                cpu_cores: 8,
                has_gps: true,
                has_camera: true,
                has_nfc: false,
                api_level: 1,
            },
            DeviceProfile::Watch => EmulatorConfig {
                screen_width: DEFAULT_WATCH_WIDTH,
                screen_height: DEFAULT_WATCH_HEIGHT,
                ram_mb: 2048,
                cpu_cores: 2,
                has_gps: true,
                has_camera: false,
                has_nfc: true,
                api_level: 1,
            },
            DeviceProfile::TV => EmulatorConfig {
                screen_width: DEFAULT_TV_WIDTH,
                screen_height: DEFAULT_TV_HEIGHT,
                ram_mb: 4096,
                cpu_cores: 4,
                has_gps: false,
                has_camera: false,
                has_nfc: false,
                api_level: 1,
            },
            DeviceProfile::Auto => EmulatorConfig {
                screen_width: DEFAULT_AUTO_WIDTH,
                screen_height: DEFAULT_AUTO_HEIGHT,
                ram_mb: 4096,
                cpu_cores: 4,
                has_gps: true,
                has_camera: true,
                has_nfc: false,
                api_level: 1,
            },
            DeviceProfile::Embedded => EmulatorConfig {
                screen_width: DEFAULT_EMBEDDED_WIDTH,
                screen_height: DEFAULT_EMBEDDED_HEIGHT,
                ram_mb: 512,
                cpu_cores: 1,
                has_gps: false,
                has_camera: false,
                has_nfc: false,
                api_level: 1,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// EmulatorState
// ---------------------------------------------------------------------------

/// Runtime state of an emulator instance
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmulatorState {
    /// Emulator is not running
    Stopped,
    /// Emulator is in the boot sequence
    Booting,
    /// Emulator is running normally
    Running,
    /// Emulator is paused (frozen)
    Paused,
    /// Emulator has crashed
    Crashed,
}

// ---------------------------------------------------------------------------
// EmulatorConfig
// ---------------------------------------------------------------------------

/// Hardware configuration for an emulator instance
#[derive(Debug, Clone)]
pub struct EmulatorConfig {
    /// Screen width in pixels
    pub screen_width: u32,
    /// Screen height in pixels
    pub screen_height: u32,
    /// RAM in megabytes
    pub ram_mb: u32,
    /// Number of CPU cores to emulate
    pub cpu_cores: u32,
    /// Whether GPS hardware is present
    pub has_gps: bool,
    /// Whether camera hardware is present
    pub has_camera: bool,
    /// Whether NFC hardware is present
    pub has_nfc: bool,
    /// System API level
    pub api_level: u32,
}

// ---------------------------------------------------------------------------
// SensorData — simulated sensor readings
// ---------------------------------------------------------------------------

/// Simulated sensor reading
#[derive(Debug, Clone)]
pub struct SensorData {
    /// Sensor type id (0=accel, 1=gyro, 2=baro, 3=light, 4=proximity)
    pub sensor_type: u8,
    /// X-axis value (Q16 fixed-point)
    pub x_q16: i32,
    /// Y-axis value (Q16 fixed-point)
    pub y_q16: i32,
    /// Z-axis value (Q16 fixed-point)
    pub z_q16: i32,
    /// Timestamp in kernel ticks
    pub timestamp: u64,
}

// ---------------------------------------------------------------------------
// GpsLocation — simulated GPS
// ---------------------------------------------------------------------------

/// Simulated GPS coordinates (Q16 fixed-point)
#[derive(Debug, Clone, Copy)]
pub struct GpsLocation {
    /// Latitude in degrees (Q16 fixed-point)
    pub latitude_q16: i32,
    /// Longitude in degrees (Q16 fixed-point)
    pub longitude_q16: i32,
    /// Altitude in meters (Q16 fixed-point)
    pub altitude_q16: i32,
    /// Accuracy in meters (Q16 fixed-point)
    pub accuracy_q16: i32,
}

// ---------------------------------------------------------------------------
// LogEntry
// ---------------------------------------------------------------------------

/// A log entry from the emulated device
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Log level (0=verbose, 1=debug, 2=info, 3=warn, 4=error)
    pub level: u8,
    /// Hash of the log tag / source
    pub tag_hash: u64,
    /// Hash of the log message
    pub message_hash: u64,
    /// Timestamp in kernel ticks
    pub timestamp: u64,
}

// ---------------------------------------------------------------------------
// Screenshot
// ---------------------------------------------------------------------------

/// A captured screenshot from the emulator
#[derive(Debug, Clone)]
pub struct Screenshot {
    /// Width in pixels
    pub width: u32,
    /// Height in pixels
    pub height: u32,
    /// Frame counter at time of capture
    pub frame_id: u64,
    /// Hash of the pixel data
    pub data_hash: u64,
}

// ---------------------------------------------------------------------------
// EmulatorInstance — one running virtual device
// ---------------------------------------------------------------------------

/// A single emulator instance
#[derive(Debug, Clone)]
struct EmulatorInstance {
    /// Unique emulator id
    id: u32,
    /// Device profile used to create this instance
    profile: DeviceProfile,
    /// Hardware configuration
    config: EmulatorConfig,
    /// Current runtime state
    state: EmulatorState,
    /// Installed app hashes (name_hash values)
    installed_apps: Vec<u64>,
    /// Simulated GPS location
    location: GpsLocation,
    /// Sensor data buffer
    sensor_data: Vec<SensorData>,
    /// Log output
    logs: Vec<LogEntry>,
    /// Captured screenshots
    screenshots: Vec<Screenshot>,
    /// Frame counter
    frame_count: u64,
    /// Boot time in ticks
    boot_tick: u64,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static EMULATOR_MANAGER: Mutex<Option<EmulatorManager>> = Mutex::new(None);

struct EmulatorManager {
    instances: Vec<EmulatorInstance>,
    next_id: u32,
    initialized: bool,
}

impl EmulatorManager {
    fn new() -> Self {
        Self {
            instances: Vec::new(),
            next_id: 1,
            initialized: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Emulator — public API
// ---------------------------------------------------------------------------

/// Device emulator management API
pub struct Emulator;

impl Emulator {
    /// Create a new emulator instance with the given device profile
    ///
    /// Returns the emulator id, or 0 on failure.
    pub fn create_device(profile: DeviceProfile) -> u32 {
        let config = profile.default_config();
        Self::create_device_custom(profile, config)
    }

    /// Create an emulator with a custom configuration
    pub fn create_device_custom(profile: DeviceProfile, config: EmulatorConfig) -> u32 {
        let mut guard = EMULATOR_MANAGER.lock();
        let mgr = match guard.as_mut() {
            Some(m) => m,
            None => return 0,
        };

        if mgr.instances.len() >= MAX_EMULATORS {
            serial_println!("[emu] maximum emulator instances reached");
            return 0;
        }

        let id = mgr.next_id;
        mgr.next_id += 1;

        let instance = EmulatorInstance {
            id,
            profile,
            config,
            state: EmulatorState::Stopped,
            installed_apps: Vec::new(),
            location: GpsLocation {
                latitude_q16: 38 << 16,   // ~38 degrees N (Washington DC area)
                longitude_q16: -77 << 16, // ~77 degrees W
                altitude_q16: 0,
                accuracy_q16: 10 << 16,
            },
            sensor_data: Vec::new(),
            logs: Vec::new(),
            screenshots: Vec::new(),
            frame_count: 0,
            boot_tick: 0,
        };

        mgr.instances.push(instance);
        serial_println!("[emu] created device id={} profile={:?}", id, profile);
        id
    }

    /// Start an emulator instance (boot sequence)
    pub fn start(emulator_id: u32) -> bool {
        let mut guard = EMULATOR_MANAGER.lock();
        let mgr = match guard.as_mut() {
            Some(m) => m,
            None => return false,
        };

        let inst = match mgr.instances.iter_mut().find(|i| i.id == emulator_id) {
            Some(i) => i,
            None => return false,
        };

        if inst.state != EmulatorState::Stopped {
            serial_println!(
                "[emu] cannot start: device {} is {:?}",
                emulator_id,
                inst.state
            );
            return false;
        }

        inst.state = EmulatorState::Booting;

        // Simulate boot: add log entries
        let boot_log = LogEntry {
            level: 2,
            tag_hash: 0xB007B007B007B007,
            message_hash: 0x0000000000000001,
            timestamp: 0,
        };
        if inst.logs.len() < MAX_LOG_ENTRIES {
            inst.logs.push(boot_log);
        }

        // Transition to running
        inst.state = EmulatorState::Running;
        inst.boot_tick = 1; // placeholder
        serial_println!("[emu] device {} started ({:?})", emulator_id, inst.profile);
        true
    }

    /// Stop an emulator instance
    pub fn stop(emulator_id: u32) -> bool {
        let mut guard = EMULATOR_MANAGER.lock();
        let mgr = match guard.as_mut() {
            Some(m) => m,
            None => return false,
        };

        let inst = match mgr.instances.iter_mut().find(|i| i.id == emulator_id) {
            Some(i) => i,
            None => return false,
        };

        if inst.state == EmulatorState::Stopped {
            return true; // already stopped
        }

        inst.state = EmulatorState::Stopped;
        serial_println!("[emu] device {} stopped", emulator_id);
        true
    }

    /// Pause a running emulator (freeze execution)
    pub fn pause(emulator_id: u32) -> bool {
        let mut guard = EMULATOR_MANAGER.lock();
        let mgr = match guard.as_mut() {
            Some(m) => m,
            None => return false,
        };

        let inst = match mgr.instances.iter_mut().find(|i| i.id == emulator_id) {
            Some(i) => i,
            None => return false,
        };

        if inst.state != EmulatorState::Running {
            return false;
        }

        inst.state = EmulatorState::Paused;
        serial_println!("[emu] device {} paused", emulator_id);
        true
    }

    /// Resume a paused emulator
    pub fn resume(emulator_id: u32) -> bool {
        let mut guard = EMULATOR_MANAGER.lock();
        let mgr = match guard.as_mut() {
            Some(m) => m,
            None => return false,
        };

        let inst = match mgr.instances.iter_mut().find(|i| i.id == emulator_id) {
            Some(i) => i,
            None => return false,
        };

        if inst.state != EmulatorState::Paused {
            return false;
        }

        inst.state = EmulatorState::Running;
        serial_println!("[emu] device {} resumed", emulator_id);
        true
    }

    /// Install an app (by name_hash) onto an emulator
    pub fn install_app(emulator_id: u32, app_name_hash: u64) -> bool {
        let mut guard = EMULATOR_MANAGER.lock();
        let mgr = match guard.as_mut() {
            Some(m) => m,
            None => return false,
        };

        let inst = match mgr.instances.iter_mut().find(|i| i.id == emulator_id) {
            Some(i) => i,
            None => return false,
        };

        if inst.state == EmulatorState::Stopped {
            serial_println!("[emu] cannot install: device {} is stopped", emulator_id);
            return false;
        }

        if inst.installed_apps.len() >= MAX_INSTALLED_APPS {
            serial_println!("[emu] app limit reached on device {}", emulator_id);
            return false;
        }

        // Check for duplicate
        if inst.installed_apps.contains(&app_name_hash) {
            serial_println!("[emu] app 0x{:016X} already installed", app_name_hash);
            return true; // idempotent
        }

        inst.installed_apps.push(app_name_hash);

        let log = LogEntry {
            level: 2,
            tag_hash: 0x1A57A11ED0000000,
            message_hash: app_name_hash,
            timestamp: inst.frame_count,
        };
        if inst.logs.len() < MAX_LOG_ENTRIES {
            inst.logs.push(log);
        }

        serial_println!(
            "[emu] installed app 0x{:016X} on device {}",
            app_name_hash,
            emulator_id
        );
        true
    }

    /// Set the simulated GPS location
    pub fn set_location(
        emulator_id: u32,
        latitude_q16: i32,
        longitude_q16: i32,
        altitude_q16: i32,
    ) -> bool {
        let mut guard = EMULATOR_MANAGER.lock();
        let mgr = match guard.as_mut() {
            Some(m) => m,
            None => return false,
        };

        let inst = match mgr.instances.iter_mut().find(|i| i.id == emulator_id) {
            Some(i) => i,
            None => return false,
        };

        if !inst.config.has_gps {
            serial_println!("[emu] device {} has no GPS", emulator_id);
            return false;
        }

        inst.location = GpsLocation {
            latitude_q16,
            longitude_q16,
            altitude_q16,
            accuracy_q16: 5 << 16, // 5 meter accuracy
        };

        serial_println!(
            "[emu] device {} location set lat={} lon={}",
            emulator_id,
            latitude_q16 >> 16,
            longitude_q16 >> 16
        );
        true
    }

    /// Simulate a sensor reading
    pub fn simulate_sensor(
        emulator_id: u32,
        sensor_type: u8,
        x_q16: i32,
        y_q16: i32,
        z_q16: i32,
    ) -> bool {
        let mut guard = EMULATOR_MANAGER.lock();
        let mgr = match guard.as_mut() {
            Some(m) => m,
            None => return false,
        };

        let inst = match mgr.instances.iter_mut().find(|i| i.id == emulator_id) {
            Some(i) => i,
            None => return false,
        };

        if inst.state != EmulatorState::Running {
            return false;
        }

        if inst.sensor_data.len() >= MAX_SENSOR_POINTS {
            // Drop oldest reading
            inst.sensor_data.remove(0);
        }

        let data = SensorData {
            sensor_type,
            x_q16,
            y_q16,
            z_q16,
            timestamp: inst.frame_count,
        };

        inst.sensor_data.push(data);
        true
    }

    /// Take a screenshot of the emulator display
    ///
    /// Returns a Screenshot with the current frame data hash.
    pub fn take_screenshot(emulator_id: u32) -> Option<Screenshot> {
        let mut guard = EMULATOR_MANAGER.lock();
        let mgr = match guard.as_mut() {
            Some(m) => m,
            None => return None,
        };

        let inst = match mgr.instances.iter_mut().find(|i| i.id == emulator_id) {
            Some(i) => i,
            None => return None,
        };

        if inst.state != EmulatorState::Running && inst.state != EmulatorState::Paused {
            return None;
        }

        inst.frame_count += 1;

        // Generate a synthetic data hash from frame count and config
        let data_hash = inst.frame_count
            ^ ((inst.config.screen_width as u64) << 32)
            ^ (inst.config.screen_height as u64);

        let screenshot = Screenshot {
            width: inst.config.screen_width,
            height: inst.config.screen_height,
            frame_id: inst.frame_count,
            data_hash,
        };

        if inst.screenshots.len() >= MAX_SCREENSHOTS {
            inst.screenshots.remove(0);
        }
        inst.screenshots.push(screenshot.clone());

        serial_println!(
            "[emu] screenshot taken on device {} frame={}",
            emulator_id,
            inst.frame_count
        );
        Some(screenshot)
    }

    /// Get log entries from the emulator
    pub fn get_logs(emulator_id: u32) -> Vec<LogEntry> {
        let guard = EMULATOR_MANAGER.lock();
        let mgr = match guard.as_ref() {
            Some(m) => m,
            None => return Vec::new(),
        };

        match mgr.instances.iter().find(|i| i.id == emulator_id) {
            Some(inst) => inst.logs.clone(),
            None => Vec::new(),
        }
    }

    /// Get the current state of an emulator
    pub fn get_state(emulator_id: u32) -> EmulatorState {
        let guard = EMULATOR_MANAGER.lock();
        let mgr = match guard.as_ref() {
            Some(m) => m,
            None => return EmulatorState::Stopped,
        };

        match mgr.instances.iter().find(|i| i.id == emulator_id) {
            Some(inst) => inst.state,
            None => EmulatorState::Stopped,
        }
    }

    /// Get the list of installed apps on an emulator
    pub fn get_installed_apps(emulator_id: u32) -> Vec<u64> {
        let guard = EMULATOR_MANAGER.lock();
        let mgr = match guard.as_ref() {
            Some(m) => m,
            None => return Vec::new(),
        };

        match mgr.instances.iter().find(|i| i.id == emulator_id) {
            Some(inst) => inst.installed_apps.clone(),
            None => Vec::new(),
        }
    }

    /// Get the config of an emulator
    pub fn get_config(emulator_id: u32) -> Option<EmulatorConfig> {
        let guard = EMULATOR_MANAGER.lock();
        let mgr = match guard.as_ref() {
            Some(m) => m,
            None => return None,
        };

        mgr.instances
            .iter()
            .find(|i| i.id == emulator_id)
            .map(|inst| inst.config.clone())
    }

    /// Destroy an emulator instance and free its resources
    pub fn destroy(emulator_id: u32) -> bool {
        let mut guard = EMULATOR_MANAGER.lock();
        let mgr = match guard.as_mut() {
            Some(m) => m,
            None => return false,
        };

        let before = mgr.instances.len();
        mgr.instances.retain(|i| i.id != emulator_id);
        let removed = mgr.instances.len() < before;
        if removed {
            serial_println!("[emu] destroyed device {}", emulator_id);
        }
        removed
    }

    /// Get the number of active emulator instances
    pub fn instance_count() -> usize {
        let guard = EMULATOR_MANAGER.lock();
        match guard.as_ref() {
            Some(m) => m.instances.len(),
            None => 0,
        }
    }

    /// Uninstall an app from an emulator
    pub fn uninstall_app(emulator_id: u32, app_name_hash: u64) -> bool {
        let mut guard = EMULATOR_MANAGER.lock();
        let mgr = match guard.as_mut() {
            Some(m) => m,
            None => return false,
        };

        let inst = match mgr.instances.iter_mut().find(|i| i.id == emulator_id) {
            Some(i) => i,
            None => return false,
        };

        let before = inst.installed_apps.len();
        inst.installed_apps.retain(|&h| h != app_name_hash);
        let removed = inst.installed_apps.len() < before;
        if removed {
            serial_println!(
                "[emu] uninstalled app 0x{:016X} from device {}",
                app_name_hash,
                emulator_id
            );
        }
        removed
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the device emulator subsystem
pub fn init() {
    let mut guard = EMULATOR_MANAGER.lock();
    *guard = Some(EmulatorManager::new());
    serial_println!("[emu] device emulator initialized");
}
