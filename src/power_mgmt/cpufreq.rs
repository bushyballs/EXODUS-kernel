/// Intel P-state CPU frequency scaling driver.
///
/// Part of the AIOS power_mgmt subsystem.
///
/// Reads available P-states from IA32_PLATFORM_INFO MSR (0xCE):
///   Bits [15:8]  = max non-turbo ratio
///   Bits [47:40] = minimum ratio
/// Each ratio × 100 MHz = frequency in MHz.
///
/// P-state control is via IA32_PERF_CTL (0x199); status from
/// IA32_PERF_STATUS (0x198).  Turbo toggle via IA32_MISC_ENABLE (0x1A0)
/// bit 38.  Energy performance bias via IA32_ENERGY_PERF_BIAS (0x1B0).
/// Thermal read from IA32_THERM_STATUS (0x19C).
use crate::sync::Mutex;

// ── MSR addresses ──────────────────────────────────────────────────────────

const IA32_PERF_STATUS: u32 = 0x198;
const IA32_PERF_CTL: u32 = 0x199;
const IA32_THERM_STATUS: u32 = 0x19C;
const IA32_MISC_ENABLE: u32 = 0x1A0;
const IA32_TEMPERATURE_TARGET: u32 = 0x1A2;
const IA32_TURBO_RATIO_LIMIT: u32 = 0x1AD;
const IA32_ENERGY_PERF_BIAS: u32 = 0x1B0;
const MSR_PLATFORM_INFO: u32 = 0xCE;

/// Bit 38 of IA32_MISC_ENABLE: IDA (Demand-Based Switching) engage.
/// When SET, turbo boost is DISABLED.
const IDA_ENGAGE_BIT: u64 = 1u64 << 38;

/// Bus clock reference frequency.
const BUS_CLOCK_MHZ: u32 = 100;

/// TjMax default if IA32_TEMPERATURE_TARGET is unreadable.
const TJMAX_DEFAULT_C: u8 = 100;

// ── P-state table ──────────────────────────────────────────────────────────

/// A single Intel P-state entry.
#[derive(Clone, Copy)]
pub struct PState {
    /// P-state ratio multiplier.  freq_mhz = ratio * BUS_CLOCK_MHZ.
    pub ratio: u8,
    /// Estimated operating voltage (mV).  Set to 1000 on platforms that do
    /// not expose per-P-state voltage via MSR.
    pub voltage_mv: u16,
    /// Whether this P-state is currently selectable.
    pub active: bool,
}

// ── Governor policy ─────────────────────────────────────────────────────────

/// CPU frequency governor policy.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Governor {
    /// Always run at maximum non-turbo frequency.
    Performance,
    /// Always run at minimum frequency.
    Powersave,
    /// Jump to maximum above load threshold, scale proportionally below.
    Ondemand,
    /// Step frequency up/down by one P-state per tick.
    Conservative,
    /// Driven by scheduler utilisation hints (proportional).
    Schedutil,
}

// ── Driver state ────────────────────────────────────────────────────────────

/// Complete driver state, protected behind a Mutex.
struct CpuFreqState {
    /// P-state table (32 entries max; entries 0..pstate_count are valid).
    pstates: [PState; 32],
    pstate_count: u32,
    /// Current hardware ratio (read from IA32_PERF_STATUS).
    current_ratio: u8,
    /// Maximum non-turbo ratio (from IA32_PLATFORM_INFO bits [15:8]).
    max_ratio: u8,
    /// Minimum ratio (from IA32_PLATFORM_INFO bits [47:40]).
    min_ratio: u8,
    /// Single-core turbo ratio (from IA32_TURBO_RATIO_LIMIT bits [7:0]).
    turbo_ratio: u8,
    /// Active governor policy.
    governor: Governor,
    /// Rolling-average CPU load estimate (0–100 %).
    load_percent: u8,
    /// Target frequency computed by the governor (MHz).
    target_freq_mhz: u32,
    /// TjMax in °C (read once during init).
    tj_max: u8,
    /// Accumulated tick counter for conservative step-rate limiting.
    tick_count: u64,
}

static STATE: Mutex<Option<CpuFreqState>> = Mutex::new(None);

// ── MSR helpers ─────────────────────────────────────────────────────────────

#[inline(always)]
unsafe fn rdmsr(addr: u32) -> u64 {
    crate::cpu::rdmsr(addr)
}

