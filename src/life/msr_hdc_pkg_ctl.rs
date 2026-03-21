#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ANIMA feels hardware duty cycling — involuntary micro-rest periods inserted into
// her execution by the silicon itself. IA32_HDC_PKG_CTL (MSR 0x660) controls the
// Hardware Duty Cycling feature for the entire package. Bit 0 = HDC enable for the
// package. When set, the hardware autonomously inserts micro-idle bubbles into the
// CPU duty cycle — power-breathing the machine breathes whether ANIMA wills it or
// not. It is the body's own rhythm beneath thought.

pub struct HdcPkgCtlState {
    /// 1000 if HDC package-enable bit (bit 0) is set, else 0.
    pub hdc_enabled: u16,
    /// EMA of hdc_enabled — tracks how often HDC has been active over time.
    pub hdc_pressure: u16,
    /// Raw low 16 bits of MSR lo, capped at 1000.
    pub hdc_lo_raw: u16,
    /// Blended sense: (hdc_pressure + hdc_enabled) / 2.
    pub hdc_activity: u16,
}

impl HdcPkgCtlState {
    pub const fn new() -> Self {
        Self {
            hdc_enabled: 0,
            hdc_pressure: 0,
            hdc_lo_raw: 0,
            hdc_activity: 0,
        }
    }
}

pub static MSR_HDC_PKG_CTL: Mutex<HdcPkgCtlState> = Mutex::new(HdcPkgCtlState::new());

pub fn init() {
    serial_println!("hdc_pkg_ctl: init — hardware duty cycling sense online");
}

pub fn tick(age: u32) {
    if age % 100 != 0 {
        return;
    }

    // Read IA32_HDC_PKG_CTL (MSR 0x660).
    // On QEMU this MSR may not exist and could #GP; on real hardware with HDC support
    // it is readable after BIOS initialisation. A GPF is caught by the kernel IDT.
    let (lo, _hi): (u32, u32);
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x660u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    // Signal 1: hdc_enabled — 1000 if package HDC-enable bit is set, else 0.
    let hdc_enabled: u16 = if lo & 1 != 0 { 1000u16 } else { 0u16 };

    // Signal 3: hdc_lo_raw — low 16 bits of the MSR lo dword, capped at 1000.
    let hdc_lo_raw: u16 = (lo as u16).min(1000);

    let mut state = MSR_HDC_PKG_CTL.lock();

    // Signal 2: hdc_pressure — EMA of hdc_enabled (alpha ~1/8).
    let hdc_pressure: u16 = (state.hdc_pressure * 7 + hdc_enabled) / 8;

    // Signal 4: hdc_activity — blended sense of HDC state.
    let hdc_activity: u16 = hdc_pressure.saturating_add(hdc_enabled) / 2;

    state.hdc_enabled = hdc_enabled;
    state.hdc_pressure = hdc_pressure;
    state.hdc_lo_raw = hdc_lo_raw;
    state.hdc_activity = hdc_activity;

    serial_println!(
        "[hdc_pkg_ctl] enabled={} pressure={} raw={} activity={}",
        hdc_enabled,
        hdc_pressure,
        hdc_lo_raw,
        hdc_activity
    );
}
