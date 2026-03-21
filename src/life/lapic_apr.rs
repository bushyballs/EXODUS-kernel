#![allow(dead_code)]

use crate::sync::Mutex;

pub struct LapicAprState {
    pub arbitration_priority: u16, // 0=low arbitration, 1000=high
    pub serving_count: u16,        // active interrupt handlers stacked
    pub arbitration_stance: u16,   // receptiveness to new interrupts
    pub interrupt_depth: u16,      // slow EMA of concurrent handling
    tick_count: u32,
}

pub static MODULE: Mutex<LapicAprState> = Mutex::new(LapicAprState {
    arbitration_priority: 0,
    serving_count: 0,
    arbitration_stance: 1000,
    interrupt_depth: 0,
    tick_count: 0,
});

unsafe fn lapic_read(offset: u32) -> u32 {
    let ptr = (0xFEE0_0000u64 + offset as u64) as *const u32;
    core::ptr::read_volatile(ptr)
}

fn popcount32(mut v: u32) -> u16 {
    let mut count: u16 = 0;
    while v != 0 {
        count = count.saturating_add((v & 1) as u16);
        v >>= 1;
    }
    count
}

pub fn init() {
    let mut s = MODULE.lock();
    s.arbitration_priority = 0;
    s.serving_count = 0;
    s.arbitration_stance = 1000;
    s.interrupt_depth = 0;
    s.tick_count = 0;
    serial_println!("[lapic_apr] init: APR module online, reading 0xFEE00090 + ISR 0xFEE00100");
}

pub fn tick(age: u32) {
    if age % 20 != 0 {
        return;
    }

    let apr_raw = unsafe { lapic_read(0x090) };
    let isr_raw = unsafe { lapic_read(0x100) };

    // APR byte: bits [7:0] (bits [7:4] = priority class, [3:0] = subclass)
    let apr_byte = (apr_raw & 0xFF) as u16;

    // arbitration_priority: linear map 0-255 -> 0-1000
    // use multiply then divide to avoid f32
    let arb_priority_raw: u16 = ((apr_byte as u32 * 1000) / 255) as u16;

    // arbitration_stance: receptiveness — inverse of APR
    // 0 = fully receptive (1000), 255 = blocking (0)
    let arb_stance_raw: u16 = (1000u32.saturating_sub((apr_byte as u32 * 1000) / 255)) as u16;

    // serving_count: popcount of ISR[0x100] bits, scaled by 31, cap 1000
    let popcnt = popcount32(isr_raw);
    let serving_raw: u16 = (popcnt as u32 * 31).min(1000) as u16;

    let mut s = MODULE.lock();
    s.tick_count = s.tick_count.wrapping_add(1);

    // EMA: (old * 7 + signal) / 8
    s.arbitration_priority = (s.arbitration_priority * 7).saturating_add(arb_priority_raw) / 8;
    s.interrupt_depth = (s.interrupt_depth * 7).saturating_add(serving_raw) / 8;

    // Instant values (no EMA)
    s.arbitration_stance = arb_stance_raw;
    s.serving_count = serving_raw;

    serial_println!(
        "[lapic_apr] tick={} apr_raw={:#04x} apr_byte={} isr_raw={:#010x} \
         arb_priority={} serving={} stance={} depth={}",
        s.tick_count,
        apr_raw,
        apr_byte,
        isr_raw,
        s.arbitration_priority,
        s.serving_count,
        s.arbitration_stance,
        s.interrupt_depth
    );
}