#[inline(always)]
unsafe fn wrmsr(addr: u32, val: u64) {
    crate::cpu::wrmsr(addr, val);
}

// ── Internal helpers ────────────────────────────────────────────────────────

/// Read TjMax from IA32_TEMPERATURE_TARGET bits [23:16].
/// Returns TJMAX_DEFAULT_C if the MSR yields zero (e.g. QEMU).
fn read_tj_max() -> u8 {
    let val = unsafe { rdmsr(IA32_TEMPERATURE_TARGET) };
    let tj = ((val >> 16) & 0xFF) as u8;
    if tj > 0 {
        tj
    } else {
        TJMAX_DEFAULT_C
    }
}

/// Detect max non-turbo ratio and min ratio from MSR_PLATFORM_INFO.
/// Returns (min_ratio, max_ratio).  Falls back to (8, 30) if unreadable.
fn detect_ratio_range() -> (u8, u8) {
    let info = unsafe { rdmsr(MSR_PLATFORM_INFO) };
    let max_r = ((info >> 8) & 0xFF) as u8;
    let min_r = ((info >> 40) & 0xFF) as u8;
    let max_r = if max_r > 0 { max_r } else { 30 }; // fallback 3.0 GHz
    let min_r = if min_r > 0 && min_r < max_r { min_r } else { 8 }; // fallback 800 MHz
    (min_r, max_r)
}

/// Read single-core turbo ratio from IA32_TURBO_RATIO_LIMIT bits [7:0].
fn detect_turbo_ratio(max_ratio: u8) -> u8 {
    let val = unsafe { rdmsr(IA32_TURBO_RATIO_LIMIT) };
    let turbo = (val & 0xFF) as u8;
    if turbo > max_ratio {
        turbo
    } else {
        max_ratio.saturating_add(2)
    }
}

/// Build the P-state table by enumerating every ratio in [min_ratio, max_ratio].
fn build_pstate_table(min_ratio: u8, max_ratio: u8, out: &mut [PState; 32]) -> u32 {
    let mut count: u32 = 0;
    let mut r = max_ratio;
    while r >= min_ratio && count < 32 {
        out[count as usize] = PState {
            ratio: r,
            voltage_mv: 1000,
            active: true,
        };
        count = count.saturating_add(1);
        if r == 0 {
            break;
        }
        r = r.saturating_sub(1);
    }
    count
}

// ── Public hardware accessors ────────────────────────────────────────────────

/// Read current hardware ratio from IA32_PERF_STATUS bits [15:8].
/// Returns the operating frequency in MHz, or 0 on QEMU/uninitialized.
pub fn get_current_freq_mhz() -> u32 {
    let val = unsafe { rdmsr(IA32_PERF_STATUS) };
    let ratio = ((val >> 8) & 0xFF) as u32;
    ratio.saturating_mul(BUS_CLOCK_MHZ as u32)
}

/// Return the maximum non-turbo frequency in MHz.
pub fn get_max_freq_mhz() -> u32 {
    if let Some(ref s) = *STATE.lock() {
        s.max_ratio as u32 * BUS_CLOCK_MHZ as u32
    } else {
        3000
    }
}

/// Return the minimum frequency in MHz.
pub fn get_min_freq_mhz() -> u32 {
    if let Some(ref s) = *STATE.lock() {
        s.min_ratio as u32 * BUS_CLOCK_MHZ as u32
    } else {
        800
    }
}

/// Return the turbo (boost) frequency for the first core in MHz.
pub fn get_turbo_freq_mhz() -> u32 {
    if let Some(ref s) = *STATE.lock() {
        s.turbo_ratio as u32 * BUS_CLOCK_MHZ as u32
    } else {
        3200
    }
}

// ── P-state write ────────────────────────────────────────────────────────────

/// Write target ratio to IA32_PERF_CTL bits [15:8].
pub fn set_pstate(ratio: u8) {
    let ctl: u64 = ((ratio as u64) & 0xFF) << 8;
    unsafe {
        wrmsr(IA32_PERF_CTL, ctl);
    }
}

// ── Governor control ─────────────────────────────────────────────────────────

