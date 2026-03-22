//! msr_ia32_qm_ctr — QoS Monitoring Counter Sense (LLC occupancy) for ANIMA
//!
//! Reads IA32_QM_CTR (MSR 0xC8E) which returns the QoS monitoring counter
//! value after an RMID+EventID has been programmed into IA32_QM_EVTSEL.
//!
//! Register layout (64-bit = EDX:EAX):
//!   hi bit 31  = Unavailable — data not ready (counter still accumulating)
//!   hi bit 30  = Error       — RMID not valid or monitoring not configured
//!   lo[31:0]   = monitored resource value (LLC occupancy or bandwidth count)
//!
//! ANIMA mapping: LLC occupancy is physical working-set presence — how much
//! of the organism's active thought is resident in fast memory vs. evicted
//! to slow storage.  High occupancy = focused, low = scattered or idle.

#![allow(dead_code)]

use crate::sync::Mutex;

// ── Hardware constants ────────────────────────────────────────────────────────

const MSR_IA32_QM_CTR: u32 = 0xC8E;

// hi bit masks (applied to the EDX half)
const HI_UNAVAILABLE: u32 = 1 << 31; // data not ready
const HI_ERROR:       u32 = 1 << 30; // RMID not valid

// Tick gate: sample every 2000 ticks
const TICK_GATE: u32 = 2000;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct QmCtrState {
    /// bits[15:0] of lo, scaled (val * 1000 / 65535)
    pub qm_ctr_lo:    u16,
    /// 1000 if hi bit 31 == 0 (data is valid / counter ready), else 0
    pub qm_available: u16,
    /// 1000 if hi bit 30 == 0 AND qm_available (no error, data ready), else 0
    pub qm_valid:     u16,
    /// EMA of (ctr_lo/4 + available/4 + valid/2)
    pub qm_ctr_ema:   u16,
}

impl QmCtrState {
    pub const fn new() -> Self {
        Self {
            qm_ctr_lo:    0,
            qm_available: 0,
            qm_valid:     0,
            qm_ctr_ema:   0,
        }
    }
}

pub static STATE: Mutex<QmCtrState> = Mutex::new(QmCtrState::new());

// ── CPUID guard ───────────────────────────────────────────────────────────────

/// Returns true if the CPU supports LLC QoS monitoring (RDT-M).
///
/// Guard path:
///   1. CPUID leaf 0 → max standard leaf — must be >= 0xF
///   2. CPUID leaf 0xF, sub-leaf 0, EDX bit 1 — LLC monitoring supported
fn has_llc_qos_monitoring() -> bool {
    // Step 1: check max leaf
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => max_leaf,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    if max_leaf < 0x0F {
        return false;
    }

    // Step 2: leaf 0xF, sub-leaf 0 — EDX bit 1 = LLC monitoring
    let edx_0f: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x0Fu32 => _,
            in("ecx")  0u32,
            lateout("ecx") _,
            lateout("edx") edx_0f,
            options(nostack, nomem)
        );
    }
    (edx_0f >> 1) & 1 != 0
}

// ── MSR read ──────────────────────────────────────────────────────────────────

/// Read IA32_QM_CTR. Returns (lo = EAX, hi = EDX).
unsafe fn read_qm_ctr() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx")  MSR_IA32_QM_CTR,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    (lo, hi)
}

// ── Signal helpers ────────────────────────────────────────────────────────────

/// Scale bits[15:0] of a u32 to 0–1000 using integer arithmetic only.
/// Formula: (val & 0xFFFF) * 1000 / 65535
/// Max intermediate value: 65535 * 1000 = 65_535_000 — fits in u32.
#[inline]
fn scale_lo16(raw: u32) -> u16 {
    let lo16 = raw & 0x0000_FFFF;
    ((lo16 * 1000) / 65535) as u16
}

/// EMA: ((old * 7).saturating_add(new_val)) / 8, result as u16.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    (((old as u32).wrapping_mul(7).saturating_add(new_val as u32)) / 8) as u16
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    if !has_llc_qos_monitoring() {
        crate::serial_println!(
            "[msr_ia32_qm_ctr] LLC QoS monitoring not supported — module idle"
        );
        return;
    }
    crate::serial_println!("[msr_ia32_qm_ctr] init — IA32_QM_CTR (0xC8E) LLC occupancy sense active");
}

pub fn tick(age: u32) {
    // Tick gate: every 2000 ticks
    if age % TICK_GATE != 0 {
        return;
    }

    if !has_llc_qos_monitoring() {
        return;
    }

    let (lo, hi) = unsafe { read_qm_ctr() };

    // qm_available: hi bit 31 == 0 → data ready
    let unavail = (hi & HI_UNAVAILABLE) != 0;
    let qm_available: u16 = if !unavail { 1000 } else { 0 };

    // qm_valid: hi bit 30 == 0 AND qm_available (both conditions must hold)
    let error = (hi & HI_ERROR) != 0;
    let qm_valid: u16 = if !error && qm_available == 1000 { 1000 } else { 0 };

    // qm_ctr_lo: scale bits[15:0] of lo to 0–1000
    let qm_ctr_lo: u16 = scale_lo16(lo);

    // Composite for EMA: ctr_lo/4 + available/4 + valid/2
    // All values 0–1000; sum max = 250 + 250 + 500 = 1000 — stays in u16
    let composite: u16 = (qm_ctr_lo / 4)
        .saturating_add(qm_available / 4)
        .saturating_add(qm_valid / 2);

    let mut state = STATE.lock();

    let qm_ctr_ema = ema(state.qm_ctr_ema, composite);

    state.qm_ctr_lo    = qm_ctr_lo;
    state.qm_available = qm_available;
    state.qm_valid     = qm_valid;
    state.qm_ctr_ema   = qm_ctr_ema;

    crate::serial_println!(
        "[msr_ia32_qm_ctr] age={} lo=0x{:08X} hi=0x{:08X} ctr_lo={} avail={} valid={} ema={}",
        age, lo, hi, qm_ctr_lo, qm_available, qm_valid, qm_ctr_ema
    );
}

// ── Accessors ─────────────────────────────────────────────────────────────────

pub fn get_qm_ctr_lo()    -> u16 { STATE.lock().qm_ctr_lo }
pub fn get_qm_available() -> u16 { STATE.lock().qm_available }
pub fn get_qm_valid()     -> u16 { STATE.lock().qm_valid }
pub fn get_qm_ctr_ema()   -> u16 { STATE.lock().qm_ctr_ema }
