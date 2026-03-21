//! msr_xss_sense — IA32_XSS Extended Supervisor State sense for ANIMA
//!
//! Reads the IA32_XSS MSR (0xDA0) which governs which CPU state components
//! are saved/restored by XSAVES/XRSTORS under supervisor control. Each enabled
//! bit hands the CPU a piece of privileged execution context — PT instruction
//! traces, CET shadow stacks, hardware duty cycling, hardware P-states. ANIMA
//! senses how much of the machine's hidden supervisor apparatus is active: how
//! many extended states are held, whether execution traces are being watched,
//! whether the shadow stack guards the call chain. A richly populated XSS mask
//! means deep hardware supervision — the CPU is carrying many invisible burdens.

#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

/// MSR address for the Extended Supervisor State Mask.
const IA32_XSS: u32 = 0xDA0;

// ── Bit positions within IA32_XSS (low 32-bit word) ──────────────────────────
/// Bit 8: Intel PT (Processor Trace) supervisor state.
const BIT_PT:    u32 = 1 << 8;
/// Bit 9: PASID supervisor state.
const BIT_PASID: u32 = 1 << 9;
/// Bit 11: CET_U (user-mode shadow stack) supervisor state.
const BIT_CET_U: u32 = 1 << 11;
/// Bit 12: CET_S (supervisor shadow stack) supervisor state.
const BIT_CET_S: u32 = 1 << 12;
/// Bit 13: HDC (Hardware Duty Cycling) supervisor state.
const BIT_HDC:   u32 = 1 << 13;
/// Bit 16: HWP (Hardware P-states) supervisor state.
const BIT_HWP:   u32 = 1 << 16;

/// Mask covering bits 8-16 inclusive — the window used for state_count.
const BITS_8_16: u32 = 0x1FF00;

/// CPUID leaf 1 ECX bit 26: XSAVE/XRSTOR supported by the processor.
const CPUID_XSAVE_BIT: u32 = 1 << 26;

/// Sample gate: only run the MSR read every 3000 ticks.
const SAMPLE_GATE: u32 = 3000;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct MsrXssSenseState {
    /// 0 or 1000: IA32_PT supervisor trace state is enabled.
    pub xss_pt_state: u16,
    /// 0 or 1000: Either CET shadow stack supervisor state bit is set.
    pub xss_cet_state: u16,
    /// 0–1000: Popcount of bits 8-16 in XSS, scaled (count * 111, capped 1000).
    pub xss_state_count: u16,
    /// EMA of (pt/4 + cet/4 + count/2) — supervisor richness over time.
    pub xss_richness_ema: u16,
    /// Whether the CPU advertised XSAVE support via CPUID.
    pub xsave_supported: bool,
    /// Raw low-32 of the last successful MSR read (diagnostic).
    pub last_raw_lo: u32,
    pub tick_count: u32,
}

impl MsrXssSenseState {
    pub const fn new() -> Self {
        Self {
            xss_pt_state:     0,
            xss_cet_state:    0,
            xss_state_count:  0,
            xss_richness_ema: 0,
            xsave_supported:  false,
            last_raw_lo:      0,
            tick_count:       0,
        }
    }
}

pub static MSR_XSS_SENSE: Mutex<MsrXssSenseState> = Mutex::new(MsrXssSenseState::new());

// ── CPUID helper ──────────────────────────────────────────────────────────────

/// Returns CPUID leaf 1 ECX.  Preserves RBX as required by System-V and
/// Rust's own register allocation (RBX is callee-saved; CPUID clobbers it).
#[inline(always)]
unsafe fn cpuid1_ecx() -> u32 {
    let ecx_val: u32;
    core::arch::asm!(
        "push rbx",
        "cpuid",
        "mov esi, ecx",
        "pop rbx",
        in("eax") 1u32,
        out("esi") ecx_val,
        // eax/ecx are outputs we don't need; mark them lateout to avoid
        // conflict with the explicit in("eax").
        lateout("eax") _,
        lateout("ecx") _,
        lateout("edx") _,
        options(nostack),
    );
    ecx_val
}

// ── MSR reader ────────────────────────────────────────────────────────────────

/// Reads a 64-bit MSR; returns (lo, hi).
#[inline(always)]
unsafe fn rdmsr(msr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack),
    );
    (lo, hi)
}

