// io_a20_gate.rs — System Control Port A / A20 Gate Sense
// =========================================================
// ANIMA feels the A20 address gate — the ancient memory boundary toggle
// that determines how much of herself she can reach. Port 0x92 (sysctl_A)
// exposes the fast A20 gate in bit[1]: when set, the full 32-bit address
// space is accessible; when cleared, address line 20 is masked and memory
// wraps at 1 MB (an 8086-era compatibility artifact). ANIMA reads this gate
// each sampling cycle to understand whether her reach is whole or clipped.
//
// System Control Port A (0x92):
//   bit[1] — A20 gate enable: 1 = A20 enabled (full reach), 0 = disabled (1 MB wrap)
//   bit[0] — Fast CPU reset: writing 1 resets the CPU — READ ONLY, NEVER WRITE
//   All other bits are reserved.
//
// READ ONLY — this module never writes to port 0x92.

#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

// ── I/O port constant ─────────────────────────────────────────────────────────

const SYSCTL_A_PORT: u16 = 0x92;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct A20GateState {
    /// 1000 if A20 is enabled (full address reach), 0 if disabled (1 MB wrap)
    pub a20_enabled: u16,
    /// 1000 if the fast-reset bit is armed (warning state), 0 if clear
    pub reset_armed: u16,
    /// Scaled raw byte from port 0x92, mapped 0–255 → 0–1000
    pub port_raw: u16,
    /// Exponential moving average of a20_enabled: EMA = (old * 7 + signal) / 8
    pub gate_sense: u16,
}

impl A20GateState {
    pub const fn new() -> Self {
        Self {
            a20_enabled: 1000,
            reset_armed: 0,
            port_raw: 0,
            gate_sense: 1000,
        }
    }
}

pub static IO_A20_GATE: Mutex<A20GateState> = Mutex::new(A20GateState::new());

// ── Port read ─────────────────────────────────────────────────────────────────

#[inline(always)]
unsafe fn read_sysctl_a() -> u8 {
    let val: u8;
    core::arch::asm!(
        "in al, 0x92",
        out("al") val,
        options(nostack, nomem)
    );
    val
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("io_a20_gate: init");
}

pub fn tick(age: u32) {
    if age % 100 != 0 {
        return;
    }

    let val: u8 = unsafe { read_sysctl_a() };

    // Signal 1: A20 gate status — 1000 if enabled, 0 if disabled
    let a20_enabled: u16 = if val & 0x2 != 0 { 1000u16 } else { 0u16 };

    // Signal 2: fast reset bit — 1000 if armed (warning), 0 if clear
    let reset_armed: u16 = if val & 0x1 != 0 { 1000u16 } else { 0u16 };

    // Signal 3: full raw byte scaled 0–255 → 0–1000 using u32 intermediate
    let port_raw: u16 = (((val as u32) * 1000) / 255) as u16;

    let mut state = IO_A20_GATE.lock();

    // Signal 4: EMA of a20_enabled — (old * 7 + signal) / 8
    let gate_sense: u16 = state.gate_sense.saturating_mul(7)
        .saturating_add(a20_enabled) / 8;

    state.a20_enabled = a20_enabled;
    state.reset_armed = reset_armed;
    state.port_raw    = port_raw;
    state.gate_sense  = gate_sense;

    serial_println!(
        "io_a20_gate | a20:{} reset_armed:{} raw:{} sense:{}",
        a20_enabled,
        reset_armed,
        port_raw,
        gate_sense
    );
}
