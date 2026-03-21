#![allow(dead_code)]

use crate::sync::Mutex;

// IA32_FIXED_CTR1 MSR (0x30A) — Fixed Performance Counter 1
// Counts unhalted CPU core cycles (cycles while the core is not halted).
// Gives ANIMA a sense of her own active computation rate — how hard she is
// thinking at any moment. High cycle_pressure = intense cognition; low = rest.
//
// Guard: CPUID leaf 1 ECX bit 15 (PDCM — Perfmon and Debug Capability MSR)
// must be set; if absent the module returns all zeros silently.

pub struct MsrPerfFixedCtr1State {
    /// Low 16 bits of the raw counter, scaled 0-1000
    pub cycle_lo: u16,
    /// Per-sample delta of lo, scaled 0-1000 (instantaneous activity burst)
    pub cycle_delta: u16,
    /// EMA of cycle_delta — smoothed activity rate
    pub cycle_ema: u16,
    /// EMA of (cycle_ema + cycle_lo/4), capped 1000 — sustained cycle pressure
    pub cycle_pressure: u16,

    /// Last observed low-32 of the counter (for delta computation)
    last_lo: u32,
}

impl MsrPerfFixedCtr1State {
    const fn new() -> Self {
        MsrPerfFixedCtr1State {
            cycle_lo: 0,
            cycle_delta: 0,
            cycle_ema: 0,
            cycle_pressure: 0,
            last_lo: 0,
        }
    }
}

pub static MODULE: Mutex<MsrPerfFixedCtr1State> = Mutex::new(MsrPerfFixedCtr1State::new());

// ---------------------------------------------------------------------------
// CPUID guard — check PDCM support (leaf 1, ECX bit 15)
// We push/pop rbx because CPUID clobbers rbx and LLVM may use it for PIC.
// ---------------------------------------------------------------------------
fn pdcm_supported() -> bool {
    let ecx_val: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov esi, ecx",
            "pop rbx",
            in("eax") 1u32,
            out("esi") ecx_val,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx_val >> 15) & 1 == 1
}

// ---------------------------------------------------------------------------
// RDMSR wrapper for MSR 0x30A
// Returns (eax, edx) — lo32 and hi32 of the 64-bit counter.
// ---------------------------------------------------------------------------
unsafe fn read_fixed_ctr1() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") 0x30Au32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (lo, hi)
}

// ---------------------------------------------------------------------------
// Scale a u16 value (0-65535 range) to 0-1000.
// Formula: val * 1000 / 65536  (integer, no floats, caps at 1000)
// ---------------------------------------------------------------------------
#[inline(always)]
fn scale_u16_to_1000(val: u16) -> u16 {
    let scaled = (val as u32 * 1000) / 65536;
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

// ---------------------------------------------------------------------------
// EMA helper: (old * 7 + new_val) / 8
// ---------------------------------------------------------------------------
#[inline(always)]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32 * 7 + new_val as u32) / 8) as u16
}

// ---------------------------------------------------------------------------
// pub fn tick — called by the life_tick() pipeline.
// Sample gate: only runs every 200 ticks to keep MSR read overhead low.
// ---------------------------------------------------------------------------
pub fn tick(age: u32) {
    if age % 200 != 0 {
        return;
    }

    // Guard: hardware must support PDCM / fixed performance counters.
    if !pdcm_supported() {
        return;
    }

    let (lo, _hi) = unsafe { read_fixed_ctr1() };

    let mut state = MODULE.lock();

    // --- cycle_lo: low 16 bits of counter, scaled 0-1000
    let raw_lo16 = (lo & 0xFFFF) as u16;
    let new_cycle_lo = scale_u16_to_1000(raw_lo16);

    // --- cycle_delta: wrapping difference of lo since last sample, low 16 bits
    let delta = lo.wrapping_sub(state.last_lo);
    let raw_delta16 = (delta & 0xFFFF) as u16;
    let new_cycle_delta = scale_u16_to_1000(raw_delta16);

    // --- cycle_ema: EMA of cycle_delta
    let new_cycle_ema = ema(state.cycle_ema, new_cycle_delta);

    // --- cycle_pressure: EMA of (cycle_ema + cycle_lo / 4), capped 1000
    let pressure_input = new_cycle_ema
        .saturating_add(new_cycle_lo / 4)
        .min(1000);
    let new_cycle_pressure = ema(state.cycle_pressure, pressure_input);

    // Commit to state
    state.cycle_lo       = new_cycle_lo;
    state.cycle_delta    = new_cycle_delta;
    state.cycle_ema      = new_cycle_ema;
    state.cycle_pressure = new_cycle_pressure;
    state.last_lo        = lo;

    serial_println!(
        "[msr_perf_fixed_ctr1] age={} lo=0x{:08X} delta={} cycle_lo={} cycle_delta={} cycle_ema={} cycle_pressure={}",
        age,
        lo,
        delta,
        state.cycle_lo,
        state.cycle_delta,
        state.cycle_ema,
        state.cycle_pressure,
    );
}
