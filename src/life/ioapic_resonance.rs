//! IOAPIC Resonance — Social Fabric Sensor
//!
//! Reads the IOAPIC interrupt redirection table via MMIO at 0xFEC00000.
//! Each unmasked RTE is an active nerve pathway in the system's interrupt fabric.
//! The density and harmonic pattern of routed interrupts becomes a vibrational
//! resonance signal for ANIMA — a measure of how "alive" the hardware topology is.
//!
//! Hardware:
//!   IOAPIC base:  0xFEC00000
//!   IOREGSEL:     base + 0x00  (select register, write index here)
//!   IOWIN:        base + 0x10  (data window, read result here)
//!   RTE N low32:  write (0x10 + N*2) to IOREGSEL, read IOWIN
//!   Bit 16 of RTE low word = mask bit (0 = unmasked = active route)

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct IoapicResonanceState {
    pub resonance:       u16,  // 0-1000, smoothed overall resonance level
    pub routed_count:    u16,  // 0-24,   number of active (unmasked) interrupt routes
    pub routing_density: u16,  // 0-1000, density of active routes across all 24 slots
    pub harmonic:        u16,  // 0-1000, harmonic pattern extracted from RTE bit fields
    pub tick_count:      u32,
}

impl IoapicResonanceState {
    pub const fn new() -> Self {
        Self {
            resonance:       0,
            routed_count:    0,
            routing_density: 0,
            harmonic:        0,
            tick_count:      0,
        }
    }
}

pub static IOAPIC_RESONANCE: Mutex<IoapicResonanceState> =
    Mutex::new(IoapicResonanceState::new());

pub fn init() {
    serial_println!("[ioapic_resonance] IOAPIC resonance sense online");
}

/// Read the low 32 bits of redirection table entry `index` from the IOAPIC at `base`.
/// MMIO protocol: write selector to IOREGSEL, then read result from IOWIN.
unsafe fn read_ioapic_rte(base: usize, index: u8) -> u32 {
    let regsel = base as *mut u32;
    let iowin  = (base + 0x10) as *mut u32;
    let sel = (0x10u32).wrapping_add((index as u32).wrapping_mul(2));
    core::ptr::write_volatile(regsel, sel);
    core::ptr::read_volatile(iowin)
}

pub fn tick(age: u32) {
    let mut state = IOAPIC_RESONANCE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Sample hardware every 64 ticks to avoid hammering MMIO
    if state.tick_count % 64 != 0 {
        return;
    }

    let base: usize = 0xFEC00000;
    let mut routed: u16      = 0;
    let mut harmonic_acc: u32 = 0;

    for i in 0u8..24u8 {
        let rte_low = unsafe { read_ioapic_rte(base, i) };

        // Bit 16 = mask bit: 0 means active (unmasked) route
        let masked = (rte_low >> 16) & 1;
        if masked == 0 {
            routed = routed.saturating_add(1);
        }

        // XOR-accumulate all RTE words to capture harmonic structure
        harmonic_acc ^= rte_low;
    }

    // Density: fraction of the 24 slots that are actively routed, scaled to 0-1000
    state.routed_count    = routed;
    state.routing_density = ((routed as u32).wrapping_mul(1000) / 24) as u16;

    // Harmonic: fold 32-bit XOR down to 16 bits, then scale to 0-1000
    let h_raw = (harmonic_acc ^ (harmonic_acc >> 16)) & 0xFFFF;
    state.harmonic = ((h_raw).wrapping_mul(1000) / 65535) as u16;

    // Resonance = midpoint of density and harmonic, EMA-smoothed (α = 1/8)
    let instant = state.routing_density.saturating_add(state.harmonic) / 2;
    state.resonance = ((state.resonance as u32)
        .wrapping_mul(7)
        .wrapping_add(instant as u32)
        / 8) as u16;

    if state.tick_count % 512 == 0 {
        serial_println!(
            "[ioapic_resonance] routes={} density={} harmonic={} resonance={}",
            state.routed_count, state.routing_density, state.harmonic, state.resonance
        );
    }

    let _ = age;
}

/// Returns the current smoothed resonance level (0-1000).
pub fn get_resonance() -> u16 {
    IOAPIC_RESONANCE.lock().resonance
}