/// Change the active governor policy.  Immediately applies the new target.
pub fn set_governor(gov: Governor) {
    if let Some(ref mut s) = *STATE.lock() {
        s.governor = gov;
        crate::serial_println!("  cpufreq: governor -> {:?}", gov);
        // Snap target to policy extremes on immediate switch.
        match gov {
            Governor::Performance => {
                s.target_freq_mhz = s.max_ratio as u32 * BUS_CLOCK_MHZ as u32;
            }
            Governor::Powersave => {
                s.target_freq_mhz = s.min_ratio as u32 * BUS_CLOCK_MHZ as u32;
            }
            _ => {}
        }
        let ratio = (s.target_freq_mhz / BUS_CLOCK_MHZ as u32) as u8;
        set_pstate(ratio);
    }
}

/// Return a copy of the current governor.
pub fn get_governor() -> Governor {
    if let Some(ref s) = *STATE.lock() {
        s.governor
    } else {
        Governor::Ondemand
    }
}

// ── Tick / load estimate ─────────────────────────────────────────────────────

/// Called from the timer ISR approximately every 10 ms.
///
/// `elapsed_ticks` is the TSC delta since the last call.  The function
/// updates a simple exponential moving average of CPU load and then applies
/// the active governor to select a new P-state.
///
/// Load estimation:
///   We treat `elapsed_ticks` as "total time."  The idle time delta is
///   approximated from the current C-state statistics (if available) but
///   since we have no OS scheduler here, we use a fixed heuristic:
///   - If the last tick completed in less than 80 % of its budget →
///     assume the CPU was mostly idle (load 20 %).
///   - Otherwise assume full load (100 %).
///   A real kernel would compare idle-thread TSC vs wall-clock TSC.
///
/// The governor then decides the target ratio and writes IA32_PERF_CTL.
pub fn cpufreq_tick(elapsed_ticks: u64) {
    let _ = elapsed_ticks; // used for future load accounting

    if let Some(ref mut s) = *STATE.lock() {
        s.tick_count = s.tick_count.saturating_add(1);

        // ── Load update (simple idle poll) ─────────────────────────────────
        // In absence of a scheduler, query the idle stats from the cpuidle
        // module to derive load.  We snapshot total vs C0 time.
        let idle_stats = crate::power_mgmt::cpuidle::get_idle_stats(0);
        let total = idle_stats.total_time_us;
        let active = idle_stats.c0_time_us;
        let load: u8 = if total > 0 {
            let pct = (active.saturating_mul(100)) / total;
            pct.min(100) as u8
        } else {
            // Fallback: assume 50 % if no stats are available yet.
            50
        };

        // Smooth with a simple IIR: new = (7*old + new) / 8
        let old = s.load_percent as u32;
        let blended = (old.saturating_mul(7).saturating_add(load as u32)) / 8;
        s.load_percent = blended.min(100) as u8;

        let load_pct = s.load_percent;
        let min_mhz = s.min_ratio as u32 * BUS_CLOCK_MHZ as u32;
        let max_mhz = s.max_ratio as u32 * BUS_CLOCK_MHZ as u32;
        let cur_mhz = s.target_freq_mhz;

        // ── Governor policy ─────────────────────────────────────────────────
        let new_mhz: u32 = match s.governor {
            Governor::Performance => max_mhz,

            Governor::Powersave => min_mhz,

            Governor::Ondemand => {
                if load_pct >= 80 {
                    max_mhz
                } else if load_pct <= 20 {
                    min_mhz
                } else {
                    // Linear interpolation between min and max.
                    let range = max_mhz.saturating_sub(min_mhz);
                    min_mhz.saturating_add(
                        (range as u64)
                            .saturating_mul(load_pct as u64)
                            .saturating_div(100) as u32,
                    )
                }
            }

            Governor::Conservative => {
                // Step by one P-state ratio (100 MHz) per tick.
                if load_pct >= 75 {
                    cur_mhz.saturating_add(BUS_CLOCK_MHZ as u32).min(max_mhz)
                } else if load_pct <= 30 {
                    if cur_mhz <= min_mhz.saturating_add(BUS_CLOCK_MHZ as u32) {
                        min_mhz
                    } else {
                        cur_mhz.saturating_sub(BUS_CLOCK_MHZ as u32)
                    }
                } else {
                    cur_mhz // hold
                }
            }

            Governor::Schedutil => {
                // Proportional to utilisation, clamped to [min, max].
                let range = max_mhz.saturating_sub(min_mhz);
                let target = min_mhz.saturating_add(
                    (range as u64)
                        .saturating_mul(load_pct as u64)
                        .saturating_div(100) as u32,
                );
                target.clamp(min_mhz, max_mhz)
            }
        };

        s.target_freq_mhz = new_mhz;
        let ratio = (new_mhz / BUS_CLOCK_MHZ as u32) as u8;
        s.current_ratio = ratio;
        // Release lock before WRMSR to avoid nesting issues.
        drop(s); // reborrow ends here — lock released after `if let` block
    }

    // Write the new ratio outside the lock guard to avoid holding the mutex
    // across MSR writes.
    if let Some(ref s) = *STATE.lock() {
        set_pstate(s.current_ratio);
    }
}

