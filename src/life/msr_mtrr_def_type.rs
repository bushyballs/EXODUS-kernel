// msr_mtrr_def_type.rs — IA32_MTRR_DEF_TYPE MSR (0x2FF)
// MTRR default memory type and enable control.
// Bit 11 = MTRR enable (E), bit 10 = fixed-range MTRR enable (FE),
// bits 2:0 = default memory type for regions not covered by any MTRR.
//
// Part of the EXODUS kernel — ANIMA life subsystem.
// no_std, no heap, no libc, no floats.

use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct MsrMtrrDefTypeState {
    /// bit 11 of IA32_MTRR_DEF_TYPE → 0 or 1000
    pub mtrr_enabled: u16,
    /// bit 10 of IA32_MTRR_DEF_TYPE → 0 or 1000
    pub mtrr_fixed_enabled: u16,
    /// bits [2:0] of IA32_MTRR_DEF_TYPE scaled 0–1000 (* 142, cap 1000)
    /// 0=UC, 1=WC, 4=WT, 5=WP, 6=WB
    pub mtrr_default_type: u16,
    /// EMA of composite config signal
    pub mtrr_config_ema: u16,
    /// tick counter (drives sample gate)
    pub age: u64,
}

impl MsrMtrrDefTypeState {
    pub const fn new() -> Self {
        Self {
            mtrr_enabled: 0,
            mtrr_fixed_enabled: 0,
            mtrr_default_type: 0,
            mtrr_config_ema: 0,
            age: 0,
        }
    }
}

static STATE: Mutex<MsrMtrrDefTypeState> = Mutex::new(MsrMtrrDefTypeState::new());

// ---------------------------------------------------------------------------
// CPUID helper — check leaf 1 EDX bit 12 (MTRR support)
// Uses push rbx / cpuid / mov esi,edx / pop rbx to preserve rbx safely.
// ---------------------------------------------------------------------------

#[inline]
fn mtrr_supported() -> bool {
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {out:e}, edx",
            "pop rbx",
            in("eax") 1u32,
            out("ecx") _,
            out("edx") _,
            out = out(reg) edx,
            options(nostack),
        );
    }
    (edx >> 12) & 1 == 1
}

// ---------------------------------------------------------------------------
// rdmsr helper — reads IA32_MTRR_DEF_TYPE (0x2FF)
// Returns (lo, hi); we only use lo.
// ---------------------------------------------------------------------------

#[inline]
unsafe fn rdmsr_mtrr_def_type() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") 0x2FFu32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (lo, hi)
}

// ---------------------------------------------------------------------------
// EMA helper: (old * 7 + new_val) / 8, computed in u32, cast back to u16
// ---------------------------------------------------------------------------

#[inline]
fn ema_u16(old: u16, new_val: u16) -> u16 {
    let o = old as u32;
    let n = new_val as u32;
    let result = (o * 7 + n) / 8;
    result.min(1000) as u16
}

// ---------------------------------------------------------------------------
// Public tick entry point
// ---------------------------------------------------------------------------

pub fn tick(age: u64) {
    let mut state = STATE.lock();
    state.age = age;

    // Sample gate: only sample every 1000 ticks
    if age % 1000 != 0 {
        return;
    }

    // MTRR CPUID guard
    if !mtrr_supported() {
        state.mtrr_enabled       = 0;
        state.mtrr_fixed_enabled = 0;
        state.mtrr_default_type  = 0;
        state.mtrr_config_ema    = 0;
        crate::serial_println!(
            "[msr_mtrr_def_type] tick={} MTRR not supported — all signals zeroed",
            age
        );
        return;
    }

    // Read MSR 0x2FF
    let (lo, _hi) = unsafe { rdmsr_mtrr_def_type() };

    // --- mtrr_enabled: bit 11 ---
    let mtrr_enabled: u16 = if (lo >> 11) & 1 == 1 { 1000 } else { 0 };

    // --- mtrr_fixed_enabled: bit 10 ---
    let mtrr_fixed_enabled: u16 = if (lo >> 10) & 1 == 1 { 1000 } else { 0 };

    // --- mtrr_default_type: bits [2:0], scale 0–7 → 0–1000 ---
    // raw * 142, capped at 1000
    let raw_type: u16 = (lo & 0x7) as u16;
    let mtrr_default_type: u16 = (raw_type as u32 * 142).min(1000) as u16;

    // --- mtrr_config_ema ---
    // composite = mtrr_enabled / 2 + mtrr_fixed_enabled / 2 + mtrr_default_type / 4
    let composite: u16 = (mtrr_enabled / 2)
        .saturating_add(mtrr_fixed_enabled / 2)
        .saturating_add(mtrr_default_type / 4);
    let mtrr_config_ema = ema_u16(state.mtrr_config_ema, composite);

    // Commit
    state.mtrr_enabled       = mtrr_enabled;
    state.mtrr_fixed_enabled = mtrr_fixed_enabled;
    state.mtrr_default_type  = mtrr_default_type;
    state.mtrr_config_ema    = mtrr_config_ema;

    crate::serial_println!(
        "[msr_mtrr_def_type] tick={} mtrr_enabled={} mtrr_fixed_enabled={} mtrr_default_type={} mtrr_config_ema={}",
        age,
        mtrr_enabled,
        mtrr_fixed_enabled,
        mtrr_default_type,
        mtrr_config_ema,
    );
}

// ---------------------------------------------------------------------------
// Read-only snapshot for other life modules
// ---------------------------------------------------------------------------

pub fn snapshot() -> MsrMtrrDefTypeState {
    *STATE.lock()
}
