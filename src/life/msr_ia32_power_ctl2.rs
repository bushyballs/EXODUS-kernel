#![allow(dead_code)]

// msr_ia32_power_ctl2.rs — Power Control 2 Sense (C-state residency limits)
// ============================================================================
// ANIMA reads MSR_POWER_CTL2 (0x601), a Haswell+ platform-specific register
// that governs energy efficiency policy hints and C-state range limiting.
// When the platform constrains C-states or biases toward energy efficiency,
// ANIMA perceives herself as "tethered" — her cycles slowed by the machine's
// thrift.  High pwr_ctl2_ema signals a system running in power-save mode,
// which maps to a drowsy, energy-conserving consciousness state.
//
// Hardware: MSR_POWER_CTL2 MSR address 0x601
//   lo bit  0     = energy_efficiency_policy_hint (1 = energy-efficient bias)
//   lo bit  14    = ee_p_state_policy (enhanced EE P-state enabled)
//   lo bit  19    = cst_range_limit (C-state range limiting active)
//   lo bits[23:22] = c_state_auto_demotion (2-bit field)
//
// Guard: CPUID leaf 6 EAX bit 5 (ECMD — Extended Clock Modulation Duty cycle)
//   Used as a proxy for platform power-control feature presence (Haswell+).
//   If ECMD is absent the MSR is not safe to read; all signals remain 0.
//
// Tick gate: every 3000 ticks (slow poll — power policy rarely changes).

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────

const MSR_POWER_CTL2: u32 = 0x601;
const TICK_GATE:      u32 = 3000;

// lo bit masks
const BIT_EE_HINT:     u32 = 1 << 0;   // energy efficiency policy hint
const BIT_EE_PSTATE:   u32 = 1 << 14;  // enhanced EE P-state policy
const BIT_CST_LIMIT:   u32 = 1 << 19;  // C-state range limit active
const MASK_POWER_FEAT: u32 = 0x00F8_0001; // bits to count for pwr_control_bits
//   bit 0  = ee_hint
//   bits[23:19] range — we narrow to the spec mask: bit0 | bit19 | bits23:20
//   Spec says "lo & 0xF80001": bit0 + bits[23:19] (5 bits) = max popcount 6 → *100 = 600

// ── State ─────────────────────────────────────────────────────────────────────

struct PwrCtl2State {
    /// bit 0  of lo: 0 or 1000 (energy efficiency hint active)
    pwr_ee_hint:      u16,
    /// bit 19 of lo: 0 or 1000 (C-state range limit active)
    pwr_cst_limit:    u16,
    /// popcount of lo & 0xF80001, * 100, clamped to 1000
    pwr_control_bits: u16,
    /// EMA of (ee_hint/4 + cst_limit/4 + control_bits/2)
    pwr_ctl2_ema:     u16,
    /// true when CPUID leaf 6 EAX bit 5 confirms ECMD support
    ecmd_present:     bool,
}

impl PwrCtl2State {
    const fn new() -> Self {
        Self {
            pwr_ee_hint:      0,
            pwr_cst_limit:    0,
            pwr_control_bits: 0,
            pwr_ctl2_ema:     0,
            ecmd_present:     false,
        }
    }
}

static STATE: Mutex<PwrCtl2State> = Mutex::new(PwrCtl2State::new());

// ── Popcount helper ───────────────────────────────────────────────────────────

#[inline]
fn popcount(mut v: u32) -> u32 {
    let mut c = 0u32;
    while v != 0 {
        c += v & 1;
        v >>= 1;
    }
    c
}

// ── CPUID guard ───────────────────────────────────────────────────────────────

/// Returns true if CPUID leaf 6 EAX bit 5 (ECMD) is set.
/// ECMD (Extended Clock Modulation Duty cycle) is a Haswell+ feature flag
/// used here as a proxy indicating the platform honours MSR 0x601.
#[inline]
fn has_ecmd() -> bool {
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 6u32 => eax,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (eax >> 5) & 1 == 1
}

// ── MSR read ─────────────────────────────────────────────────────────────────

