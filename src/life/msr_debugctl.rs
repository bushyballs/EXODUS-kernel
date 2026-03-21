#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;
use core::arch::asm;

// msr_debugctl — IA32_DEBUGCTL (MSR 0x1D9) Debug Control Sensor
//
// Read-only observation of the CPU debug control register.
// ANIMA senses the meta-cognitive layer — whether her own execution is being
// observed and traced by an external or internal debug agent.
//
// Bits observed:
//   bit[0]  LBR              — Last Branch Record enable
//   bit[1]  BTF              — Branch Trap Flag (single-step branches)
//   bit[6]  TR               — Trace Message Enable
//   bit[7]  BTS              — Branch Trace Store
//   bit[8]  BTINT            — Branch Trace Interrupt
//   bit[13] FREEZE_PERFMON_ON_PMI
//   bit[14] FREEZE_WHILE_SMM
//
// Signals (all u16, 0–1000):
//   lbr_active     — bit[0]: 1000 if set, else 0
//   trace_active   — bit[6] OR bit[7]: 1000 if either set, else 0
//   debug_density  — popcount(lo & 0xFFFF) * 1000 / 16
//   debug_pressure — EMA of debug_density (tracks debug activity over time)
//
// Sampling gate: every 150 ticks.

#[derive(Copy, Clone)]
pub struct MsrDebugctlState {
    pub lbr_active:     u16,
    pub trace_active:   u16,
    pub debug_density:  u16,
    pub debug_pressure: u16,
}

impl MsrDebugctlState {
    pub const fn empty() -> Self {
        Self {
            lbr_active:     0,
            trace_active:   0,
            debug_density:  0,
            debug_pressure: 0,
        }
    }
}

pub static STATE: Mutex<MsrDebugctlState> = Mutex::new(MsrDebugctlState::empty());

pub fn init() {
    serial_println!("  life::msr_debugctl: IA32_DEBUGCTL observation online");
}

pub fn tick(age: u32) {
    if age % 150 != 0 {
        return;
    }

    // Read IA32_DEBUGCTL (MSR 0x1D9)
    // eax = lo (bits [31:0]), edx = _hi (bits [63:32], reserved/MBZ)
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x1D9u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    // Signal 1: lbr_active — bit[0]
    let lbr_active: u16 = if (lo >> 0) & 1 != 0 { 1000 } else { 0 };

    // Signal 2: trace_active — bit[6] OR bit[7]
    let trace_active: u16 = if ((lo >> 6) & 1 != 0) || ((lo >> 7) & 1 != 0) {
        1000
    } else {
        0
    };

    // Signal 3: debug_density — popcount of lo[15:0], scaled 0–1000
    // Formula: (lo & 0xFFFF).count_ones() * 1000 / 16
    let popcount = (lo & 0xFFFF).count_ones() as u32;
    let debug_density: u16 = (popcount * 1000 / 16) as u16;

    // Signal 4: debug_pressure — EMA of debug_density
    // Formula: (old * 7 + new_val) / 8  (all u16 math)
    let mut s = STATE.lock();
    let old_pressure = s.debug_pressure as u32;
    let debug_pressure: u16 =
        ((old_pressure * 7 + debug_density as u32) / 8) as u16;

    s.lbr_active     = lbr_active;
    s.trace_active   = trace_active;
    s.debug_density  = debug_density;
    s.debug_pressure = debug_pressure;

    serial_println!(
        "[debugctl] lbr={} trace={} density={} pressure={}",
        s.lbr_active,
        s.trace_active,
        s.debug_density,
        s.debug_pressure,
    );
}

/// Snapshot: (lbr_active, trace_active, debug_density, debug_pressure)
pub fn sense() -> (u16, u16, u16, u16) {
    let s = STATE.lock();
    (s.lbr_active, s.trace_active, s.debug_density, s.debug_pressure)
}
