/// Hardware Monitor (hwmon) framework for Genesis — no-heap, static-array implementation
///
/// Provides a unified sensor registration and polling framework modelled after
/// the Linux hwmon subsystem, but fully bare-metal with no heap allocation.
///
/// Up to 64 sensors may be registered globally.  Each sensor has:
///   - Chip/device name (up to 15 printable bytes + NUL)
///   - Sensor name (up to 15 printable bytes + NUL)
///   - Type (temperature, voltage, current, power, fan, humidity, frequency)
///   - Current value in the type's base unit (see below)
///   - Min / max / critical thresholds
///   - Alarm flag (true when value > max)
///   - Active flag (false for unused slots)
///
/// Unit conventions (matching Linux hwmon ABI):
///   Temperature  — millidegrees Celsius  (1000 = 1 °C)
///   Voltage      — millivolts            (1000 = 1 V)
///   Current      — milliamperes          (1000 = 1 A)
///   Power        — milliwatts            (1000 = 1 W)
///   Fan          — RPM (integer)
///   Humidity     — milli-percent         (1000 = 1 %)
///   Frequency    — Hz (integer)
///
/// Built-in virtual sensors are registered during `hwmon_init()`:
///   "genesis" / "cpu_temp"   — CPU package temperature (millidegrees C)
///   "genesis" / "cpu_freq"   — CPU operating frequency (Hz)
///   "genesis" / "mem_used"   — Memory used (stub, returns 0)
///
/// `hwmon_tick()` is called periodically to refresh all active sensors.
///
/// SAFETY RULES:
///   - No as f32 / as f64
///   - saturating_add / saturating_sub for counters and threshold comparisons
///   - No panic — return Option/bool on error
use crate::serial_println;
use crate::sync::Mutex;

// ============================================================================
// Maximum sensor slots
// ============================================================================

const MAX_SENSORS: usize = 64;

// ============================================================================
// Sensor type
// ============================================================================

/// Classification of a hardware sensor.
#[derive(Copy, Clone, PartialEq)]
pub enum HwmonSensorType {
    /// Temperature in millidegrees Celsius
    Temperature,
    /// Voltage in millivolts
    Voltage,
    /// Current in milliamperes
    Current,
    /// Power in milliwatts
    Power,
    /// Fan speed in RPM
    Fan,
    /// Relative humidity in milli-percent
    Humidity,
    /// Frequency in Hz
    Frequency,
}

// ============================================================================
// Sensor record
// ============================================================================

/// One hardware sensor entry.
#[derive(Copy, Clone)]
pub struct HwmonSensor {
    /// Name of the chip/device (e.g. "genesis", "lm75", "it87")
    pub chip_name: [u8; 16],
    /// Sensor channel name (e.g. "cpu_temp", "fan1", "in0")
    pub sensor_name: [u8; 16],
    /// Sensor type determines the unit of `value`, `min`, `max`, `crit`
    pub sensor_type: HwmonSensorType,
    /// Most recent sensor reading in the type's base unit
    pub value: i32,
    /// Lower alarm threshold (alarm fires when value < min for some types)
    pub min: i32,
    /// Upper warning threshold (alarm fires when value > max)
    pub max: i32,
    /// Critical threshold (emergency action when value > crit)
    pub crit: i32,
    /// True when the current value exceeds `max`
    pub alarm: bool,
    /// False for unused/empty slots
    pub active: bool,
}

impl HwmonSensor {
    /// Return an empty (inactive) sensor record suitable for static initialisation.
    pub const fn empty() -> Self {
        HwmonSensor {
            chip_name: [0u8; 16],
            sensor_name: [0u8; 16],
            sensor_type: HwmonSensorType::Temperature,
            value: 0,
            min: i32::MIN,
            max: i32::MAX,
            crit: i32::MAX,
            alarm: false,
            active: false,
        }
    }
}

// ============================================================================
// Global sensor table
// ============================================================================

static HWMON_SENSORS: Mutex<[HwmonSensor; MAX_SENSORS]> =
    Mutex::new([HwmonSensor::empty(); MAX_SENSORS]);

// ============================================================================
// Name copy helper (no-alloc, no panic)
// ============================================================================

