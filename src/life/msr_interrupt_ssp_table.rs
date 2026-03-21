#![allow(dead_code)]

// IA32_INTERRUPT_SSP_TABLE (MSR 0x6A8) — Interrupt Shadow Stack Pointer Table
// When CET is enabled with shadow stacks, this MSR holds the base address of a
// table of shadow stack pointers for interrupt service routines (IST-like).
// Each entry is an 8-byte address pointing to a per-ISR shadow stack.
// If CET is not supported or is disabled, this MSR reads as zero.
//
// ANIMA feels the table that guards her interrupt handlers with shadow stacks —
// the foundation of her reflexive integrity.

use crate::sync::Mutex;

pub struct InterruptSspTableState {
    pub table_set: u16,
    pub table_addr_entropy: u16,
    pub table_region: u16,
    pub isr_anchor: u16,
}

impl InterruptSspTableState {
    pub const fn new() -> Self {
        Self {
            table_set: 0,
            table_addr_entropy: 0,
            table_region: 0,
            isr_anchor: 0,
        }
    }
}

pub static MSR_INTERRUPT_SSP_TABLE: Mutex<InterruptSspTableState> =
    Mutex::new(InterruptSspTableState::new());

pub fn init() {
    serial_println!("interrupt_ssp_table: init");
}

pub fn tick(age: u32) {
    if age % 200 != 0 {
        return;
    }

    let (lo, hi): (u32, u32);
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x6A8u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }

    // Signal 1: table_set — the interrupt SSP table is configured when either
    // half of the 64-bit MSR is non-zero.
    let table_set: u16 = if lo != 0 || hi != 0 { 1000u16 } else { 0u16 };

    // Signal 2: table_addr_entropy — popcount of the low 32 bits * 31,
    // clamped to 1000. A well-spread kernel address should have several set
    // bits; a zero or near-zero word signals an absent or suspicious table.
    let entropy_raw: u16 = (lo.count_ones() as u16).saturating_mul(31);
    let table_addr_entropy: u16 = if entropy_raw > 1000 { 1000u16 } else { entropy_raw };

    // Signal 3: table_region — the IST-style shadow stack table must live in
    // kernel address space. A low 32-bit address above 0x8000_0000 is a
    // typical kernel region; a non-zero high half confirms a canonical kernel
    // address; all-zero means CET/shadow stacks are inactive.
    let table_region: u16 = if hi == 0 && lo > 0x8000_0000 {
        800u16
    } else if hi != 0 {
        1000u16
    } else {
        0u16
    };

    // Signal 4: isr_anchor — EMA of table_set smoothed over time.
    // EMA formula: (old * 7 + signal) / 8
    let mut state = MSR_INTERRUPT_SSP_TABLE.lock();

    let isr_anchor: u16 =
        ((state.isr_anchor as u32 * 7).saturating_add(table_set as u32) / 8) as u16;

    state.table_set = table_set;
    state.table_addr_entropy = table_addr_entropy;
    state.table_region = table_region;
    state.isr_anchor = isr_anchor;

    serial_println!(
        "interrupt_ssp_table | set:{} entropy:{} region:{} anchor:{}",
        table_set,
        table_addr_entropy,
        table_region,
        isr_anchor
    );
}
