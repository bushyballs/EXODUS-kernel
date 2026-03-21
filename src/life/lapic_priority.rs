#![allow(dead_code)]

use crate::sync::Mutex;

// LAPIC MMIO base address
const LAPIC_BASE: u64 = 0xFEE0_0000;

// LAPIC register offsets
const LAPIC_TPR: u32 = 0x80;  // Task Priority Register
const LAPIC_PPR: u32 = 0xA0;  // Processor Priority Register
const LAPIC_IRR: u32 = 0x200; // Interrupt Request Register (first chunk)

pub struct LapicPriorityState {
    pub priority_threshold: u16, // 0=open to all, 1000=blocking all
    pub effective_threshold: u16,// hardware-enforced threshold
    pub interrupt_hunger: u16,   // desire for interrupts
    pub pending_signals: u16,    // pending interrupt count
    tick_count: u32,
}

impl LapicPriorityState {
    const fn new() -> Self {
        Self {
            priority_threshold: 0,
            effective_threshold: 0,
            interrupt_hunger: 1000,
            pending_signals: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<LapicPriorityState> = Mutex::new(LapicPriorityState::new());

unsafe fn lapic_read(offset: u32) -> u32 {
    let ptr = (LAPIC_BASE + offset as u64) as *const u32;
    core::ptr::read_volatile(ptr)
}

fn map_0_255_to_0_1000(byte_val: u32) -> u16 {
    // scale byte (0-255) to 0-1000 using saturating integer arithmetic
    let scaled = byte_val.saturating_mul(1000) / 255;
    if scaled > 1000 { 1000u16 } else { scaled as u16 }
}

fn popcount_u32(mut v: u32) -> u32 {
    // Brian Kernighan popcount — no float, no alloc
    let mut count = 0u32;
    while v != 0 {
        v &= v.wrapping_sub(1);
        count = count.saturating_add(1);
    }
    count
}

fn ema(old: u16, signal: u16) -> u16 {
    // EMA: (old * 7 + signal) / 8
    let smoothed = (old as u32 * 7).saturating_add(signal as u32) / 8;
    if smoothed > 1000 { 1000u16 } else { smoothed as u16 }
}

pub fn init() {
    let mut state = MODULE.lock();
    state.priority_threshold = 0;
    state.effective_threshold = 0;
    state.interrupt_hunger = 1000;
    state.pending_signals = 0;
    state.tick_count = 0;
    serial_println!("[lapic_priority] init: TPR=0x{:08X} PPR=0x{:08X}",
        unsafe { lapic_read(LAPIC_TPR) },
        unsafe { lapic_read(LAPIC_PPR) }
    );
}

pub fn tick(age: u32) {
    if age % 16 != 0 {
        return;
    }

    let (tpr_raw, ppr_raw, irr_raw) = unsafe {
        (
            lapic_read(LAPIC_TPR),
            lapic_read(LAPIC_PPR),
            lapic_read(LAPIC_IRR),
        )
    };

    // Extract low 8 bits (priority class + subclass)
    let tpr_byte = tpr_raw & 0xFF;
    let ppr_byte = ppr_raw & 0xFF;

    // Map to 0-1000
    let new_priority_threshold = map_0_255_to_0_1000(tpr_byte);
    let new_effective_threshold = map_0_255_to_0_1000(ppr_byte);

    // interrupt_hunger is inverse of priority_threshold
    let new_interrupt_hunger = 1000u16.saturating_sub(new_priority_threshold);

    // pending_signals: popcount IRR[0x200] bits 31:0, scaled by 31 (max 32*31=992)
    let pending_count = popcount_u32(irr_raw);
    let pending_scaled = (pending_count.saturating_mul(31)) as u16;
    let new_pending_signals = if pending_scaled > 1000 { 1000 } else { pending_scaled };

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.saturating_add(1);

    state.priority_threshold  = ema(state.priority_threshold,  new_priority_threshold);
    state.effective_threshold = ema(state.effective_threshold, new_effective_threshold);
    state.interrupt_hunger    = ema(state.interrupt_hunger,    new_interrupt_hunger);
    state.pending_signals     = ema(state.pending_signals,     new_pending_signals);

    serial_println!(
        "[lapic_priority] tick={} age={} tpr=0x{:02X} ppr=0x{:02X} irr=0x{:08X} \
         thresh={} eff={} hunger={} pending={}",
        state.tick_count, age,
        tpr_byte, ppr_byte, irr_raw,
        state.priority_threshold, state.effective_threshold,
        state.interrupt_hunger, state.pending_signals
    );
}