// ── Turbo boost ──────────────────────────────────────────────────────────────

/// Enable Intel Turbo Boost.
///
/// Clears bit 38 (IDA Engage) of IA32_MISC_ENABLE.  When this bit is 0,
/// the hardware is free to boost above the max non-turbo ratio.
pub fn enable_turbo() {
    let val = unsafe { rdmsr(IA32_MISC_ENABLE) };
    if val & IDA_ENGAGE_BIT != 0 {
        unsafe {
            wrmsr(IA32_MISC_ENABLE, val & !IDA_ENGAGE_BIT);
        }
        crate::serial_println!("  cpufreq: turbo enabled (IDA bit cleared)");
    }
}

/// Disable Intel Turbo Boost.
///
/// Sets bit 38 (IDA Engage) of IA32_MISC_ENABLE, preventing the CPU from
/// operating above its max non-turbo ratio.
pub fn disable_turbo() {
    let val = unsafe { rdmsr(IA32_MISC_ENABLE) };
    if val & IDA_ENGAGE_BIT == 0 {
        unsafe {
            wrmsr(IA32_MISC_ENABLE, val | IDA_ENGAGE_BIT);
        }
        crate::serial_println!("  cpufreq: turbo disabled (IDA bit set)");
    }
}

// ── Energy performance bias ──────────────────────────────────────────────────

/// Write the energy/performance bias hint.
///
/// `bias` is in the range 0–15:
///   0  = maximum performance preference
///   15 = maximum power-saving preference
///
/// Writes to IA32_ENERGY_PERF_BIAS MSR (0x1B0).  The value is clamped to
/// [0, 15] before writing.
pub fn set_energy_perf_bias(bias: u8) {
    let clamped = bias.min(15) as u64;
    unsafe {
        wrmsr(IA32_ENERGY_PERF_BIAS, clamped);
    }
    crate::serial_println!("  cpufreq: energy perf bias = {}", clamped);
}

// ── Thermal read ─────────────────────────────────────────────────────────────

/// Return true if the CPU is in a PROCHOT thermal throttle event.
///
/// Reads bit 4 of IA32_THERM_STATUS (0x19C).
pub fn is_throttling() -> bool {
    let val = unsafe { rdmsr(IA32_THERM_STATUS) };
    (val & (1 << 4)) != 0
}

/// Return the current CPU core temperature in degrees Celsius.
///
/// IA32_THERM_STATUS bits [22:16] = digital readout below TjMax.
/// temp_c = TjMax − readout.
///
/// TjMax is read once during `init()` from IA32_TEMPERATURE_TARGET.
/// Defaults to 100 °C if the MSR yields 0 (e.g. in QEMU).
pub fn get_temperature_c() -> u8 {
    let therm = unsafe { rdmsr(IA32_THERM_STATUS) };
    let readout = ((therm >> 16) & 0x7F) as u8;
    let tj_max = if let Some(ref s) = *STATE.lock() {
        s.tj_max
    } else {
        TJMAX_DEFAULT_C
    };
    tj_max.saturating_sub(readout)
}

// ── Thermal subsystem integration ────────────────────────────────────────────

/// Reduce the BSP frequency by `delta_khz` below its current operating point,
/// clamped at the minimum supported frequency.
///
/// Called by the thermal policy engine on a Passive trip-point crossing.
/// `delta_khz` uses the same kHz unit as the legacy cpufreq driver.
pub fn reduce_by(delta_khz: u64) {
    if let Some(ref mut s) = *STATE.lock() {
        let delta_mhz = (delta_khz / 1000) as u32;
        let cur_mhz = s.target_freq_mhz;
        let min_mhz = s.min_ratio as u32 * BUS_CLOCK_MHZ as u32;
        let new_mhz = cur_mhz.saturating_sub(delta_mhz).max(min_mhz);
        s.target_freq_mhz = new_mhz;
        let ratio = (new_mhz / BUS_CLOCK_MHZ as u32) as u8;
        s.current_ratio = ratio;
        crate::serial_println!(
            "  cpufreq: thermal throttle {} MHz -> {} MHz",
            cur_mhz,
            new_mhz
        );
        set_pstate(ratio);
    }
}

