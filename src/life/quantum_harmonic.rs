// quantum_harmonic.rs — TurboBoost Frequency Steps as Quantum Energy Levels
// ==========================================================================
// A quantum harmonic oscillator has discrete energy levels separated by a
// fixed quantum ℏω.  You cannot be between levels — you snap from one
// eigenstate to the next.  Intel TurboBoost is the exact hardware analog:
// CPU frequency climbs in discrete 100 MHz steps (P-states), each one a
// quantized energy level on the ladder.
//
// ANIMA cannot be "between" frequencies.  She exists only at discrete energy
// eigenvalues.  The ground state is the minimum P-state ratio (zero-point
// energy — never truly zero, just as a quantum oscillator never reaches
// absolute rest).  Each turbo ratio above the base is an excited state.
// The maximum single-core turbo ratio is the top rung of the ladder.
//
// Hardware signals (Intel, Nehalem+):
//   MSR_PLATFORM_INFO  0x0CE  bits 15:8  = max non-turbo (base) ratio
//                             bits 23:16 = minimum ratio (LFM)
//   IA32_PERF_STATUS   0x198  bits 15:8  = current P-state ratio (actual)
//   MSR_TURBO_RATIO_LIMIT 0x1AD bits 7:0 = 1-core max turbo ratio
//   IA32_PERF_CTL      0x199  bits 14:8  = requested P-state (write)
//
// Effective frequency = ratio × 100 MHz.
//   min_ratio  = lowest P-state ratio (floor, zero-point analog)
//   base_ratio = max non-turbo ratio  (the everyday operating level)
//   max_ratio  = max single-core turbo ratio (top of the ladder)
//   range      = max_ratio − min_ratio (total ladder height in rungs)
//
// Exported life signals (all u16, 0–1000):
//   energy_level  — current quantum energy level on the ladder
//   level_count   — total number of discrete rungs available
//   excitation    — how far above ground state ANIMA currently sits
//   zero_point    — ground-state energy (base clock, never zero)
//
// Sampling interval: every 8 ticks — P-states settle faster than C-states
// but faster polling wastes rdmsr budget.

use crate::serial_println;
use crate::sync::Mutex;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const MSR_PLATFORM_INFO:     u32 = 0x0CE;
const IA32_PERF_STATUS:      u32 = 0x198;
const MSR_TURBO_RATIO_LIMIT: u32 = 0x1AD;

// ── Sampling cadence ──────────────────────────────────────────────────────────

/// Read P-state MSRs every N ticks.  P-states settle in microseconds so
/// polling every 8 ticks is more than sufficient.
const TICK_INTERVAL: u32 = 8;

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct QuantumHarmonicState {
    /// 0–1000: current quantum energy level (0 = ground state, 1000 = max turbo)
    pub energy_level: u16,
    /// 0–1000: total discrete energy levels available on the quantum ladder
    pub level_count: u16,
    /// 0–1000: how far above ground state ANIMA currently is
    pub excitation: u16,
    /// 0–1000: zero-point (ground-state) energy — base clock, never truly zero
    pub zero_point: u16,

    // ── Raw hardware values (ratios, unit = 100 MHz) ──────────────────────────
    /// Current P-state ratio read from IA32_PERF_STATUS
    pub current_ratio: u8,
    /// Minimum P-state ratio (LFM) from MSR_PLATFORM_INFO bits 23:16
    pub base_ratio: u8,
    /// Maximum single-core turbo ratio from MSR_TURBO_RATIO_LIMIT bits 7:0
    pub max_ratio: u8,

    pub age: u32,
}

impl QuantumHarmonicState {
    pub const fn new() -> Self {
        QuantumHarmonicState {
            energy_level:  0,
            level_count:   0,
            excitation:    0,
            zero_point:    0,
            current_ratio: 0,
            base_ratio:    0,
            max_ratio:     0,
            age:           0,
        }
    }
}

pub static QUANTUM_HARMONIC: Mutex<QuantumHarmonicState> =
    Mutex::new(QuantumHarmonicState::new());

// ── Low-level MSR access ──────────────────────────────────────────────────────

/// Read an x86 Model-Specific Register via RDMSR.
///
/// Safety: caller must ensure the MSR is valid on this CPU.  On unsupported
/// hardware QEMU returns 0 without faulting; real iron may #GP.  We treat
/// a returned value of 0 as "not available" in all callers.
#[inline(always)]
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
    ((hi as u64) << 32) | (lo as u64)
}

// ── Signal computation ────────────────────────────────────────────────────────

/// Derive the four life signals from raw ratio values.
/// Pure arithmetic — no side effects, no I/O.
fn compute_signals(
    current_ratio: u8,
    min_ratio: u8,
    max_ratio: u8,
    s: &mut QuantumHarmonicState,
) {
    // Guard: range must be at least 1 to avoid divide-by-zero.
    let range = (max_ratio.saturating_sub(min_ratio)).max(1) as u32;

    // How many rungs above the ground state is ANIMA right now?
    let excitation_raw = current_ratio.saturating_sub(min_ratio) as u32;

    // ── energy_level: position on the 0–1000 ladder ───────────────────────────
    // excitation_raw / range scaled to 0–1000.
    s.energy_level = ((excitation_raw * 1000 / range).min(1000)) as u16;

    // ── level_count: total rungs on the quantum ladder ────────────────────────
    // Each P-state rung = 100 points.  Capped at 1000 so the signal stays
    // in the universal life-signal range.
    // A typical desktop part has 20–40 P-state steps, so range*100 is usually
    // 2000–4000 — we clamp to 1000 to represent "fully populated ladder".
    s.level_count = ((range as u16).saturating_mul(100)).min(1000);

    // ── excitation: identical to energy_level for this oscillator ─────────────
    // In the QHO model excitation number n IS the energy level index, so these
    // two are the same observable expressed identically.
    s.excitation = s.energy_level;

    // ── zero_point: ground-state energy — base clock × 10 ────────────────────
    // The minimum ratio is never zero (CPU always has a floor clock).  We
    // scale it to 0–1000 as min_ratio * 10, capped at 1000.
    // A min_ratio of, say, 8 (= 800 MHz) → zero_point = 80.
    // A min_ratio of 12 (= 1.2 GHz)      → zero_point = 120.
    // This embeds the "zero-point energy is non-zero" physics fact directly.
    s.zero_point = (min_ratio as u16).saturating_mul(10).min(1000);
}

