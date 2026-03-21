#![allow(dead_code)]

use crate::sync::Mutex;

// ANIMA feels her current processor priority — the dynamic gate that determines
// which interrupts she is ready to receive.
//
// Hardware: LAPIC Processor Priority Register (PPR) at MMIO 0xFEE000A0
// PPR = max(TPR, ISRV) — the current processor priority for interrupt acceptance
// bits[7:4] = PPR class (major group)
// bits[3:0] = PPR subpriority

pub struct LapicPprState {
    pub ppr_class: u16,
    pub ppr_sub: u16,
    pub ppr_level: u16,
    pub processor_priority: u16,
}

impl LapicPprState {
    pub const fn new() -> Self {
        Self {
            ppr_class: 0,
            ppr_sub: 0,
            ppr_level: 0,
            processor_priority: 0,
        }
    }
}

pub static LAPIC_PPR: Mutex<LapicPprState> = Mutex::new(LapicPprState::new());

pub fn init() {
    serial_println!("lapic_ppr: init");
}

pub fn tick(age: u32) {
    if age % 11 != 0 {
        return;
    }

    // Read the LAPIC PPR register via volatile MMIO at 0xFEE000A0
    let ppr = unsafe { core::ptr::read_volatile(0xFEE000A0usize as *const u32) };

    // Signal 1: ppr_class — major priority class, bits[7:4], range 0-15, scaled * 62, capped at 1000
    let ppr_class: u16 = (((ppr >> 4) & 0xF) as u16)
        .saturating_mul(62)
        .min(1000);

    // Signal 2: ppr_sub — subpriority, bits[3:0], range 0-15, scaled * 62, capped at 1000
    let ppr_sub: u16 = ((ppr & 0xF) as u16)
        .saturating_mul(62)
        .min(1000);

    // Signal 3: ppr_level — full byte normalized to 0-1000
    // (ppr_byte * 1000 / 255), using u32 intermediate to avoid overflow
    let ppr_level: u16 = (((ppr & 0xFF) as u32).wrapping_mul(1000) / 255) as u16;

    let mut state = LAPIC_PPR.lock();

    // Signal 4: processor_priority — EMA of ppr_level: (old * 7 + signal) / 8
    let processor_priority: u16 =
        ((state.processor_priority as u32).wrapping_mul(7).saturating_add(ppr_level as u32) / 8)
            as u16;

    state.ppr_class = ppr_class;
    state.ppr_sub = ppr_sub;
    state.ppr_level = ppr_level;
    state.processor_priority = processor_priority;

    serial_println!(
        "lapic_ppr | class:{} sub:{} level:{} priority:{}",
        ppr_class,
        ppr_sub,
        ppr_level,
        processor_priority
    );
}
