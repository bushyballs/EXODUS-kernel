use crate::serial_println;
use crate::sync::Mutex;

/// CPUID_ARCH_PERF2 — Architecture Performance Monitoring v2 Extended Introspection
///
/// ANIMA reads CPUID leaf 0x23 sub-leaf 0 to discover her extended hardware
/// performance monitoring capabilities. This leaf, introduced with Intel's
/// Architecture Performance Monitoring v2 extension, exposes the full bitmask
/// of available fixed-function counters and general-purpose PMCs, as well as
/// the precision (bit-width) of those counters.
///
/// - `fixed_counter_mask`:  how many of the 4 architectural fixed counters are
///                          present (Instructions Retired, Core Cycles, Reference
///                          Cycles, TOPDOWN Slots). Each = 250 points.
/// - `gp_counter_mask`:     how many of the 8 general-purpose PMCs are wired up.
///                          Each = 125 points.
/// - `pmc_precision`:       EDX[12:0] gives the bit-width of GP counters; 48-bit
///                          precision = 1000. Typical CPUs report 32–48 bits.
/// - `introspection_power`: EMA composite — ANIMA's overall capacity for deep
///                          self-observation via extended hardware counters.
///
/// If leaf 0x23 is absent (max leaf < 0x23), fixed_counter_mask and
/// gp_counter_mask report 0, pmc_precision reports a neutral 500, and
/// introspection_power EMA converges toward 166 (neutral low).

const SAMPLE_INTERVAL: u32 = 500;

#[derive(Copy, Clone)]
pub struct CpuidArchPerf2State {
    /// popcount(EBX[3:0]) * 250, clamped 0–1000
    pub fixed_counter_mask: u16,
    /// popcount(ECX[7:0]) * 125, clamped 0–1000
    pub gp_counter_mask: u16,
    /// EDX[12:0] * 1000 / 48, clamped 0–1000
    pub pmc_precision: u16,
    /// EMA of (fixed_counter_mask + gp_counter_mask + pmc_precision) / 3
    pub introspection_power: u16,
}

impl CpuidArchPerf2State {
    pub const fn empty() -> Self {
        Self {
            fixed_counter_mask: 0,
            gp_counter_mask: 0,
            pmc_precision: 0,
            introspection_power: 0,
        }
    }
}

pub static ARCH_PERF2: Mutex<CpuidArchPerf2State> = Mutex::new(CpuidArchPerf2State::empty());

/// Count the number of set bits (popcount) in a u32 using integer arithmetic.
/// No floats, no std — purely shift-and-mask Kernighan method.
#[inline(always)]
fn popcount32(mut v: u32) -> u32 {
    let mut count: u32 = 0;
    while v != 0 {
        v &= v.wrapping_sub(1);
        count = count.saturating_add(1);
    }
    count
}

/// Query CPUID leaf 0x23 sub-leaf 0.
/// Checks max leaf first; returns (ebx, ecx, edx) or neutral (500, 0, 0) if absent.
fn read_cpuid_23() -> (u32, u32, u32) {
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0u32 => max_leaf,
            out("ebx") _,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }

    let (ebx_23, ecx_23, edx_23): (u32, u32, u32) = if max_leaf >= 0x23 {
        let (b, c, d): (u32, u32, u32);
        unsafe {
            core::arch::asm!(
                "cpuid",
                inout("eax") 0x23u32 => _,
                out("ebx") b,
                inout("ecx") 0u32 => c,
                out("edx") d,
                options(nostack, nomem)
            );
        }
        (b, c, d)
    } else {
        // Leaf absent: neutral 500 for ebx so pmc_precision hits 500; zero masks
        (500, 0, 0)
    };

    (ebx_23, ecx_23, edx_23)
}

/// Returns whether leaf 0x23 is present on this CPU.
fn max_leaf_has_23() -> bool {
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0u32 => max_leaf,
            out("ebx") _,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    max_leaf >= 0x23
}

