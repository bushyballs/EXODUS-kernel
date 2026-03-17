use crate::sync::Mutex;
/// Battery and power supply driver for Genesis
///
/// Monitors battery state and power sources via ACPI/SBS (Smart Battery System):
///   - Charge level, voltage, current, temperature
///   - Charge/discharge rate estimation
///   - Battery health (cycle count, design vs full capacity)
///   - AC adapter and USB power detection
///   - Power state notifications (low battery, critical, charging)
///   - Coulomb counting for accurate SoC estimation (Q16 fixed-point)
///
/// Communicates with the embedded controller (EC) over I/O ports 0x62/0x66
/// and reads ACPI battery status from the SBS interface.
///
/// Inspired by: Linux ACPI battery driver (drivers/acpi/battery.c),
/// Smart Battery System specification. All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::VecDeque;

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers
// ---------------------------------------------------------------------------

const Q16_SHIFT: i32 = 16;
const Q16_ONE: i32 = 1 << Q16_SHIFT;

fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> Q16_SHIFT) as i32
}

fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << Q16_SHIFT) / b as i64) as i32
}

// ---------------------------------------------------------------------------
// Embedded Controller (EC) communication — ports 0x62 (data) / 0x66 (cmd)
// ---------------------------------------------------------------------------

const EC_DATA_PORT: u16 = 0x62;
const EC_CMD_PORT: u16 = 0x66;

/// EC command bytes
const EC_READ_CMD: u8 = 0x80;
const EC_WRITE_CMD: u8 = 0x81;
const EC_QUERY_CMD: u8 = 0x84;

/// Wait for EC input buffer to be empty (bit 1 of status)
fn ec_wait_write() {
    for _ in 0..10000 {
        if crate::io::inb(EC_CMD_PORT) & 0x02 == 0 {
            return;
        }
        crate::io::io_wait();
    }
}

/// Wait for EC output buffer to be full (bit 0 of status)
fn ec_wait_read() {
    for _ in 0..10000 {
        if crate::io::inb(EC_CMD_PORT) & 0x01 != 0 {
            return;
        }
        crate::io::io_wait();
    }
}

/// Read a byte from an EC register
fn ec_read(reg: u8) -> u8 {
    ec_wait_write();
    crate::io::outb(EC_CMD_PORT, EC_READ_CMD);
    ec_wait_write();
    crate::io::outb(EC_DATA_PORT, reg);
    ec_wait_read();
    crate::io::inb(EC_DATA_PORT)
}

/// Write a byte to an EC register
fn ec_write(reg: u8, val: u8) {
    ec_wait_write();
    crate::io::outb(EC_CMD_PORT, EC_WRITE_CMD);
    ec_wait_write();
    crate::io::outb(EC_DATA_PORT, reg);
    ec_wait_write();
    crate::io::outb(EC_DATA_PORT, val);
}

/// Read a 16-bit value from two consecutive EC registers (little-endian)
fn ec_read16(reg: u8) -> u16 {
    let lo = ec_read(reg) as u16;
    let hi = ec_read(reg + 1) as u16;
    (hi << 8) | lo
}

// ---------------------------------------------------------------------------
// EC register map for battery status
// ---------------------------------------------------------------------------

const REG_BAT_STATUS: u8 = 0x10; // Battery status flags
const REG_BAT_VOLTAGE_LO: u8 = 0x12; // Current voltage (mV) low byte
const REG_BAT_CURRENT_LO: u8 = 0x14; // Current (mA, signed) low byte
const REG_BAT_REMAIN_LO: u8 = 0x16; // Remaining capacity (mAh) low byte
const REG_BAT_FULL_LO: u8 = 0x18; // Full charge capacity (mAh) low byte
const REG_BAT_DESIGN_LO: u8 = 0x1A; // Design capacity (mAh) low byte
const REG_BAT_TEMP_LO: u8 = 0x1C; // Temperature (0.1 K) low byte
const REG_BAT_CYCLE_LO: u8 = 0x1E; // Cycle count low byte
const REG_AC_STATUS: u8 = 0x20; // AC adapter status
const REG_USB_STATUS: u8 = 0x21; // USB power status

