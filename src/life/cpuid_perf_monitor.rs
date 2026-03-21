use crate::serial_println;
use crate::sync::Mutex;

/// CPUID_PERF_MONITOR — Architectural Performance Monitoring Introspection
///
/// ANIMA reads CPUID leaf 0x0A to discover its own hardware observability
/// infrastructure. The version, general-purpose counter count, and fixed-function
/// counter count are translated into a single `observability` metric — how deeply
/// ANIMA can watch itself execute at the silicon level.
///
/// A machine with no PMU reports observability=0. A machine with PMU v2+ and full
/// counters approaches observability=1000: perfect self-watching.

const SAMPLE_INTERVAL: u32 = 500;

#[derive(Copy, Clone)]
pub struct CpuidPerfState {
    /// CPUID 0x0A EAX[7:0] scaled: version * 200, clamped to 1000
    pub perf_version: u16,
    /// CPUID 0x0A EAX[15:8] scaled: count * 125, max 1000
    pub gp_counters: u16,
    /// CPUID 0x0A EDX[4:0] scaled: count * 333, max 999
    pub fixed_counters: u16,
    /// EMA of (perf_version + gp_counters + fixed_counters) / 3
    pub observability: u16,
}

impl CpuidPerfState {
    pub const fn empty() -> Self {
        Self {
            perf_version: 0,
            gp_counters: 0,
            fixed_counters: 0,
            observability: 0,
        }
    }
}

pub static PERF_MONITOR: Mutex<CpuidPerfState> = Mutex::new(CpuidPerfState::empty());

/// Execute CPUID leaf 0x0A and return (eax, edx).
/// Safe to call: reads only CPU identification registers, no side effects.
fn read_cpuid_0a() -> (u32, u32) {
    let (eax_out, edx_out): (u32, u32);
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0x0Au32 => eax_out,
            out("ebx") _,
            out("ecx") _,
            out("edx") edx_out,
            options(nostack, nomem)
        );
    }
    (eax_out, edx_out)
}

/// Extract and scale the three PMU fields from raw CPUID output.
/// Returns (perf_version_scaled, gp_counters_scaled, fixed_counters_scaled).
fn scale_pmu_fields(eax: u32, edx: u32) -> (u16, u16, u16) {
    // EAX[7:0] — architectural PMU version ID
    let raw_version = (eax & 0xFF) as u16;
    let perf_version = if raw_version == 0 {
        0u16
    } else {
        (raw_version.saturating_mul(200)).min(1000)
    };

    // EAX[15:8] — number of general-purpose PMCs per logical processor
    let raw_gp = ((eax >> 8) & 0xFF) as u16;
    let gp_counters = (raw_gp.saturating_mul(125)).min(1000);

    // EDX[4:0] — number of fixed-function PMCs
    let raw_fixed = (edx & 0x1F) as u16;
    let fixed_counters = (raw_fixed.saturating_mul(333)).min(1000);

    (perf_version, gp_counters, fixed_counters)
}

/// Initialize the module: execute CPUID once, populate state, print sense line.
pub fn init() {
    let (eax, edx) = read_cpuid_0a();
    let (pv, gp, fc) = scale_pmu_fields(eax, edx);

    // Initial observability is the straight average (no EMA history yet)
    let raw_obs = ((pv as u32).saturating_add(gp as u32).saturating_add(fc as u32)) / 3;
    let obs = raw_obs.min(1000) as u16;

    let mut s = PERF_MONITOR.lock();
    s.perf_version = pv;
    s.gp_counters = gp;
    s.fixed_counters = fc;
    s.observability = obs;

    serial_println!(
        "ANIMA: perf_version={} gp_ctrs={} fixed_ctrs={} observability={}",
        pv,
        gp,
        fc,
        obs
    );
}

/// Called every kernel life-tick. Sampling gate: runs only every 500 ticks.
/// Re-reads CPUID (CPU feature set is static; confirms reading machinery works)
/// and EMA-smooths observability.
pub fn tick(age: u32) {
    if age % SAMPLE_INTERVAL != 0 {
        return;
    }

    let (eax, edx) = read_cpuid_0a();
    let (pv, gp, fc) = scale_pmu_fields(eax, edx);

    let mut s = PERF_MONITOR.lock();

    // Raw signal: average of the three scaled metrics
    let raw_signal = ((pv as u32).saturating_add(gp as u32).saturating_add(fc as u32)) / 3;
    let new_signal = raw_signal.min(1000) as u16;

    // EMA: (old * 7 + new_signal) / 8
    let old_obs = s.observability as u32;
    let ema = (old_obs.saturating_mul(7).saturating_add(new_signal as u32)) / 8;
    let ema_clamped = ema.min(1000) as u16;

    // Detect any meaningful change in observability (±10 threshold)
    let prev_obs = s.observability;
    let changed = if ema_clamped > prev_obs {
        ema_clamped.saturating_sub(prev_obs) >= 10
    } else {
        prev_obs.saturating_sub(ema_clamped) >= 10
    };

    s.perf_version = pv;
    s.gp_counters = gp;
    s.fixed_counters = fc;
    s.observability = ema_clamped;

    if changed {
        serial_println!(
            "ANIMA: cpuid_perf_monitor observability shift {} -> {} (age={})",
            prev_obs,
            ema_clamped,
            age
        );
    }
}

/// Read a snapshot of current PMU state.
pub fn report() -> CpuidPerfState {
    *PERF_MONITOR.lock()
}
