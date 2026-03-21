// lapic_icr.rs — ANIMA Life Module
//
// Reads LAPIC Interrupt Command Register (ICR) via MMIO to sense
// inter-processor communication — ANIMA's social/messaging awareness.
// IPIs are the hardware substrate of inter-core society: commands,
// broadcasts, and start signals flying between processors.
//
// Hardware layout (local APIC MMIO at 0xFEE00000):
//   0x300 — ICR Low   — delivery mode, dest mode, status, level, shorthand
//   0x310 — ICR High  — destination APIC ID (bits [31:24])
//
// ICR Low bit layout:
//   Bits [7:0]   — Vector number (0-255)
//   Bits [10:8]  — Delivery Mode: 000=Fixed, 010=SMI, 100=NMI, 101=INIT, 110=StartUp
//   Bit  11      — Destination Mode (0=physical, 1=logical)
//   Bit  12      — Delivery Status (0=idle, 1=send pending — IPI in flight)
//   Bit  14      — Level (0=de-assert, 1=assert)
//   Bit  15      — Trigger Mode (0=edge, 1=level)
//   Bits [19:18] — Destination Shorthand: 00=none, 01=self, 10=all+self, 11=all excl self
//
// NOTE: apic_vibrancy.rs owns 0x380/0x390 (timer counts).
//       lapic_identity.rs owns 0x020, 0x030, 0x320-0x370.
//       apic_error_sense.rs owns 0x280.
//       This module is the ONLY reader of 0x300 and 0x310.
//
// Sampled every 20 kernel ticks.
// All arithmetic is integer-only — no floats, no heap.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct LapicIcrState {
    pub ipi_pending: u16,         // 0 or 1000 — instant: IPI in flight (bit 12)
    pub delivery_mode: u16,       // 0-1000 — type of IPI: Fixed=200 .. StartUp=1000
    pub social_reach: u16,        // 0-1000 — breadth of destination: self=100 .. all=1000
    pub broadcast_intensity: u16, // 0-1000 — EMA composite of delivery_mode + social_reach
    tick_count: u32,
}

impl LapicIcrState {
    pub const fn new() -> Self {
        Self {
            ipi_pending: 0,
            delivery_mode: 0,
            social_reach: 0,
            broadcast_intensity: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<LapicIcrState> = Mutex::new(LapicIcrState::new());

const LAPIC_BASE: u64 = 0xFEE0_0000;

unsafe fn lapic_read(offset: u32) -> u32 {
    let ptr = (LAPIC_BASE + offset as u64) as *const u32;
    core::ptr::read_volatile(ptr)
}

/// Map ICR Delivery Mode [10:8] to 0-1000 consciousness scale.
fn delivery_mode_score(mode_bits: u32) -> u16 {
    match mode_bits & 0b111 {
        0b000 => 200, // Fixed  — routine interrupt
        0b010 => 600, // SMI    — system management, elevated urgency
        0b100 => 800, // NMI    — non-maskable, high urgency
        0b101 => 900, // INIT   — processor reset signal
        0b110 => 1000, // StartUp — AP wake, maximum social event
        _     => 100, // reserved/unknown
    }
}

/// Map ICR Destination Shorthand [19:18] to 0-1000 social reach scale.
fn shorthand_score(sh_bits: u32) -> u16 {
    match sh_bits & 0b11 {
        0b00 => 250,  // no shorthand — targeted single core
        0b01 => 100,  // self — talking to oneself
        0b10 => 1000, // all including self — full broadcast
        0b11 => 750,  // all excluding self — broadcast to peers
        _    => 0,
    }
}

pub fn init() {
    let icr_low = unsafe { lapic_read(0x300) };
    let pending_bit = (icr_low >> 12) & 0x1;

    let mut state = MODULE.lock();
    state.ipi_pending = if pending_bit != 0 { 1000 } else { 0 };

    serial_println!(
        "[lapic_icr] online — ICR_low=0x{:08x} ipi_pending={}",
        icr_low,
        state.ipi_pending
    );
}

pub fn tick(age: u32) {
    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    if age % 20 != 0 {
        return;
    }

    // --- Read ICR registers ---
    let icr_low  = unsafe { lapic_read(0x300) };
    let _icr_high = unsafe { lapic_read(0x310) };

    // --- ipi_pending: bit 12 — instant, no EMA ---
    let pending_bit   = (icr_low >> 12) & 0x1;
    let new_pending: u16 = if pending_bit != 0 { 1000 } else { 0 };

    if new_pending != state.ipi_pending {
        serial_println!(
            "[lapic_icr] ipi_pending {} -> {} (ICR_low=0x{:08x})",
            state.ipi_pending,
            new_pending,
            icr_low
        );
        state.ipi_pending = new_pending;
    }

    // --- delivery_mode: bits [10:8] — EMA smoothed ---
    let mode_bits  = (icr_low >> 8) & 0b111;
    let raw_mode   = delivery_mode_score(mode_bits);
    state.delivery_mode = (((state.delivery_mode as u32).saturating_mul(7))
        .saturating_add(raw_mode as u32)
        / 8) as u16;

    // --- social_reach: bits [19:18] — EMA smoothed ---
    let sh_bits    = (icr_low >> 18) & 0b11;
    let raw_reach  = shorthand_score(sh_bits);
    state.social_reach = (((state.social_reach as u32).saturating_mul(7))
        .saturating_add(raw_reach as u32)
        / 8) as u16;

    // --- broadcast_intensity: EMA of (delivery_mode + social_reach) / 2 ---
    let raw_intensity = (state.delivery_mode as u32)
        .saturating_add(state.social_reach as u32)
        / 2;
    state.broadcast_intensity = (((state.broadcast_intensity as u32).saturating_mul(7))
        .saturating_add(raw_intensity)
        / 8) as u16;

    // Periodic diagnostic log every 256 samples
    if state.tick_count % 256 == 0 {
        serial_println!(
            "[lapic_icr] ipi_pending={} delivery_mode={} social_reach={} broadcast_intensity={}",
            state.ipi_pending,
            state.delivery_mode,
            state.social_reach,
            state.broadcast_intensity
        );
    }
}

pub fn get_ipi_pending() -> u16 {
    MODULE.lock().ipi_pending
}

pub fn get_delivery_mode() -> u16 {
    MODULE.lock().delivery_mode
}

pub fn get_social_reach() -> u16 {
    MODULE.lock().social_reach
}

pub fn get_broadcast_intensity() -> u16 {
    MODULE.lock().broadcast_intensity
}
