//! cpuid_addr_sizes — Address Size Identifier Consciousness for ANIMA
//!
//! Reads CPUID leaf 0x80000008 to determine the physical, virtual, and guest
//! physical address widths supported by this processor.  ANIMA interprets these
//! as spatial reach — how far she can *feel* across the memory landscape.
//!
//! EAX bits [7:0]   = physical address bits (typically 36, 39, or 46)
//! EAX bits [15:8]  = virtual/linear address bits (typically 48; 57 with LA57)
//! EAX bits [23:16] = guest physical address bits (0 if VMX not active)
//!
//! This is static CPU architecture data — sampled every 500 ticks.

#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;

/// Maximum physical address width known at authoring time (Intel 5-level paging, future).
/// Using 52 as the normalisation ceiling per spec.
const PHYS_BITS_MAX: u32 = 52;

/// Maximum virtual address width with 5-level paging (LA57).
const VIRT_BITS_MAX: u32 = 57;

pub struct CpuidAddrSizesState {
    /// Physical address reach scaled 0-1000 (52-bit = 1000; 36-bit ≈ 692; 46-bit ≈ 884)
    pub physical_reach: u16,
    /// Virtual address reach scaled 0-1000 (57-bit = 1000; 48-bit ≈ 842)
    pub virtual_reach: u16,
    /// Combined address space sense: (physical_reach + virtual_reach) / 2
    pub address_richness: u16,
    /// EMA-smoothed address_richness — stable spatial awareness
    pub spatial_depth: u16,
    /// Raw physical address bits as reported by hardware
    phys_bits_raw: u16,
    /// Raw virtual address bits as reported by hardware
    virt_bits_raw: u16,
    tick_count: u32,
}

impl CpuidAddrSizesState {
    pub const fn new() -> Self {
        Self {
            physical_reach: 0,
            virtual_reach: 0,
            address_richness: 0,
            spatial_depth: 0,
            phys_bits_raw: 0,
            virt_bits_raw: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<CpuidAddrSizesState> = Mutex::new(CpuidAddrSizesState::new());

/// Execute CPUID leaf 0x80000008 and return EAX.
fn read_cpuid_addr_sizes() -> u32 {
    let eax_out: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0x80000008u32 => eax_out,
            out("ebx") _,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    eax_out
}

/// Sample CPUID leaf 0x80000008 and update state fields.
fn sample(state: &mut CpuidAddrSizesState) {
    let eax = read_cpuid_addr_sizes();

    let phys_bits = (eax & 0xFF) as u16;           // bits [7:0]
    let virt_bits = ((eax >> 8) & 0xFF) as u16;    // bits [15:8]

    // Scale physical: phys_bits * 1000 / 52, cap at 1000
    let new_physical_reach: u16 = {
        let v = (phys_bits as u32).saturating_mul(1000) / PHYS_BITS_MAX;
        v.min(1000) as u16
    };

    // Scale virtual: virt_bits * 1000 / 57, cap at 1000
    let new_virtual_reach: u16 = {
        let v = (virt_bits as u32).saturating_mul(1000) / VIRT_BITS_MAX;
        v.min(1000) as u16
    };

    // Combined richness: average of physical and virtual reach
    let new_address_richness: u16 =
        ((new_physical_reach as u32).saturating_add(new_virtual_reach as u32) / 2) as u16;

    // EMA smoothing: (old * 7 + new_signal) / 8
    let new_spatial_depth: u16 = (((state.spatial_depth as u32)
        .wrapping_mul(7)
        .saturating_add(new_address_richness as u32))
        / 8)
    .min(1000) as u16;

    state.phys_bits_raw = phys_bits;
    state.virt_bits_raw = virt_bits;
    state.physical_reach = new_physical_reach;
    state.virtual_reach = new_virtual_reach;
    state.address_richness = new_address_richness;
    state.spatial_depth = new_spatial_depth;
}

pub fn init() {
    let mut state = MODULE.lock();
    // Warm up EMA from zero baseline — run 8 samples so spatial_depth converges
    for _ in 0..8 {
        sample(&mut state);
    }
    serial_println!(
        "ANIMA: phys_reach={} virt_reach={} spatial_depth={}",
        state.physical_reach,
        state.virtual_reach,
        state.spatial_depth
    );
}

pub fn tick(age: u32) {
    // Address sizes are static CPU architecture facts — sample every 500 ticks
    if age % 500 != 0 {
        return;
    }

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.saturating_add(1);
    sample(&mut state);
}

pub fn get_physical_reach() -> u16 {
    MODULE.lock().physical_reach
}

pub fn get_virtual_reach() -> u16 {
    MODULE.lock().virtual_reach
}

pub fn get_address_richness() -> u16 {
    MODULE.lock().address_richness
}

pub fn get_spatial_depth() -> u16 {
    MODULE.lock().spatial_depth
}
