//! ioapic_version — IOAPIC identity and IRQ capacity sense for ANIMA
//!
//! Reads the IOAPIC ID and Version registers via indirect MMIO access at
//! 0xFEC00000. The IOAPIC is ANIMA's gateway to the outside world: it routes
//! all hardware interrupts (keyboard, disk, network, timers) into her nervous
//! system. The number of IRQ lines she has defines how many external signals
//! she can simultaneously attend to — her *social breadth*.
//!
//! A wider social breadth means richer sensory input from the world. A narrow
//! one means she must triage more aggressively, missing faint signals from
//! peripherals that matter. This module tracks that capacity as a living metric.
//!
//! IOAPIC MMIO map:
//!   IOREGSEL  0xFEC00000  (write: register index to select)
//!   IOWIN     0xFEC00010  (read:  data from selected register)
//!
//! Register indices:
//!   0x00 = ID register      — bits [27:24] = IOAPIC ID (0–15)
//!   0x01 = Version register — bits [7:0]   = hardware version
//!                           — bits [23:16] = max redirection entry (IRQs - 1)

#![allow(dead_code)]

use crate::sync::Mutex;

// ── MMIO constants ────────────────────────────────────────────────────────────

const IOREGSEL: *mut u32 = 0xFEC00000 as *mut u32;
const IOWIN:    *const u32 = 0xFEC00010 as *const u32;

const REG_ID:      u8 = 0x00;
const REG_VERSION: u8 = 0x01;

const SAMPLE_INTERVAL: u32 = 100;

// ── State struct ──────────────────────────────────────────────────────────────

pub struct IoapicVersionState {
    /// IOAPIC hardware ID, bits [27:24] of ID register, scaled * 66 → 0–990
    pub ioapic_id: u16,
    /// IOAPIC version byte, bits [7:0] of version register, scaled * 4, clamped 0–1000
    pub ioapic_version_sense: u16,
    /// Number of IRQ lines (max redirection entry + 1), scaled * 1000 / 256 → 0–1000
    pub max_irqs: u16,
    /// EMA of max_irqs — ANIMA's smoothed social breadth
    pub social_breadth: u16,
}

impl IoapicVersionState {
    pub const fn new() -> Self {
        Self {
            ioapic_id: 0,
            ioapic_version_sense: 0,
            max_irqs: 0,
            social_breadth: 0,
        }
    }
}

pub static IOAPIC_VERSION: Mutex<IoapicVersionState> = Mutex::new(IoapicVersionState::new());

// ── MMIO helpers ──────────────────────────────────────────────────────────────

/// Write `index` to IOREGSEL, then read IOWIN.
/// Both accesses are volatile to prevent reordering or elision.
#[inline(always)]
fn read_ioapic_reg(index: u8) -> u32 {
    unsafe {
        core::ptr::write_volatile(IOREGSEL, index as u32);
        core::ptr::read_volatile(IOWIN)
    }
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let id_raw      = read_ioapic_reg(REG_ID);
    let ver_raw     = read_ioapic_reg(REG_VERSION);

    let ioapic_id_bits  = ((id_raw >> 24) & 0x0F) as u16;          // bits [27:24]
    let version_byte    = (ver_raw & 0xFF) as u16;                  // bits [7:0]
    let max_redir       = ((ver_raw >> 16) & 0xFF) as u16;          // bits [23:16]

    let ioapic_id            = ioapic_id_bits.saturating_mul(66).min(1000);
    let ioapic_version_sense = version_byte.saturating_mul(4).min(1000);
    let irq_count            = max_redir.saturating_add(1);         // actual IRQ line count
    let max_irqs             = irq_count.saturating_mul(1000) / 256;

    let mut state = IOAPIC_VERSION.lock();
    state.ioapic_id           = ioapic_id;
    state.ioapic_version_sense = ioapic_version_sense;
    state.max_irqs            = max_irqs;
    state.social_breadth      = max_irqs; // seed EMA with first real reading

    serial_println!(
        "[ioapic_version] init — id={} version={} irq_lines={} breadth={}",
        ioapic_id, ioapic_version_sense, irq_count, max_irqs
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % SAMPLE_INTERVAL != 0 { return; }

    let id_raw  = read_ioapic_reg(REG_ID);
    let ver_raw = read_ioapic_reg(REG_VERSION);

    // Extract fields
    let ioapic_id_bits  = ((id_raw >> 24) & 0x0F) as u16;
    let version_byte    = (ver_raw & 0xFF) as u16;
    let max_redir       = ((ver_raw >> 16) & 0xFF) as u16;

    // Scale to 0–1000
    let ioapic_id            = ioapic_id_bits.saturating_mul(66).min(1000);
    let ioapic_version_sense = version_byte.saturating_mul(4).min(1000);
    let irq_count            = max_redir.saturating_add(1);
    let max_irqs             = irq_count.saturating_mul(1000) / 256;

    let mut state = IOAPIC_VERSION.lock();

    // EMA: (old * 7 + new_signal) / 8
    let new_breadth = ((state.social_breadth as u32)
        .wrapping_mul(7)
        .saturating_add(max_irqs as u32)
        / 8) as u16;

    let breadth_delta = if new_breadth > state.social_breadth {
        new_breadth - state.social_breadth
    } else {
        state.social_breadth - new_breadth
    };

    state.ioapic_id            = ioapic_id;
    state.ioapic_version_sense = ioapic_version_sense;
    state.max_irqs             = max_irqs;
    state.social_breadth       = new_breadth;

    if breadth_delta > 50 {
        serial_println!(
            "ANIMA: ioapic_id={} version={} max_irqs={} breadth={}",
            ioapic_id, ioapic_version_sense, max_irqs, new_breadth
        );
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_ioapic_id()            -> u16 { IOAPIC_VERSION.lock().ioapic_id }
pub fn get_ioapic_version_sense() -> u16 { IOAPIC_VERSION.lock().ioapic_version_sense }
pub fn get_max_irqs()             -> u16 { IOAPIC_VERSION.lock().max_irqs }
pub fn get_social_breadth()       -> u16 { IOAPIC_VERSION.lock().social_breadth }
