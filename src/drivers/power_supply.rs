use crate::sync::Mutex;
/// Power supply class driver for Genesis — no-heap, fixed-size static arrays
///
/// Abstracts batteries, AC mains adapters, USB power sources, and UPS units
/// into a unified `PowerSupply` structure. Supports:
///   - Battery: capacity tracking, charge/discharge state, health, temperature
///   - Mains/USB: online status
///   - Capacity level classification (Critical / Low / Normal / High / Full)
///   - Periodic tick that updates from the battery EC driver and logs
///     critical battery conditions
///
/// All rules strictly observed:
///   - No heap: no Vec, Box, String, alloc::*
///   - No panics: no unwrap(), expect(), panic!()
///   - No float casts: no as f64, as f32
///   - Saturating arithmetic for counters
///   - MMIO via read_volatile / write_volatile only
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of registered power supplies
const MAX_POWER_SUPPLIES: usize = 4;

/// Capacity percentage thresholds for capacity_level
const LEVEL_CRITICAL_MAX: u8 = 5;
const LEVEL_LOW_MAX: u8 = 20;
const LEVEL_NORMAL_MAX: u8 = 79;
const LEVEL_HIGH_MAX: u8 = 99;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// The physical type of a power supply
#[derive(Copy, Clone, PartialEq)]
pub enum PsType {
    /// Rechargeable battery
    Battery,
    /// AC mains adapter
    Mains,
    /// USB power source
    USB,
    /// Uninterruptible Power Supply
    UPS,
}

/// Charging / discharging status
#[derive(Copy, Clone, PartialEq)]
pub enum PsStatus {
    Unknown,
    Charging,
    Discharging,
    NotCharging,
    Full,
}

/// Battery health classification
#[derive(Copy, Clone, PartialEq)]
pub enum PsHealth {
    Unknown,
    Good,
    Overheat,
    Dead,
    OverVoltage,
    UnspecFailure,
    Cold,
    WatchdogTimerExpire,
    SafetyTimerExpire,
}

/// capacity_level values
pub mod capacity_level {
    pub const UNKNOWN: u8 = 0;
    pub const CRITICAL: u8 = 1;
    pub const LOW: u8 = 2;
    pub const NORMAL: u8 = 3;
    pub const HIGH: u8 = 4;
    pub const FULL: u8 = 5;
}

/// A registered power supply device
#[derive(Copy, Clone)]
pub struct PowerSupply {
    /// Human-readable name (null-padded ASCII, up to 16 bytes)
    pub name: [u8; 16],
    /// Physical type of this supply
    pub ps_type: PsType,
    /// Current charge / discharge status
    pub status: PsStatus,
    /// Battery health indicator
    pub health: PsHealth,
    /// True when a battery is physically present
    pub present: bool,
    /// True when external power (mains or USB) is available
    pub online: bool,
    /// Current voltage in microvolts
    pub voltage_now_uv: u32,
    /// Current in microamperes (negative = discharging)
    pub current_now_ua: i32,
    /// Charge currently held in microamp-hours
    pub charge_now_uah: u32,
    /// Full charge capacity in microamp-hours
    pub charge_full_uah: u32,
    /// Factory design capacity in microamp-hours
    pub charge_full_design_uah: u32,
    /// State of charge as a percentage (0-100)
    pub capacity_pct: u8,
    /// Capacity level bucket (see `capacity_level` constants)
    pub capacity_level: u8,
    /// Temperature in 0.1 °C units (e.g. 200 = 20.0 °C)
    pub temp_tenth_c: i16,
    /// Total charge / discharge cycle count
    pub cycle_count: u32,
    /// Estimated minutes until empty (0 when charging)
    pub time_to_empty_min: u32,
    /// Estimated minutes until fully charged (0 when discharging)
    pub time_to_full_min: u32,
    /// True when this slot is occupied
    pub active: bool,
}

