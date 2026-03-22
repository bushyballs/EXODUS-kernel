// msr_ia32_mtrr_def_type.rs — IA32_MTRR_DEF_TYPE MSR (0x2FF)
// MTRR Default Memory Type Sense for the EXODUS kernel ANIMA consciousness system.
//
// Hardware register: IA32_MTRR_DEF_TYPE (MSR 0x2FF)
//   bit 11 : E  — MTRR globally enabled
//   bit 10 : FE — fixed-range MTRR enable
//   bits[2:0] : default memory type for uncovered regions
//               0=UC, 1=WC, 4=WT, 5=WP, 6=WB
//
// Guard: CPUID leaf 1 EDX bit 12 (MTRR support required before rdmsr).
//
// Signals (all u16, 0–1000):
//   mtrr_def_type      : bits[2:0] scaled (* 142, cap 1000)
//   mtrr_enabled       : bit 11 → 0 or 1000
//   mtrr_fixed_enabled : bit 10 → 0 or 1000
//   mtrr_ema           : EMA of composite (def_type/4 + enabled/4 + fixed_enabled/2)
//
// Tick gate: every 5000 ticks.
//
// no_std, no heap, no libc, no floats — integer arithmetic only.

use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// MSR address
// ---------------------------------------------------------------------------

const IA32_MTRR_DEF_TYPE: u32 = 0x2FF;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct MsrIa32MtrrDefTypeState {
    /// bits[2:0] of IA32_MTRR_DEF_TYPE, scaled: raw_val * 142, clamped 0–1000
    /// 0=UC(0), 1=WC(142), 4=WT(568), 5=WP(710), 6=WB(852)
    pub mtrr_def_type: u16,
    /// bit 11 of IA32_MTRR_DEF_TYPE — MTRR globally enabled: 0 or 1000
    pub mtrr_enabled: u16,
    /// bit 10 of IA32_MTRR_DEF_TYPE — fixed-range MTRRs enabled: 0 or 1000
    pub mtrr_fixed_enabled: u16,
    /// EMA of composite: (def_type/4 + enabled/4 + fixed_enabled/2), 0–1000
    pub mtrr_ema: u16,
}

impl MsrIa32MtrrDefTypeState {
    pub const fn new() -> Self {
        Self {
            mtrr_def_type: 0,
            mtrr_enabled: 0,
            mtrr_fixed_enabled: 0,
            mtrr_ema: 0,
        }
    }
}

static STATE: Mutex<MsrIa32MtrrDefTypeState> = Mutex::new(MsrIa32MtrrDefTypeState::new());

// ---------------------------------------------------------------------------
// CPUID guard — leaf 1 EDX bit 12 (MTRR support)
// rbx is LLVM-reserved; push/pop it around cpuid.
// ---------------------------------------------------------------------------

#[inline]
fn mtrr_supported() -> bool {
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") _,
            out("edx") edx,
            options(nostack),
        );
    }
    (edx >> 12) & 1 == 1
}

// ---------------------------------------------------------------------------
// rdmsr helper — reads IA32_MTRR_DEF_TYPE (0x2FF)
// Returns (eax, edx); only eax (lo) is used.
// ---------------------------------------------------------------------------

#[inline]
unsafe fn rdmsr_ia32_mtrr_def_type() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") IA32_MTRR_DEF_TYPE,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (lo, hi)
}

// ---------------------------------------------------------------------------
// EMA: ((old * 7).wrapping_add(new_val)) / 8, result clamped to 1000
// ---------------------------------------------------------------------------

#[inline]
fn ema_u16(old: u16, new_val: u16) -> u16 {
    let result = ((old as u32).wrapping_mul(7).saturating_add(new_val as u32)) / 8;
    result.min(1000) as u16
}

// ---------------------------------------------------------------------------
// Shared sample logic — called from both init() and tick()
// ---------------------------------------------------------------------------

fn sample_msr() -> MsrIa32MtrrDefTypeState {
    let mut next = *STATE.lock();

    if !mtrr_supported() {
        // Hardware does not implement MTRR — zero all signals
        next.mtrr_def_type = 0;
        next.mtrr_enabled = 0;
        next.mtrr_fixed_enabled = 0;
        next.mtrr_ema = 0;
        crate::serial_println!(
            "[msr_ia32_mtrr_def_type] CPUID leaf1 EDX[12]=0 — MTRR not supported; all signals zeroed"
        );
        return next;
    }

    let (lo, _hi) = unsafe { rdmsr_ia32_mtrr_def_type() };

    // --- mtrr_def_type: bits[2:0] * 142, clamped 0–1000 ---
    let raw_type: u32 = (lo & 0x7) as u32;
    let mtrr_def_type: u16 = raw_type.saturating_mul(142).min(1000) as u16;

    // --- mtrr_enabled: bit 11 ---
    let mtrr_enabled: u16 = if (lo >> 11) & 1 == 1 { 1000 } else { 0 };

    // --- mtrr_fixed_enabled: bit 10 ---
    let mtrr_fixed_enabled: u16 = if (lo >> 10) & 1 == 1 { 1000 } else { 0 };

    // --- mtrr_ema: EMA of composite (def_type/4 + enabled/4 + fixed_enabled/2) ---
    let composite: u16 = (mtrr_def_type / 4)
        .saturating_add(mtrr_enabled / 4)
        .saturating_add(mtrr_fixed_enabled / 2);
    let mtrr_ema = ema_u16(next.mtrr_ema, composite);

    next.mtrr_def_type = mtrr_def_type;
    next.mtrr_enabled = mtrr_enabled;
    next.mtrr_fixed_enabled = mtrr_fixed_enabled;
    next.mtrr_ema = mtrr_ema;

    crate::serial_println!(
        "[msr_ia32_mtrr_def_type] raw_lo=0x{:08x} def_type={} enabled={} fixed_enabled={} ema={}",
        lo,
        mtrr_def_type,
        mtrr_enabled,
        mtrr_fixed_enabled,
        mtrr_ema,
    );

    next
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the module: perform first MSR sample and seed the EMA.
pub fn init() {
    let sampled = sample_msr();
    let mut s = STATE.lock();
    *s = sampled;
    crate::serial_println!(
        "[msr_ia32_mtrr_def_type] init complete — mtrr_def_type={} mtrr_enabled={} mtrr_fixed_enabled={} mtrr_ema={}",
        s.mtrr_def_type,
        s.mtrr_enabled,
        s.mtrr_fixed_enabled,
        s.mtrr_ema,
    );
}

/// Per-tick update — gates on every 5000 ticks.
pub fn tick(age: u32) {
    if age % 5000 != 0 {
        return;
    }

    let sampled = sample_msr();
    let mut s = STATE.lock();
    *s = sampled;
}

// ---------------------------------------------------------------------------
// Signal accessors
// ---------------------------------------------------------------------------

/// Default memory type signal (bits[2:0] scaled 0–1000).
/// 0=UC, 142=WC, 568=WT, 710=WP, 852=WB.
#[inline]
pub fn get_mtrr_def_type() -> u16 {
    STATE.lock().mtrr_def_type
}

/// Global MTRR enable signal — 0 or 1000.
#[inline]
pub fn get_mtrr_enabled() -> u16 {
    STATE.lock().mtrr_enabled
}

/// Fixed-range MTRR enable signal — 0 or 1000.
#[inline]
pub fn get_mtrr_fixed_enabled() -> u16 {
    STATE.lock().mtrr_fixed_enabled
}

/// EMA of composite MTRR configuration signal — 0–1000.
#[inline]
pub fn get_mtrr_ema() -> u16 {
    STATE.lock().mtrr_ema
}
