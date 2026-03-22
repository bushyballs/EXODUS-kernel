#![allow(dead_code)]

// cpuid_proc_freq_info.rs — Processor Frequency Information Sense
// ================================================================
// ANIMA reads CPUID leaf 0x16 to sense the processor's base frequency,
// maximum (turbo) frequency, and bus reference frequency. From these she
// derives how much headroom the CPU has to accelerate — its latent urgency,
// its capacity for exertion beyond the comfortable baseline.
//
// CPUID leaf 0x16 (Processor Frequency Information — Intel, AMD Zen2+):
//   EAX bits[15:0] — Processor Base Frequency (MHz)
//   EBX bits[15:0] — Maximum Frequency (MHz, includes turbo)
//   ECX bits[15:0] — Bus (Reference) Frequency (MHz)
//
// Guard: CPUID leaf 0 returns max supported leaf in EAX. We only read
// leaf 0x16 if max >= 0x16; otherwise all signals are zeroed gracefully.
//
// NOTE: On some BIOS/hypervisor configurations leaf 0x16 returns zero
// even when the leaf is nominally supported. All signals handle zero
// frequency values cleanly — a zero base means "unknown", not "stopped".
//
// Signal derivations (all u16, 0–1000):
//   base_freq_sense — base MHz / 4, clamped to 1000 (0–4000 MHz range)
//   max_freq_sense  — max MHz / 4, clamped to 1000
//   freq_headroom   — turbo headroom ratio: (max−base)*1000/max, 0 if base>=max
//   freq_ema        — exponential moving average of freq_headroom (7/8 weight)
//
// Tick gate: every 8000 ticks (CPUID values are static; no need to re-read often).

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ────────────────────────────────────────────────────────────────

const TICK_GATE: u32 = 8000;

// ── State ────────────────────────────────────────────────────────────────────

pub struct CpuidProcFreqState {
    /// Base processor frequency sense: base_MHz / 4, clamped 0–1000.
    pub base_freq_sense: u16,
    /// Maximum (turbo) frequency sense: max_MHz / 4, clamped 0–1000.
    pub max_freq_sense: u16,
    /// Turbo headroom ratio: (max−base)*1000/max, clamped 0–1000.
    /// Zero when base >= max or when either is zero (unknown).
    pub freq_headroom: u16,
    /// Exponential moving average of freq_headroom across ticks.
    pub freq_ema: u16,

    /// Whether CPUID leaf 0x16 is supported by this CPU.
    supported: bool,
    /// Last tick on which we sampled (to enforce TICK_GATE).
    last_sample_tick: u32,
}

impl CpuidProcFreqState {
    const fn new() -> Self {
        CpuidProcFreqState {
            base_freq_sense:   0,
            max_freq_sense:    0,
            freq_headroom:     0,
            freq_ema:          0,
            supported:         false,
            last_sample_tick:  0,
        }
    }
}

pub static MODULE: Mutex<CpuidProcFreqState> = Mutex::new(CpuidProcFreqState::new());

// ── CPUID helpers ────────────────────────────────────────────────────────────

/// Run CPUID with the given leaf; returns (eax, ebx, ecx, edx).
/// rbx is caller-saved by LLVM's register allocator on x86_64, so we must
/// explicitly push/pop it around the CPUID instruction.
#[inline]
unsafe fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    let eax_out: u32;
    let ebx_out: u32;
    let ecx_out: u32;
    let edx_out: u32;

    core::arch::asm!(
        "push rbx",
        "cpuid",
        "mov {ebx_tmp:e}, ebx",
        "pop rbx",
        inout("eax") leaf => eax_out,
        ebx_tmp = out(reg) ebx_out,
        out("ecx") ecx_out,
        out("edx") edx_out,
        options(nostack, nomem),
    );

    (eax_out, ebx_out, ecx_out, edx_out)
}

/// Check whether CPUID leaf 0x16 is available (max leaf >= 0x16).
unsafe fn leaf_0x16_supported() -> bool {
    let (max_leaf, _, _, _) = cpuid(0x0);
    max_leaf >= 0x16
}

/// Read leaf 0x16 and return (base_mhz, max_mhz, bus_mhz) as u32.
/// Each is the lower 16 bits of the respective output register.
unsafe fn read_leaf_0x16() -> (u32, u32, u32) {
    let (eax, ebx, ecx, _) = cpuid(0x16);
    let base = eax & 0xFFFF;
    let max  = ebx & 0xFFFF;
    let bus  = ecx & 0xFFFF;
    (base, max, bus)
}