/// Restore the BSP frequency to the maximum non-turbo ratio.
///
/// Called by the thermal engine after all trip points have been cleared.
pub fn restore_governor_target() {
    if let Some(ref mut s) = *STATE.lock() {
        let max_mhz = s.max_ratio as u32 * BUS_CLOCK_MHZ as u32;
        s.target_freq_mhz = max_mhz;
        let ratio = s.max_ratio;
        s.current_ratio = ratio;
        crate::serial_println!("  cpufreq: governor restored to {} MHz (BSP)", max_mhz);
        set_pstate(ratio);
    }
}

// ── Report ───────────────────────────────────────────────────────────────────

/// Snapshot of driver state for diagnostics or UI.
pub struct CpuFreqReport {
    pub current_freq_mhz: u32,
    pub max_freq_mhz: u32,
    pub governor: Governor,
    pub load_percent: u8,
    pub temp_c: u8,
    pub throttling: bool,
    pub turbo_active: bool,
}

/// Build a diagnostic report from current driver state.
pub fn report() -> CpuFreqReport {
    let current_freq_mhz = get_current_freq_mhz();
    let throttling = is_throttling();
    let temp_c = get_temperature_c();

    // Turbo is active when IDA Engage bit is clear AND the hardware ratio
    // exceeds the max non-turbo ratio.
    let misc = unsafe { rdmsr(IA32_MISC_ENABLE) };
    let turbo_enabled = (misc & IDA_ENGAGE_BIT) == 0;

    let (max_freq_mhz, governor, load_percent, turbo_active) = if let Some(ref s) = *STATE.lock() {
        let max_mhz = s.max_ratio as u32 * BUS_CLOCK_MHZ as u32;
        let turbo_a = turbo_enabled && (current_freq_mhz > max_mhz);
        (max_mhz, s.governor, s.load_percent, turbo_a)
    } else {
        (3000, Governor::Ondemand, 0, false)
    };

    CpuFreqReport {
        current_freq_mhz,
        max_freq_mhz,
        governor,
        load_percent,
        temp_c,
        throttling,
        turbo_active,
    }
}

// ── Initialisation ───────────────────────────────────────────────────────────

/// Initialise the Intel P-state driver.
///
/// Reads hardware limits from MSRs, builds the P-state table, and sets
/// the default governor (Ondemand).
pub fn init() {
    let (min_ratio, max_ratio) = detect_ratio_range();
    let turbo_ratio = detect_turbo_ratio(max_ratio);
    let tj_max = read_tj_max();

    let mut pstates = [PState {
        ratio: 0,
        voltage_mv: 0,
        active: false,
    }; 32];
    let pstate_count = build_pstate_table(min_ratio, max_ratio, &mut pstates);

    let current_ratio = {
        let val = unsafe { rdmsr(IA32_PERF_STATUS) };
        let r = ((val >> 8) & 0xFF) as u8;
        if r > 0 {
            r
        } else {
            max_ratio
        }
    };

    let target_freq_mhz = max_ratio as u32 * BUS_CLOCK_MHZ as u32;

    let state = CpuFreqState {
        pstates,
        pstate_count,
        current_ratio,
        max_ratio,
        min_ratio,
        turbo_ratio,
        governor: Governor::Ondemand,
        load_percent: 0,
        target_freq_mhz,
        tj_max,
        tick_count: 0,
    };

    crate::serial_println!(
        "  cpufreq: {} P-states, {}–{} MHz, turbo {} MHz, TjMax {}°C, governor Ondemand",
        pstate_count,
        min_ratio as u32 * BUS_CLOCK_MHZ as u32,
        max_ratio as u32 * BUS_CLOCK_MHZ as u32,
        turbo_ratio as u32 * BUS_CLOCK_MHZ as u32,
        tj_max,
    );

    *STATE.lock() = Some(state);

    // Start at maximum non-turbo frequency.
    set_pstate(max_ratio);
}
