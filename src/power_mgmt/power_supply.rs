use crate::power_mgmt::battery::BatteryState;
/// Power supply abstraction and adaptive profile controller.
///
/// Part of the AIOS power_mgmt subsystem.
///
/// Sits above the battery and lid drivers.  Tracks the active power source
/// (AC vs. battery), exposes named power profiles, and automatically adjusts
/// CPU governor, backlight, and other policies when battery level changes.
///
/// No heap, no floats, no panics.
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Power source
// ---------------------------------------------------------------------------

/// Where the system is currently drawing power from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PowerSource {
    /// Running on mains / AC adapter.
    AC,
    /// Running on internal battery.
    Battery,
    /// Source is not yet determined (pre-first-poll).
    Unknown,
}

// ---------------------------------------------------------------------------
// Power profiles
// ---------------------------------------------------------------------------

/// Named operating profiles that control CPU frequency, backlight, and
/// peripheral power.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PowerProfile {
    /// Maximum performance: cpufreq at max, backlight at 100%, all devices on.
    Performance,
    /// Balanced: cpufreq ondemand, backlight at 80%, default device policy.
    Balanced,
    /// Power save: cpufreq at min, backlight at 40%, wifi in powersave mode.
    Powersave,
    /// Custom: caller-supplied level (0-100) drives a linear interpolation
    /// between Powersave (0) and Performance (100).
    Custom(u8),
}

// ---------------------------------------------------------------------------
// Thresholds for auto_adjust_profile
// ---------------------------------------------------------------------------

/// Below this percent, switch to Powersave automatically.
const THRESHOLD_LOW: u8 = 20;
/// Below this percent, also emit a serial warning.
const THRESHOLD_CRITICAL: u8 = 5;

// ---------------------------------------------------------------------------
// Backlight levels per profile
// ---------------------------------------------------------------------------
const BL_PERFORMANCE: u8 = 100;
const BL_BALANCED: u8 = 80;
const BL_POWERSAVE: u8 = 40;

// ---------------------------------------------------------------------------
// Energy performance bias values (IA32_ENERGY_PERF_BIAS)
// 0 = max performance, 15 = max power-saving
// ---------------------------------------------------------------------------
const EPB_PERFORMANCE: u8 = 0;
const EPB_BALANCED: u8 = 6;
const EPB_POWERSAVE: u8 = 15;

// ---------------------------------------------------------------------------
// Module state
// ---------------------------------------------------------------------------

struct PowerSupplyState {
    source: PowerSource,
    profile: PowerProfile,
}

static STATE: Mutex<PowerSupplyState> = Mutex::new(PowerSupplyState {
    source: PowerSource::Unknown,
    profile: PowerProfile::Balanced,
});

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Return the current power source.
pub fn current_source() -> PowerSource {
    STATE.lock().source
}

/// Return the currently active power profile.
pub fn current_profile() -> PowerProfile {
    STATE.lock().profile
}

/// Apply a specific power profile.
///
/// Adjusts CPU governor, energy-performance bias, and display backlight to
/// match the requested profile.  Safe to call from any context (no blocking).
pub fn set_profile(p: PowerProfile) {
    use crate::power_mgmt::cpufreq;

    let (bl_level, epb) = match p {
        PowerProfile::Performance => (BL_PERFORMANCE, EPB_PERFORMANCE),
        PowerProfile::Balanced => (BL_BALANCED, EPB_BALANCED),
        PowerProfile::Powersave => (BL_POWERSAVE, EPB_POWERSAVE),
        PowerProfile::Custom(v) => {
            // Linear interpolation for backlight (v=0 → BL_POWERSAVE, v=100 → BL_PERFORMANCE)
            let bl = BL_POWERSAVE as u32
                + ((BL_PERFORMANCE as u32 - BL_POWERSAVE as u32) * v as u32) / 100;
            let bl = if bl > 100 { 100 } else { bl as u8 };
            // EPB: 0 = perf (v=100), 15 = save (v=0)
            let epb = 15u32.saturating_sub((15u32 * v as u32) / 100) as u8;
            (bl, epb)
        }
    };

    // CPU governor
    let gov = match p {
        PowerProfile::Performance => cpufreq::Governor::Performance,
        PowerProfile::Powersave => cpufreq::Governor::Powersave,
        _ => cpufreq::Governor::Ondemand,
    };
    cpufreq::set_governor(gov);
    cpufreq::set_energy_perf_bias(epb);

    // Backlight — only adjust if lid is open
    if crate::power_mgmt::lid::lid_is_open() {
        crate::power_mgmt::lid::backlight_set(bl_level);
    }

    serial_println!(
        "  power_supply: profile={:?} bl={}% epb={}",
        p,
        bl_level,
        epb
    );

    STATE.lock().profile = p;
}

/// Automatically select a profile based on the current battery state.
///
/// Rules (in priority order):
///   1. AC online             → Balanced
///   2. capacity < 5%         → Powersave + serial warning
///   3. capacity < 20%        → Powersave
///   4. Otherwise             → keep current profile (no auto-downgrade from Performance
///      chosen by the user when the battery is healthy)
pub fn auto_adjust_profile() {
    let (ac, cap) = {
        let s = crate::power_mgmt::battery::battery_get();
        (s.ac_online, s.capacity_percent)
    };

    if ac {
        let cur = STATE.lock().profile;
        // Only auto-switch away from Powersave when AC reconnects; leave
        // Performance alone if the user set it intentionally.
        if cur == PowerProfile::Powersave {
            set_profile(PowerProfile::Balanced);
        }
        STATE.lock().source = PowerSource::AC;
        return;
    }

    STATE.lock().source = PowerSource::Battery;

    if cap < THRESHOLD_CRITICAL {
        serial_println!(
            "  power_supply: CRITICAL battery {}% — forcing Powersave",
            cap
        );
        set_profile(PowerProfile::Powersave);
    } else if cap < THRESHOLD_LOW {
        set_profile(PowerProfile::Powersave);
    }
    // Above THRESHOLD_LOW on battery: respect current profile selection.
}

/// Called by `battery::battery_sci_event` when the ACPI SCI fires for a
/// battery change.  Re-evaluates the adaptive profile.
pub fn notify_battery_event(state: &BatteryState) {
    // Update source from the fresh state snapshot
    {
        let mut guard = STATE.lock();
        guard.source = if state.ac_online {
            PowerSource::AC
        } else {
            PowerSource::Battery
        };
    }
    auto_adjust_profile();
}

/// Initialise the power supply abstraction.
///
/// Reads the initial battery state and selects a sensible starting profile:
///   - AC online → Balanced
///   - Battery present → check level, may start in Powersave
///   - No battery → Performance (desktop / always-AC system)
pub fn init() {
    let s = crate::power_mgmt::battery::battery_get();

    let (source, profile) = if s.ac_online {
        (PowerSource::AC, PowerProfile::Balanced)
    } else if s.present {
        let p = if s.capacity_percent < THRESHOLD_LOW {
            PowerProfile::Powersave
        } else {
            PowerProfile::Balanced
        };
        (PowerSource::Battery, p)
    } else {
        // No battery detected — assume always-AC desktop
        (PowerSource::AC, PowerProfile::Performance)
    };

    {
        let mut guard = STATE.lock();
        guard.source = source;
        guard.profile = profile;
    }

    // Apply the chosen profile to hardware
    set_profile(profile);

    serial_println!(
        "  power_supply: source={:?} profile={:?} cap={}%",
        source,
        profile,
        s.capacity_percent
    );
}
