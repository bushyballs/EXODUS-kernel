use crate::sync::Mutex;
/// Thermal zone management driver for Genesis — no-heap, fixed-size arrays
///
/// Manages up to 8 thermal zones with configurable trip points:
///   - Active trip: trigger cooling fans
///   - Passive trip: request CPU frequency reduction
///   - Hot trip: log a warning
///   - Critical trip: initiate system power-off
///
/// Temperature values are stored as millidegrees Celsius (milli°C).
/// Hysteresis prevents rapid toggling around a trip point.
///
/// Reads temperature via `crate::power_mgmt::cpufreq::get_temperature_c()`
/// for CPU zones (returns °C, converted to milli°C here).
///
/// All rules strictly observed:
///   - No heap: no Vec, Box, String, alloc::*
///   - No panics: no unwrap(), expect(), panic!()
///   - No float casts: no as f64, as f32
///   - Saturating arithmetic for counters
///   - Wrapping arithmetic for sequence numbers
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of thermal zones
const MAX_THERMAL_ZONES: usize = 8;

/// Maximum trip points per zone
const MAX_TRIPS_PER_ZONE: usize = 8;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Classification of a thermal trip point
#[derive(Copy, Clone, PartialEq)]
pub enum ThermalTripType {
    /// Activate a cooling device (fan)
    Active,
    /// Request passive cooling (CPU throttle)
    Passive,
    /// Temperature is dangerously high — log warning
    Hot,
    /// Temperature is critical — initiate power-off
    Critical,
}

/// A single thermal trip point
#[derive(Copy, Clone)]
pub struct ThermalTrip {
    /// Trip classification
    pub trip_type: ThermalTripType,
    /// Temperature at which this trip fires (millidegrees Celsius)
    pub temp_milli_c: i32,
    /// Hysteresis: trip de-asserts when temp falls below (temp_milli_c - hysteresis)
    pub hysteresis: i32,
    /// True when this trip is currently active (temp >= trip threshold)
    pub triggered: bool,
}

impl ThermalTrip {
    /// Return a zeroed, inactive trip slot
    pub const fn empty() -> Self {
        ThermalTrip {
            trip_type: ThermalTripType::Active,
            temp_milli_c: 0,
            hysteresis: 0,
            triggered: false,
        }
    }
}

/// A thermal zone with an array of trip points
#[derive(Copy, Clone)]
pub struct ThermalZone {
    /// Human-readable zone name (null-padded ASCII, up to 16 bytes)
    pub name: [u8; 16],
    /// Unique zone ID (index in the global table)
    pub zone_id: u32,
    /// Current temperature (millidegrees Celsius)
    pub temp_milli_c: i32,
    /// Array of trip points
    pub trips: [ThermalTrip; MAX_TRIPS_PER_ZONE],
    /// Number of valid trip points
    pub trip_count: u8,
    /// Index of the associated hardware monitor cooling device (driver-defined)
    pub cooling_device_idx: u32,
    /// True when this slot is occupied
    pub active: bool,
    /// Previous temperature reading (for trend calculation)
    pub prev_temp: i32,
    /// Polling interval in milliseconds (default 1000)
    pub poll_interval_ms: u32,
    /// Timestamp of the last poll (ms, internal)
    last_poll_ms: u64,
}