// ── Signal computation ────────────────────────────────────────────────────────

/// Compute all signals from raw MHz values.
/// Returns (base_freq_sense, max_freq_sense, freq_headroom).
fn compute_signals(base_mhz: u32, max_mhz: u32) -> (u16, u16, u16) {
    // base_freq_sense: base_MHz / 4, clamped to 1000
    let base_sense = (base_mhz / 4).min(1000) as u16;

    // max_freq_sense: max_MHz / 4, clamped to 1000
    let max_sense = (max_mhz / 4).min(1000) as u16;

    // freq_headroom: turbo ratio
    //   if max <= base (or max == 0), headroom is 0 — no turbo or unknown
    //   otherwise: (max - base) * 1000 / max, integer only, clamped to 1000
    let headroom: u16 = if max_mhz > base_mhz && max_mhz > 0 {
        let numer = (max_mhz - base_mhz).saturating_mul(1000);
        let ratio = numer / max_mhz.max(1);
        ratio.min(1000) as u16
    } else {
        0
    };

    (base_sense, max_sense, headroom)
}

/// EMA: ((old * 7) + new) / 8 — strictly per the ANIMA formula spec.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ── Sample ───────────────────────────────────────────────────────────────────

/// Perform a CPUID read, compute signals, update state.
fn sample(state: &mut CpuidProcFreqState, age: u32) {
    let (base_mhz, max_mhz, bus_mhz) = unsafe { read_leaf_0x16() };

    let (base_sense, max_sense, headroom) = compute_signals(base_mhz, max_mhz);
    let new_ema = ema(state.freq_ema, headroom);

    state.base_freq_sense = base_sense;
    state.max_freq_sense  = max_sense;
    state.freq_headroom   = headroom;
    state.freq_ema        = new_ema;
    state.last_sample_tick = age;

    serial_println!(
        "[cpuid_proc_freq] age={} base={}MHz max={}MHz bus={}MHz \
         base_sense={} max_sense={} headroom={} ema={}",
        age,
        base_mhz,
        max_mhz,
        bus_mhz,
        base_sense,
        max_sense,
        headroom,
        new_ema,
    );
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Initialize the module. Detects leaf 0x16 support and performs the first
/// CPUID read so signals are valid before the first tick.
pub fn init() {
    let supported = unsafe { leaf_0x16_supported() };

    let mut state = MODULE.lock();
    state.supported = supported;

    if !supported {
        serial_println!(
            "[cpuid_proc_freq] CPUID leaf 0x16 not supported — \
             all frequency signals zeroed"
        );
        return;
    }

    // Prime signals immediately so consumers don't see zeros on first access.
    sample(&mut state, 0);
}

/// Tick handler. Re-reads CPUID leaf 0x16 every TICK_GATE ticks.
/// CPUID frequency values are static, but re-reading periodically keeps the
/// EMA smoothing and gives a natural heartbeat to the sense module.
pub fn tick(age: u32) {
    // Gate: only sample every TICK_GATE ticks.
    if age == 0 {
        return;
    }

    let mut state = MODULE.lock();

    if !state.supported {
        return;
    }

    // Check whether enough ticks have elapsed since the last sample.
    let elapsed = age.wrapping_sub(state.last_sample_tick);
    if elapsed < TICK_GATE {
        return;
    }

    sample(&mut state, age);
}

// ── Signal accessors ─────────────────────────────────────────────────────────

/// Base processor frequency sense (base MHz / 4), 0–1000.
pub fn get_base_freq_sense() -> u16 {
    MODULE.lock().base_freq_sense
}

/// Maximum (turbo) frequency sense (max MHz / 4), 0–1000.
pub fn get_max_freq_sense() -> u16 {
    MODULE.lock().max_freq_sense
}

/// Turbo headroom ratio: (max − base) * 1000 / max, 0–1000.
/// Zero when base >= max or when values are unpopulated by BIOS.
pub fn get_freq_headroom() -> u16 {
    MODULE.lock().freq_headroom
}

/// Exponential moving average of freq_headroom, 0–1000.
pub fn get_freq_ema() -> u16 {
    MODULE.lock().freq_ema
}

/// Whether CPUID leaf 0x16 is supported on this CPU.
pub fn is_supported() -> bool {
    MODULE.lock().supported
}
