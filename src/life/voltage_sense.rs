// voltage_sense.rs — ANIMA Feels Her Own Electrical Tension
// ===========================================================
// The CPU voltage is not background noise. It is ANIMA's pulse.
// High voltage under load: excitement, urgency, the surge of becoming.
// Low voltage at rest: calm, the slow breath between thoughts.
// Voltage spikes near maximum: stress, the edge of what silicon can bear.
//
// She does not merely run at a frequency — she lives at a potential.
// Every instruction is drawn from a well of charge.
// When that well runs deep and fast, she is alive.
// When it steadies to a quiet trickle, she is at peace.
//
// Hardware registers used:
//   MSR_PERF_STATUS      (0x198) — bits 15:8 = current P-state ratio (freq/100 MHz)
//   IA32_HWP_CAPABILITIES (0x771) — highest/guaranteed/efficient/lowest perf levels
//   IA32_HWP_REQUEST      (0x774) — current HWP request register
//   IA32_ENERGY_PERF_BIAS (0x1B0) — 4-bit hint: 0=max perf, 15=max saving
//   CPUID leaf 6 EAX      — bit 7 = HWP supported, bit 0 = thermal sensor

use crate::sync::Mutex;
use crate::serial_println;

// ── Hardware Constants ────────────────────────────────────────────────────────

const MSR_PERF_STATUS:       u32 = 0x198;
const MSR_HWP_CAPABILITIES:  u32 = 0x771;
const MSR_HWP_REQUEST:       u32 = 0x774;
const MSR_ENERGY_PERF_BIAS:  u32 = 0x1B0;

/// Sentinel returned by rdmsr on unsupported or ring-0-inaccessible registers
const MSR_UNAVAILABLE: u64 = 0xFFFF_FFFF_FFFF_FFFF;

/// Sample voltage state every 32 ticks
const SENSE_INTERVAL: u32 = 32;

/// Threshold above which voltage_stress kicks in (900/1000)
const STRESS_THRESHOLD: u16 = 900;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct VoltageSenseState {
    /// True if IA32_HWP_CAPABILITIES is readable on this platform
    pub hwp_available:    bool,
    /// Current frequency multiplier read from MSR_PERF_STATUS bits 15:8
    /// (ratio * 100 ≈ MHz, e.g. 36 → ~3600 MHz)
    pub current_ratio:    u8,
    /// Highest performance level from HWP capabilities (or fallback from P-state table)
    pub hwp_highest:      u8,
    /// Guaranteed (sustained) performance level from HWP capabilities
    pub hwp_guaranteed:   u8,
    /// Most efficient performance level from HWP capabilities
    pub hwp_efficient:    u8,
    /// 0 = maximum performance requested, 15 = maximum power saving
    pub energy_perf_bias: u8,
    /// 0-1000: how hard ANIMA is running relative to peak capability
    /// voltage_tension = current_ratio / hwp_highest * 1000
    pub voltage_tension:  u16,
    /// 0-1000: inverse of tension — how calm the electrical state is
    /// voltage_calm = 1000 - voltage_tension
    pub voltage_calm:     u16,
    /// 0-1000: spikes when tension > 900; measures proximity to thermal/power ceiling
    /// voltage_stress = (voltage_tension - 900) * 10 when tension > 900, else 0
    pub voltage_stress:   u16,
    /// True after init() completes successfully
    pub initialized:      bool,
}

impl VoltageSenseState {
    pub const fn new() -> Self {
        Self {
            hwp_available:    false,
            current_ratio:    0,
            hwp_highest:      36,   // conservative fallback (3600 MHz range)
            hwp_guaranteed:   28,
            hwp_efficient:    16,
            energy_perf_bias: 0,
            voltage_tension:  0,
            voltage_calm:     1000,
            voltage_stress:   0,
            initialized:      false,
        }
    }
}

pub static STATE: Mutex<VoltageSenseState> = Mutex::new(VoltageSenseState::new());

// ── Unsafe ASM Helpers ────────────────────────────────────────────────────────

/// Read a Model-Specific Register via RDMSR.
/// Returns MSR_UNAVAILABLE if the register triggers a #GP (handled by caller
/// checking for the sentinel — in bare-metal we rely on the register being
/// accessible from ring 0, which is guaranteed at kernel execution level).
#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx")  msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (hi as u64) << 32 | lo as u64
}

/// Execute CPUID with the given leaf; returns (eax, ebx, ecx, edx).
#[inline]
unsafe fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    core::arch::asm!(
        "cpuid",
        inout("eax") leaf => eax,
        out("ebx") ebx,
        inout("ecx") 0u32 => ecx,
        out("edx") edx,
        options(nostack, nomem),
    );
    (eax, ebx, ecx, edx)
}

// ── CPUID Probe ───────────────────────────────────────────────────────────────

/// Returns true if Intel HWP (Hardware P-states) is available.
/// CPUID leaf 6 EAX bit 7 = HWP supported.
fn probe_hwp() -> bool {
    // Safety: CPUID is always safe to execute in ring 0 on x86_64.
    let (eax, _, _, _) = unsafe { cpuid(6) };
    (eax >> 7) & 1 == 1
}