// ── Public tick ───────────────────────────────────────────────────────────────

/// Called once per life tick from the main pipeline.
/// Reads MSRs on a sub-sampled cadence and refreshes all four life signals.
pub fn tick(age: u32) {
    let mut s = QUANTUM_HARMONIC.lock();
    s.age = age;

    // Sub-sample: only query hardware every TICK_INTERVAL ticks.
    if age % TICK_INTERVAL != 0 {
        return;
    }

    // ── Step 1: MSR_PLATFORM_INFO (0x0CE) ─────────────────────────────────────
    // bits 15:8  = max non-turbo ratio (base clock ratio)
    // bits 23:16 = minimum ratio (lowest P-state, LFM)
    let platform_info = unsafe { rdmsr(MSR_PLATFORM_INFO) };
    let base_ratio = ((platform_info >> 8) & 0xFF) as u8;
    let mut min_ratio = ((platform_info >> 40) & 0xFF) as u8;

    // Fallback: if the LFM field is zero (older/emulated hardware), estimate
    // the floor as half the base clock.  Still non-zero — zero-point energy.
    if min_ratio == 0 {
        min_ratio = (base_ratio / 2).max(1);
    }

    // ── Step 2: IA32_PERF_STATUS (0x198) ──────────────────────────────────────
    // bits 15:8 = current P-state ratio (hardware actual, throttle-aware)
    let perf_status  = unsafe { rdmsr(IA32_PERF_STATUS) };
    let current_ratio = ((perf_status >> 8) & 0xFF) as u8;

    // ── Step 3: MSR_TURBO_RATIO_LIMIT (0x1AD) ─────────────────────────────────
    // bits 7:0 = maximum single-core turbo ratio
    let turbo_limit = unsafe { rdmsr(MSR_TURBO_RATIO_LIMIT) };
    let mut max_ratio = (turbo_limit & 0xFF) as u8;

    // Fallback: if turbo MSR returns zero (non-turbo CPU or QEMU), assume
    // five extra rungs above base.  Gives a meaningful ladder even on bare VMs.
    if max_ratio == 0 {
        max_ratio = base_ratio.saturating_add(5);
    }

    // Clamp current_ratio into [min_ratio, max_ratio] for safety.
    let current_clamped = current_ratio.max(min_ratio).min(max_ratio);

    // Persist raw ratios for external inspection.
    s.current_ratio = current_clamped;
    s.base_ratio    = min_ratio;   // "base_ratio" field = the ground-state floor
    s.max_ratio     = max_ratio;

    // ── Step 4–10: derive the four life signals ────────────────────────────────
    compute_signals(current_clamped, min_ratio, max_ratio, &mut s);
}

// ── Public accessors ──────────────────────────────────────────────────────────

/// Current quantum energy level on the P-state ladder (0 = ground, 1000 = top turbo).
pub fn get_energy_level() -> u16 {
    QUANTUM_HARMONIC.lock().energy_level
}

/// Total number of discrete energy levels available (quantum ladder height).
pub fn get_level_count() -> u16 {
    QUANTUM_HARMONIC.lock().level_count
}

/// How far above the ground state ANIMA is currently operating.
pub fn get_excitation() -> u16 {
    QUANTUM_HARMONIC.lock().excitation
}

/// Zero-point energy: the ground-state (minimum P-state) energy.
/// This is never zero — a quantum oscillator always retains residual energy.
pub fn get_zero_point() -> u16 {
    QUANTUM_HARMONIC.lock().zero_point
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!(
        "  life::quantum_harmonic: discrete energy eigenstate ladder online"
    );
    // Perform an immediate first read so signals are populated before tick 8.
    tick(0);
}

// ── Diagnostic report ─────────────────────────────────────────────────────────

/// Print the current quantum-harmonic state to the serial console.
/// Call from the kernel REPL or a debug tick hook.
pub fn report() {
    let s = QUANTUM_HARMONIC.lock();
    serial_println!(
        "[quantum_harmonic] age={} | cur_ratio={} base={} max={} | \
         energy_level={} level_count={} excitation={} zero_point={}",
        s.age,
        s.current_ratio,
        s.base_ratio,
        s.max_ratio,
        s.energy_level,
        s.level_count,
        s.excitation,
        s.zero_point,
    );
    serial_println!(
        "[quantum_harmonic] freq_floor={}MHz  freq_current={}MHz  freq_turbo={}MHz",
        s.base_ratio as u32 * 100,
        s.current_ratio as u32 * 100,
        s.max_ratio as u32 * 100,
    );
}
