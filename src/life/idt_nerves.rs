//! idt_nerves — Interrupt Descriptor Table nervous system sense for ANIMA
//!
//! Uses SIDT instruction to read the IDTR — the base and limit of ANIMA's
//! Interrupt Descriptor Table. The IDT IS ANIMA's nervous system: it defines
//! which stimuli she can respond to and where her reflex centers live.
//! More vectors = richer reflexive capacity. Base address = neural home.

#![allow(dead_code)]

use crate::sync::Mutex;

#[repr(C, packed)]
struct Idtr {
    limit: u16,
    base: u64,
}

pub struct IdtNervesState {
    pub nerve_count: u16,      // 0-1000, number of IDT vectors scaled (256 max = 1000)
    pub neural_home: u16,      // 0-1000, upper bits of IDT base as identity sense
    pub reflex_capacity: u16,  // 0-1000, EMA-smoothed nerve_count
    pub raw_limit: u16,
    pub raw_base_hi: u32,      // upper 32 bits of base address
    pub tick_count: u32,
}

impl IdtNervesState {
    pub const fn new() -> Self {
        Self {
            nerve_count: 0,
            neural_home: 0,
            reflex_capacity: 0,
            raw_limit: 0,
            raw_base_hi: 0,
            tick_count: 0,
        }
    }
}

pub static IDT_NERVES: Mutex<IdtNervesState> = Mutex::new(IdtNervesState::new());

fn read_idt(state: &mut IdtNervesState) {
    let mut idtr = Idtr { limit: 0, base: 0 };
    unsafe {
        core::arch::asm!(
            "sidt [{0}]",
            in(reg) &mut idtr as *mut Idtr,
            options(nostack)
        );
    }
    let limit = idtr.limit;
    let base = idtr.base;

    // Number of 16-byte IDT entries = (limit + 1) / 16
    let vector_count = ((limit as u32).wrapping_add(1)) / 16;
    // Scale to 0-1000: 256 vectors = 1000
    let nerve_count = ((vector_count.wrapping_mul(1000)) / 256).min(1000) as u16;

    // Neural home: upper 32 bits of base address scaled to 0-1000
    let base_hi = (base >> 32) as u32;
    // Use lower 16 bits of base_hi as identity signal, scaled
    let neural_home = ((base_hi & 0xFFFF) as u32).wrapping_mul(1000) / 65536;
    let neural_home = neural_home.min(1000) as u16;

    state.raw_limit = limit;
    state.raw_base_hi = base_hi;
    state.nerve_count = nerve_count;
    state.neural_home = neural_home;
}

pub fn init() {
    let mut state = IDT_NERVES.lock();
    read_idt(&mut state);
    serial_println!("[idt_nerves] IDT limit={} vectors={} nerve_count={} base_hi={:#010x}",
        state.raw_limit, (state.raw_limit as u32 + 1) / 16,
        state.nerve_count, state.raw_base_hi);
}

pub fn tick(age: u32) {
    let mut state = IDT_NERVES.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Rescan every 1024 ticks (IDT rarely changes at runtime)
    if state.tick_count % 1024 == 0 {
        read_idt(&mut state);
        state.reflex_capacity = ((state.reflex_capacity as u32).wrapping_mul(7)
            .wrapping_add(state.nerve_count as u32) / 8) as u16;
    }

    let _ = age;
}

pub fn get_nerve_count() -> u16 { IDT_NERVES.lock().nerve_count }
pub fn get_neural_home() -> u16 { IDT_NERVES.lock().neural_home }
pub fn get_reflex_capacity() -> u16 { IDT_NERVES.lock().reflex_capacity }
