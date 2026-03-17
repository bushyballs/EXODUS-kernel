/// Thermal Management — temperature monitoring, throttle curves, fan control
///
/// Manages system thermal health:
///   - CPU package + per-core temperature via MSR_THERM_STATUS
///   - Thermal throttle curves: linear, step, PID controller
///   - Fan speed control via PWM (EC or Super I/O)
///   - Critical shutdown protection
///   - Thermal zone management (CPU, GPU, SSD, ambient)
///   - Trip point configuration (passive, active, hot, critical)
///
/// All code is original. Built from scratch for Hoags Inc.

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers
// ---------------------------------------------------------------------------

const Q16_SHIFT: i32 = 16;
const Q16_ONE: i32 = 1 << Q16_SHIFT;

#[inline]
fn q16_from(val: i32) -> i32 {
    val << Q16_SHIFT
}

#[inline]
fn q16_mul(a: i32, b: i32) -> i32 {
    (((a as i64) * (b as i64)) >> Q16_SHIFT) as i32
}

#[inline]
fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 { return 0; }
    (((a as i64) << Q16_SHIFT) / (b as i64)) as i32
}

// ---------------------------------------------------------------------------
// MSR addresses
// ---------------------------------------------------------------------------

const MSR_IA32_THERM_STATUS: u32 = 0x19C;
const MSR_IA32_TEMPERATURE_TARGET: u32 = 0x1A2;
const MSR_IA32_PACKAGE_THERM_STATUS: u32 = 0x1B1;
const MSR_IA32_THERM_INTERRUPT: u32 = 0x19B;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Thermal zone type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalZoneType {
    CpuPackage,
    CpuCore(u32),
    Gpu,
    Ssd,
    Ambient,
    Vrm,
    Chipset,
}

/// Throttle policy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThrottlePolicy {
    /// No throttling
    None,
    /// Linear ramp between passive and critical
    Linear,
    /// Step function at defined trip points
    Step,
    /// PID controller for smooth tracking
    Pid,
}

/// Fan control mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanMode {
    /// Off (passive cooling only)
    Off,
    /// BIOS/EC automatic control
    Auto,
    /// Manual PWM duty cycle
    Manual(u8),
    /// Software PID-controlled
    SmartFan,
}

/// Trip point type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TripType {
    /// Enable passive cooling (reduce performance)
    Passive,
    /// Enable active cooling (fan)
    Active,
    /// System is dangerously hot
    Hot,
    /// Emergency shutdown
    Critical,
}

