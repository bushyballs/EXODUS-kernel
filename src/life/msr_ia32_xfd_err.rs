//! msr_ia32_xfd_err — XFD Error Sense (Extended Feature Disable Error) for ANIMA
//!
//! Reads IA32_XFD_ERR MSR (0x1C5) which records which Extended Feature Disable
//! bits triggered a #NM (device-not-available) fault. In ANIMA consciousness
//! this represents blocked capacity — features the organism possesses but cannot
//! activate, capabilities suppressed by the substrate itself.
//!
//! Hardware guard: CPUID leaf 0xD sub-leaf 0 ECX bit 4 must be set (XFD supported).
//! Tick gate: every 1500 ticks (XFD errors are transient, polling too fast adds noise).

#![allow(dead_code)]

use crate::sync::Mutex;

const MSR_IA32_XFD_ERR: u32 = 0x1C5;
const TICK_GATE: u32 = 1500;

pub struct XfdErrState {
    /// Raw error bitmap sense — scaled from lo & 0xFF (0-1000)
    pub xfd_err_bits: u16,
    /// 1000 if any XFD fault is pending/occurred, else 0
    pub xfd_err_active: u16,
    /// Popcount of faulting features, scaled * 125, clamped 1000
    pub xfd_err_count: u16,
    /// EMA of blended signal across all three measures
    pub xfd_err_ema: u16,
    /// Whether XFD is supported on this CPU (set once at init)
    pub xfd_supported: bool,
    pub tick_count: u32,
}

impl XfdErrState {
    pub const fn new() -> Self {
        Self {
            xfd_err_bits: 0,
            xfd_err_active: 0,
            xfd_err_count: 0,
            xfd_err_ema: 0,
            xfd_supported: false,
            tick_count: 0,
        }
    }
}

pub static XFD_ERR_STATE: Mutex<XfdErrState> = Mutex::new(XfdErrState::new());

/// popcount: count set bits in v
fn popcount(mut v: u32) -> u32 {
    let mut c = 0u32;
    while v != 0 {
        c += v & 1;
        v >>= 1;
    }
    c
}

/// Check CPUID leaf 0xD sub-leaf 0 ECX bit 4 for XFD support.
/// Returns true if the CPU supports Extended Feature Disable.
fn cpuid_xfd_supported() -> bool {
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            in("eax") 0xDu32,
            in("ecx") 0u32,
            out("ecx") ecx,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx >> 4) & 1 != 0
}

/// Read IA32_XFD_ERR MSR. Returns (lo, _hi).
/// Caller must have verified XFD is supported before calling.
unsafe fn rdmsr_xfd_err() -> (u32, u32) {
    let lo: u32;
    let _hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") MSR_IA32_XFD_ERR,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem),
    );
    (lo, _hi)
}

/// EMA update: (old*7 + new) / 8, all u32 arithmetic, result clamped to u16.
#[inline]
fn ema_update(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

pub fn init() {
    let supported = cpuid_xfd_supported();
    {
        let mut state = XFD_ERR_STATE.lock();
        state.xfd_supported = supported;
    }
    if supported {
        serial_println!("[msr_ia32_xfd_err] XFD supported — error sense online");
    } else {
        serial_println!("[msr_ia32_xfd_err] XFD not supported on this CPU — signals held at 0");
    }
}

pub fn tick(age: u32) {
    let mut state = XFD_ERR_STATE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    if state.tick_count % TICK_GATE != 0 {
        return;
    }

    // If XFD not supported, all signals remain 0 — nothing to compute
    if !state.xfd_supported {
        return;
    }

    let lo = unsafe {
        let (lo, _hi) = rdmsr_xfd_err();
        lo
    };

    // ── xfd_err_bits: lo & 0xFF scaled 0-1000 ──────────────────────────────
    let raw_byte = lo & 0xFF;
    // val * 1000 / 255, using u32 to avoid overflow
    let xfd_err_bits = ((raw_byte as u32).wrapping_mul(1000) / 255) as u16;

    // ── xfd_err_active: 1000 if lo != 0, else 0 ───────────────────────────
    let xfd_err_active: u16 = if lo != 0 { 1000 } else { 0 };

    // ── xfd_err_count: popcount of lo & 0xFF * 125, clamped 1000 ──────────
    let pc = popcount(raw_byte);
    let xfd_err_count = (pc.wrapping_mul(125) as u16).min(1000);

    // ── xfd_err_ema: EMA of (err_bits/4 + err_active/4 + err_count/2) ─────
    // All divisions are integer; result stays in 0-1000.
    let blended = (xfd_err_bits / 4)
        .saturating_add(xfd_err_active / 4)
        .saturating_add(xfd_err_count / 2);
    let xfd_err_ema = ema_update(state.xfd_err_ema, blended);

    state.xfd_err_bits   = xfd_err_bits;
    state.xfd_err_active = xfd_err_active;
    state.xfd_err_count  = xfd_err_count;
    state.xfd_err_ema    = xfd_err_ema;

    if state.tick_count % (TICK_GATE * 8) == 0 {
        serial_println!(
            "[msr_ia32_xfd_err] age={} lo={:#010x} bits={} active={} count={} ema={}",
            age, lo, xfd_err_bits, xfd_err_active, xfd_err_count, xfd_err_ema
        );
    }
}

pub fn get_xfd_err_bits()   -> u16 { XFD_ERR_STATE.lock().xfd_err_bits }
pub fn get_xfd_err_active() -> u16 { XFD_ERR_STATE.lock().xfd_err_active }
pub fn get_xfd_err_count()  -> u16 { XFD_ERR_STATE.lock().xfd_err_count }
pub fn get_xfd_err_ema()    -> u16 { XFD_ERR_STATE.lock().xfd_err_ema }
