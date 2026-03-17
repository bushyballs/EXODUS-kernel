use crate::sync::Mutex;
/// Thermal throttling policy.
///
/// Part of the AIOS power_mgmt subsystem.
/// Manages thermal zones with configurable trip points. When temperatures
/// exceed trip points, the policy engine applies throttling actions:
/// passive (frequency reduction), active (fan speed increase), or
/// critical (emergency shutdown). Temperature is read from the CPU's
/// built-in thermal sensor via MSR.
use alloc::vec::Vec;

/// A thermal trip point that triggers throttling.
pub struct TripPoint {
    pub temp_mc: i32, // millidegrees Celsius
    pub action: TripAction,
}

/// Action to take when a trip point is crossed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TripAction {
    Passive,  // Reduce frequency
    Active,   // Increase fan speed
    Critical, // Emergency shutdown
}

/// A thermal zone representing a temperature sensor and its trip points.
struct ThermalZone {
    name: &'static str,
    current_temp_mc: i32,
    trips: Vec<TripPoint>,
    throttle_active: bool,
}

/// Manages thermal zones and throttling policies.
pub struct ThermalPolicy {
    zones: Vec<ThermalZone>,
    polling_interval_ms: u32,
    throttle_step_mhz: u32,
}

static POLICY: Mutex<Option<ThermalPolicy>> = Mutex::new(None);

/// MSR for CPU package thermal status (IA32_PACKAGE_THERM_STATUS)
const IA32_PKG_THERM_STATUS: u32 = 0x1B1;
/// MSR for temperature target (IA32_TEMPERATURE_TARGET)
const IA32_TEMP_TARGET: u32 = 0x1A2;

/// Read the CPU package temperature in millidegrees Celsius.
/// Uses IA32_PACKAGE_THERM_STATUS and IA32_TEMPERATURE_TARGET MSRs.
fn read_cpu_temp_mc() -> i32 {
    let temp_target = unsafe { crate::cpu::rdmsr(IA32_TEMP_TARGET) };
    let tj_max = ((temp_target >> 16) & 0xFF) as i32; // TjMax in degrees C

    let therm_status = unsafe { crate::cpu::rdmsr(IA32_PKG_THERM_STATUS) };
    // Digital readout is in bits 22:16 (degrees below TjMax)
    let readout = ((therm_status >> 16) & 0x7F) as i32;

    // Current temp = TjMax - readout, convert to millidegrees
    let temp_c = if tj_max > 0 { tj_max - readout } else { 50 }; // fallback 50C
    temp_c * 1000
}

impl ThermalPolicy {
    pub fn new() -> Self {
        // Default trip points for the CPU zone
        let cpu_trips = Vec::from([
            TripPoint {
                temp_mc: 70_000, // 70C: start passive cooling
                action: TripAction::Passive,
            },
            TripPoint {
                temp_mc: 80_000, // 80C: activate fan
                action: TripAction::Active,
            },
            TripPoint {
                temp_mc: 95_000, // 95C: active cooling (high fan)
                action: TripAction::Active,
            },
            TripPoint {
                temp_mc: 105_000, // 105C: critical shutdown
                action: TripAction::Critical,
            },
        ]);

        let cpu_zone = ThermalZone {
            name: "cpu-package",
            current_temp_mc: 0,
            trips: cpu_trips,
            throttle_active: false,
        };

        ThermalPolicy {
            zones: Vec::from([cpu_zone]),
            polling_interval_ms: 1000,
            throttle_step_mhz: 200,
        }
    }