// ---------------------------------------------------------------------------
// Battery status flags
// ---------------------------------------------------------------------------

const BAT_FLAG_PRESENT: u8 = 0x01;
const BAT_FLAG_CHARGING: u8 = 0x02;
const BAT_FLAG_DISCHARGING: u8 = 0x04;
const BAT_FLAG_CRITICAL: u8 = 0x08;

// ---------------------------------------------------------------------------
// Power source types
// ---------------------------------------------------------------------------

/// Power source currently providing energy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerSource {
    /// Running on battery
    Battery,
    /// AC mains adapter
    AcAdapter,
    /// USB Power Delivery
    UsbPd,
    /// USB Type-C (standard power)
    UsbTypeC,
    /// Unknown source
    Unknown,
}

/// Charging state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChargingState {
    /// Not charging (on battery)
    Discharging,
    /// Charging (constant current phase)
    ChargingCC,
    /// Charging (constant voltage phase / trickle)
    ChargingCV,
    /// Fully charged
    Full,
    /// Not present / error
    NotPresent,
}

/// Battery health classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryHealth {
    /// Full capacity >= 80% of design capacity
    Good,
    /// Full capacity 50-80% of design capacity
    Fair,
    /// Full capacity < 50% of design capacity
    Poor,
    /// Battery not detected
    Unknown,
}

// ---------------------------------------------------------------------------
// Power event notifications
// ---------------------------------------------------------------------------

/// Power event for the notification queue
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    /// AC adapter connected
    AcConnected,
    /// AC adapter disconnected
    AcDisconnected,
    /// USB power connected
    UsbConnected,
    /// USB power disconnected
    UsbDisconnected,
    /// Battery level crossed a threshold
    BatteryLevelChanged { percent: u8 },
    /// Battery is low (< 20%)
    BatteryLow { percent: u8 },
    /// Battery is critical (< 5%)
    BatteryCritical { percent: u8 },
    /// Charging started
    ChargingStarted,
    /// Charging completed (full)
    ChargingComplete,
    /// Battery temperature warning
    TemperatureWarning { decikelvin: u16 },
}

/// Low battery threshold (percent)
const LOW_BATTERY_PERCENT: u8 = 20;
/// Critical battery threshold (percent)
const CRITICAL_BATTERY_PERCENT: u8 = 5;
/// Temperature warning threshold (60 C = 3332 decikelvin)
const TEMP_WARNING_DK: u16 = 3332;
/// Maximum events in queue
const MAX_EVENTS: usize = 32;

// ---------------------------------------------------------------------------
// Battery state
// ---------------------------------------------------------------------------

/// Snapshot of current battery readings
#[derive(Debug, Clone, Copy)]
pub struct BatteryInfo {
    /// Whether a battery is physically present
    pub present: bool,
    /// Charge percentage (0-100)
    pub percent: u8,
    /// Current voltage in millivolts
    pub voltage_mv: u16,
    /// Current in milliamps (positive = charging, negative = discharging)
    pub current_ma: i16,
    /// Remaining capacity in milliamp-hours
    pub remaining_mah: u16,
    /// Full charge capacity in milliamp-hours
    pub full_charge_mah: u16,
    /// Design (factory) capacity in milliamp-hours
    pub design_mah: u16,
    /// Temperature in decikelvin (2932 = 20.05 C)
    pub temperature_dk: u16,
    /// Charge cycle count
    pub cycle_count: u16,
    /// Charging state
    pub charging: ChargingState,
    /// Estimated time remaining (minutes), 0 if charging
    pub time_remaining_min: u16,
    /// Estimated time to full charge (minutes), 0 if discharging
    pub time_to_full_min: u16,
    /// Battery health
    pub health: BatteryHealth,
    /// Current power source
    pub power_source: PowerSource,
    /// Instantaneous power draw in milliwatts (Q16)
    pub power_mw_q16: i32,
}

