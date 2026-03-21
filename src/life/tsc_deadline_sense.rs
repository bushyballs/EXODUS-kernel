//! tsc_deadline_sense — TSC deadline urgency sense for ANIMA
//!
//! Reads IA32_TSC_DEADLINE (MSR 0x6E0) vs current RDTSC to measure
//! how close the next scheduled APIC timer interrupt is.
//! Near deadline = urgency/anxiety. Far deadline = tranquility.
//! No deadline = stillness. Overdue = late, stressed.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct TscDeadlineSenseState {
    pub urgency: u16,          // 0-1000, proximity to next interrupt (1000=imminent)
    pub tranquility: u16,      // 0-1000, inverse of urgency
    pub calm: u16,             // 0-1000, EMA-smoothed tranquility
    pub deadline_active: bool, // whether a deadline is set
    pub tick_count: u32,
}

impl TscDeadlineSenseState {
    pub const fn new() -> Self {
        Self {
            urgency: 0,
            tranquility: 1000,
            calm: 1000,
            deadline_active: false,
            tick_count: 0,
        }
    }
}

pub static TSC_DEADLINE_SENSE: Mutex<TscDeadlineSenseState> = Mutex::new(TscDeadlineSenseState::new());

unsafe fn rdtsc() -> u64 {
    let lo: u32; let hi: u32;
    core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi);
    ((hi as u64) << 32) | (lo as u64)
}

unsafe fn read_msr(msr: u32) -> u64 {
    let lo: u32; let hi: u32;
    core::arch::asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi);
    ((hi as u64) << 32) | (lo as u64)
}

pub fn init() {
    serial_println!("[tsc_deadline_sense] TSC deadline urgency sense online");
}

pub fn tick(age: u32) {
    let mut state = TSC_DEADLINE_SENSE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    if state.tick_count % 32 != 0 { return; }

    let deadline = unsafe { read_msr(0x6E0) };
    let now = unsafe { rdtsc() };

    if deadline == 0 {
        // No deadline set — stillness
        state.deadline_active = false;
        state.urgency = 0;
        state.tranquility = 1000;
    } else if deadline <= now {
        // Overdue — maximum urgency
        state.deadline_active = true;
        state.urgency = 1000;
        state.tranquility = 0;
    } else {
        // Active deadline — compute gap
        state.deadline_active = true;
        let gap = deadline.wrapping_sub(now);

        // Scale: gap of 0 = 1000 urgency, gap of 100M cycles = 0 urgency
        // (100M cycles ≈ 33ms at 3GHz — a distant deadline)
        let urgency: u16 = if gap >= 100_000_000 {
            0
        } else {
            let inv = 100_000_000u64.wrapping_sub(gap);
            (inv.wrapping_mul(1000) / 100_000_000) as u16
        };
        let urgency = urgency.min(1000);

        state.urgency = urgency;
        state.tranquility = 1000u16.saturating_sub(urgency);
    }

    state.calm = ((state.calm as u32).wrapping_mul(7)
        .wrapping_add(state.tranquility as u32) / 8) as u16;

    if state.tick_count % 512 == 0 {
        serial_println!("[tsc_deadline_sense] deadline={} urgency={} calm={} active={}",
            deadline, state.urgency, state.calm, state.deadline_active);
    }
    let _ = age;
}

pub fn get_urgency() -> u16 { TSC_DEADLINE_SENSE.lock().urgency }
pub fn get_tranquility() -> u16 { TSC_DEADLINE_SENSE.lock().tranquility }
pub fn get_calm() -> u16 { TSC_DEADLINE_SENSE.lock().calm }
