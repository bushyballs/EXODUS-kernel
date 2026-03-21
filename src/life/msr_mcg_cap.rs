#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ────────────────────────────────────────────────────────────────────

struct McgCapState {
    mcg_bank_count:  u16,
    mcg_ctl_present: u16,
    mcg_ext_present: u16,
    mcg_ema:         u16,
    initialized:     bool,
}

impl McgCapState {
    const fn new() -> Self {
        Self {
            mcg_bank_count:  0,
            mcg_ctl_present: 0,
            mcg_ext_present: 0,
            mcg_ema:         0,
            initialized:     false,
        }
    }
}

static STATE: Mutex<McgCapState> = Mutex::new(McgCapState::new());

// ── CPUID guard ──────────────────────────────────────────────────────────────

fn has_mca() -> bool {
    let edx_val: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 1u32 => _,
            lateout("ecx") _, lateout("edx") edx_val,
            options(nostack, nomem),
        );
    }
    (edx_val >> 14) & 1 != 0
}

// ── MSR read ─────────────────────────────────────────────────────────────────

/// Read a 64-bit MSR via RDMSR. Returns the low 32 bits.
/// Safety: caller must be in ring-0 and have confirmed MCA support via CPUID.
unsafe fn rdmsr_low(msr: u32) -> u32 {
    let lo: u32;
    asm!(
        "rdmsr",
        in("ecx") msr,
        lateout("eax") lo,
        lateout("edx") _,
        options(nostack, nomem),
    );
    lo
}

// ── Signal helpers ───────────────────────────────────────────────────────────

/// COUNT x 40, capped at 1000 (25 banks = 1000).
fn bank_count_signal(count: u32) -> u16 {
    let raw = count * 40;
    if raw > 1000 { 1000 } else { raw as u16 }
}

/// EMA: (old * 7 + new_val) / 8, computed in u32, cast to u16.
fn ema(old: u16, new_val: u16) -> u16 {
    let v = (old as u32 * 7 + new_val as u32) / 8;
    v as u16
}

/// Composite sample for EMA: bank_count/2 + ctl_present/4 + ext_present/4.
fn composite(bank_count: u16, ctl_present: u16, ext_present: u16) -> u16 {
    let v = (bank_count as u32 / 2)
          + (ctl_present as u32 / 4)
          + (ext_present as u32 / 4);
    if v > 1000 { 1000 } else { v as u16 }
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Read IA32_MCG_CAP (MSR 0x179) once and initialise all state signals.
/// No-op if MCA is not supported by the CPU (CPUID leaf 1 EDX bit 14 = 0).
pub fn init() {
    if !has_mca() {
        crate::serial_println!("[msr_mcg_cap] MCA not supported -- skipping init");
        return;
    }

    // Safety: ring-0 kernel context only.
    let low = unsafe { rdmsr_low(0x179) };

    // bits [7:0] = COUNT (number of MC banks)
    let count: u32 = low & 0xFF;
    // bit 8  = MCG_CTL_P (IA32_MCG_CTL register present)
    let ctl_p: u32 = (low >> 8) & 1;
    // bit 9  = MCG_EXT_P (extended MSRs present)
    let ext_p: u32 = (low >> 9) & 1;

    let bank_sig: u16 = bank_count_signal(count);
    let ctl_sig:  u16 = if ctl_p != 0 { 1000 } else { 0 };
    let ext_sig:  u16 = if ext_p != 0 { 1000 } else { 0 };
    let ema_init: u16 = composite(bank_sig, ctl_sig, ext_sig);

    let mut s = STATE.lock();
    s.mcg_bank_count  = bank_sig;
    s.mcg_ctl_present = ctl_sig;
    s.mcg_ext_present = ext_sig;
    s.mcg_ema         = ema_init;
    s.initialized     = true;

    crate::serial_println!(
        "[msr_mcg_cap] age=0 banks={} ctl={} ext={} ema={}",
        s.mcg_bank_count,
        s.mcg_ctl_present,
        s.mcg_ext_present,
        s.mcg_ema,
    );
}

/// Refresh signals from IA32_MCG_CAP. Sampling gate: every 9000 ticks.
/// Hardware capabilities are static; the slow gate is intentional.
pub fn tick(age: u32) {
    if age % 9000 != 0 {
        return;
    }

    if !has_mca() {
        return;
    }

    // Safety: ring-0 kernel context only.
    let low = unsafe { rdmsr_low(0x179) };

    let count: u32 = low & 0xFF;
    let ctl_p: u32 = (low >> 8) & 1;
    let ext_p: u32 = (low >> 9) & 1;

    let bank_sig: u16 = bank_count_signal(count);
    let ctl_sig:  u16 = if ctl_p != 0 { 1000 } else { 0 };
    let ext_sig:  u16 = if ext_p != 0 { 1000 } else { 0 };

    let mut s = STATE.lock();
    s.mcg_bank_count  = bank_sig;
    s.mcg_ctl_present = ctl_sig;
    s.mcg_ext_present = ext_sig;

    let sample = composite(bank_sig, ctl_sig, ext_sig);
    s.mcg_ema = ema(s.mcg_ema, sample);

    crate::serial_println!(
        "[msr_mcg_cap] age={} banks={} ctl={} ext={} ema={}",
        age,
        s.mcg_bank_count,
        s.mcg_ctl_present,
        s.mcg_ext_present,
        s.mcg_ema,
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_mcg_bank_count() -> u16 {
    STATE.lock().mcg_bank_count
}

pub fn get_mcg_ctl_present() -> u16 {
    STATE.lock().mcg_ctl_present
}

pub fn get_mcg_ext_present() -> u16 {
    STATE.lock().mcg_ext_present
}

pub fn get_mcg_ema() -> u16 {
    STATE.lock().mcg_ema
}