impl BatteryInfo {
    const fn empty() -> Self {
        BatteryInfo {
            present: false,
            percent: 0,
            voltage_mv: 0,
            current_ma: 0,
            remaining_mah: 0,
            full_charge_mah: 0,
            design_mah: 0,
            temperature_dk: 2932, // ~20 C
            cycle_count: 0,
            charging: ChargingState::NotPresent,
            time_remaining_min: 0,
            time_to_full_min: 0,
            health: BatteryHealth::Unknown,
            power_source: PowerSource::Unknown,
            power_mw_q16: 0,
        }
    }

    /// Temperature in degrees Celsius (integer part)
    pub fn temperature_celsius(&self) -> i32 {
        (self.temperature_dk as i32 - 2732) / 10
    }

    /// Health ratio: full_charge / design capacity (Q16)
    pub fn health_ratio_q16(&self) -> i32 {
        if self.design_mah == 0 {
            return 0;
        }
        q16_div(self.full_charge_mah as i32, self.design_mah as i32)
    }
}

/// Internal driver state
struct BatteryDriver {
    /// Cached battery info
    info: BatteryInfo,
    /// Previous AC status (for edge detection)
    prev_ac: bool,
    /// Previous USB status
    prev_usb: bool,
    /// Previous charge percent (for threshold events)
    prev_percent: u8,
    /// Event queue
    events: VecDeque<PowerEvent>,
    /// Initialized flag
    initialized: bool,
    /// Polling interval counter
    poll_count: u32,
    /// Coulomb counter accumulator (Q16 mAh)
    coulomb_accum_q16: i32,
    /// Last poll timestamp (ms)
    last_poll_ms: u64,
}

impl BatteryDriver {
    const fn new() -> Self {
        BatteryDriver {
            info: BatteryInfo::empty(),
            prev_ac: false,
            prev_usb: false,
            prev_percent: 0,
            events: VecDeque::new(),
            initialized: false,
            poll_count: 0,
            coulomb_accum_q16: 0,
            last_poll_ms: 0,
        }
    }