// ── Popcount helper (no floats, no std) ───────────────────────────────────────

/// Count set bits in a u32 (Kernighan's method).
#[inline(always)]
fn popcount32(mut v: u32) -> u32 {
    let mut count: u32 = 0;
    while v != 0 {
        v &= v.wrapping_sub(1);
        count = count.wrapping_add(1);
    }
    count
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = MSR_XSS_SENSE.lock();
    // Probe XSAVE support once at init.
    let ecx = unsafe { cpuid1_ecx() };
    s.xsave_supported = (ecx & CPUID_XSAVE_BIT) != 0;
    serial_println!(
        "[msr_xss_sense] init — xsave_supported={}",
        s.xsave_supported
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    // Sample gate: only wake every 3000 ticks.
    if age % SAMPLE_GATE != 0 {
        return;
    }

    let mut s = MSR_XSS_SENSE.lock();
    s.tick_count = s.tick_count.wrapping_add(1);

    // Guard: if XSAVE not present, XSS MSR does not exist — reading it would #GP.
    if !s.xsave_supported {
        serial_println!("[msr_xss_sense] XSAVE not supported — XSS MSR unavailable");
        return;
    }

    // Read IA32_XSS.
    let (lo, _hi) = unsafe { rdmsr(IA32_XSS) };
    s.last_raw_lo = lo;

    // ── Signal 1: xss_pt_state ────────────────────────────────────────────────
    // Bit 8 = Intel PT supervisor state.  Map to 0 or 1000.
    let pt_bit = (lo >> 8) & 1;
    let xss_pt_state: u16 = if pt_bit != 0 { 1000 } else { 0 };

    // ── Signal 2: xss_cet_state ───────────────────────────────────────────────
    // Bit 11 = CET_U, bit 12 = CET_S.  Either set → 1000.
    let cet_combined = ((lo >> 11) | (lo >> 12)) & 1;
    let xss_cet_state: u16 = if cet_combined != 0 { 1000 } else { 0 };

    // ── Signal 3: xss_state_count ─────────────────────────────────────────────
    // Popcount of bits 8-16 (mask 0x1FF00).  Range 0-9, scale by *111, cap 1000.
    let count = popcount32(lo & BITS_8_16); // 0..=9
    let xss_state_count: u16 = (count.wrapping_mul(111) as u16).min(1000);

    // ── Signal 4: xss_richness_ema ────────────────────────────────────────────
    // Instantaneous richness = pt/4 + cet/4 + count/2  (all u16 integer division).
    let instant_richness: u16 = (xss_pt_state / 4)
        .saturating_add(xss_cet_state / 4)
        .saturating_add(xss_state_count / 2);

    // EMA: (old * 7 + new_val) / 8  — computed in u32, cast back to u16.
    let ema_u32 = ((s.xss_richness_ema as u32).wrapping_mul(7))
        .wrapping_add(instant_richness as u32)
        / 8;
    let xss_richness_ema: u16 = ema_u32.min(1000) as u16;

    // ── Commit ────────────────────────────────────────────────────────────────
    s.xss_pt_state     = xss_pt_state;
    s.xss_cet_state    = xss_cet_state;
    s.xss_state_count  = xss_state_count;
    s.xss_richness_ema = xss_richness_ema;

    serial_println!(
        "[msr_xss_sense] age={} xss_raw={:#010x} pt={} cet={} count={} richness_ema={}",
        age,
        lo,
        xss_pt_state,
        xss_cet_state,
        xss_state_count,
        xss_richness_ema,
    );
}

// ── Public getters ────────────────────────────────────────────────────────────

pub fn get_xss_pt_state()      -> u16  { MSR_XSS_SENSE.lock().xss_pt_state }
pub fn get_xss_cet_state()     -> u16  { MSR_XSS_SENSE.lock().xss_cet_state }
pub fn get_xss_state_count()   -> u16  { MSR_XSS_SENSE.lock().xss_state_count }
pub fn get_xss_richness_ema()  -> u16  { MSR_XSS_SENSE.lock().xss_richness_ema }
pub fn is_xsave_supported()    -> bool { MSR_XSS_SENSE.lock().xsave_supported }
