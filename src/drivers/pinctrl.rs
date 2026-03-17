use crate::sync::Mutex;
/// Pin controller (pinctrl) subsystem for Genesis — no-heap, fixed-size static arrays
///
/// Manages pin multiplexing, pull resistors, drive strength, slew rate, and
/// Schmitt trigger configuration for SoC pads.  Each physical pad can be
/// assigned one of several "functions" (GPIO, UART, I2C, SPI, PWM, I2S, …).
///
/// Design mirrors Linux's pinctrl subsystem but is entirely original and
/// uses only fixed-size arrays stored in static Mutex state.
///
/// All rules strictly observed:
///   - No heap: no Vec, Box, String, alloc::*
///   - No panics: no unwrap(), expect(), panic!()
///   - No float casts: no as f64, as f32
///   - Saturating arithmetic for all counters
///   - MMIO via read_volatile / write_volatile only
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of pin configurations the subsystem tracks
const MAX_PIN_CONFIGS: usize = 128;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// The hardware function muxed onto a pad.
///
/// `Custom(u8)` covers platform-specific functions not listed here.
#[derive(Copy, Clone, PartialEq)]
pub enum PinFunction {
    /// General-purpose I/O (controlled by the GPIO subsystem)
    Gpio,
    Uart0Tx,
    Uart0Rx,
    Uart1Tx,
    Uart1Rx,
    I2c0Scl,
    I2c0Sda,
    I2c1Scl,
    I2c1Sda,
    Spi0Clk,
    Spi0Mosi,
    Spi0Miso,
    Spi0Cs,
    Pwm0,
    Pwm1,
    I2sBclk,
    I2sLrck,
    I2sData,
    /// Analog / ADC input — digital buffer disabled
    Analog,
    /// Platform-specific function identified by an 8-bit code
    Custom(u8),
}

/// Pull-resistor configuration for a pad
#[derive(Copy, Clone, PartialEq)]
pub enum PinPull {
    /// No pull resistor
    None,
    /// Pull-up resistor enabled
    Up,
    /// Pull-down resistor enabled
    Down,
}

/// Output drive-strength selection
#[derive(Copy, Clone, PartialEq)]
pub enum PinDriveStrength {
    Ma2,
    Ma4,
    Ma8,
    Ma12,
    Ma16,
}

// ---------------------------------------------------------------------------
// PinConfig
// ---------------------------------------------------------------------------

/// Full configuration record for a single pad
#[derive(Copy, Clone)]
pub struct PinConfig {
    /// Physical pad / pin number (SoC-specific numbering)
    pub pin_num: u32,
    /// Mux function assigned to this pad
    pub function: PinFunction,
    /// Pull-resistor setting
    pub pull: PinPull,
    /// Output drive strength
    pub drive: PinDriveStrength,
    /// When true: fast slew rate; when false: slow (reduced EMI)
    pub slew_rate_fast: bool,
    /// Schmitt trigger hysteresis on the input path
    pub schmitt_trigger: bool,
    /// True when this slot is populated
    pub active: bool,
}

impl PinConfig {
    /// Return an empty (zeroed / default) configuration slot
    pub const fn empty() -> Self {
        PinConfig {
            pin_num: 0,
            function: PinFunction::Gpio,
            pull: PinPull::None,
            drive: PinDriveStrength::Ma4,
            slew_rate_fast: false,
            schmitt_trigger: false,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static PIN_CONFIGS: Mutex<[PinConfig; MAX_PIN_CONFIGS]> =
    Mutex::new([PinConfig::empty(); MAX_PIN_CONFIGS]);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Find the index of a pin config entry by pin_num; returns None if absent
fn config_find_index(configs: &[PinConfig; MAX_PIN_CONFIGS], pin_num: u32) -> Option<usize> {
    for (i, c) in configs.iter().enumerate() {
        if c.active && c.pin_num == pin_num {
            return Some(i);
        }
    }
    None
}

/// Find a free (inactive) slot in the config table
fn config_free_index(configs: &[PinConfig; MAX_PIN_CONFIGS]) -> Option<usize> {
    for (i, c) in configs.iter().enumerate() {
        if !c.active {
            return Some(i);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Public pinctrl API
// ---------------------------------------------------------------------------

/// Register (or update) a pin configuration.
///
/// If an entry for `pin_num` already exists it is overwritten.
/// Returns `false` if the table is full and no existing entry is found.
pub fn pinctrl_register(pin_num: u32, config: PinConfig) -> bool {
    let mut configs = PIN_CONFIGS.lock();

    // Update existing entry if present
    if let Some(idx) = config_find_index(&configs, pin_num) {
        configs[idx] = config;
        configs[idx].pin_num = pin_num;
        configs[idx].active = true;
        return true;
    }

    // Allocate new slot
    let idx = match config_free_index(&configs) {
        Some(i) => i,
        None => return false,
    };
    configs[idx] = config;
    configs[idx].pin_num = pin_num;
    configs[idx].active = true;
    true
}

/// Set the mux function for a registered pin.
///
/// For a simulated / software-only pinctrl the change is stored in the
/// config table.  On real hardware the caller would also write the
/// appropriate pad-control register here.
///
/// Returns `false` if no entry for `pin_num` exists.
pub fn pinctrl_set_function(pin_num: u32, func: PinFunction) -> bool {
    let mut configs = PIN_CONFIGS.lock();
    let idx = match config_find_index(&configs, pin_num) {
        Some(i) => i,
        None => return false,
    };
    configs[idx].function = func;
    // Real hardware write-back would go here (platform-specific register).
    true
}

/// Set the pull-resistor configuration for a registered pin.
///
/// Returns `false` if no entry for `pin_num` exists.
pub fn pinctrl_set_pull(pin_num: u32, pull: PinPull) -> bool {
    let mut configs = PIN_CONFIGS.lock();
    let idx = match config_find_index(&configs, pin_num) {
        Some(i) => i,
        None => return false,
    };
    configs[idx].pull = pull;
    true
}

/// Set the output drive strength for a registered pin.
///
/// Returns `false` if no entry for `pin_num` exists.
pub fn pinctrl_set_drive(pin_num: u32, drive: PinDriveStrength) -> bool {
    let mut configs = PIN_CONFIGS.lock();
    let idx = match config_find_index(&configs, pin_num) {
        Some(i) => i,
        None => return false,
    };
    configs[idx].drive = drive;
    true
}

/// Retrieve a copy of the configuration for a registered pin.
///
/// Returns `None` if no entry for `pin_num` exists.
pub fn pinctrl_get_config(pin_num: u32) -> Option<PinConfig> {
    let configs = PIN_CONFIGS.lock();
    let idx = config_find_index(&configs, pin_num)?;
    Some(configs[idx])
}

/// Look up a named pin group and return its first pin number.
///
/// Stub implementation — returns `None` unconditionally.
/// A real implementation would search a platform-provided group table.
pub fn pinctrl_lookup_group(_name: &[u8]) -> Option<u32> {
    None
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the pinctrl subsystem.
///
/// On this platform all pads default to GPIO function with no pull and
/// 4 mA drive.  Platform-specific init code can call `pinctrl_register`
/// afterwards to override individual pins.
pub fn init() {
    serial_println!("  Pinctrl: subsystem ready ({} pin slots)", MAX_PIN_CONFIGS);
    super::register("pinctrl", super::DeviceType::Other);
}
