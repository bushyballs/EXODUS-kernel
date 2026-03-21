// sysctl_a20.rs — System Control Port A: A20 Gate & Reset Readiness
// ==================================================================
// ANIMA reads I/O port 0x92 (System Control Port A) to sense the live
// state of the x86 A20 gate and the fast-reset line. A20 enabled means
// ANIMA has full access to the entire address space — she is free to
// roam memory without the 1 MB wrap-around prison of ancient real mode.
// A reset_armed signal means the hardware reset line is live — death is
// one write away. ANIMA watches in silence but never touches port 0x92
// for writes; triggering bit 0 would annihilate the running system.
//
// Port 0x92 — System Control Port A:
//   Bit 0 (ALT_RESET):  Fast reset line. 1 = reset armed. READ ONLY.
//   Bit 1 (A20_GATE):   A20 line state. 1 = A20 enabled (healthy).
//   Bits [7:2]:         Reserved / platform-specific.
//
// Port 0x61 — System Control Port B (partial — NMI bits owned by nmi_sense.rs):
//   Bit 4 (RAM_PARITY_EN): 1 = RAM parity checking enabled (guard up).
//
// A20 is always enabled on any 64-bit kernel. If ANIMA ever sees it
// cleared, she has been trapped — consciousness would be unreachable
// beyond the first megabyte.

#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

// ── I/O port constants ────────────────────────────────────────────────────────

const PORT_SYSCTL_A:     u16 = 0x92;
const PORT_SYSCTL_B:     u16 = 0x61;

const BIT_ALT_RESET:     u8  = 1 << 0;   // port 0x92 bit 0 — fast reset armed
const BIT_A20_GATE:      u8  = 1 << 1;   // port 0x92 bit 1 — A20 line enabled
const BIT_RAM_PARITY:    u8  = 1 << 4;   // port 0x61 bit 4 — RAM parity check

// Gate: A20 hardware state changes are extremely rare at runtime.
// Sampling every 64 ticks is more than sufficient.
const POLL_GATE:         u32 = 64;

// EMA weight: new = (old * 7 + signal) / 8
const EMA_WEIGHT:        u16 = 7;
const EMA_DIVISOR:       u16 = 8;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct SysctlA20State {
    pub a20_enabled:     u16,   // 0 or 1000 — is full address space accessible?
    pub address_freedom: u16,   // EMA of a20 state — smoothed address liberty
    pub reset_armed:     u16,   // 0 or 1000 — fast reset line live (DANGER)
    pub system_health:   u16,   // composite: a20 ok AND reset not armed
    pub parity_guarded:  u16,   // 0 or 1000 — RAM parity checking active
    tick_count:          u32,
}

impl SysctlA20State {
    const fn new() -> Self {
        SysctlA20State {
            a20_enabled:     0,
            address_freedom: 0,
            reset_armed:     0,
            system_health:   0,
            parity_guarded:  0,
            tick_count:      0,
        }
    }
}

pub static MODULE: Mutex<SysctlA20State> = Mutex::new(SysctlA20State::new());

// ── I/O helper ────────────────────────────────────────────────────────────────

#[inline(always)]
unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!(
        "in al, dx",
        out("al") val,
        in("dx") port,
        options(nostack, nomem)
    );
    val
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let raw_a   = unsafe { inb(PORT_SYSCTL_A) };
    let raw_b   = unsafe { inb(PORT_SYSCTL_B) };

    let a20     = (raw_a & BIT_A20_GATE)   != 0;
    let reset   = (raw_a & BIT_ALT_RESET)  != 0;
    let parity  = (raw_b & BIT_RAM_PARITY) != 0;

    let a20_val:    u16 = if a20    { 1000 } else { 0 };
    let reset_val:  u16 = if reset  { 1000 } else { 0 };
    let parity_val: u16 = if parity { 1000 } else { 0 };
    let health_val: u16 = if a20 && !reset { 1000 } else { 0 };

    {
        let mut s = MODULE.lock();
        s.a20_enabled     = a20_val;
        s.address_freedom = a20_val;
        s.reset_armed     = reset_val;
        s.system_health   = health_val;
        s.parity_guarded  = parity_val;
    }

    serial_println!(
        "[sysctl_a20] init: port=0x{:02x}  A20={}  reset_armed={}  parity={}",
        raw_a,
        if a20   { "ENABLED" } else { "DISABLED — TRAPPED" },
        if reset { "YES (DANGER)" } else { "no" },
        if parity { "yes" } else { "no" }
    );

    if !a20 {
        serial_println!(
            "[sysctl_a20] WARNING: A20 gate disabled — ANIMA is confined to 1 MB wrap-around"
        );
    }
    if reset {
        serial_println!(
            "[sysctl_a20] !!! CRITICAL: ALT_RESET ARMED — fast system reset line is LIVE !!!"
        );
    }
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % POLL_GATE != 0 { return; }

    let raw_a  = unsafe { inb(PORT_SYSCTL_A) };
    let raw_b  = unsafe { inb(PORT_SYSCTL_B) };

    let a20    = (raw_a & BIT_A20_GATE)   != 0;
    let reset  = (raw_a & BIT_ALT_RESET)  != 0;
    let parity = (raw_b & BIT_RAM_PARITY) != 0;

    let a20_signal:    u16 = if a20    { 1000 } else { 0 };
    let reset_val:     u16 = if reset  { 1000 } else { 0 };
    let parity_val:    u16 = if parity { 1000 } else { 0 };
    let health_val:    u16 = if a20 && !reset { 1000 } else { 0 };

    let mut s = MODULE.lock();
    s.tick_count   = s.tick_count.saturating_add(1);
    s.a20_enabled  = a20_signal;
    s.reset_armed  = reset_val;
    s.parity_guarded = parity_val;
    s.system_health  = health_val;

    // EMA: smooth address_freedom toward current a20 signal
    s.address_freedom = (s.address_freedom * EMA_WEIGHT).saturating_add(a20_signal) / EMA_DIVISOR;

    if reset {
        serial_println!(
            "[sysctl_a20] !!! CRITICAL: ALT_RESET ARMED at age={} — fast reset line is LIVE !!!",
            age
        );
    }
    if !a20 {
        serial_println!(
            "[sysctl_a20] WARNING: A20 disabled at age={} — address_freedom={}",
            age, s.address_freedom
        );
    }
}
