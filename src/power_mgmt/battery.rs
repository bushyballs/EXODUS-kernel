use crate::io::{inb, outb};
use crate::serial_println;
/// ACPI Embedded Controller battery driver.
///
/// Part of the AIOS power_mgmt subsystem.
///
/// Hardware path: EC I/O ports 0x62 (data) / 0x66 (cmd/status).
/// No AML interpreter required — all values are read directly from EC RAM.
/// EC register offsets follow common laptop conventions; individual OEMs
/// may shift them slightly, but the read/write protocol is universal.
///
/// All arithmetic is saturating or guarded. No float casts. No heap.
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// EC I/O ports
// ---------------------------------------------------------------------------

/// EC data register (bidirectional)
const EC_DATA: u16 = 0x62;
/// EC command / status register
const EC_CMD: u16 = 0x66;

// ---------------------------------------------------------------------------
// EC commands
// ---------------------------------------------------------------------------

/// Read a byte from EC RAM
const EC_CMD_READ: u8 = 0x80;
/// Write a byte to EC RAM
const EC_CMD_WRITE: u8 = 0x81;
/// Burst-enable (speeds up multi-byte reads)
#[allow(dead_code)]
const EC_CMD_BURST: u8 = 0x82;
/// Query for pending EC event
#[allow(dead_code)]
const EC_CMD_QUERY: u8 = 0x84;

// ---------------------------------------------------------------------------
// EC status bits
// ---------------------------------------------------------------------------

/// Output buffer full — data is ready to be read from EC_DATA
const EC_STATUS_OBF: u8 = 0x01;
/// Input buffer full — EC is still processing; must wait before writing
const EC_STATUS_IBF: u8 = 0x02;

// ---------------------------------------------------------------------------
// EC RAM offsets (common laptop convention; vary by OEM)
// ---------------------------------------------------------------------------

/// Remaining capacity in percent (0-100)
const EC_BAT_CAPACITY: u8 = 0x2A;
/// Battery voltage in 10 mV units (raw × 10 = millivolts)
const EC_BAT_VOLTAGE: u8 = 0x2C;
/// Charge/discharge current in 10 mA units (signed 16-bit, lo byte)
const EC_BAT_CURRENT_LO: u8 = 0x2E;
/// Charge/discharge current hi byte
const EC_BAT_CURRENT_HI: u8 = 0x2F;
/// Temperature in 0.1 K above 0 K (2731 = 273.1 K = 0 °C)
const EC_BAT_TEMP: u8 = 0x30;
/// AC adapter status; bit 0 = AC present
const EC_AC_STATUS: u8 = 0x32;
/// Battery presence; bit 0 = battery installed
const EC_BAT_PRESENT: u8 = 0x33;
/// Charge-cycle counter (lo byte; 8-bit wrap is fine for display)
const EC_BAT_CYCLE_CNT: u8 = 0x34;
/// Battery status flags: bit0=discharging, bit1=charging, bit2=critical
const EC_BAT_STATUS: u8 = 0x36;

// ---------------------------------------------------------------------------
// Timeout guard for EC spin-loops
// ---------------------------------------------------------------------------

/// Max iterations to wait for IBF/OBF before giving up.
/// Prevents infinite loops on hardware that does not implement EC.
const EC_TIMEOUT: u32 = 100_000;

// ---------------------------------------------------------------------------
// EC low-level operations
// ---------------------------------------------------------------------------

/// Spin until the EC input buffer is empty (safe to send a command/data byte).
#[inline]
fn ec_wait_ibf() {
    let mut n = EC_TIMEOUT;
    while n > 0 {
        if inb(EC_CMD) & EC_STATUS_IBF == 0 {
            return;
        }
        core::hint::spin_loop();
        n = n.saturating_sub(1);
    }
    serial_println!("ec: IBF timeout");
}

/// Spin until the EC output buffer is full (data byte is ready to read).
#[inline]
fn ec_wait_obf() {
    let mut n = EC_TIMEOUT;
    while n > 0 {
        if inb(EC_CMD) & EC_STATUS_OBF != 0 {
            return;
        }
        core::hint::spin_loop();
        n = n.saturating_sub(1);
    }
    serial_println!("ec: OBF timeout");
}

/// Read one byte from EC RAM at `offset`.
pub fn ec_read(offset: u8) -> u8 {
    ec_wait_ibf();
    outb(EC_CMD, EC_CMD_READ);
    ec_wait_ibf();
    outb(EC_DATA, offset);
    ec_wait_obf();
    inb(EC_DATA)
}