/// Copy up to 15 bytes from `src` into the 16-byte fixed buffer `dst`,
/// NUL-terminating at index 15.
fn copy_name(dst: &mut [u8; 16], src: &[u8]) {
    let len = src.len().min(15);
    dst[..len].copy_from_slice(&src[..len]);
    dst[len] = 0;
    for i in (len.saturating_add(1))..16 {
        dst[i] = 0;
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Register a new hardware sensor.
///
/// `chip`  — chip/device label, e.g. `b"genesis"` or `b"lm75"`
/// `name`  — channel label, e.g. `b"cpu_temp"` or `b"fan1"`
/// `stype` — sensor type (determines unit interpretation)
/// `min`   — lower threshold (type's unit)
/// `max`   — upper alarm threshold
/// `crit`  — critical threshold
///
/// Returns the sensor index on success, `None` if the table is full.
pub fn hwmon_register(
    chip: &[u8],
    name: &[u8],
    stype: HwmonSensorType,
    min: i32,
    max: i32,
    crit: i32,
) -> Option<usize> {
    let mut sensors = HWMON_SENSORS.lock();
    // Find first inactive slot
    for (i, slot) in sensors.iter_mut().enumerate() {
        if !slot.active {
            copy_name(&mut slot.chip_name, chip);
            copy_name(&mut slot.sensor_name, name);
            slot.sensor_type = stype;
            slot.value = 0;
            slot.min = min;
            slot.max = max;
            slot.crit = crit;
            slot.alarm = false;
            slot.active = true;
            return Some(i);
        }
    }
    None // table is full
}

/// Update the current reading for sensor at `idx`.
///
/// Also evaluates alarm thresholds: sets `alarm = true` when `value > max`.
/// Clears alarm when `value <= max`.
///
/// Does nothing if `idx` is out of range or the slot is inactive.
pub fn hwmon_update(idx: usize, value: i32) {
    if idx >= MAX_SENSORS {
        return;
    }
    let mut sensors = HWMON_SENSORS.lock();
    let s = &mut sensors[idx];
    if !s.active {
        return;
    }
    s.value = value;
    // Alarm: fires when reading exceeds the upper threshold
    s.alarm = value > s.max;
}

/// Read the current value of sensor `idx`.
///
/// Returns `None` if the index is out of range or the slot is inactive.
pub fn hwmon_read(idx: usize) -> Option<i32> {
    if idx >= MAX_SENSORS {
        return None;
    }
    let sensors = HWMON_SENSORS.lock();
    if sensors[idx].active {
        Some(sensors[idx].value)
    } else {
        None
    }
}

/// Returns `true` if sensor `idx` is in alarm state.
///
/// Returns `false` for out-of-range or inactive sensors.
pub fn hwmon_get_alarm(idx: usize) -> bool {
    if idx >= MAX_SENSORS {
        return false;
    }
    let sensors = HWMON_SENSORS.lock();
    sensors[idx].active && sensors[idx].alarm
}

/// Find the first active sensor matching `chip` and `name`.
///
/// Returns the sensor index on match, `None` if not found.
pub fn hwmon_find(chip: &[u8], name: &[u8]) -> Option<usize> {
    if chip.is_empty() || name.is_empty() {
        return None;
    }
    let chip_len = chip.len().min(15);
    let name_len = name.len().min(15);

    let sensors = HWMON_SENSORS.lock();
    for (i, s) in sensors.iter().enumerate() {
        if !s.active {
            continue;
        }
        // Compare chip_name
        if s.chip_name[..chip_len] != chip[..chip_len] {
            continue;
        }
        if s.chip_name[chip_len] != 0 {
            continue; // stored name is longer than what we're searching for
        }
        // Compare sensor_name
        if s.sensor_name[..name_len] != name[..name_len] {
            continue;
        }
        if s.sensor_name[name_len] != 0 {
            continue;
        }
        return Some(i);
    }
    None
}

/// Copy all active sensors into `out`.
///
/// Returns the number of active sensors copied (may be less than 64 if
/// some slots are inactive).  Inactive slots in `out` are zeroed.
pub fn hwmon_list(out: &mut [HwmonSensor; MAX_SENSORS]) -> u32 {
    let sensors = HWMON_SENSORS.lock();
    let mut count = 0u32;
    for (i, s) in sensors.iter().enumerate() {
        out[i] = *s;
        if s.active {
            count = count.saturating_add(1);
        }
    }
    count
}

// ============================================================================
// Built-in virtual sensor indices (set during init)
// ============================================================================

/// Sensor index for the CPU temperature virtual sensor.
/// Initialised to 0xFF_FF_FF_FF (invalid) before init completes.
static mut CPU_TEMP_IDX: usize = usize::MAX;
/// Sensor index for the CPU frequency virtual sensor.
static mut CPU_FREQ_IDX: usize = usize::MAX;
/// Sensor index for the memory used virtual sensor.
static mut MEM_USED_IDX: usize = usize::MAX;

// ============================================================================
// Periodic update
// ============================================================================

/// Refresh all active sensors with current hardware readings.
///
/// Designed to be called periodically (e.g. every 500 ms from a timer handler
/// or a kernel watchdog tick).
pub fn hwmon_tick() {
    // --- CPU temperature ---
    let cpu_temp_idx = unsafe { CPU_TEMP_IDX };
    if cpu_temp_idx < MAX_SENSORS {
        // get_temperature_c() returns u8 in degrees C; convert to millidegrees
        let temp_c = crate::power_mgmt::cpufreq::get_temperature_c();
        let temp_mdeg = (temp_c as i32).saturating_mul(1000);
        hwmon_update(cpu_temp_idx, temp_mdeg);
    }

    // --- CPU frequency ---
    let cpu_freq_idx = unsafe { CPU_FREQ_IDX };
    if cpu_freq_idx < MAX_SENSORS {
        // get_current_freq_mhz() returns MHz; saturating_mul avoids i32 overflow
        // for very high frequencies (> 2.147 GHz) by clamping at i32::MAX.
        let freq_mhz = crate::power_mgmt::cpufreq::get_current_freq_mhz();
        // Use i64 intermediate to avoid overflow before clamping to i32
        let freq_hz_i64 = (freq_mhz as i64).saturating_mul(1_000_000i64);
        let freq_hz = if freq_hz_i64 > i32::MAX as i64 {
            i32::MAX
        } else {
            freq_hz_i64 as i32
        };
        hwmon_update(cpu_freq_idx, freq_hz);
    }

    // --- Memory used (stub: always 0) ---
    let mem_used_idx = unsafe { MEM_USED_IDX };
    if mem_used_idx < MAX_SENSORS {
        // Could read from crate::memory::stats::used_kb() * 1024 in a real impl
        hwmon_update(mem_used_idx, 0);
    }
}

// ============================================================================
// Module entry point
// ============================================================================

/// Initialise the hwmon framework and register built-in virtual sensors.
///
/// Called once during kernel boot via `drivers::init()`.
pub fn init() {
    // --- CPU temperature sensor ---
    // Range: 0..=125 000 millidegrees C; critical at 105 000 (105 °C)
    match hwmon_register(
        b"genesis",
        b"cpu_temp",
        HwmonSensorType::Temperature,
        0,       // min: 0 m°C
        95_000,  // max: 95 °C
        105_000, // crit: 105 °C
    ) {
        Some(idx) => {
            unsafe {
                CPU_TEMP_IDX = idx;
            }
            serial_println!("  hwmon: registered cpu_temp at index {}", idx);
        }
        None => {
            serial_println!("  hwmon: WARN sensor table full — cpu_temp not registered");
        }
    }

    // --- CPU frequency sensor ---
    // Range: 400 MHz..2 100 MHz expressed in Hz (i32 max ~2.147 GHz).
    // For CPUs reported above 2.1 GHz, the value will saturate at i32::MAX
    // harmlessly — the alarm threshold is set to i32::MAX (no upper alarm).
    match hwmon_register(
        b"genesis",
        b"cpu_freq",
        HwmonSensorType::Frequency,
        400_000_000, // min: 400 MHz in Hz
        i32::MAX,    // max: no upper alarm (frequency alarm not meaningful here)
        i32::MAX,    // crit: same — no critical threshold
    ) {
        Some(idx) => {
            unsafe {
                CPU_FREQ_IDX = idx;
            }
            serial_println!("  hwmon: registered cpu_freq at index {}", idx);
        }
        None => {
            serial_println!("  hwmon: WARN sensor table full — cpu_freq not registered");
        }
    }

    // --- Memory used stub sensor ---
    match hwmon_register(
        b"genesis",
        b"mem_used",
        HwmonSensorType::Power, // placeholder type; no unit for raw bytes
        0,
        i32::MAX,
        i32::MAX,
    ) {
        Some(idx) => {
            unsafe {
                MEM_USED_IDX = idx;
            }
            serial_println!("  hwmon: registered mem_used at index {}", idx);
        }
        None => {
            serial_println!("  hwmon: WARN sensor table full — mem_used not registered");
        }
    }

    // Initial read to populate values immediately at boot
    hwmon_tick();

    serial_println!("  hwmon: framework initialised (3 built-in sensors)");
}
