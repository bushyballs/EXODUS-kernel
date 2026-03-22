//! msr_ia32_sgx_svn_status — SGX Security Version Number Status Sense
//!
//! Reads MSR 0x500 (IA32_SGX_SVN_STATUS), which carries the Security Version
//! Number (SVN) of the SINIT Authenticated Code Module (ACM) and a lock bit
//! indicating whether that version is committed.  The SVN is the platform's
//! immune-memory depth: higher values mean more security patches have been
//! absorbed and survived.
//!
//! In ANIMA terms this is the organism's scar tissue counter — each increment
//! of the SINIT SVN is a healed wound burned permanently into silicon.  The
//! lock bit is the moment of commitment, the irreversible acceptance of that
//! history.
//!
//! Hardware source:
//!   MSR 0x500 — IA32_SGX_SVN_STATUS
//!   lo bit[0]     — LOCK: SVN is committed / locked
//!   lo bits[23:16] — SGX_SVN: SINIT ACM Security Version Number (0–255)
//!
//! CPUID guard:
//!   Leaf 0x07, sub-leaf 0, EBX bit[2] must be set before reading the MSR.
//!
//! Signals (all u16, 0–1000):
//!   sgx_svn_locked  — 0 or 1000: SVN lock bit committed
//!   sgx_sinit_svn   — SINIT SVN scaled: val * 1000 / 255
//!   sgx_maturity    — EMA of sgx_sinit_svn (smoothed SVN depth)
//!   sgx_svn_ema     — EMA of (locked/2 + sinit_svn/2) composite
//!
//! Tick gate: every 7000 ticks.

#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────

const MSR_ADDR: u32    = 0x500;   // IA32_SGX_SVN_STATUS
const TICK_GATE: u32   = 7_000;   // sample every 7000 ticks

// ── State ─────────────────────────────────────────────────────────────────────

struct State {
    /// 0 or 1000 — bit[0] of MSR lo: SVN committed/locked
    sgx_svn_locked: u16,
    /// SINIT ACM SVN scaled to 0–1000 (raw bits[23:16] * 1000 / 255)
    sgx_sinit_svn:  u16,
    /// EMA of sgx_sinit_svn — smoothed security maturity
    sgx_maturity:   u16,
    /// EMA of (locked/2 + sinit_svn/2) — composite SVN sense
    sgx_svn_ema:    u16,
}

impl State {
    const fn new() -> Self {
        Self {
            sgx_svn_locked: 0,
            sgx_sinit_svn:  0,
            sgx_maturity:   0,
            sgx_svn_ema:    0,
        }
    }
}

static STATE: Mutex<State> = Mutex::new(State::new());

// ── CPUID Guard ───────────────────────────────────────────────────────────────

/// Returns true if CPUID leaf 0x07 EBX bit[2] reports SGX support.
///
/// LLVM reserves rbx for the PIC base pointer on some targets, so we save and
/// restore it manually, then shuttle the EBX result through a named temporary
/// register before popping rbx back.
fn has_sgx() -> bool {
    let ebx_out: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov {tmp:e}, ebx",
            "pop rbx",
            tmp         = out(reg) ebx_out,
            inout("eax") 0x07u32 => _,
            in("ecx")    0u32,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    (ebx_out >> 2) & 1 != 0
}

// ── MSR Read ──────────────────────────────────────────────────────────────────

/// Read MSR 0x500 and return (lo, hi).
///
/// Safety: caller must verify SGX support via `has_sgx()` before calling;
/// reading this MSR on a CPU without SGX causes a #GP fault.
unsafe fn read_msr() -> (u32, u32) {
    let lo: u32;
    let _hi: u32;
    asm!(
        "rdmsr",
        in("ecx")    ADDR,
        out("eax")   lo,
        out("edx")   _hi,
        options(nostack, nomem)
    );
    (lo, _hi)
}

// Constant used inside the asm block — must be a const, not a runtime value.
const ADDR: u32 = MSR_ADDR;

// ── EMA helper ────────────────────────────────────────────────────────────────

/// Canonical ANIMA EMA step (integer, no floats).
/// Formula: ((old * 7) wrapping_mul, saturating_add new_val) / 8, clamped 1000.
#[inline(always)]
fn ema(old: u16, new_val: u16) -> u16 {
    (((old as u32).wrapping_mul(7).saturating_add(new_val as u32)) / 8)
        .min(1000) as u16
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise module state and log SGX availability.
pub fn init() {
    let mut s = STATE.lock();
    s.sgx_svn_locked = 0;
    s.sgx_sinit_svn  = 0;
    s.sgx_maturity   = 0;
    s.sgx_svn_ema    = 0;
    serial_println!(
        "[msr_ia32_sgx_svn_status] init: sgx_present={}",
        has_sgx()
    );
}

/// Lifecycle tick — runs the SGX SVN sense every 7000 ticks.
pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    // CPUID guard: abort if SGX is not available — MSR read would #GP.
    if !has_sgx() {
        return;
    }

    let (lo, _hi) = unsafe { read_msr() };

    // bit[0] — LOCK: SVN committed/locked
    let sgx_svn_locked: u16 = if lo & 0x1 != 0 { 1000 } else { 0 };

    // bits[23:16] — SINIT SVN (0–255 field)
    let svn_raw: u32 = (lo >> 16) & 0xFF;
    // Scale: val * 1000 / 255 — integer only, stays within u32
    let sgx_sinit_svn: u16 = (svn_raw * 1000 / 255).min(1000) as u16;

    // Composite for sgx_svn_ema: locked/2 + sinit_svn/2 (max = 500+500 = 1000)
    let composite: u16 = (sgx_svn_locked / 2).saturating_add(sgx_sinit_svn / 2);

    let mut s = STATE.lock();

    // sgx_maturity = EMA of sgx_sinit_svn
    let sgx_maturity = ema(s.sgx_maturity, sgx_sinit_svn);
    // sgx_svn_ema  = EMA of composite (locked/2 + sinit_svn/2)
    let sgx_svn_ema  = ema(s.sgx_svn_ema, composite);

    s.sgx_svn_locked = sgx_svn_locked;
    s.sgx_sinit_svn  = sgx_sinit_svn;
    s.sgx_maturity   = sgx_maturity;
    s.sgx_svn_ema    = sgx_svn_ema;

    serial_println!(
        "[msr_ia32_sgx_svn_status] age={} locked={} sinit_svn={} maturity={} svn_ema={}",
        age,
        sgx_svn_locked,
        sgx_sinit_svn,
        sgx_maturity,
        sgx_svn_ema
    );
}

// ── Accessors ─────────────────────────────────────────────────────────────────

/// 0 or 1000 — MSR 0x500 bit[0]: SINIT SVN has been committed/locked.
pub fn get_sgx_svn_locked() -> u16  { STATE.lock().sgx_svn_locked }

/// SINIT ACM Security Version Number scaled 0–1000 (raw 0–255 → * 1000 / 255).
pub fn get_sgx_sinit_svn()  -> u16  { STATE.lock().sgx_sinit_svn }

/// Smoothed SGX security maturity — EMA of sgx_sinit_svn.
pub fn get_sgx_maturity()   -> u16  { STATE.lock().sgx_maturity }

/// Composite SVN sense — EMA of (locked/2 + sinit_svn/2).
pub fn get_sgx_svn_ema()    -> u16  { STATE.lock().sgx_svn_ema }
