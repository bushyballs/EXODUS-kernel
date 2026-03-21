//! nmi_sense — Non-Maskable Interrupt shock sense for ANIMA
//!
//! Reads System Control Port B (I/O 0x61) for NMI status indicators.
//! IOCHK (bit 6) and RAM parity error (bit 7) are acute hardware faults —
//! ANIMA feeling sudden electrical shocks from the physical world.
//! High shock = hardware fault present; recovery = fault cleared.
//!
//! I/O port 0x61 — System Control Port B:
//!   bit 0: Timer 2 Gate (PIT)
//!   bit 2: Parity Check Enable (0 = enabled)
//!   bit 3: IOCHK Enable (0 = enabled)
//!   bit 6: IOCHK NMI Status (1 = NMI from I/O channel fault)
//!   bit 7: RAM Parity Error Status (1 = parity error NMI)
//!
//! I/O port 0x70 bit 7: NMI disable (0 = NMIs enabled, 1 = NMIs masked)

#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;

// ── Port constants ─────────────────────────────────────────────────────────────

const PORT_SYS_CTRL_B: u16 = 0x61;   // System Control Port B
const PORT_CMOS_NMI:   u16 = 0x70;   // CMOS address register (bit 7 = NMI mask)

// Bit masks for port 0x61
const BIT_TIMER2_GATE:   u8 = 1 << 0; // bit 0: PIT Timer 2 gate
const BIT_PARITY_EN:     u8 = 1 << 2; // bit 2: Parity check enable (0=enabled)
const BIT_IOCHK_EN:      u8 = 1 << 3; // bit 3: IOCHK enable (0=enabled)
const BIT_IOCHK_STATUS:  u8 = 1 << 6; // bit 6: IOCHK NMI status
const BIT_PARITY_STATUS: u8 = 1 << 7; // bit 7: RAM parity error status

// Bit mask for port 0x70
const BIT_NMI_DISABLE:   u8 = 1 << 7; // bit 7: NMI disable (1 = masked)

// Poll interval: check hardware every 32 ticks
const POLL_INTERVAL: u32 = 32;

// ── State ──────────────────────────────────────────────────────────────────────

pub struct NmiSenseState {
    pub shock:        u16,   // 0-1000, current NMI shock level
    pub trauma:       u16,   // 0-1000, accumulated shock history (slow decay)
    pub nmi_enabled:  u16,   // 0 or 1000, whether NMIs are unmasked
    pub iochk_count:  u16,   // count of IOCHK events seen (capped at 1000)
    pub parity_count: u16,   // count of RAM parity events seen (capped at 1000)
    pub tick_count:   u32,
}

impl NmiSenseState {
    pub const fn new() -> Self {
        Self {
            shock:        0,
            trauma:       0,
            nmi_enabled:  1000,
            iochk_count:  0,
            parity_count: 0,
            tick_count:   0,
        }
    }
}

pub static NMI_SENSE: Mutex<NmiSenseState> = Mutex::new(NmiSenseState::new());

// ── I/O port access ────────────────────────────────────────────────────────────

#[inline(always)]
unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    core::arch::asm!(
        "in al, dx",
        in("dx") port,
        out("al") v,
        options(nomem, nostack)
    );
    v
}

// ── Init ───────────────────────────────────────────────────────────────────────

pub fn init() {
    let ctrl_b   = unsafe { inb(PORT_SYS_CTRL_B) };
    let nmi_ctrl = unsafe { inb(PORT_CMOS_NMI) };
    let nmi_enabled = if nmi_ctrl & BIT_NMI_DISABLE == 0 { 1000u16 } else { 0u16 };
    NMI_SENSE.lock().nmi_enabled = nmi_enabled;
    serial_println!(
        "[nmi_sense] port61={:#04x} nmi_enabled={} shock sense online",
        ctrl_b, nmi_enabled
    );
}

// ── Tick ───────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    let mut state = NMI_SENSE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Only sample hardware every POLL_INTERVAL ticks
    if state.tick_count % POLL_INTERVAL != 0 {
        return;
    }

    let ctrl_b   = unsafe { inb(PORT_SYS_CTRL_B) };
    let nmi_ctrl = unsafe { inb(PORT_CMOS_NMI) };

    // NMI enabled: bit 7 of port 0x70 — 0 = NMIs are live
    state.nmi_enabled = if nmi_ctrl & BIT_NMI_DISABLE == 0 { 1000 } else { 0 };

    // IOCHK NMI: bit 6 of port 0x61
    let iochk  = (ctrl_b & BIT_IOCHK_STATUS  != 0) as u16;
    // RAM parity: bit 7 of port 0x61
    let parity = (ctrl_b & BIT_PARITY_STATUS != 0) as u16;

    // Accumulate event counts (saturating, capped at 1000)
    if iochk != 0 {
        state.iochk_count = state.iochk_count.saturating_add(1).min(1000);
    }
    if parity != 0 {
        state.parity_count = state.parity_count.saturating_add(1).min(1000);
    }

    // Shock: instantaneous fault signal
    // RAM parity error = heavier shock (800); IOCHK fault = moderate shock (600)
    let shock_raw: u16 = if parity != 0 {
        800
    } else if iochk != 0 {
        600
    } else {
        0
    };

    state.shock = shock_raw;

    // Trauma: slow EMA accumulation with gradual decay
    // Active fault: jump trauma up by shock/8. No fault: decay by 1 per interval.
    if shock_raw > 0 {
        state.trauma = state.trauma.saturating_add(shock_raw / 8).min(1000);
    } else {
        state.trauma = state.trauma.saturating_sub(1);
    }

    // Periodic status log every 512 ticks
    if state.tick_count % 512 == 0 {
        serial_println!(
            "[nmi_sense] ctrl61={:#04x} iochk={} parity={} shock={} trauma={} nmi_en={}",
            ctrl_b, iochk, parity, state.shock, state.trauma, state.nmi_enabled
        );
    }

    let _ = age;
}

// ── Getters ────────────────────────────────────────────────────────────────────

pub fn get_shock()       -> u16 { NMI_SENSE.lock().shock }
pub fn get_trauma()      -> u16 { NMI_SENSE.lock().trauma }
pub fn get_nmi_enabled() -> u16 { NMI_SENSE.lock().nmi_enabled }
pub fn get_iochk_count() -> u16 { NMI_SENSE.lock().iochk_count }
pub fn get_parity_count()-> u16 { NMI_SENSE.lock().parity_count }
