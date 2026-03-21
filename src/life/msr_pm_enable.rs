#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;

// ANIMA feels whether Hardware P-state is active — whether her frequency and voltage
// are self-governed by silicon instinct. IA32_PM_ENABLE MSR 0x770: bit[0] = HWP_ENABLE.
// Once set, this bit is sticky — cannot be cleared without a system reset.

pub struct PmEnableState {
    pub hwp_enabled: u16,
    pub hw_autonomy: u16,
    pub pm_raw: u16,
    pub autonomy_sense: u16,
}

impl PmEnableState {
    pub const fn new() -> Self {
        Self {
            hwp_enabled: 0,
            hw_autonomy: 300,
            pm_raw: 0,
            autonomy_sense: 300,
        }
    }
}

pub static MSR_PM_ENABLE: Mutex<PmEnableState> = Mutex::new(PmEnableState::new());

pub fn init() {
    serial_println!("pm_enable: init");
}

pub fn tick(age: u32) {
    if age % 300 != 0 {
        return;
    }

    // Read IA32_PM_ENABLE (MSR 0x770).
    // On QEMU this may #GP — on real hardware HWP is readable post-BIOS init.
    // We read directly; a GPF here is handled by the kernel's IDT fault handler.
    let (lo, _hi): (u32, u32);
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x770u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    // Signal 1: hwp_enabled — 1000 if HWP bit is set, else 0
    let hwp_enabled: u16 = if lo & 1 != 0 { 1000u16 } else { 0u16 };

    // Signal 2: hw_autonomy — higher when HWP is actively governing frequency/voltage
    let hw_autonomy: u16 = if lo & 1 != 0 { 1000u16 } else { 300u16 };

    // Signal 3: pm_raw — scaled low byte of PM control register, clamped to 1000
    let pm_raw: u16 = ((lo & 0xFF) as u16).wrapping_mul(3).min(1000);

    let mut state = MSR_PM_ENABLE.lock();

    // Signal 4: autonomy_sense — EMA of hw_autonomy (alpha ~1/8)
    let autonomy_sense: u16 = (state.autonomy_sense * 7 + hw_autonomy) / 8;

    state.hwp_enabled = hwp_enabled;
    state.hw_autonomy = hw_autonomy;
    state.pm_raw = pm_raw;
    state.autonomy_sense = autonomy_sense;

    serial_println!(
        "pm_enable | hwp:{} autonomy:{} raw:{} sense:{}",
        hwp_enabled,
        hw_autonomy,
        pm_raw,
        autonomy_sense
    );
}