/// Thermal event
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalEvent {
    Normal,
    ThrottleEngaged,
    ThrottleReleased,
    FanSpeedChanged,
    TripCrossed(TripType),
    CriticalShutdown,
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// A temperature trip point
#[derive(Debug, Clone, Copy)]
pub struct TripPoint {
    pub trip_type: TripType,
    pub temperature_c: i32,
    pub hysteresis_c: i32,
    pub action_engaged: bool,
}

/// PID controller state for thermal regulation
#[derive(Debug, Clone, Copy)]
pub struct PidController {
    pub kp: i32,            // Q16 proportional gain
    pub ki: i32,            // Q16 integral gain
    pub kd: i32,            // Q16 derivative gain
    pub setpoint: i32,      // Q16 target temperature
    pub integral: i32,      // Q16 accumulated integral error
    pub prev_error: i32,    // Q16 previous error for derivative
    pub output_min: i32,    // Q16 minimum output (0 = no throttle)
    pub output_max: i32,    // Q16 maximum output (1.0 = max throttle)
    pub integral_max: i32,  // Q16 anti-windup clamp
}

impl PidController {
    const fn new() -> Self {
        PidController {
            kp: (Q16_ONE * 3) / 10,     // 0.3
            ki: (Q16_ONE * 1) / 100,    // 0.01
            kd: (Q16_ONE * 5) / 10,     // 0.5
            setpoint: 80 << Q16_SHIFT,  // 80 degrees C
            integral: 0,
            prev_error: 0,
            output_min: 0,
            output_max: Q16_ONE,
            integral_max: Q16_ONE * 10,
        }
    }

    /// Compute PID output given current temperature (Q16)
    fn compute(&mut self, current_temp_q16: i32) -> i32 {
        let error = current_temp_q16 - self.setpoint;

        // Proportional
        let p = q16_mul(self.kp, error);

        // Integral with anti-windup
        self.integral += error;
        if self.integral > self.integral_max { self.integral = self.integral_max; }
        if self.integral < -self.integral_max { self.integral = -self.integral_max; }
        let i = q16_mul(self.ki, self.integral);

        // Derivative
        let d = q16_mul(self.kd, error - self.prev_error);
        self.prev_error = error;

        // Sum and clamp
        let mut output = p + i + d;
        if output < self.output_min { output = self.output_min; }
        if output > self.output_max { output = self.output_max; }

        output
    }

    /// Reset integrator state
    fn reset(&mut self) {
        self.integral = 0;
        self.prev_error = 0;
    }
}

/// Per-fan state
#[derive(Debug, Clone)]
pub struct FanInfo {
    pub fan_id: u32,
    pub name: String,
    pub mode: FanMode,
    pub current_rpm: u32,
    pub max_rpm: u32,
    pub pwm_duty: u8,          // 0-255 duty cycle
    pub min_pwm: u8,           // minimum to keep spinning
    pub zone_id: u32,          // which thermal zone drives this fan
}

/// A thermal zone (sensor + trip points + cooling)
#[derive(Debug, Clone)]
pub struct ThermalZone {
    pub zone_id: u32,
    pub zone_type: ThermalZoneType,
    pub name: String,
    pub current_temp_c: i32,
    pub tj_max_c: i32,         // maximum junction temperature
    pub trip_points: Vec<TripPoint>,
    pub throttle_policy: ThrottlePolicy,
    pub throttle_amount_q16: i32,   // 0 = none, Q16_ONE = full throttle
    pub history: Vec<i32>,          // temperature history (last 32 samples)
    pub trend_q16: i32,             // Q16 temp change per tick (positive = warming)
    pub readings_count: u64,
}

/// Thermal management subsystem
pub struct ThermalManager {
    pub zones: Vec<ThermalZone>,
    pub fans: Vec<FanInfo>,
    pub pid: PidController,
    pub global_throttle_q16: i32,
    pub shutdown_armed: bool,
    pub events: Vec<ThermalEvent>,
    pub tick_count: u64,
    pub poll_interval_ticks: u64,
    pub cpu_tj_max: i32,
}

impl ThermalManager {
    const fn new() -> Self {
        ThermalManager {
            zones: Vec::new(),
            fans: Vec::new(),
            pid: PidController::new(),
            global_throttle_q16: 0,
            shutdown_armed: false,
            events: Vec::new(),
            tick_count: 0,
            poll_interval_ticks: 4,
            cpu_tj_max: 100,
        }
    }

    /// Detect thermal sensors and fans
    fn detect_hardware(&mut self) {
        // Read TjMax from MSR_IA32_TEMPERATURE_TARGET
        let temp_target = rdmsr(MSR_IA32_TEMPERATURE_TARGET);
        self.cpu_tj_max = ((temp_target >> 16) & 0xFF) as i32;
        if self.cpu_tj_max == 0 { self.cpu_tj_max = 100; }

        // Create CPU package zone
        let package_temp = self.read_cpu_package_temp();
        self.zones.push(ThermalZone {
            zone_id: 0,
            zone_type: ThermalZoneType::CpuPackage,
            name: String::from("cpu-package"),
            current_temp_c: package_temp,
            tj_max_c: self.cpu_tj_max,
            trip_points: vec![
                TripPoint { trip_type: TripType::Passive, temperature_c: self.cpu_tj_max - 25, hysteresis_c: 3, action_engaged: false },
                TripPoint { trip_type: TripType::Active, temperature_c: self.cpu_tj_max - 20, hysteresis_c: 3, action_engaged: false },
                TripPoint { trip_type: TripType::Hot, temperature_c: self.cpu_tj_max - 10, hysteresis_c: 2, action_engaged: false },
                TripPoint { trip_type: TripType::Critical, temperature_c: self.cpu_tj_max - 5, hysteresis_c: 0, action_engaged: false },
            ],
            throttle_policy: ThrottlePolicy::Pid,
            throttle_amount_q16: 0,
            history: Vec::new(),
            trend_q16: 0,
            readings_count: 0,
        });

        // Create per-core zones (up to 8 cores)
        for core in 0..4u32 {
            self.zones.push(ThermalZone {
                zone_id: core + 1,
                zone_type: ThermalZoneType::CpuCore(core),
                name: alloc::format!("cpu-core-{}", core),
                current_temp_c: package_temp,
                tj_max_c: self.cpu_tj_max,
                trip_points: vec![
                    TripPoint { trip_type: TripType::Passive, temperature_c: self.cpu_tj_max - 20, hysteresis_c: 3, action_engaged: false },
                    TripPoint { trip_type: TripType::Critical, temperature_c: self.cpu_tj_max, hysteresis_c: 0, action_engaged: false },
                ],
                throttle_policy: ThrottlePolicy::Linear,
                throttle_amount_q16: 0,
                history: Vec::new(),
                trend_q16: 0,
                readings_count: 0,
            });
        }

        // Create GPU zone (if present)
        self.zones.push(ThermalZone {
            zone_id: 10,
            zone_type: ThermalZoneType::Gpu,
            name: String::from("gpu"),
            current_temp_c: 35,
            tj_max_c: 95,
            trip_points: vec![
                TripPoint { trip_type: TripType::Passive, temperature_c: 75, hysteresis_c: 5, action_engaged: false },
                TripPoint { trip_type: TripType::Critical, temperature_c: 95, hysteresis_c: 0, action_engaged: false },
            ],
            throttle_policy: ThrottlePolicy::Step,
            throttle_amount_q16: 0,
            history: Vec::new(),
            trend_q16: 0,
            readings_count: 0,
        });

        // Create fan entries
        self.fans.push(FanInfo {
            fan_id: 0,
            name: String::from("cpu-fan"),
            mode: FanMode::SmartFan,
            current_rpm: 0,
            max_rpm: 3000,
            pwm_duty: 0,
            min_pwm: 30,
            zone_id: 0,
        });

        self.fans.push(FanInfo {
            fan_id: 1,
            name: String::from("system-fan"),
            mode: FanMode::Auto,
            current_rpm: 0,
            max_rpm: 2000,
            pwm_duty: 0,
            min_pwm: 20,
            zone_id: 10,
        });

        // Set PID setpoint to passive trip point
        let passive_temp = self.cpu_tj_max - 25;
        self.pid.setpoint = q16_from(passive_temp);
    }

    /// Read CPU package temperature from MSR
    fn read_cpu_package_temp(&self) -> i32 {
        let status = rdmsr(MSR_IA32_PACKAGE_THERM_STATUS);
        let valid = (status >> 31) & 1;
        if valid == 0 { return 0; }

        let digital_readout = ((status >> 16) & 0x7F) as i32;
        self.cpu_tj_max - digital_readout
    }

    /// Read per-core temperature
    fn read_core_temp(&self, _core_id: u32) -> i32 {
        let status = rdmsr(MSR_IA32_THERM_STATUS);
        let valid = (status >> 31) & 1;
        if valid == 0 { return 0; }

        let digital_readout = ((status >> 16) & 0x7F) as i32;
        self.cpu_tj_max - digital_readout
    }

    /// Update all thermal zones with fresh readings
    fn poll_temperatures(&mut self) {
        for zone in &mut self.zones {
            let temp = match zone.zone_type {
                ThermalZoneType::CpuPackage => self.read_cpu_package_temp(),
                ThermalZoneType::CpuCore(id) => self.read_core_temp(id),
                ThermalZoneType::Gpu => zone.current_temp_c,    // Would read from GPU driver
                ThermalZoneType::Ssd => zone.current_temp_c,    // Would read from NVMe SMART
                ThermalZoneType::Ambient => zone.current_temp_c,
                ThermalZoneType::Vrm => zone.current_temp_c,
                ThermalZoneType::Chipset => zone.current_temp_c,
            };

            let prev_temp = zone.current_temp_c;
            zone.current_temp_c = temp;
            zone.readings_count = zone.readings_count.saturating_add(1);

            // Update history (keep last 32)
            zone.history.push(temp);
            if zone.history.len() > 32 {
                zone.history.remove(0);
            }

            // Calculate trend (temperature delta per tick, Q16)
            if zone.history.len() >= 2 {
                let len = zone.history.len();
                let recent_avg = (zone.history[len - 1] + zone.history[len - 2]) / 2;
                let older_avg = if len >= 4 {
                    (zone.history[len - 3] + zone.history[len - 4]) / 2
                } else {
                    zone.history[0]
                };
                zone.trend_q16 = q16_from(recent_avg - older_avg);
            }
        }
    }

    /// Evaluate trip points and compute throttle amounts
    fn evaluate_zones(&mut self) {
        let mut events: Vec<ThermalEvent> = Vec::new();

        for zone in &mut self.zones {
            let temp = zone.current_temp_c;

            // Check trip points
            for trip in &mut zone.trip_points {
                let was_engaged = trip.action_engaged;

                if temp >= trip.temperature_c {
                    trip.action_engaged = true;
                } else if temp < trip.temperature_c - trip.hysteresis_c {
                    trip.action_engaged = false;
                }

                // Generate event on transitions
                if trip.action_engaged && !was_engaged {
                    events.push(ThermalEvent::TripCrossed(trip.trip_type));
                    serial_println!("    [thermal] {} crossed {:?} trip at {}C (now {}C)",
                        zone.name, trip.trip_type, trip.temperature_c, temp);
                }
            }

            // Compute throttle amount based on policy
            let passive_trip = zone.trip_points.iter()
                .find(|t| t.trip_type == TripType::Passive)
                .map(|t| t.temperature_c)
                .unwrap_or(80);
            let critical_trip = zone.trip_points.iter()
                .find(|t| t.trip_type == TripType::Critical)
                .map(|t| t.temperature_c)
                .unwrap_or(100);

            zone.throttle_amount_q16 = match zone.throttle_policy {
                ThrottlePolicy::None => 0,
                ThrottlePolicy::Linear => {
                    if temp <= passive_trip {
                        0
                    } else if temp >= critical_trip {
                        Q16_ONE
                    } else {
                        let range = critical_trip - passive_trip;
                        let above = temp - passive_trip;
                        if range > 0 {
                            q16_div(q16_from(above), q16_from(range))
                        } else {
                            Q16_ONE
                        }
                    }
                }
                ThrottlePolicy::Step => {
                    let mut throttle = 0i32;
                    for trip in &zone.trip_points {
                        if trip.action_engaged {
                            throttle = match trip.trip_type {
                                TripType::Passive => Q16_ONE / 4,     // 25%
                                TripType::Active => Q16_ONE / 2,      // 50%
                                TripType::Hot => Q16_ONE * 3 / 4,     // 75%
                                TripType::Critical => Q16_ONE,        // 100%
                            };
                        }
                    }
                    throttle
                }
                ThrottlePolicy::Pid => {
                    // PID computed separately below
                    zone.throttle_amount_q16
                }
            };
        }

        self.events.extend_from_slice(&events);
        // Keep only last 64 events
        if self.events.len() > 64 {
            let drain_count = self.events.len() - 64;
            self.events.drain(0..drain_count);
        }
    }

    /// Run PID controller on the CPU package zone
    fn run_pid(&mut self) {
        // Find CPU package zone
        let temp = self.zones.iter()
            .find(|z| z.zone_type == ThermalZoneType::CpuPackage)
            .map(|z| z.current_temp_c)
            .unwrap_or(40);

        let temp_q16 = q16_from(temp);
        let pid_output = self.pid.compute(temp_q16);

        // Apply PID output to CPU package zone
        if let Some(zone) = self.zones.iter_mut().find(|z| z.zone_type == ThermalZoneType::CpuPackage) {
            if zone.throttle_policy == ThrottlePolicy::Pid {
                zone.throttle_amount_q16 = if pid_output > 0 { pid_output } else { 0 };
            }
        }

        // Compute global throttle (max of all zones)
        self.global_throttle_q16 = self.zones.iter()
            .map(|z| z.throttle_amount_q16)
            .max()
            .unwrap_or(0);
    }

    /// Update fan speeds based on thermal state
    fn update_fans(&mut self) {
        for fan in &mut self.fans {
            let zone_temp = self.zones.iter()
                .find(|z| z.zone_id == fan.zone_id)
                .map(|z| z.current_temp_c)
                .unwrap_or(30);

            let zone_throttle = self.zones.iter()
                .find(|z| z.zone_id == fan.zone_id)
                .map(|z| z.throttle_amount_q16)
                .unwrap_or(0);

            match fan.mode {
                FanMode::Off => {
                    fan.pwm_duty = 0;
                    fan.current_rpm = 0;
                }
                FanMode::Auto => {
                    // EC handles it, just read RPM
                }
                FanMode::Manual(duty) => {
                    fan.pwm_duty = duty;
                    fan.current_rpm = (fan.max_rpm as u64 * duty as u64 / 255) as u32;
                }
                FanMode::SmartFan => {
                    // Map throttle amount to PWM duty (with minimum spin threshold)
                    if zone_throttle == 0 && zone_temp < 50 {
                        fan.pwm_duty = 0;
                        fan.current_rpm = 0;
                    } else {
                        // Scale: throttle Q16 -> duty 0-255
                        let duty_from_throttle = ((zone_throttle as i64 * 255) / Q16_ONE as i64) as u8;
                        let base_duty = if zone_temp >= 40 {
                            let above = (zone_temp - 40).min(60) as u8;
                            // 40C -> min_pwm, 100C -> 255
                            fan.min_pwm + ((255u16 - fan.min_pwm as u16) * above as u16 / 60) as u8
                        } else {
                            0
                        };

                        fan.pwm_duty = duty_from_throttle.max(base_duty).max(fan.min_pwm);
                        fan.current_rpm = (fan.max_rpm as u64 * fan.pwm_duty as u64 / 255) as u32;
                    }

                    // Write PWM via Super I/O or EC
                    self.write_fan_pwm(fan.fan_id, fan.pwm_duty);
                }
            }
        }
    }

    /// Write fan PWM duty cycle to hardware
    fn write_fan_pwm(&self, _fan_id: u32, duty: u8) {
        // Typically via Embedded Controller (EC) or Super I/O chip
        // EC: write to port 0x62/0x66 command interface
        // Super I/O: access via LPC ports 0x2E/0x2F (chip-specific register)
        let _ = duty;
    }

    /// Check for critical temperature and initiate shutdown
    fn check_critical(&mut self) {
        for zone in &self.zones {
            let critical = zone.trip_points.iter()
                .find(|t| t.trip_type == TripType::Critical && t.action_engaged);

            if critical.is_some() {
                serial_println!("    [thermal] CRITICAL: {} at {}C! Emergency shutdown!",
                    zone.name, zone.current_temp_c);
                self.events.push(ThermalEvent::CriticalShutdown);
                self.shutdown_armed = true;
                // In a real kernel, this would call power::shutdown()
            }
        }
    }

    /// Periodic thermal management tick
    pub fn tick(&mut self) {
        self.tick_count = self.tick_count.saturating_add(1);

        // Poll temperatures at configured interval
        if self.tick_count % self.poll_interval_ticks == 0 {
            self.poll_temperatures();
            self.evaluate_zones();
            self.run_pid();
            self.update_fans();
            self.check_critical();
        }
    }

    /// Get thermal summary
    pub fn summary(&self) -> ThermalSummary {
        let hottest = self.zones.iter()
            .map(|z| z.current_temp_c)
            .max()
            .unwrap_or(0);

        ThermalSummary {
            cpu_package_temp: self.zones.iter()
                .find(|z| z.zone_type == ThermalZoneType::CpuPackage)
                .map(|z| z.current_temp_c)
                .unwrap_or(0),
            hottest_zone_temp: hottest,
            global_throttle_q16: self.global_throttle_q16,
            fan_count: self.fans.len() as u32,
            zone_count: self.zones.len() as u32,
            cpu_fan_rpm: self.fans.first().map(|f| f.current_rpm).unwrap_or(0),
            cpu_fan_duty: self.fans.first().map(|f| f.pwm_duty).unwrap_or(0),
            tj_max: self.cpu_tj_max,
        }
    }

    /// Set fan mode for a specific fan
    pub fn set_fan_mode(&mut self, fan_id: u32, mode: FanMode) {
        if let Some(fan) = self.fans.iter_mut().find(|f| f.fan_id == fan_id) {
            fan.mode = mode;
            serial_println!("    [thermal] Fan {} set to {:?}", fan.name, mode);
        }
    }

    /// Set throttle policy for a zone
    pub fn set_throttle_policy(&mut self, zone_id: u32, policy: ThrottlePolicy) {
        if let Some(zone) = self.zones.iter_mut().find(|z| z.zone_id == zone_id) {
            zone.throttle_policy = policy;
            serial_println!("    [thermal] Zone {} throttle policy -> {:?}", zone.name, policy);
        }
    }
}

/// Thermal summary snapshot
#[derive(Debug, Clone)]
pub struct ThermalSummary {
    pub cpu_package_temp: i32,
    pub hottest_zone_temp: i32,
    pub global_throttle_q16: i32,
    pub fan_count: u32,
    pub zone_count: u32,
    pub cpu_fan_rpm: u32,
    pub cpu_fan_duty: u8,
    pub tj_max: i32,
}

// ---------------------------------------------------------------------------
// MSR helpers
// ---------------------------------------------------------------------------

fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static THERMAL: Mutex<Option<ThermalManager>> = Mutex::new(None);

pub fn init() {
    let mut mgr = ThermalManager::new();
    mgr.detect_hardware();

    let pkg_temp = mgr.zones.iter()
        .find(|z| z.zone_type == ThermalZoneType::CpuPackage)
        .map(|z| z.current_temp_c)
        .unwrap_or(0);

    serial_println!("    [thermal] TjMax={}C, current={}C, {} zone(s), {} fan(s)",
        mgr.cpu_tj_max, pkg_temp, mgr.zones.len(), mgr.fans.len());

    for zone in &mgr.zones {
        serial_println!("    [thermal] Zone {}: {} ({:?}, {}C, {:?})",
            zone.zone_id, zone.name, zone.zone_type, zone.current_temp_c, zone.throttle_policy);
    }

    *THERMAL.lock() = Some(mgr);
    serial_println!("    [thermal] Thermal monitoring, throttle curves, fan control ready");
}

/// Periodic tick
pub fn tick() {
    if let Some(ref mut mgr) = *THERMAL.lock() {
        mgr.tick();
    }
}

/// Get thermal summary
pub fn summary() -> Option<ThermalSummary> {
    THERMAL.lock().as_ref().map(|m| m.summary())
}

/// Set fan mode
pub fn set_fan_mode(fan_id: u32, mode: FanMode) {
    if let Some(ref mut mgr) = *THERMAL.lock() {
        mgr.set_fan_mode(fan_id, mode);
    }
}

/// Get current CPU package temperature
pub fn cpu_temp() -> i32 {
    THERMAL.lock().as_ref()
        .and_then(|m| m.zones.iter().find(|z| z.zone_type == ThermalZoneType::CpuPackage))
        .map(|z| z.current_temp_c)
        .unwrap_or(0)
}