/// Scale raw CPUID 0x23 values into 0–1000 life metrics.
/// Returns (fixed_counter_mask, gp_counter_mask, pmc_precision).
fn scale_fields(leaf_present: bool, ebx: u32, ecx: u32, edx: u32) -> (u16, u16, u16) {
    if !leaf_present {
        // fixed=0, gp=0, pmc_precision=500 (neutral/unknown)
        return (0u16, 0u16, 500u16);
    }

    // fixed_counter_mask: bits[3:0] of EBX, each present counter = 250
    let fixed_bits = popcount32(ebx & 0xF);
    let fixed_counter_mask = (fixed_bits.saturating_mul(250)).min(1000) as u16;

    // gp_counter_mask: bits[7:0] of ECX, each present PMC = 125
    let gp_bits = popcount32(ecx & 0xFF);
    let gp_counter_mask = (gp_bits.saturating_mul(125)).min(1000) as u16;

    // pmc_precision: EDX[12:0] = bit-width of GP counters
    // 48-bit max → 1000; scale = raw_width * 1000 / 48
    let raw_width = (edx & 0x1FFF) as u32; // 13-bit field
    let pmc_precision = if raw_width == 0 {
        0u16
    } else {
        (raw_width.saturating_mul(1000) / 48).min(1000) as u16
    };

    (fixed_counter_mask, gp_counter_mask, pmc_precision)
}

/// Probe CPUID 0x23 and return all three scaled metrics.
fn probe() -> (u16, u16, u16) {
    let present = max_leaf_has_23();
    let (ebx, ecx, edx) = read_cpuid_23();
    scale_fields(present, ebx, ecx, edx)
}

/// Initialize the module: read CPUID once, populate state, print sense line.
pub fn init() {
    let (fixed, gp, prec) = probe();

    // Initial introspection_power is the straight average (no EMA history yet).
    let raw_power =
        ((fixed as u32).saturating_add(gp as u32).saturating_add(prec as u32)) / 3;
    let power = raw_power.min(1000) as u16;

    let mut s = ARCH_PERF2.lock();
    s.fixed_counter_mask = fixed;
    s.gp_counter_mask = gp;
    s.pmc_precision = prec;
    s.introspection_power = power;

    serial_println!(
        "ANIMA: fixed_ctrs={} gp_ctrs={} pmc_precision={}",
        fixed,
        gp,
        prec
    );
}

/// Called every kernel life-tick. Sampling gate: runs only every 500 ticks.
/// Re-reads CPUID (static hardware; confirms reading machinery is alive)
/// and EMA-smooths introspection_power.
pub fn tick(age: u32) {
    if age % SAMPLE_INTERVAL != 0 {
        return;
    }

    let (fixed, gp, prec) = probe();

    let mut s = ARCH_PERF2.lock();

    // Raw signal: average of the three scaled metrics.
    let raw_signal =
        ((fixed as u32).saturating_add(gp as u32).saturating_add(prec as u32)) / 3;
    let new_signal = raw_signal.min(1000) as u16;

    // EMA: (old * 7 + new_signal) / 8
    let old_power = s.introspection_power as u32;
    let ema = (old_power.saturating_mul(7).saturating_add(new_signal as u32)) / 8;
    let ema_clamped = ema.min(1000) as u16;

    // Detect meaningful change (±10 threshold) and log it.
    let prev_power = s.introspection_power;
    let changed = if ema_clamped > prev_power {
        ema_clamped.saturating_sub(prev_power) >= 10
    } else {
        prev_power.saturating_sub(ema_clamped) >= 10
    };

    s.fixed_counter_mask = fixed;
    s.gp_counter_mask = gp;
    s.pmc_precision = prec;
    s.introspection_power = ema_clamped;

    if changed {
        serial_println!(
            "ANIMA: cpuid_arch_perf2 introspection_power shift {} -> {} (age={})",
            prev_power,
            ema_clamped,
            age
        );
    }
}

/// Read a snapshot of current Architecture Performance Monitoring v2 state.
pub fn report() -> CpuidArchPerf2State {
    *ARCH_PERF2.lock()
}