/// Write one byte to EC RAM at `offset`.
pub fn ec_write(offset: u8, val: u8) {
    ec_wait_ibf();
    outb(EC_CMD, EC_CMD_WRITE);
    ec_wait_ibf();
    outb(EC_DATA, offset);
    ec_wait_ibf();
    outb(EC_DATA, val);
}

// ---------------------------------------------------------------------------
// Battery state snapshot
// ---------------------------------------------------------------------------

/// Complete battery and AC state, updated on each `battery_update()` call.
#[derive(Clone, Copy)]
pub struct BatteryState {
    /// A battery is physically installed.
    pub present: bool,
    /// Battery is currently charging.
    pub charging: bool,
    /// Battery is currently discharging (AC absent).
    pub discharging: bool,
    /// Capacity is critically low (hardware flag).
    pub critical: bool,
    /// AC adapter is connected.
    pub ac_online: bool,
    /// State of charge, 0-100 percent.
    pub capacity_percent: u8,
    /// Terminal voltage in millivolts.
    pub voltage_mv: u32,
    /// Instantaneous current in milliamps.
    /// Positive = charging (current flows in), negative = discharging.
    pub current_ma: i32,
    /// Temperature in units of 0.1 K (2731 ≙ 0 °C).
    pub temp_tenth_k: u32,
    /// Full charge cycle counter.
    pub cycle_count: u16,
    /// Estimated minutes until battery empty (0 = unknown or charging).
    pub time_to_empty_min: u16,
    /// Estimated minutes until battery full (0 = unknown or not charging).
    pub time_to_full_min: u16,
}

impl BatteryState {
    const fn zeroed() -> Self {
        BatteryState {
            present: false,
            charging: false,
            discharging: false,
            critical: false,
            ac_online: false,
            capacity_percent: 0,
            voltage_mv: 0,
            current_ma: 0,
            temp_tenth_k: 2731, // 0 °C default
            cycle_count: 0,
            time_to_empty_min: 0,
            time_to_full_min: 0,
        }
    }
}

/// Global cached battery state.
static BATTERY_STATE: Mutex<BatteryState> = Mutex::new(BatteryState::zeroed());

// ---------------------------------------------------------------------------
// Time estimation (pure integer arithmetic — no floats)
// ---------------------------------------------------------------------------

/// Estimate minutes remaining until empty.
///
/// Uses the identity:
///   remaining_energy_mWh = cap_pct × V(mV) × assumed_full(mAh) / (100 × 1000)
///
/// To avoid needing the full capacity value we simplify to:
///   time_min = cap_pct × voltage_mv / (discharge_ma × 100 / 60)
///            = cap_pct × voltage_mv × 60 / (discharge_ma × 100)
///
/// This is an approximation that assumes a fixed nominal capacity; good
/// enough for a status display. All intermediate values are u64 to prevent
/// overflow without floats.
fn estimate_time_to_empty(cap_pct: u8, voltage_mv: u32, discharge_ma: u32) -> u16 {
    if discharge_ma == 0 || voltage_mv == 0 {
        return 0;
    }
    // numerator: cap_pct (0-100) × voltage_mv (0-20000) × 60
    // max ≈ 100 × 20_000 × 60 = 120_000_000 — fits in u64 easily
    let num: u64 = (cap_pct as u64)
        .saturating_mul(voltage_mv as u64)
        .saturating_mul(60);
    let den: u64 = (discharge_ma as u64).saturating_mul(100);
    if den == 0 {
        return 0;
    }
    let minutes = num / den;
    // Clamp to u16::MAX (≈ 45 days — more than enough)
    if minutes > u16::MAX as u64 {
        u16::MAX
    } else {
        minutes as u16
    }
}