impl ThermalZone {
    /// Return a zeroed, inactive thermal zone slot
    pub const fn empty() -> Self {
        ThermalZone {
            name: [0u8; 16],
            zone_id: 0,
            temp_milli_c: 0,
            trips: [ThermalTrip::empty(); MAX_TRIPS_PER_ZONE],
            trip_count: 0,
            cooling_device_idx: 0,
            active: false,
            prev_temp: 0,
            poll_interval_ms: 1000,
            last_poll_ms: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static THERMAL_ZONES: Mutex<[ThermalZone; MAX_THERMAL_ZONES]> =
    Mutex::new([ThermalZone::empty(); MAX_THERMAL_ZONES]);

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

/// Evaluate trip points for a zone given a new temperature reading.
///
/// Handles hysteresis: a trip fires when `temp >= trip_temp`; it de-asserts
/// when `temp < trip_temp - hysteresis`.
///
/// Side effects (on trip state change):
///   - Critical: log an emergency message (power-off stub)
///   - Hot:      log a warning
///   - Passive:  call cpufreq::reduce_by to throttle the CPU
///   - Active:   log fan activation (hardware fan control stub)
fn evaluate_trips(zone: &mut ThermalZone, temp: i32) {
    for i in 0..zone.trip_count as usize {
        if i >= MAX_TRIPS_PER_ZONE {
            break;
        }
        let trip = &mut zone.trips[i];
        let should_trigger = temp >= trip.temp_milli_c;
        let below_hyst = temp < trip.temp_milli_c.saturating_sub(trip.hysteresis);

        if should_trigger && !trip.triggered {
            trip.triggered = true;
            match trip.trip_type {
                ThermalTripType::Critical => {
                    serial_println!(
                        "  THERMAL CRITICAL: zone {} temp {} mC >= {} mC — \
                         initiating power-off stub",
                        zone.zone_id,
                        temp,
                        trip.temp_milli_c
                    );
                    // Stub: a real implementation would call
                    // crate::power_mgmt::suspend::system_poweroff()
                }
                ThermalTripType::Hot => {
                    serial_println!(
                        "  THERMAL WARNING: zone {} temp {} mC >= {} mC",
                        zone.zone_id,
                        temp,
                        trip.temp_milli_c
                    );
                }
                ThermalTripType::Passive => {
                    // Request a 100 MHz reduction per crossing
                    crate::power_mgmt::cpufreq::reduce_by(100_000);
                    serial_println!(
                        "  Thermal: passive cooling triggered for zone {} \
                         at {} mC",
                        zone.zone_id,
                        temp
                    );
                }
                ThermalTripType::Active => {
                    // Stub: activate cooling fan via hwmon index
                    serial_println!(
                        "  Thermal: active cooling triggered for zone {} \
                         (cooling_dev {})",
                        zone.zone_id,
                        zone.cooling_device_idx
                    );
                }
            }
        } else if below_hyst && trip.triggered {
            trip.triggered = false;
            // Trip de-asserted — log recovery for passive/hot
            match trip.trip_type {
                ThermalTripType::Passive => {
                    serial_println!(
                        "  Thermal: passive cooling released for zone {}",
                        zone.zone_id
                    );
                }
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a new thermal zone.
///
/// `name`     — ASCII name (up to 16 bytes).
/// `poll_ms`  — polling interval in milliseconds.
///
/// Returns the zone index on success, or `None` if the table is full.
pub fn thermal_register_zone(name: &[u8], poll_ms: u32) -> Option<usize> {
    let mut zones = THERMAL_ZONES.lock();
    for (i, z) in zones.iter_mut().enumerate() {
        if !z.active {
            copy_name(&mut z.name, name);
            z.zone_id = i as u32;
            z.poll_interval_ms = if poll_ms == 0 { 1000 } else { poll_ms };
            z.active = true;
            z.trip_count = 0;
            z.temp_milli_c = 0;
            z.prev_temp = 0;
            z.last_poll_ms = 0;
            return Some(i);
        }
    }
    None
}

/// Add a trip point to an existing zone.
///
/// Returns `true` on success, `false` if the zone is full or invalid.
pub fn thermal_add_trip(zone_idx: usize, trip: ThermalTrip) -> bool {
    if zone_idx >= MAX_THERMAL_ZONES {
        return false;
    }
    let mut zones = THERMAL_ZONES.lock();
    if !zones[zone_idx].active {
        return false;
    }
    let tc = zones[zone_idx].trip_count as usize;
    if tc >= MAX_TRIPS_PER_ZONE {
        return false;
    }
    zones[zone_idx].trips[tc] = trip;
    zones[zone_idx].trip_count = zones[zone_idx].trip_count.saturating_add(1);
    true
}

/// Update the temperature for a zone and evaluate trip points.
///
/// `temp_milli_c` — new temperature in millidegrees Celsius.
pub fn thermal_update_temp(zone_idx: usize, temp_milli_c: i32) {
    if zone_idx >= MAX_THERMAL_ZONES {
        return;
    }
    let mut zones = THERMAL_ZONES.lock();
    if !zones[zone_idx].active {
        return;
    }
    zones[zone_idx].prev_temp = zones[zone_idx].temp_milli_c;
    zones[zone_idx].temp_milli_c = temp_milli_c;
    // Evaluate trips — need to pass a mutable reference to the zone
    // while holding the lock.  We work on the specific slot directly.
    evaluate_trips(&mut zones[zone_idx], temp_milli_c);
}

/// Get the current temperature for a zone.
///
/// Returns `None` if `zone_idx` is out of range or inactive.
pub fn thermal_get_temp(zone_idx: usize) -> Option<i32> {
    if zone_idx >= MAX_THERMAL_ZONES {
        return None;
    }
    let zones = THERMAL_ZONES.lock();
    if !zones[zone_idx].active {
        return None;
    }
    Some(zones[zone_idx].temp_milli_c)
}

/// Periodic tick — call with the current system uptime in milliseconds.
///
/// For each active zone whose poll interval has elapsed, reads the current
/// CPU temperature from `cpufreq::get_temperature_c()` and calls
/// `thermal_update_temp()`.
pub fn thermal_tick(current_ms: u64) {
    // Collect which zones need polling (record idx + last_poll + interval)
    // without holding the lock during the actual temperature read.
    let mut to_poll: [usize; MAX_THERMAL_ZONES] = [MAX_THERMAL_ZONES; MAX_THERMAL_ZONES];
    let mut poll_count = 0usize;

    {
        let mut zones = THERMAL_ZONES.lock();
        for i in 0..MAX_THERMAL_ZONES {
            if !zones[i].active {
                continue;
            }
            let interval = zones[i].poll_interval_ms as u64;
            let elapsed = current_ms.saturating_sub(zones[i].last_poll_ms);
            if elapsed >= interval {
                zones[i].last_poll_ms = current_ms;
                if poll_count < MAX_THERMAL_ZONES {
                    to_poll[poll_count] = i;
                    poll_count = poll_count.saturating_add(1);
                }
            }
        }
    }

    // Now read temperatures and update zones (lock re-acquired per update)
    for pi in 0..poll_count {
        let zone_idx = to_poll[pi];
        // Read temperature from the CPU thermal MSR (returns °C as u8)
        let temp_c = crate::power_mgmt::cpufreq::get_temperature_c();
        // Convert to milli-Celsius: °C × 1000
        let temp_mc = (temp_c as i32).saturating_mul(1000);
        thermal_update_temp(zone_idx, temp_mc);
    }
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the thermal zone subsystem.
///
/// Registers the built-in "cpu-thermal" zone with:
///   - Passive trip at 85 000 mC (85 °C)
///   - Critical trip at 105 000 mC (105 °C)
pub fn init() {
    if let Some(idx) = thermal_register_zone(b"cpu-thermal", 1000) {
        let passive = ThermalTrip {
            trip_type: ThermalTripType::Passive,
            temp_milli_c: 85_000,
            hysteresis: 2_000,
            triggered: false,
        };
        let critical = ThermalTrip {
            trip_type: ThermalTripType::Critical,
            temp_milli_c: 105_000,
            hysteresis: 2_000,
            triggered: false,
        };
        thermal_add_trip(idx, passive);
        thermal_add_trip(idx, critical);
        serial_println!(
            "  ThermalZones: 'cpu-thermal' zone registered (idx {}, \
             passive=85°C, critical=105°C)",
            idx
        );
    } else {
        serial_println!("  ThermalZones: failed to register 'cpu-thermal' zone");
    }

    super::register("thermal-zones", super::DeviceType::Other);
    serial_println!(
        "  ThermalZones: subsystem initialized (max {} zones, {} trips/zone)",
        MAX_THERMAL_ZONES,
        MAX_TRIPS_PER_ZONE
    );
}
