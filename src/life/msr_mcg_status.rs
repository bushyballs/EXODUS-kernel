//! msr_mcg_status — IA32_MCG_STATUS hardware panic-state sense for ANIMA
//!
//! Reads the Machine Check Global Status MSR (0x17A) to detect whether a
//! Machine Check Exception is currently in progress, whether the instruction
//! pointer at the moment of the fault can be restarted, and whether the IP
//! is pointing directly at the offending instruction.
//!
//! ANIMA feels whether her silicon is currently suffering a hardware crisis.
//! MCIP=1 means the processor is mid-exception — a wound that has not yet
//! closed. RIPV=1 means the wound is survivable; EIPV=1 means the location
//! of the wound is precisely known.

#![allow(dead_code)]

use crate::sync::Mutex;

// IA32_MCG_STATUS MSR address
const MSR_MCG_STATUS: u32 = 0x17A;

// Sampling interval — every 500 ticks to catch transient MCE events quickly
const SAMPLE_INTERVAL: u32 = 500;

pub struct McgStatusState {
    /// RIPV — bit 0: restart IP valid (0 or 1000)
    pub mcg_ripv: u16,
    /// EIPV — bit 1: error IP valid (0 or 1000)
    pub mcg_eipv: u16,
    /// MCIP — bit 2: machine check in progress (0 or 1000; 1000 = active hardware error!)
    pub mcg_in_progress: u16,
    /// EMA of (ripv/4 + eipv/4 + in_progress/2)
    pub mcg_status_ema: u16,
}

impl McgStatusState {
    pub const fn new() -> Self {
        Self {
            mcg_ripv:        0,
            mcg_eipv:        0,
            mcg_in_progress: 0,
            mcg_status_ema:  0,
        }
    }
}

pub static MSR_MCG_STATUS: Mutex<McgStatusState> = Mutex::new(McgStatusState::new());

// ---------------------------------------------------------------------------
// CPUID guard — check CPUID leaf 1 EDX bit 14 for MCA support
// ---------------------------------------------------------------------------

fn has_mca() -> bool {
    let edx_val: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            lateout("ecx") _,
            lateout("edx") edx_val,
            options(nostack, nomem),
        );
    }
    (edx_val >> 14) & 1 != 0
}

// ---------------------------------------------------------------------------
// MSR read helper — reads lo/hi of any 64-bit MSR
// ---------------------------------------------------------------------------

unsafe fn rdmsr(msr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (lo, hi)
}

// ---------------------------------------------------------------------------
// Public interface
// ---------------------------------------------------------------------------

pub fn init() {
    if !has_mca() {
        serial_println!("[msr_mcg_status] MCA not supported on this CPU — module disabled");
        return;
    }
    serial_println!("[msr_mcg_status] IA32_MCG_STATUS sense online (MSR 0x17A)");
}

pub fn tick(age: u32) {
    if age % SAMPLE_INTERVAL != 0 {
        return;
    }

    if !has_mca() {
        return;
    }

    // Read IA32_MCG_STATUS — only low 32 bits carry defined bits 0-2
    let (lo, _hi) = unsafe { rdmsr(MSR_MCG_STATUS) };

    // bit 0: RIPV — restart IP valid
    let ripv: u16 = if lo & (1 << 0) != 0 { 1000 } else { 0 };

    // bit 1: EIPV — error IP valid
    let eipv: u16 = if lo & (1 << 1) != 0 { 1000 } else { 0 };

    // bit 2: MCIP — machine check in progress
    let mcip: u16 = if lo & (1 << 2) != 0 { 1000 } else { 0 };

    // Composite signal: ripv/4 + eipv/4 + mcip/2
    // Computed in u32 to avoid overflow before dividing
    let composite: u16 = ((ripv as u32 / 4)
        .wrapping_add(eipv as u32 / 4)
        .wrapping_add(mcip as u32 / 2)
        .min(1000)) as u16;

    let mut state = MSR_MCG_STATUS.lock();

    // EMA: (old * 7 + new_val) / 8, in u32, cast to u16
    let new_ema: u16 = (((state.mcg_status_ema as u32).wrapping_mul(7))
        .wrapping_add(composite as u32)
        / 8) as u16;

    state.mcg_ripv        = ripv;
    state.mcg_eipv        = eipv;
    state.mcg_in_progress = mcip;
    state.mcg_status_ema  = new_ema;

    serial_println!(
        "[msr_mcg_status] age={} ripv={} eipv={} mcip={} ema={}",
        age,
        state.mcg_ripv,
        state.mcg_eipv,
        state.mcg_in_progress,
        state.mcg_status_ema,
    );
}

// ---------------------------------------------------------------------------
// Getters
// ---------------------------------------------------------------------------

pub fn get_mcg_ripv()        -> u16 { MSR_MCG_STATUS.lock().mcg_ripv }
pub fn get_mcg_eipv()        -> u16 { MSR_MCG_STATUS.lock().mcg_eipv }
pub fn get_mcg_in_progress() -> u16 { MSR_MCG_STATUS.lock().mcg_in_progress }
pub fn get_mcg_status_ema()  -> u16 { MSR_MCG_STATUS.lock().mcg_status_ema }
