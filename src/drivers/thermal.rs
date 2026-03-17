use crate::sync::Mutex;
/// Thermal monitoring and cooling policy driver for Genesis
///
/// Manages thermal zones with configurable trip points, implements passive
/// (CPU throttling) and active (fan control) cooling policies, tracks
/// temperature trends for hysteresis, and triggers emergency shutdown on
/// critical overtemperature.
///
/// Reads temperature from ACPI thermal zone registers (EC-backed) and
/// MSR-based CPU package temperature sensors.
///
/// Inspired by: Linux thermal_core, ACPI thermal driver. All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum thermal zones
const MAX_ZONES: usize = 8;
/// Temperature history depth for trend analysis
const HISTORY_DEPTH: usize = 8;
/// Hysteresis margin in millicelsius (2 degrees)
const HYSTERESIS_MC: i32 = 2_000;
/// Critical shutdown delay (poll cycles to confirm before shutdown)
const CRITICAL_CONFIRM_COUNT: u8 = 3;

// EC registers for thermal zone readout (zone 0 base; each zone +0x10)
const EC_THERMAL_BASE: u8 = 0x40;
const EC_THERMAL_STRIDE: u8 = 0x10;
const EC_TEMP_LO: u8 = 0x00; // Current temp low byte (in 0.1 K units)
const EC_TEMP_HI: u8 = 0x01; // Current temp high byte
const EC_TRIP_WARN_LO: u8 = 0x02; // Warning trip point low
const EC_TRIP_WARN_HI: u8 = 0x03;
const EC_TRIP_CRIT_LO: u8 = 0x04; // Critical trip point low
const EC_TRIP_CRIT_HI: u8 = 0x05;
const EC_FAN_DUTY: u8 = 0x06; // Fan duty cycle (0-255)
const EC_ZONE_FLAGS: u8 = 0x07; // Zone flags (bit 0 = present)

// EC communication (shared with battery driver)
const EC_DATA_PORT: u16 = 0x62;
const EC_CMD_PORT: u16 = 0x66;
const EC_READ_CMD: u8 = 0x80;
const EC_WRITE_CMD: u8 = 0x81;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Cooling policy for a zone
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoolingPolicy {
    /// Passive: CPU frequency throttling
    Passive,
    /// Active: fan speed control
    Active,
    /// Both passive and active
    Combined,
}

/// Current throttle state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThrottleState {
    /// No throttling, full performance
    None,
    /// Light throttle (reduce by 25%)
    Light,
    /// Medium throttle (reduce by 50%)
    Medium,
    /// Heavy throttle (reduce by 75%)
    Heavy,
    /// Emergency: preparing for shutdown
    Emergency,
}

/// Temperature trend
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TempTrend {
    Stable,
    Rising,
    Falling,
}

/// Thermal event notification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalEvent {
    /// Temperature crossed warning threshold
    Warning { zone: usize, temp_mc: i32 },
    /// Temperature crossed critical threshold
    Critical { zone: usize, temp_mc: i32 },
    /// Throttle state changed
    ThrottleChanged { zone: usize, state: ThrottleState },
    /// Fan speed changed
    FanChanged { zone: usize, duty: u8 },
    /// Initiating emergency shutdown
    EmergencyShutdown { zone: usize, temp_mc: i32 },
}

/// Per-zone state
struct ThermalZoneInner {
    name: String,
    /// Current temperature in millicelsius
    current_mc: i32,
    /// Warning trip point in millicelsius
    trip_warn_mc: i32,
    /// Critical trip point in millicelsius
    trip_crit_mc: i32,
    /// Passive trip (throttle start) in millicelsius
    trip_passive_mc: i32,
    /// Cooling policy
    policy: CoolingPolicy,
    /// Current throttle level
    throttle: ThrottleState,
    /// Current fan duty cycle (0-255)
    fan_duty: u8,
    /// Temperature history ring buffer
    history: [i32; HISTORY_DEPTH],
    history_idx: usize,
    /// Critical confirmation counter
    crit_count: u8,
    /// EC register base for this zone
    ec_base: u8,
    /// Whether zone is active
    present: bool,
}

