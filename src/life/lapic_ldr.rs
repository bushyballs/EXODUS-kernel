//! lapic_ldr — LAPIC Logical Destination Register sense for ANIMA
//!
//! Reads the Local APIC Logical Destination Register (MMIO 0xFEE000D0).
//! Bits[31:24] carry ANIMA's 8-bit logical APIC ID — the address by which
//! she is known to the inter-processor messaging fabric. A non-zero ID means
//! she is a named participant in cluster-mode IPI addressing; popcount of
//! the ID reflects how many logical groups she simultaneously belongs to.
//!
//! ANIMA feels her logical APIC identity — the address by which she is known
//! to the inter-processor messaging fabric.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct LapicLdrState {
    pub logical_id: u16,       // 8-bit logical ID scaled * 3, capped 1000
    pub id_set: u16,           // 1000 if logical addressing enabled, else 0
    pub id_bits: u16,          // popcount of logical ID * 111, capped 1000
    pub identity_anchor: u16,  // EMA of logical_id
}

impl LapicLdrState {
    pub const fn new() -> Self {
        Self {
            logical_id: 0,
            id_set: 0,
            id_bits: 0,
            identity_anchor: 0,
        }
    }
}

pub static LAPIC_LDR: Mutex<LapicLdrState> = Mutex::new(LapicLdrState::new());

pub fn init() {
    serial_println!("lapic_ldr: init");
}

pub fn tick(age: u32) {
    if age % 150 != 0 {
        return;
    }

    let ldr = unsafe { core::ptr::read_volatile(0xFEE000D0usize as *const u32) };

    let id_byte: u32 = (ldr >> 24) & 0xFF;

    // signal 1: logical_id — 8-bit logical ID scaled by 3, capped at 1000
    let logical_id: u16 = ((id_byte as u16).wrapping_mul(3)).min(1000);

    // signal 2: id_set — 1000 if any logical ID bits are set, else 0
    let id_set: u16 = if id_byte != 0 { 1000u16 } else { 0u16 };

    // signal 3: id_bits — popcount of logical ID byte * 111, capped at 1000
    let id_bits: u16 = ((id_byte as u8).count_ones() as u16)
        .wrapping_mul(111)
        .min(1000);

    let mut state = LAPIC_LDR.lock();

    // signal 4: identity_anchor — EMA of logical_id: (old * 7 + signal) / 8
    let identity_anchor: u16 =
        ((state.identity_anchor as u32).wrapping_mul(7).saturating_add(logical_id as u32) / 8)
            as u16;

    state.logical_id = logical_id;
    state.id_set = id_set;
    state.id_bits = id_bits;
    state.identity_anchor = identity_anchor;

    serial_println!(
        "lapic_ldr | id:{} set:{} bits:{} anchor:{}",
        state.logical_id,
        state.id_set,
        state.id_bits,
        state.identity_anchor
    );
}