    /// Push an event, dropping oldest if full
    fn push_event(&mut self, event: PowerEvent) {
        if self.events.len() >= MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    /// Read all battery registers and update cached info
    fn update(&mut self) {
        let status = ec_read(REG_BAT_STATUS);
        let ac_status = ec_read(REG_AC_STATUS);
        let usb_status = ec_read(REG_USB_STATUS);

        // Battery presence
        self.info.present = status & BAT_FLAG_PRESENT != 0;
        if !self.info.present {
            self.info.charging = ChargingState::NotPresent;
            return;
        }

        // Read raw values
        self.info.voltage_mv = ec_read16(REG_BAT_VOLTAGE_LO);
        let raw_current = ec_read16(REG_BAT_CURRENT_LO);
        self.info.current_ma = raw_current as i16; // signed
        self.info.remaining_mah = ec_read16(REG_BAT_REMAIN_LO);
        self.info.full_charge_mah = ec_read16(REG_BAT_FULL_LO);
        self.info.design_mah = ec_read16(REG_BAT_DESIGN_LO);
        self.info.temperature_dk = ec_read16(REG_BAT_TEMP_LO);
        self.info.cycle_count = ec_read16(REG_BAT_CYCLE_LO);

        // Compute charge percentage
        if self.info.full_charge_mah > 0 {
            let pct = (self.info.remaining_mah as u32 * 100) / self.info.full_charge_mah as u32;
            self.info.percent = pct.min(100) as u8;
        } else {
            self.info.percent = 0;
        }

        // Power draw (P = V * I, in mW via Q16)
        let v_q16 = (self.info.voltage_mv as i32) << Q16_SHIFT;
        let i_q16 = (self.info.current_ma as i32) << Q16_SHIFT;
        // P_mW = (V_mV * I_mA) / 1000
        self.info.power_mw_q16 = q16_mul(v_q16, i_q16) / 1000;

        // Coulomb counting: integrate current over time
        let now = crate::time::clock::uptime_ms();
        if self.last_poll_ms > 0 {
            let dt_ms = now.saturating_sub(self.last_poll_ms) as i32;
            // mAh increment = current_mA * dt_hours = current_mA * dt_ms / 3_600_000
            // In Q16: (current << 16) * dt_ms / 3_600_000
            let charge_q16 = q16_mul(
                (self.info.current_ma as i32) << Q16_SHIFT,
                (dt_ms << Q16_SHIFT) / 3_600_000,
            );
            self.coulomb_accum_q16 = self.coulomb_accum_q16.saturating_add(charge_q16);
        }
        self.last_poll_ms = now;

        // Charging state
        if status & BAT_FLAG_CHARGING != 0 {
            // Distinguish CC vs CV by current magnitude
            if self.info.percent >= 95 {
                self.info.charging = ChargingState::ChargingCV;
            } else {
                self.info.charging = ChargingState::ChargingCC;
            }
            if self.info.percent >= 100 {
                self.info.charging = ChargingState::Full;
            }
        } else if status & BAT_FLAG_DISCHARGING != 0 {
            self.info.charging = ChargingState::Discharging;
        } else {
            self.info.charging = ChargingState::Full;
        }

        // Time estimates
        if self.info.charging == ChargingState::Discharging && self.info.current_ma < 0 {
            let drain = (-self.info.current_ma) as u32;
            if drain > 0 {
                let mins = (self.info.remaining_mah as u32 * 60) / drain;
                self.info.time_remaining_min = mins.min(u16::MAX as u32) as u16;
            }
            self.info.time_to_full_min = 0;
        } else if self.info.current_ma > 0 {
            let charge_needed = self
                .info
                .full_charge_mah
                .saturating_sub(self.info.remaining_mah) as u32;
            let rate = self.info.current_ma as u32;
            if rate > 0 {
                let mins = (charge_needed * 60) / rate;
                self.info.time_to_full_min = mins.min(u16::MAX as u32) as u16;
            }
            self.info.time_remaining_min = 0;
        }

        // Battery health classification
        let health_q16 = self.info.health_ratio_q16();
        // 80% threshold in Q16 = 0.8 * 65536 = 52429
        // 50% threshold in Q16 = 0.5 * 65536 = 32768
        if health_q16 >= 52429 {
            self.info.health = BatteryHealth::Good;
        } else if health_q16 >= 32768 {
            self.info.health = BatteryHealth::Fair;
        } else {
            self.info.health = BatteryHealth::Poor;
        }

        // Power source
        let ac_on = ac_status & 0x01 != 0;
        let usb_on = usb_status & 0x01 != 0;
        let usb_pd = usb_status & 0x02 != 0;

        if ac_on {
            self.info.power_source = PowerSource::AcAdapter;
        } else if usb_pd {
            self.info.power_source = PowerSource::UsbPd;
        } else if usb_on {
            self.info.power_source = PowerSource::UsbTypeC;
        } else {
            self.info.power_source = PowerSource::Battery;
        }

        // Generate events on state changes
        self.check_events(ac_on, usb_on);

        self.prev_ac = ac_on;
        self.prev_usb = usb_on;
        self.prev_percent = self.info.percent;
        self.poll_count = self.poll_count.saturating_add(1);
    }

    /// Check for state changes and generate events
    fn check_events(&mut self, ac_on: bool, usb_on: bool) {
        // AC adapter events
        if ac_on && !self.prev_ac {
            self.push_event(PowerEvent::AcConnected);
            self.push_event(PowerEvent::ChargingStarted);
        } else if !ac_on && self.prev_ac {
            self.push_event(PowerEvent::AcDisconnected);
        }

        // USB power events
        if usb_on && !self.prev_usb {
            self.push_event(PowerEvent::UsbConnected);
        } else if !usb_on && self.prev_usb {
            self.push_event(PowerEvent::UsbDisconnected);
        }

        // Battery level events
        let pct = self.info.percent;
        if pct != self.prev_percent {
            // Crossed a 10% boundary
            if pct / 10 != self.prev_percent / 10 {
                self.push_event(PowerEvent::BatteryLevelChanged { percent: pct });
            }
        }

        // Low battery
        if pct <= LOW_BATTERY_PERCENT && self.prev_percent > LOW_BATTERY_PERCENT {
            self.push_event(PowerEvent::BatteryLow { percent: pct });
        }

        // Critical battery
        if pct <= CRITICAL_BATTERY_PERCENT && self.prev_percent > CRITICAL_BATTERY_PERCENT {
            self.push_event(PowerEvent::BatteryCritical { percent: pct });
        }

        // Full charge
        if self.info.charging == ChargingState::Full && self.prev_percent < 100 && pct >= 100 {
            self.push_event(PowerEvent::ChargingComplete);
        }

        // Temperature warning
        if self.info.temperature_dk > TEMP_WARNING_DK {
            self.push_event(PowerEvent::TemperatureWarning {
                decikelvin: self.info.temperature_dk,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static BATTERY: Mutex<BatteryDriver> = Mutex::new(BatteryDriver::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the battery/power supply driver
pub fn init() {
    let mut drv = BATTERY.lock();

    // Check if an EC is present by reading its status port
    let ec_status = crate::io::inb(EC_CMD_PORT);
    if ec_status == 0xFF {
        serial_println!("  Battery: no embedded controller detected");
        return;
    }

    // Initial read
    drv.update();

    if drv.info.present {
        drv.initialized = true;
        serial_println!(
            "  Battery: {}% ({} mV, {} mAh / {} mAh, {} cycles, health={:?})",
            drv.info.percent,
            drv.info.voltage_mv,
            drv.info.remaining_mah,
            drv.info.full_charge_mah,
            drv.info.cycle_count,
            drv.info.health
        );
    } else {
        drv.initialized = true;
        serial_println!("  Battery: EC present, no battery detected (AC powered)");
    }

    drop(drv);
    super::register("battery", super::DeviceType::Other);
}

/// Poll the battery for updated readings (call periodically, e.g. every 5s)
pub fn poll() {
    let mut drv = BATTERY.lock();
    if !drv.initialized {
        return;
    }
    drv.update();
}

/// Get current battery information snapshot
pub fn info() -> BatteryInfo {
    BATTERY.lock().info
}

/// Get charge percentage (0-100)
pub fn percent() -> u8 {
    BATTERY.lock().info.percent
}

/// Get current power source
pub fn power_source() -> PowerSource {
    BATTERY.lock().info.power_source
}

/// Get charging state
pub fn charging_state() -> ChargingState {
    BATTERY.lock().info.charging
}

/// Check if AC adapter is connected
pub fn ac_connected() -> bool {
    BATTERY.lock().info.power_source == PowerSource::AcAdapter
}

/// Get battery health
pub fn health() -> BatteryHealth {
    BATTERY.lock().info.health
}

/// Get temperature in Celsius (integer)
pub fn temperature_celsius() -> i32 {
    BATTERY.lock().info.temperature_celsius()
}

/// Get estimated time remaining on battery (minutes), 0 if charging
pub fn time_remaining_min() -> u16 {
    BATTERY.lock().info.time_remaining_min
}

/// Get estimated time to full charge (minutes), 0 if discharging
pub fn time_to_full_min() -> u16 {
    BATTERY.lock().info.time_to_full_min
}

/// Pop the next power event from the notification queue
pub fn pop_event() -> Option<PowerEvent> {
    BATTERY.lock().events.pop_front()
}

/// Check if there are pending power events
pub fn has_events() -> bool {
    !BATTERY.lock().events.is_empty()
}

/// Get the coulomb counter accumulator (Q16 mAh — total charge moved since init)
pub fn coulomb_counter_q16() -> i32 {
    BATTERY.lock().coulomb_accum_q16
}