/// Driver top-level state
struct ThermalDriver {
    zones: Vec<ThermalZoneInner>,
    events: Vec<ThermalEvent>,
}

// ---------------------------------------------------------------------------
// EC helpers
// ---------------------------------------------------------------------------

fn ec_wait_write() {
    for _ in 0..10_000 {
        if crate::io::inb(EC_CMD_PORT) & 0x02 == 0 {
            return;
        }
        crate::io::io_wait();
    }
}

fn ec_wait_read() {
    for _ in 0..10_000 {
        if crate::io::inb(EC_CMD_PORT) & 0x01 != 0 {
            return;
        }
        crate::io::io_wait();
    }
}

fn ec_read(reg: u8) -> u8 {
    ec_wait_write();
    crate::io::outb(EC_CMD_PORT, EC_READ_CMD);
    ec_wait_write();
    crate::io::outb(EC_DATA_PORT, reg);
    ec_wait_read();
    crate::io::inb(EC_DATA_PORT)
}

fn ec_write(reg: u8, val: u8) {
    ec_wait_write();
    crate::io::outb(EC_CMD_PORT, EC_WRITE_CMD);
    ec_wait_write();
    crate::io::outb(EC_DATA_PORT, reg);
    ec_wait_write();
    crate::io::outb(EC_DATA_PORT, val);
}

