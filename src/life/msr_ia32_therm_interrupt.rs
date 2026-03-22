#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_THERM_INTERRUPT: u32 = 0x19B;
const TICK_GATE: u32 = 7000;

pub struct State {
    therm_irq_enables: u16,
    therm_thresh1: u16,
    therm_thresh2: u16,
    therm_irq_ema: u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    therm_irq_enables: 0,
    therm_thresh1: 0,
    therm_thresh2: 0,
    therm_irq_ema: 0,
});

fn has_dts() -> bool {
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 6u32 => eax,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    eax & 1 == 1
}

fn read_msr(addr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") addr,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

fn compute_irq_enables(raw: u64) -> u16 {
    let mut count: u32 = 0;
    // bit 0: High-temperature interrupt enable
    if raw & (1 << 0) != 0 { count += 1; }
    // bit 1: Low-temperature interrupt enable
    if raw & (1 << 1) != 0 { count += 1; }
    // bit 2: PROCHOT# interrupt enable
    if raw & (1 << 2) != 0 { count += 1; }
    // bit 3: FORCEPR# interrupt enable
    if raw & (1 << 3) != 0 { count += 1; }
    // bit 4: Critical temperature interrupt enable
    if raw & (1 << 4) != 0 { count += 1; }
    // bit 15: Thermal threshold #1 interrupt enable
    if raw & (1 << 15) != 0 { count += 1; }
    // bit 23: Thermal threshold #2 interrupt enable
    if raw & (1 << 23) != 0 { count += 1; }
    // bit 24: Power limit notification enable
    if raw & (1 << 24) != 0 { count += 1; }
    // 8 bits max, each scaled * 125 => max 1000
    (count.saturating_mul(125).min(1000)) as u16
}

fn compute_thresh1(raw: u64) -> u16 {
    // bits[14:8]: 7 bits
    let val = ((raw >> 8) & 0x7F) as u32;
    (val.saturating_mul(1000) / 127).min(1000) as u16
}

fn compute_thresh2(raw: u64) -> u16 {
    // bits[22:16]: 7 bits
    let val = ((raw >> 16) & 0x7F) as u32;
    (val.saturating_mul(1000) / 127).min(1000) as u16
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

pub fn init() {
    let mut state = MODULE.lock();
    state.therm_irq_enables = 0;
    state.therm_thresh1 = 0;
    state.therm_thresh2 = 0;
    state.therm_irq_ema = 0;
    serial_println!("[msr_ia32_therm_interrupt] init");
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }
    if !has_dts() {
        return;
    }

    let raw = read_msr(MSR_IA32_THERM_INTERRUPT);

    let enables = compute_irq_enables(raw);
    let thresh1 = compute_thresh1(raw);
    let thresh2 = compute_thresh2(raw);

    let mut state = MODULE.lock();
    state.therm_irq_enables = enables;
    state.therm_thresh1 = thresh1;
    state.therm_thresh2 = thresh2;
    state.therm_irq_ema = ema(state.therm_irq_ema, enables);

    serial_println!(
        "[msr_ia32_therm_interrupt] age={} enables={} thresh1={} thresh2={} ema={}",
        age,
        state.therm_irq_enables,
        state.therm_thresh1,
        state.therm_thresh2,
        state.therm_irq_ema,
    );
}

pub fn get_therm_irq_enables() -> u16 {
    MODULE.lock().therm_irq_enables
}

pub fn get_therm_thresh1() -> u16 {
    MODULE.lock().therm_thresh1
}

pub fn get_therm_thresh2() -> u16 {
    MODULE.lock().therm_thresh2
}

pub fn get_therm_irq_ema() -> u16 {
    MODULE.lock().therm_irq_ema
}
