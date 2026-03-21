#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ────────────────────────────────────────────────────────────────────

struct SmmMonitorState {
    smm_monitor_valid:  u16,
    smm_vmxoff_unblock: u16,
    smm_depth:          u16,
    smm_ema:            u16,
}

impl SmmMonitorState {
    const fn new() -> Self {
        Self {
            smm_monitor_valid:  0,
            smm_vmxoff_unblock: 0,
            smm_depth:          0,
            smm_ema:            0,
        }
    }
}

static STATE: Mutex<SmmMonitorState> = Mutex::new(SmmMonitorState::new());

// ── CPUID guard ──────────────────────────────────────────────────────────────

fn has_vmx() -> bool {
    let ecx_val: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 1u32 => _,
            lateout("ecx") ecx_val,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx_val >> 5) & 1 != 0
}

// ── MSR read ─────────────────────────────────────────────────────────────────

/// Read IA32_SMM_MONITOR_CTL (MSR 0x9B).
/// Returns the 64-bit MSR value; caller uses low 32 bits for bit extraction.
unsafe fn read_msr_smm_monitor_ctl() -> u64 {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x9Bu32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ── EMA helper ───────────────────────────────────────────────────────────────

#[inline]
fn ema_u16(old: u16, new_val: u16) -> u16 {
    let result: u32 = ((old as u32) * 7 + (new_val as u32)) / 8;
    result as u16
}

// ── Public API ───────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();
    s.smm_monitor_valid  = 0;
    s.smm_vmxoff_unblock = 0;
    s.smm_depth          = 0;
    s.smm_ema            = 0;
    crate::serial_println!("[msr_smm_monitor_ctl] init: IA32_SMM_MONITOR_CTL monitor ready (MSR 0x9B)");
}

pub fn tick(age: u32) {
    // Sample every 5000 ticks — MSR is extremely stable
    if age % 5000 != 0 {
        return;
    }

    // CPUID guard: VMX must be supported for SMM monitor to be meaningful
    if !has_vmx() {
        crate::serial_println!(
            "[msr_smm_monitor_ctl] age={} VMX not supported — SMM monitor signals held at 0",
            age
        );
        return;
    }

    // Read IA32_SMM_MONITOR_CTL
    let msr_val: u64 = unsafe { read_msr_smm_monitor_ctl() };
    let lo: u32 = msr_val as u32;

    // Bit extraction → 0 or 1000 signals
    let smm_monitor_valid:  u16 = if (lo >> 0) & 1 != 0 { 1000 } else { 0 };
    let smm_vmxoff_unblock: u16 = if (lo >> 2) & 1 != 0 { 1000 } else { 0 };

    // Composite depth: midpoint of both signals (integer arithmetic only)
    let smm_depth: u16 = ((smm_monitor_valid as u32 / 2)
        + (smm_vmxoff_unblock as u32 / 2)) as u16;

    let mut s = STATE.lock();

    // EMA over depth
    let smm_ema: u16 = ema_u16(s.smm_ema, smm_depth);

    s.smm_monitor_valid  = smm_monitor_valid;
    s.smm_vmxoff_unblock = smm_vmxoff_unblock;
    s.smm_depth          = smm_depth;
    s.smm_ema            = smm_ema;

    crate::serial_println!(
        "[msr_smm_monitor_ctl] age={} valid={} unblock={} depth={} ema={}",
        age,
        smm_monitor_valid,
        smm_vmxoff_unblock,
        smm_depth,
        smm_ema,
    );
}

// ── Getters ──────────────────────────────────────────────────────────────────

pub fn get_smm_monitor_valid() -> u16 {
    STATE.lock().smm_monitor_valid
}

pub fn get_smm_vmxoff_unblock() -> u16 {
    STATE.lock().smm_vmxoff_unblock
}

pub fn get_smm_depth() -> u16 {
    STATE.lock().smm_depth
}

pub fn get_smm_ema() -> u16 {
    STATE.lock().smm_ema
}