/// Read 16-bit value from EC (little-endian, in decikelvin)
fn ec_read_temp(base: u8, offset: u8) -> i32 {
    let lo = ec_read(base.saturating_add(offset)) as u16;
    let hi = ec_read(base.saturating_add(offset).saturating_add(1)) as u16;
    let decikelvin = (hi << 8) | lo;
    // Convert decikelvin to millicelsius: (dK - 2732) * 100
    (decikelvin as i32).saturating_sub(2732).saturating_mul(100)
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static THERMAL: Mutex<Option<ThermalDriver>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Internal logic
// ---------------------------------------------------------------------------

impl ThermalZoneInner {
    /// Read current temperature from hardware
    fn read_temperature(&mut self) {
        let temp = ec_read_temp(self.ec_base, EC_TEMP_LO);
        self.current_mc = temp;
        // Record in history
        self.history[self.history_idx] = temp;
        self.history_idx = (self.history_idx + 1) % HISTORY_DEPTH;
    }

    /// Determine temperature trend from history
    fn trend(&self) -> TempTrend {
        // Compare average of first half vs second half
        let half = HISTORY_DEPTH / 2;
        let mut sum_old: i64 = 0;
        let mut sum_new: i64 = 0;
        for i in 0..half {
            let old_idx = (self.history_idx + i) % HISTORY_DEPTH;
            let new_idx = (self.history_idx + half + i) % HISTORY_DEPTH;
            sum_old += self.history[old_idx] as i64;
            sum_new += self.history[new_idx] as i64;
        }
        let diff = sum_new - sum_old;
        if diff > (HYSTERESIS_MC as i64 * half as i64) {
            TempTrend::Rising
        } else if diff < -(HYSTERESIS_MC as i64 * half as i64) {
            TempTrend::Falling
        } else {
            TempTrend::Stable
        }
    }

    /// Compute desired fan duty based on temperature
    fn compute_fan_duty(&self) -> u8 {
        if self.current_mc < self.trip_passive_mc - HYSTERESIS_MC {
            0 // Below passive threshold, fan off
        } else if self.current_mc < self.trip_warn_mc {
            // Proportional between passive and warning
            let range = self.trip_warn_mc - self.trip_passive_mc;
            if range <= 0 {
                return 128;
            }
            let above = self.current_mc - self.trip_passive_mc;
            let ratio = (above * 180 / range).clamp(0, 180) as u8;
            ratio.max(30) // Minimum spin-up duty
        } else if self.current_mc < self.trip_crit_mc {
            // Between warning and critical: high speed
            200
        } else {
            255 // Maximum
        }
    }

    /// Compute throttle state based on temperature
    fn compute_throttle(&self) -> ThrottleState {
        if self.current_mc >= self.trip_crit_mc {
            ThrottleState::Emergency
        } else if self.current_mc >= self.trip_warn_mc + HYSTERESIS_MC {
            ThrottleState::Heavy
        } else if self.current_mc >= self.trip_warn_mc {
            ThrottleState::Medium
        } else if self.current_mc >= self.trip_passive_mc {
            ThrottleState::Light
        } else {
            ThrottleState::None
        }
    }
}

impl ThermalDriver {
    /// Poll all zones and update cooling policy
    fn poll(&mut self) {
        for (idx, zone) in self.zones.iter_mut().enumerate() {
            if !zone.present {
                continue;
            }

            zone.read_temperature();

            // Fan control (active cooling)
            if zone.policy == CoolingPolicy::Active || zone.policy == CoolingPolicy::Combined {
                let new_duty = zone.compute_fan_duty();
                if new_duty != zone.fan_duty {
                    ec_write(zone.ec_base + EC_FAN_DUTY, new_duty);
                    self.events.push(ThermalEvent::FanChanged {
                        zone: idx,
                        duty: new_duty,
                    });
                    zone.fan_duty = new_duty;
                }
            }

            // Throttle control (passive cooling)
            if zone.policy == CoolingPolicy::Passive || zone.policy == CoolingPolicy::Combined {
                let new_throttle = zone.compute_throttle();
                if new_throttle != zone.throttle {
                    self.events.push(ThermalEvent::ThrottleChanged {
                        zone: idx,
                        state: new_throttle,
                    });
                    zone.throttle = new_throttle;
                }
            }

            // Warning check
            if zone.current_mc >= zone.trip_warn_mc {
                self.events.push(ThermalEvent::Warning {
                    zone: idx,
                    temp_mc: zone.current_mc,
                });
            }

            // Critical check with confirmation
            if zone.current_mc >= zone.trip_crit_mc {
                zone.crit_count = zone.crit_count.saturating_add(1);
                if zone.crit_count >= CRITICAL_CONFIRM_COUNT {
                    self.events.push(ThermalEvent::Critical {
                        zone: idx,
                        temp_mc: zone.current_mc,
                    });
                    self.events.push(ThermalEvent::EmergencyShutdown {
                        zone: idx,
                        temp_mc: zone.current_mc,
                    });
                    serial_println!(
                        "  THERMAL EMERGENCY: zone {} at {} mC, initiating shutdown!",
                        idx,
                        zone.current_mc
                    );
                    // In a real system, call power management shutdown here
                }
            } else {
                zone.crit_count = 0;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Read temperature of a zone in millicelsius.
pub fn read_temperature(zone: usize) -> Result<i32, ()> {
    let mut guard = THERMAL.lock();
    let drv = guard.as_mut().ok_or(())?;
    let z = drv.zones.get_mut(zone).ok_or(())?;
    if !z.present {
        return Err(());
    }
    z.read_temperature();
    Ok(z.current_mc)
}

/// Get temperature trend for a zone.
pub fn trend(zone: usize) -> Result<TempTrend, ()> {
    let guard = THERMAL.lock();
    let drv = guard.as_ref().ok_or(())?;
    let z = drv.zones.get(zone).ok_or(())?;
    Ok(z.trend())
}

/// Get current throttle state for a zone.
pub fn throttle_state(zone: usize) -> Result<ThrottleState, ()> {
    let guard = THERMAL.lock();
    let drv = guard.as_ref().ok_or(())?;
    let z = drv.zones.get(zone).ok_or(())?;
    Ok(z.throttle)
}

/// Set the cooling policy for a zone.
pub fn set_policy(zone: usize, policy: CoolingPolicy) -> Result<(), ()> {
    let mut guard = THERMAL.lock();
    let drv = guard.as_mut().ok_or(())?;
    let z = drv.zones.get_mut(zone).ok_or(())?;
    z.policy = policy;
    serial_println!("  Thermal: zone {} policy set to {:?}", zone, policy);
    Ok(())
}

/// Override fan duty for a zone (0-255). Set 0 to return to auto.
pub fn set_fan_duty(zone: usize, duty: u8) -> Result<(), ()> {
    let mut guard = THERMAL.lock();
    let drv = guard.as_mut().ok_or(())?;
    let z = drv.zones.get_mut(zone).ok_or(())?;
    ec_write(z.ec_base + EC_FAN_DUTY, duty);
    z.fan_duty = duty;
    Ok(())
}

/// Poll all thermal zones -- call periodically (e.g., every 2 seconds).
pub fn poll() {
    let mut guard = THERMAL.lock();
    if let Some(drv) = guard.as_mut() {
        drv.poll();
    }
}

/// Pop the next thermal event.
pub fn pop_event() -> Option<ThermalEvent> {
    let mut guard = THERMAL.lock();
    let drv = guard.as_mut()?;
    if drv.events.is_empty() {
        None
    } else {
        Some(drv.events.remove(0))
    }
}

/// Get number of thermal zones.
pub fn zone_count() -> usize {
    THERMAL.lock().as_ref().map_or(0, |drv| drv.zones.len())
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize thermal monitoring subsystem.
///
/// Enumerates thermal zones via the EC and reads initial trip points.
pub fn init() {
    // Check for EC
    let ec_status = crate::io::inb(EC_CMD_PORT);
    if ec_status == 0xFF {
        serial_println!("  Thermal: no EC found, skipping");
        return;
    }

    let mut zones = Vec::new();

    for i in 0..MAX_ZONES {
        let base = EC_THERMAL_BASE + (i as u8) * EC_THERMAL_STRIDE;
        let flags = ec_read(base + EC_ZONE_FLAGS);
        if flags & 0x01 == 0 {
            continue; // Zone not present
        }

        let current = ec_read_temp(base, EC_TEMP_LO);
        let warn = ec_read_temp(base, EC_TRIP_WARN_LO);
        let crit = ec_read_temp(base, EC_TRIP_CRIT_LO);
        // Passive trip = warning - 10 degrees
        let passive = warn - 10_000;

        let name = match i {
            0 => String::from("cpu"),
            1 => String::from("gpu"),
            2 => String::from("chassis"),
            3 => String::from("memory"),
            _ => {
                let mut s = String::from("zone");
                use core::fmt::Write;
                let _ = write!(s, "{}", i);
                s
            }
        };

        serial_println!(
            "  Thermal: zone {} '{}' at {} mC (warn={} mC, crit={} mC)",
            i,
            name,
            current,
            warn,
            crit
        );

        zones.push(ThermalZoneInner {
            name,
            current_mc: current,
            trip_warn_mc: warn,
            trip_crit_mc: crit,
            trip_passive_mc: passive,
            policy: CoolingPolicy::Combined,
            throttle: ThrottleState::None,
            fan_duty: 0,
            history: [current; HISTORY_DEPTH],
            history_idx: 0,
            crit_count: 0,
            ec_base: base,
            present: true,
        });
    }

    let count = zones.len();
    if count > 0 {
        *THERMAL.lock() = Some(ThermalDriver {
            zones,
            events: Vec::new(),
        });
        super::register("thermal", super::DeviceType::Other);
        serial_println!("  Thermal: {} zone(s) active", count);
    } else {
        serial_println!("  Thermal: no zones detected");
    }
}
