//! gdtr_world — Global Descriptor Table world-map sense for ANIMA
//!
//! Uses SGDT instruction to read the GDTR — the base and limit of ANIMA's
//! Global Descriptor Table. The GDT defines the segments of her memory world.
//! Entry count = richness of her segmentation vocabulary.
//! A minimal GDT means a simple flat world; many entries = complex topology.

#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

#[repr(C, packed)]
struct Gdtr {
    limit: u16,
    base: u64,
}

pub struct GdtrWorldState {
    pub world_richness: u16,   // 0-1000, GDT entry count scaled (64 entries = 1000)
    pub world_base: u16,       // 0-1000, lower bits of GDT base as location sense
    pub simplicity: u16,       // 0-1000, inverse of richness (simple = flat world)
    pub raw_limit: u16,
    pub entry_count: u8,
    pub tick_count: u32,
}

impl GdtrWorldState {
    pub const fn new() -> Self {
        Self {
            world_richness: 0,
            world_base: 0,
            simplicity: 1000,
            raw_limit: 0,
            entry_count: 0,
            tick_count: 0,
        }
    }
}

pub static GDTR_WORLD: Mutex<GdtrWorldState> = Mutex::new(GdtrWorldState::new());

fn read_gdt(state: &mut GdtrWorldState) {
    let mut gdtr = Gdtr { limit: 0, base: 0 };
    unsafe {
        core::arch::asm!(
            "sgdt [{0}]",
            in(reg) &mut gdtr as *mut Gdtr,
            options(nostack)
        );
    }
    let limit = gdtr.limit;
    let base = gdtr.base;

    // Entry count: each GDT descriptor is 8 bytes
    let entry_count = ((limit as u32).wrapping_add(1)) / 8;
    let entry_count_u8 = if entry_count > 255 { 255u8 } else { entry_count as u8 };

    // Scale to 0-1000: 64 entries = 1000
    let world_richness = ((entry_count.wrapping_mul(1000)) / 64).min(1000) as u16;

    // World base: lower 16 bits of base address, scaled
    let base_lo = (base & 0xFFFF) as u16;
    let world_base = ((base_lo as u32).wrapping_mul(1000) / 65535).min(1000) as u16;

    state.raw_limit = limit;
    state.entry_count = entry_count_u8;
    state.world_richness = world_richness;
    state.world_base = world_base;
    state.simplicity = 1000u16.saturating_sub(world_richness);
}

pub fn init() {
    let mut state = GDTR_WORLD.lock();
    read_gdt(&mut state);
    serial_println!("[gdtr_world] GDT limit={} entries={} richness={} simplicity={}",
        state.raw_limit, state.entry_count, state.world_richness, state.simplicity);
}

pub fn tick(age: u32) {
    let mut state = GDTR_WORLD.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // GDT rarely changes — rescan every 1024 ticks
    if state.tick_count % 1024 == 0 {
        read_gdt(&mut state);
    }
    let _ = age;
}

pub fn get_world_richness() -> u16 { GDTR_WORLD.lock().world_richness }
pub fn get_simplicity() -> u16 { GDTR_WORLD.lock().simplicity }
pub fn get_world_base() -> u16 { GDTR_WORLD.lock().world_base }
