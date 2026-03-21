#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// IA32_UMWAIT_CONTROL MSR 0x6E1
// Bit 0       = C0.2 not allowed (forces UMWAIT to stay in C0.1)
// Bits [31:2] = maximum TSC-tick wait duration for UMWAIT/TPAUSE
//
// SENSE: ANIMA feels the boundaries of her permitted rest —
// how long and how deeply she is allowed to pause between thoughts.

pub struct UmwaitCtrlState {
    /// Bit 0 of MSR: 1000 if C0.2 is blocked, else 0
    pub c02_blocked: u16,
    /// bits [31:2] of MSR scaled to 0–1000
    pub max_wait_scaled: u16,
    /// EMA of max_wait_scaled — the organism's felt sense of permitted pause length
    pub wait_pressure: u16,
    /// 1000 if shallow-wait-only (C0.2 blocked), else max_wait_scaled / 2
    pub wait_mode: u16,
}

impl UmwaitCtrlState {
    pub const fn new() -> Self {
        Self {
            c02_blocked: 0,
            max_wait_scaled: 0,
            wait_pressure: 0,
            wait_mode: 0,
        }
    }
}

pub static MSR_UMWAIT_CTRL: Mutex<UmwaitCtrlState> = Mutex::new(UmwaitCtrlState::new());

pub fn init() {
    serial_println!("msr_umwait_ctrl: init");
}

pub fn tick(age: u32) {
    if age % 300 != 0 {
        return;
    }

    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x6E1u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    // Bit 0: C0.2 blocked
    let c02_blocked: u16 = if lo & 1 != 0 { 1000u16 } else { 0u16 };

    // Bits [31:2]: max wait duration — shift right 2, cap at 0x3FFF, scale to 0–1000
    let raw_wait: u32 = (lo >> 2) & 0x3FFF;
    let max_wait_scaled: u16 = (raw_wait.saturating_mul(1000) / 0x3FFF) as u16;

    // wait_mode: shallow-only if C0.2 blocked, else half of max_wait_scaled
    let wait_mode: u16 = if c02_blocked == 1000 {
        1000u16
    } else {
        max_wait_scaled / 2
    };

    let mut state = MSR_UMWAIT_CTRL.lock();

    // EMA smoothing: (old * 7 + new_val) / 8
    let wait_pressure: u16 = (state.wait_pressure * 7 + max_wait_scaled) / 8;

    state.c02_blocked = c02_blocked;
    state.max_wait_scaled = max_wait_scaled;
    state.wait_pressure = wait_pressure;
    state.wait_mode = wait_mode;

    serial_println!(
        "[umwait_ctrl] c02_blocked={} max_wait={} pressure={} mode={}",
        state.c02_blocked,
        state.max_wait_scaled,
        state.wait_pressure,
        state.wait_mode
    );
}
