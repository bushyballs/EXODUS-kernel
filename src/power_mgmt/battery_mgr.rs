/// Battery charge management and health monitoring.
///
/// Part of the AIOS power_mgmt subsystem.
/// Reads battery state via ACPI embedded controller registers.
/// Tracks charge level, charge/discharge rate, cycle count, and health.
/// Provides time-remaining estimates based on current draw.
use crate::sync::Mutex;

/// Battery state and charge management.
pub struct BatteryManager {
    level_pct: u8,
    state: ChargeState,
    voltage_mv: u32,
    current_ma: i32,
    design_capacity_mah: u32,
    full_charge_capacity_mah: u32,
    remaining_capacity_mah: u32,
    cycle_count: u32,
    present: bool,
}

/// Battery charging state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChargeState {
    Discharging,
    Charging,
    Full,
    NotPresent,
}

static BATTERY: Mutex<Option<BatteryManager>> = Mutex::new(None);

/// ACPI Embedded Controller ports
const EC_CMD_PORT: u16 = 0x66;
const EC_DATA_PORT: u16 = 0x62;

/// Read a byte from the ACPI Embedded Controller at the given register.
fn ec_read(reg: u8) -> u8 {
    // Wait for IBF (Input Buffer Full) flag to clear
    let mut timeout = 10000u32;
    while timeout > 0 {
        let status = crate::io::inb(EC_CMD_PORT);
        if status & 0x02 == 0 {
            break;
        }
        timeout -= 1;
        core::hint::spin_loop();
    }
    // Send read command
    crate::io::outb(EC_CMD_PORT, 0x80);
    // Wait for IBF to clear again
    timeout = 10000;
    while timeout > 0 {
        let status = crate::io::inb(EC_CMD_PORT);
        if status & 0x02 == 0 {
            break;
        }
        timeout -= 1;
        core::hint::spin_loop();
    }
    // Send register address
    crate::io::outb(EC_DATA_PORT, reg);
    // Wait for OBF (Output Buffer Full) flag to set
    timeout = 10000;
    while timeout > 0 {
        let status = crate::io::inb(EC_CMD_PORT);
        if status & 0x01 != 0 {
            break;
        }
        timeout -= 1;
        core::hint::spin_loop();
    }
    crate::io::inb(EC_DATA_PORT)
}

/// Poll the EC for updated battery status
fn poll_battery_status(mgr: &mut BatteryManager) {
    // Check battery present flag (EC register 0x38 is a common location)
    let status_byte = ec_read(0x38);
    mgr.present = (status_byte & 0x01) != 0;

    if !mgr.present {
        mgr.state = ChargeState::NotPresent;
        mgr.level_pct = 0;
        return;
    }

    // Read charge state
    let charge_byte = ec_read(0x39);
    mgr.state = match charge_byte & 0x03 {
        0x00 => ChargeState::Full,
        0x01 => ChargeState::Charging,
        0x02 => ChargeState::Discharging,
        _ => ChargeState::Discharging,
    };

    // Read level percentage from EC register
    let level = ec_read(0x3A);
    mgr.level_pct = if level > 100 { 100 } else { level };

    // Read current in mA (signed: positive=charging, negative=discharging)
    let current_lo = ec_read(0x3C) as i32;
    let current_hi = ec_read(0x3D) as i32;
    mgr.current_ma = (current_hi << 8) | current_lo;
    if mgr.current_ma > 32767 {
        mgr.current_ma -= 65536; // sign extend 16-bit
    }

    // Derive remaining capacity from level
    mgr.remaining_capacity_mah =
        (mgr.full_charge_capacity_mah as u64 * mgr.level_pct as u64 / 100) as u32;
}

impl BatteryManager {
    pub fn new() -> Self {
        BatteryManager {
            level_pct: 0,
            state: ChargeState::NotPresent,
            voltage_mv: 0,
            current_ma: 0,
            design_capacity_mah: 5000,
            full_charge_capacity_mah: 4800,
            remaining_capacity_mah: 0,
            cycle_count: 0,
            present: false,
        }
    }

    /// Get the current battery level as a percentage.
    pub fn level_percent(&self) -> u8 {
        self.level_pct
    }

    /// Get the current charging state.
    pub fn charge_state(&self) -> ChargeState {
        self.state
    }

    /// Estimate remaining battery time in minutes.
    /// Returns 0 if not discharging or current draw is unknown.
    pub fn time_remaining_min(&self) -> u32 {
        if self.state != ChargeState::Discharging {
            return 0;
        }
        let discharge_ma = if self.current_ma < 0 {
            (-self.current_ma) as u32
        } else {
            return 0;
        };
        if discharge_ma == 0 {
            return 0;
        }
        // remaining_mAh / discharge_mA * 60 min/hr
        (self.remaining_capacity_mah as u64 * 60 / discharge_ma as u64) as u32
    }
}

/// Update battery status (call periodically from a timer or polling loop)
pub fn poll() {
    let mut guard = BATTERY.lock();
    if let Some(mgr) = guard.as_mut() {
        poll_battery_status(mgr);
    }
}

/// Get a snapshot of the current battery state
pub fn level_percent() -> u8 {
    let guard = BATTERY.lock();
    guard.as_ref().map_or(0, |m| m.level_pct)
}

pub fn init() {
    let mgr = BatteryManager::new();
    *BATTERY.lock() = Some(mgr);
    // Do an initial poll
    poll();
    crate::serial_println!("  battery: manager initialized");
}