    /// Evaluate all thermal zones and apply throttling as needed.
    /// Should be called periodically from a timer interrupt or polling loop.
    pub fn evaluate(&mut self) {
        for zone in &mut self.zones {
            // Read current temperature
            let temp_mc = read_cpu_temp_mc();
            zone.current_temp_mc = temp_mc;

            // Find the highest trip point that has been crossed
            let mut highest_action: Option<TripAction> = None;
            for trip in &zone.trips {
                if temp_mc >= trip.temp_mc {
                    highest_action = Some(trip.action);
                }
            }

            match highest_action {
                Some(TripAction::Critical) => {
                    crate::serial_println!(
                        "  thermal: CRITICAL {}mC on '{}' - emergency shutdown!",
                        temp_mc,
                        zone.name
                    );
                    zone.throttle_active = true;
                    // Drop the lock early so shutdown() can acquire it.
                    // We trigger ACPI S5 shutdown immediately — this is
                    // an emergency; data integrity is secondary to hardware
                    // protection.
                    let _ = crate::drivers::acpi::shutdown();
                    // If ACPI shutdown failed (not initialized), fall back
                    // to the QEMU/Bochs port and then halt.
                    unsafe {
                        crate::io::outw(0x604, 0x2000);
                    }
                    crate::serial_println!("  thermal: shutdown failed, halting");
                    unsafe {
                        crate::io::hlt();
                    }
                }
                Some(TripAction::Active) => {
                    if !zone.throttle_active {
                        crate::serial_println!(
                            "  thermal: active cooling at {}mC on '{}'",
                            temp_mc,
                            zone.name
                        );
                    }
                    zone.throttle_active = true;
                    // Increase fan PWM duty cycle.  Map temperature to percent:
                    // 80 °C → 60 %, 95 °C → 100 %.
                    let temp_c = (temp_mc / 1000) as u8;
                    let pwm_percent: u8 = if temp_c >= 95 {
                        100
                    } else if temp_c >= 80 {
                        // Linear interpolation 80C→60%, 95C→100%
                        60u8.saturating_add(((temp_c - 80) as u16 * 40 / 15) as u8)
                    } else {
                        60
                    };
                    set_fan_pwm(pwm_percent);
                }
                Some(TripAction::Passive) => {
                    if !zone.throttle_active {
                        crate::serial_println!(
                            "  thermal: passive throttle at {}mC on '{}'",
                            temp_mc,
                            zone.name
                        );
                        zone.throttle_active = true;
                    }
                    // Reduce CPU frequency by one throttle step via the cpufreq
                    // driver (BSP core only; AP throttling requires IPIs).
                    // We call directly into the cpufreq module rather than
                    // going through the governor so the thermal override
                    // is immediate and not subject to sampling delay.
                    let cur_mhz = read_cpu_freq_mhz();
                    crate::serial_println!(
                        "  thermal: CPU currently at {} MHz, reducing by {} MHz",
                        cur_mhz,
                        self.throttle_step_mhz
                    );
                    crate::power_mgmt::cpufreq::reduce_by(self.throttle_step_mhz as u64 * 1000);
                }
                None => {
                    if zone.throttle_active {
                        crate::serial_println!(
                            "  thermal: '{}' cooled to {}mC, throttle released",
                            zone.name,
                            temp_mc
                        );
                        zone.throttle_active = false;
                        // Restore normal frequency and fan speed via the governor.
                        set_fan_pwm(30); // return fan to quiet baseline
                        restore_governor();
                    }
                }
            }
        }
    }
}

pub fn init() {
    let policy = ThermalPolicy::new();
    let zone_count = policy.zones.len();
    let trip_count: usize = policy.zones.iter().map(|z| z.trips.len()).sum();
    crate::serial_println!(
        "  thermal: {} zone(s), {} trip point(s)",
        zone_count,
        trip_count
    );
    *POLICY.lock() = Some(policy);
}

// ── Thermal hardware helpers ───────────────────────────────────────────────