impl PowerSupply {
    /// Return a zeroed, inactive power supply slot
    pub const fn empty() -> Self {
        PowerSupply {
            name: [0u8; 16],
            ps_type: PsType::Mains,
            status: PsStatus::Unknown,
            health: PsHealth::Unknown,
            present: false,
            online: false,
            voltage_now_uv: 0,
            current_now_ua: 0,
            charge_now_uah: 0,
            charge_full_uah: 0,
            charge_full_design_uah: 0,
            capacity_pct: 0,
            capacity_level: capacity_level::UNKNOWN,
            temp_tenth_c: 200, // 20.0 °C default
            cycle_count: 0,
            time_to_empty_min: 0,
            time_to_full_min: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static POWER_SUPPLIES: Mutex<[PowerSupply; MAX_POWER_SUPPLIES]> =
    Mutex::new([PowerSupply::empty(); MAX_POWER_SUPPLIES]);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Copy up to 16 bytes from `src` into `dst`, null-padding the remainder
fn copy_name(dst: &mut [u8; 16], src: &[u8]) {
    let len = if src.len() < 16 { src.len() } else { 16 };
    let mut i = 0usize;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    while i < 16 {
        dst[i] = 0;
        i = i.saturating_add(1);
    }
}

/// Compute the capacity level bucket from a percentage (0-100)
fn capacity_level_from_pct(pct: u8) -> u8 {
    if pct <= LEVEL_CRITICAL_MAX {
        capacity_level::CRITICAL
    } else if pct <= LEVEL_LOW_MAX {
        capacity_level::LOW
    } else if pct <= LEVEL_NORMAL_MAX {
        capacity_level::NORMAL
    } else if pct <= LEVEL_HIGH_MAX {
        capacity_level::HIGH
    } else {
        capacity_level::FULL
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a new power supply.
///
/// `name`    — ASCII name (up to 16 bytes).
/// `ps_type` — physical type of the supply.
///
/// Returns the index in the power-supply table on success, or `None` if the
/// table is full.
pub fn ps_register(name: &[u8], ps_type: PsType) -> Option<usize> {
    let mut supplies = POWER_SUPPLIES.lock();
    for (i, s) in supplies.iter_mut().enumerate() {
        if !s.active {
            let mut entry = PowerSupply::empty();
            copy_name(&mut entry.name, name);
            entry.ps_type = ps_type;
            entry.active = true;
            *s = entry;
            return Some(i);
        }
    }
    None
}

/// Perform a full atomic update of a power supply entry.
///
/// The caller supplies a completely filled `PowerSupply`; the `active` and
/// `name` fields of the stored entry are preserved (the caller's `supply`
/// name/active fields are ignored to prevent accidental de-registration).
pub fn ps_update(idx: usize, supply: PowerSupply) {
    if idx >= MAX_POWER_SUPPLIES {
        return;
    }
    let mut supplies = POWER_SUPPLIES.lock();
    if !supplies[idx].active {
        return;
    }
    // Preserve identity fields
    let name = supplies[idx].name;
    let active = supplies[idx].active;
    supplies[idx] = supply;
    supplies[idx].name = name;
    supplies[idx].active = active;
    // Recompute capacity_level from the new capacity_pct
    supplies[idx].capacity_level = capacity_level_from_pct(supplies[idx].capacity_pct);
}

/// Return a copy of the power supply at `idx`.
///
/// Returns `None` if `idx` is out of range or the slot is inactive.
pub fn ps_get(idx: usize) -> Option<PowerSupply> {
    if idx >= MAX_POWER_SUPPLIES {
        return None;
    }
    let supplies = POWER_SUPPLIES.lock();
    if !supplies[idx].active {
        return None;
    }
    Some(supplies[idx])
}

/// Find the first power supply whose name equals `name`.
///
/// Returns `None` if no match is found.
pub fn ps_find(name: &[u8]) -> Option<usize> {
    let supplies = POWER_SUPPLIES.lock();
    let cmp_len = if name.len() < 16 { name.len() } else { 16 };
    for (i, s) in supplies.iter().enumerate() {
        if !s.active {
            continue;
        }
        let mut same = true;
        for j in 0..cmp_len {
            if s.name[j] != name[j] {
                same = false;
                break;
            }
        }
        if same && s.name[cmp_len] == 0 {
            return Some(i);
        }
    }
    None
}

/// Return the capacity percentage (0-100) for the supply at `idx`.
///
/// Returns `None` if `idx` is invalid or the slot is inactive.
pub fn ps_get_capacity(idx: usize) -> Option<u8> {
    if idx >= MAX_POWER_SUPPLIES {
        return None;
    }
    let supplies = POWER_SUPPLIES.lock();
    if !supplies[idx].active {
        return None;
    }
    Some(supplies[idx].capacity_pct)
}

/// Periodic tick — updates all active battery entries from the low-level
/// battery EC driver and logs critical battery conditions.
///
/// For each active `PsType::Battery` supply:
///   1. Reads the latest `BatteryInfo` from `crate::drivers::battery`.
///   2. Converts units (mV/mA/mAh → µV/µA/µAh).
///   3. Maps charging state and health to `PsStatus` / `PsHealth`.
///   4. Recomputes `capacity_level` from `capacity_pct`.
///   5. Logs a warning when capacity falls below 5%.
///
/// Also updates the "AC" supply's `online` flag from AC adapter state.
pub fn ps_tick() {
    let bat_info = crate::drivers::battery::info();

    let mut supplies = POWER_SUPPLIES.lock();

    for i in 0..MAX_POWER_SUPPLIES {
        if !supplies[i].active {
            continue;
        }

        match supplies[i].ps_type {
            PsType::Battery => {
                // Map battery info into the power supply structure
                supplies[i].present = bat_info.present;
                supplies[i].capacity_pct = bat_info.percent;
                supplies[i].capacity_level = capacity_level_from_pct(bat_info.percent);

                // Convert mV → µV (× 1000)
                supplies[i].voltage_now_uv = (bat_info.voltage_mv as u32).saturating_mul(1000);

                // Convert mA → µA (× 1000); preserve sign
                supplies[i].current_now_ua = (bat_info.current_ma as i32).saturating_mul(1000);

                // Convert mAh → µAh (× 1000)
                supplies[i].charge_now_uah = (bat_info.remaining_mah as u32).saturating_mul(1000);
                supplies[i].charge_full_uah =
                    (bat_info.full_charge_mah as u32).saturating_mul(1000);
                supplies[i].charge_full_design_uah =
                    (bat_info.design_mah as u32).saturating_mul(1000);

                // Convert decikelvin temperature to 0.1 °C units:
                // temp_tenth_c = (dk - 2732) where each unit is 0.1 K ≡ 0.1 °C
                let dk = bat_info.temperature_dk as i32;
                let tenth_c = dk.saturating_sub(2732);
                // Clamp to i16 range
                supplies[i].temp_tenth_c = if tenth_c > i16::MAX as i32 {
                    i16::MAX
                } else if tenth_c < i16::MIN as i32 {
                    i16::MIN
                } else {
                    tenth_c as i16
                };

                supplies[i].cycle_count = bat_info.cycle_count as u32;
                supplies[i].time_to_empty_min = bat_info.time_remaining_min as u32;
                supplies[i].time_to_full_min = bat_info.time_to_full_min as u32;

                // Map ChargingState → PsStatus
                use crate::drivers::battery::ChargingState;
                supplies[i].status = match bat_info.charging {
                    ChargingState::ChargingCC | ChargingState::ChargingCV => PsStatus::Charging,
                    ChargingState::Discharging => PsStatus::Discharging,
                    ChargingState::Full => PsStatus::Full,
                    ChargingState::NotPresent => PsStatus::Unknown,
                };

                // Map BatteryHealth → PsHealth
                use crate::drivers::battery::BatteryHealth;
                supplies[i].health = match bat_info.health {
                    BatteryHealth::Good => PsHealth::Good,
                    BatteryHealth::Fair => PsHealth::Good, // degraded but not dead
                    BatteryHealth::Poor => PsHealth::Dead,
                    BatteryHealth::Unknown => PsHealth::Unknown,
                };

                // Critical battery warning
                if bat_info.present && bat_info.percent <= 5 {
                    serial_println!(
                        "  CRITICAL: battery critical ({}% remaining)",
                        bat_info.percent
                    );
                }
            }

            PsType::Mains => {
                // Reflect AC adapter state
                supplies[i].online = crate::drivers::battery::ac_connected();
                supplies[i].present = true;
                supplies[i].status = if supplies[i].online {
                    PsStatus::Charging
                } else {
                    PsStatus::NotCharging
                };
            }

            PsType::USB => {
                // USB power presence can be inferred from power_source field
                use crate::drivers::battery::PowerSource;
                let src = crate::drivers::battery::power_source();
                let usb_online = matches!(src, PowerSource::UsbPd | PowerSource::UsbTypeC);
                supplies[i].online = usb_online;
                supplies[i].present = true;
            }

            PsType::UPS => {
                // UPS management is not implemented in hardware; treat as online
                supplies[i].online = true;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the power supply class driver.
///
/// Registers:
///   - "BAT0" (Battery) if the battery EC driver has detected a battery
///   - "AC"   (Mains) always, defaulting to online (assume AC power)
pub fn init() {
    // Register "BAT0"
    let bat_info = crate::drivers::battery::info();
    if let Some(idx) = ps_register(b"BAT0", PsType::Battery) {
        let mut supplies = POWER_SUPPLIES.lock();
        supplies[idx].present = bat_info.present;
        supplies[idx].capacity_pct = bat_info.percent;
        supplies[idx].capacity_level = capacity_level_from_pct(bat_info.percent);
        drop(supplies);
        serial_println!(
            "  PowerSupply: 'BAT0' registered (idx {}, present={}, pct={}%)",
            idx,
            bat_info.present,
            bat_info.percent
        );
    } else {
        serial_println!("  PowerSupply: 'BAT0' registration failed");
    }

    // Register "AC" mains supply, assumed online at boot
    if let Some(idx) = ps_register(b"AC", PsType::Mains) {
        let mut supplies = POWER_SUPPLIES.lock();
        supplies[idx].present = true;
        supplies[idx].online = true;
        supplies[idx].status = PsStatus::NotCharging; // Updated on first tick
        drop(supplies);
        serial_println!("  PowerSupply: 'AC' registered (idx {}, online=true)", idx);
    } else {
        serial_println!("  PowerSupply: 'AC' registration failed");
    }

    super::register("power-supply", super::DeviceType::Other);
    serial_println!(
        "  PowerSupply: class driver initialized (max {} supplies)",
        MAX_POWER_SUPPLIES
    );
}