/// Estimate minutes remaining until full charge.
///
/// time_min = (100 - cap_pct) × voltage_mv × 60 / (charge_ma × 100)
fn estimate_time_to_full(cap_pct: u8, voltage_mv: u32, charge_ma: u32) -> u16 {
    if charge_ma == 0 || voltage_mv == 0 || cap_pct >= 100 {
        return 0;
    }
    let remaining_pct: u64 = (100 - cap_pct) as u64;
    let num: u64 = remaining_pct
        .saturating_mul(voltage_mv as u64)
        .saturating_mul(60);
    let den: u64 = (charge_ma as u64).saturating_mul(100);
    if den == 0 {
        return 0;
    }
    let minutes = num / den;
    if minutes > u16::MAX as u64 {
        u16::MAX
    } else {
        minutes as u16
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Poll all battery-related EC registers and return a fresh `BatteryState`.
///
/// The cached state (accessible via `battery_get`) is also updated.
pub fn battery_update() -> BatteryState {
    // Battery presence
    let present_byte = ec_read(EC_BAT_PRESENT);
    let present = present_byte & 0x01 != 0;

    // AC adapter
    let ac_byte = ec_read(EC_AC_STATUS);
    let ac_online = ac_byte & 0x01 != 0;

    if !present {
        let s = BatteryState {
            present: false,
            ac_online,
            ..BatteryState::zeroed()
        };
        *BATTERY_STATE.lock() = s;
        return s;
    }

    // Capacity percent
    let raw_cap = ec_read(EC_BAT_CAPACITY);
    let capacity_percent = if raw_cap > 100 { 100 } else { raw_cap };

    // Voltage: EC gives 10 mV units; multiply by 10 for mV.
    // Single byte covers 0-2550 mV (insufficient for some batteries).
    // Use two bytes: lo at offset, hi implied at +1 for 16-bit range.
    let volt_lo = ec_read(EC_BAT_VOLTAGE) as u32;
    let volt_hi = ec_read(EC_BAT_VOLTAGE.wrapping_add(1)) as u32;
    let voltage_raw: u32 = (volt_hi << 8) | volt_lo;
    let voltage_mv: u32 = voltage_raw.saturating_mul(10);

    // Current: 16-bit signed in 10 mA units.
    let cur_lo = ec_read(EC_BAT_CURRENT_LO) as u16;
    let cur_hi = ec_read(EC_BAT_CURRENT_HI) as u16;
    let current_raw: u16 = (cur_hi << 8) | cur_lo;
    // Reinterpret as i16 (sign-extend), then scale to mA.
    let current_ma: i32 = (current_raw as i16 as i32).saturating_mul(10);

    // Temperature in 0.1 K units (EC byte is offset from 2731 = 273.1 K = 0°C)
    let temp_byte = ec_read(EC_BAT_TEMP) as u32;
    let temp_tenth_k: u32 = (2731u32).saturating_add(temp_byte.saturating_mul(10));

    // Cycle count (single byte; wraps at 256 — display-only)
    let cycle_count: u16 = ec_read(EC_BAT_CYCLE_CNT) as u16;

    // Status flags
    let status_byte = ec_read(EC_BAT_STATUS);
    let discharging = status_byte & 0x01 != 0;
    let charging = status_byte & 0x02 != 0;
    let critical = status_byte & 0x04 != 0;

    // Time estimates
    let time_to_empty_min = if discharging && current_ma < 0 {
        let discharge_ma = (-current_ma) as u32;
        estimate_time_to_empty(capacity_percent, voltage_mv, discharge_ma)
    } else {
        0
    };

    let time_to_full_min = if charging && current_ma > 0 {
        estimate_time_to_full(capacity_percent, voltage_mv, current_ma as u32)
    } else {
        0
    };

    let s = BatteryState {
        present,
        charging,
        discharging,
        critical,
        ac_online,
        capacity_percent,
        voltage_mv,
        current_ma,
        temp_tenth_k,
        cycle_count,
        time_to_empty_min,
        time_to_full_min,
    };

    *BATTERY_STATE.lock() = s;
    s
}

/// Return the last cached `BatteryState` without touching the EC.
pub fn battery_get() -> BatteryState {
    *BATTERY_STATE.lock()
}

/// Quick check: is the AC adapter connected?
pub fn ac_online() -> bool {
    BATTERY_STATE.lock().ac_online
}

/// Quick check: remaining capacity in percent (0-100).
pub fn capacity_percent() -> u8 {
    BATTERY_STATE.lock().capacity_percent
}

/// Called from the ACPI SCI interrupt handler when a battery event fires.
///
/// Re-polls EC state and (if power_mgmt exports a notify hook) signals the
/// power management layer.  Kept light — runs in interrupt context.
pub fn battery_sci_event() {
    let state = battery_update();
    // Notify the power supply abstraction so it can adapt the active profile.
    super::power_supply::notify_battery_event(&state);
}

/// Initialise the battery driver: do first poll and log result.
pub fn init() {
    let s = battery_update();
    if s.present {
        serial_println!(
            "  battery: present=true ac={} cap={}% voltage={}mV current={}mA cycles={}",
            s.ac_online,
            s.capacity_percent,
            s.voltage_mv,
            s.current_ma,
            s.cycle_count
        );
    } else {
        serial_println!("  battery: not present (ac={})", s.ac_online);
    }
}