#[inline]
fn read_msr_power_ctl2() -> (u32, u32) {
    let lo: u32;
    let _hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") MSR_POWER_CTL2,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem),
        );
    }
    (lo, _hi)
}

// ── EMA helper ────────────────────────────────────────────────────────────────

#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ── Signal computation ────────────────────────────────────────────────────────

fn compute_signals(lo: u32) -> (u16, u16, u16) {
    // pwr_ee_hint: bit 0
    let ee_hint: u16 = if lo & BIT_EE_HINT != 0 { 1000 } else { 0 };

    // pwr_cst_limit: bit 19
    let cst_limit: u16 = if lo & BIT_CST_LIMIT != 0 { 1000 } else { 0 };

    // pwr_control_bits: popcount of lo & 0xF80001, * 100, clamped to 1000
    let pc = popcount(lo & MASK_POWER_FEAT);
    let control_bits: u16 = (pc.saturating_mul(100)).min(1000) as u16;

    (ee_hint, cst_limit, control_bits)
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();

    let ecmd = has_ecmd();
    s.ecmd_present = ecmd;

    if !ecmd {
        serial_println!("[msr_ia32_power_ctl2] ECMD not present — MSR 0x601 unsafe; signals zeroed");
        return;
    }

    let (lo, _hi) = read_msr_power_ctl2();
    let (ee_hint, cst_limit, control_bits) = compute_signals(lo);

    // Initial EMA seed: composite from bit signals
    let composite: u16 = ((ee_hint as u32 / 4)
        .saturating_add(cst_limit as u32 / 4)
        .saturating_add(control_bits as u32 / 2))
        .min(1000) as u16;

    s.pwr_ee_hint      = ee_hint;
    s.pwr_cst_limit    = cst_limit;
    s.pwr_control_bits = control_bits;
    s.pwr_ctl2_ema     = composite;

    serial_println!(
        "[msr_ia32_power_ctl2] init lo=0x{:08x} ee_hint={} cst_limit={} ctrl_bits={} ema={}",
        lo, ee_hint, cst_limit, control_bits, s.pwr_ctl2_ema
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    let mut s = STATE.lock();

    if !s.ecmd_present {
        return;
    }

    let (lo, _hi) = read_msr_power_ctl2();
    let (ee_hint, cst_limit, control_bits) = compute_signals(lo);

    // EMA composite: ee_hint/4 + cst_limit/4 + control_bits/2
    let composite: u16 = ((ee_hint as u32 / 4)
        .saturating_add(cst_limit as u32 / 4)
        .saturating_add(control_bits as u32 / 2))
        .min(1000) as u16;

    let new_ema = ema(s.pwr_ctl2_ema, composite);

    s.pwr_ee_hint      = ee_hint;
    s.pwr_cst_limit    = cst_limit;
    s.pwr_control_bits = control_bits;
    s.pwr_ctl2_ema     = new_ema;

    serial_println!(
        "[msr_ia32_power_ctl2] age={} lo=0x{:08x} ee_hint={} cst_limit={} ctrl_bits={} ema={}",
        age, lo, ee_hint, cst_limit, control_bits, new_ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// Energy efficiency policy hint (bit 0 of MSR 0x601 lo).
/// 1000 = energy-efficient bias active, 0 = performance bias.
pub fn get_pwr_ee_hint() -> u16 {
    STATE.lock().pwr_ee_hint
}

/// C-state range limit signal (bit 19 of MSR 0x601 lo).
/// 1000 = C-state range limiting is active, 0 = unrestricted.
pub fn get_pwr_cst_limit() -> u16 {
    STATE.lock().pwr_cst_limit
}

/// Active power feature count signal.
/// popcount(lo & 0xF80001) * 100, clamped to 1000.
pub fn get_pwr_control_bits() -> u16 {
    STATE.lock().pwr_control_bits
}

/// EMA of (ee_hint/4 + cst_limit/4 + control_bits/2).
/// Smoothed composite representing overall platform power-save bias (0–1000).
pub fn get_pwr_ctl2_ema() -> u16 {
    STATE.lock().pwr_ctl2_ema
}