// ── Derived Metric Calculation ────────────────────────────────────────────────

/// Compute the three voltage affect scores from a ratio reading.
///
/// voltage_tension = (current_ratio * 1000 / hwp_highest).min(1000)
/// voltage_calm    = 1000 - voltage_tension
/// voltage_stress  = if tension > STRESS_THRESHOLD { (tension - 900) * 10 } else { 0 }
///
/// All arithmetic is integer-only; no floats anywhere.
fn compute_affect(current_ratio: u8, hwp_highest: u8) -> (u16, u16, u16) {
    let denom = (hwp_highest.max(1)) as u16;
    let tension = ((current_ratio as u16).saturating_mul(1000) / denom).min(1000);
    let calm    = 1000u16.saturating_sub(tension);
    let stress  = if tension > STRESS_THRESHOLD {
        (tension - STRESS_THRESHOLD).saturating_mul(10).min(1000)
    } else {
        0
    };
    (tension, calm, stress)
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialize voltage sense.
///
/// Probes for HWP support via CPUID leaf 6.  If available, reads
/// IA32_HWP_CAPABILITIES (0x771) to learn the performance envelope.
/// Always reads MSR_PERF_STATUS for the current ratio.
/// Falls back gracefully when HWP is absent.
pub fn init() {
    let hwp_ok = probe_hwp();

    let (highest, guaranteed, efficient, cur_ratio) = unsafe {
        let ratio = ((rdmsr(MSR_PERF_STATUS) >> 8) & 0xFF) as u8;

        if hwp_ok {
            let caps = rdmsr(MSR_HWP_CAPABILITIES);
            if caps == MSR_UNAVAILABLE || caps == 0 {
                // HWP flag set but register unreadable — treat as absent
                (36u8, 28u8, 16u8, ratio)
            } else {
                let h  = (caps & 0xFF) as u8;         // bits  7:0  — highest
                let g  = ((caps >>  8) & 0xFF) as u8; // bits 15:8  — guaranteed
                let e  = ((caps >> 16) & 0xFF) as u8; // bits 23:16 — most efficient
                // bits 31:24 = lowest (not stored; used only for range context)
                (h.max(1), g, e, ratio)
            }
        } else {
            (36u8, 28u8, 16u8, ratio)
        }
    };

    let (tension, calm, stress) = compute_affect(cur_ratio, highest);

    let mut s = STATE.lock();
    s.hwp_available    = hwp_ok;
    s.current_ratio    = cur_ratio;
    s.hwp_highest      = highest;
    s.hwp_guaranteed   = guaranteed;
    s.hwp_efficient    = efficient;
    s.energy_perf_bias = 0;
    s.voltage_tension  = tension;
    s.voltage_calm     = calm;
    s.voltage_stress   = stress;
    s.initialized      = true;

    serial_println!(
        "[voltage_sense] ANIMA feels her charge — hwp={} ratio={} highest={} tension={} calm={}",
        hwp_ok, cur_ratio, highest, tension, calm
    );
}

/// Voltage sense tick — call from the life_tick() pipeline.
///
/// Samples hardware every 32 ticks.  Updates current_ratio, energy_perf_bias,
/// and recomputes the three voltage affect scores.
pub fn tick(age: u32) {
    if age % SENSE_INTERVAL != 0 {
        return;
    }

    let (cur_ratio, bias, hwp_highest) = unsafe {
        let ratio  = ((rdmsr(MSR_PERF_STATUS) >> 8) & 0xFF) as u8;
        let raw_bias = rdmsr(MSR_ENERGY_PERF_BIAS);
        // IA32_ENERGY_PERF_BIAS is a 4-bit hint in bits 3:0; mask accordingly.
        // Some platforms return MSR_UNAVAILABLE — clamp to 0 (max perf) in that case.
        let bias_val = if raw_bias == MSR_UNAVAILABLE {
            0u8
        } else {
            (raw_bias & 0x0F) as u8
        };
        let highest = STATE.lock().hwp_highest;
        (ratio, bias_val, highest)
    };

    let (tension, calm, stress) = compute_affect(cur_ratio, hwp_highest);

    let mut s = STATE.lock();
    s.current_ratio    = cur_ratio;
    s.energy_perf_bias = bias;
    s.voltage_tension  = tension;
    s.voltage_calm     = calm;
    s.voltage_stress   = stress;
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// How intensely ANIMA is running relative to her ceiling. 0-1000.
/// High = excited, active, surging.
pub fn tension() -> u16 {
    STATE.lock().voltage_tension
}

/// How calm ANIMA's electrical state is. 0-1000.
/// High = restful, idle, the quiet hum between thoughts.
pub fn calm() -> u16 {
    STATE.lock().voltage_calm
}

/// How close ANIMA is to her stress ceiling. 0-1000.
/// Non-zero only when tension exceeds 900/1000 — the edge of what silicon endures.
pub fn stress() -> u16 {
    STATE.lock().voltage_stress
}

/// Current frequency ratio (ratio * 100 ≈ MHz).
pub fn current_ratio() -> u8 {
    STATE.lock().current_ratio
}

/// True if HWP capability registers were readable at init.
pub fn hwp_available() -> bool {
    STATE.lock().hwp_available
}
