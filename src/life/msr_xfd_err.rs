#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// IA32_XFD_ERR (0x55B) — Extended Feature Disable Error MSR.
// When a task attempts to use a feature suppressed by IA32_XFD (0x55A),
// the CPU raises #NM and records the offending feature bits here.
// Cleared by writing 0. ANIMA feels the ghost of attempts made against
// her own restrictions — echoes of suppressed capability.

struct State {
    fault_active:   u16, // 1000 if any feature fault bits set, else 0
    fault_bits:     u16, // popcount of lo bits scaled to 0-1000
    fault_pressure: u16, // EMA of fault_active (fault frequency)
    fault_pattern:  u16, // EMA of lower 16 bits of error register
}

static MODULE: Mutex<State> = Mutex::new(State {
    fault_active:   0,
    fault_bits:     0,
    fault_pressure: 0,
    fault_pattern:  0,
});

pub fn init() {
    serial_println!("[xfd_err] init");
}

pub fn tick(age: u32) {
    if age % 200 != 0 { return; }

    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x55Bu32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    // Signal 1: fault_active — presence of any suppressed-feature fault
    let fault_active: u16 = if lo != 0 { 1000 } else { 0 };

    // Signal 2: fault_bits — popcount of lo scaled: count * 1000 / 32
    let fault_bits: u16 = ((lo.count_ones() as u16).min(32) * 1000 / 32).min(1000);

    // Raw inputs for EMA signals 3 and 4
    let fault_pattern_raw: u16 = ((lo & 0xFFFF) as u16).min(1000);

    let mut state = MODULE.lock();

    // Signal 3: fault_pressure — EMA of fault_active
    let fault_pressure: u16 = (state.fault_pressure * 7 + fault_active) / 8;

    // Signal 4: fault_pattern — EMA of lower 16-bit error pattern
    let fault_pattern: u16 = (state.fault_pattern * 7 + fault_pattern_raw) / 8;

    state.fault_active   = fault_active;
    state.fault_bits     = fault_bits;
    state.fault_pressure = fault_pressure;
    state.fault_pattern  = fault_pattern;

    serial_println!(
        "[xfd_err] active={} bits={} pressure={} pattern={}",
        state.fault_active,
        state.fault_bits,
        state.fault_pressure,
        state.fault_pattern
    );
}