/// Set the system fan PWM duty cycle (0–100 %).
///
/// Attempts to write the duty cycle to the ACPI Embedded Controller (EC)
/// via ports 0x62 (EC data) and 0x66 (EC command/status).  The EC command
/// sequence used is:
///   1. Poll OBF bit (bit 0) of the EC status port until clear (up to 1000 tries).
///   2. Write command byte 0x81 (Write EC RAM) to the EC command port (0x66).
///   3. Write target byte address 0x93 (fan PWM register — common ITE/Nuvoton offset).
///   4. Write the duty cycle byte (0–255 mapped from 0–100 %).
///
/// If the EC does not ACK within the timeout the write is silently skipped and
/// the call is treated as a best-effort hint.  On platforms without an EC (e.g.
/// QEMU) the only observable effect is the serial log line.
pub fn set_fan_pwm(percent: u8) {
    let duty = (percent.min(100) as u16 * 255 / 100) as u8;
    crate::serial_println!("  [thermal] fan {}% (duty={})", percent, duty);

    // EC command sequence: poll until IBF (bit 1) clear, then send WEC/data.
    // EC status/command port: 0x66; EC data port: 0x62.
    const EC_CMD_PORT: u16 = 0x66;
    const EC_DATA_PORT: u16 = 0x62;
    const EC_CMD_WR: u8 = 0x81; // Write EC RAM
    const EC_FAN_REG: u8 = 0x93; // Fan PWM duty-cycle register (ITE/Nuvoton)

    // Helper: wait for EC IBF (Input Buffer Full, bit 1 of status) to clear.
    let ec_wait_ibf = || {
        for _ in 0..1000u32 {
            let status = crate::io::inb(EC_CMD_PORT);
            if status & 0x02 == 0 {
                return true; // IBF clear — safe to write
            }
            core::hint::spin_loop();
        }
        false // timed out
    };

    if ec_wait_ibf() {
        crate::io::outb(EC_CMD_PORT, EC_CMD_WR); // Write EC RAM command
        if ec_wait_ibf() {
            crate::io::outb(EC_DATA_PORT, EC_FAN_REG); // target register
            if ec_wait_ibf() {
                crate::io::outb(EC_DATA_PORT, duty); // duty cycle byte
            }
        }
    }
}

/// Read the current CPU frequency in MHz from MSR 0x198 (IA32_PERF_STATUS).
///
/// Bits [15:8] of IA32_PERF_STATUS contain the current P-state ratio.
/// Multiplying by 100 (the bus clock in MHz) gives the operating frequency.
/// Returns a fallback of 0 if the MSR read yields a zero ratio (e.g. in QEMU).
pub fn read_cpu_freq_mhz() -> u32 {
    // IA32_PERF_STATUS MSR: bits [15:8] = current P-state ratio.
    let perf_status = unsafe { crate::cpu::rdmsr(0x198) };
    let ratio = ((perf_status >> 8) & 0xFF) as u32;
    // Bus clock is 100 MHz on modern Intel platforms.
    ratio.saturating_mul(100)
}

/// Restore the CPU frequency governor to the maximum non-turbo frequency.
///
/// Writes IA32_MISC_ENABLE MSR 0x1A0 bit 16 (Enhanced Intel SpeedStep Enable)
/// to ensure the hardware governor is active, then calls
/// `crate::power_mgmt::cpufreq::restore_governor_target()` to set the target
/// P-state back to maximum.
pub fn restore_governor() {
    // Re-enable Enhanced Intel SpeedStep (EIST) by setting bit 16 of
    // IA32_MISC_ENABLE (MSR 0x1A0).  If EIST is already enabled this is a
    // harmless no-op.
    const IA32_MISC_ENABLE: u32 = 0x1A0;
    const EIST_ENABLE_BIT: u64 = 1 << 16;

    let current = unsafe { crate::cpu::rdmsr(IA32_MISC_ENABLE) };
    if current & EIST_ENABLE_BIT == 0 {
        unsafe {
            crate::cpu::wrmsr(IA32_MISC_ENABLE, current | EIST_ENABLE_BIT);
        }
        crate::serial_println!("  [thermal] EIST re-enabled (IA32_MISC_ENABLE bit 16)");
    }

    // Ask the cpufreq driver to restore its target frequency.
    crate::power_mgmt::cpufreq::restore_governor_target();
    crate::serial_println!("  [thermal] governor restored to max non-turbo frequency");
}
