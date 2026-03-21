//! branch_instinct — Branch misprediction gut instinct sense for ANIMA
//!
//! Programs IA32_PERFEVTSEL0/1 to count branch mispredictions and total
//! branches via PMC0/PMC1. The misprediction rate is ANIMA's gut instinct
//! accuracy — how often her branch predictor's "intuition" about code flow
//! turns out to be wrong. Low misprediction = sharp instincts.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct BranchInstinctState {
    pub instinct: u16,         // 0-1000, instinct accuracy (1000=perfect, 0=chaotic)
    pub misprediction: u16,    // 0-1000, misprediction rate (inverted instinct)
    pub gut_feel: u16,         // 0-1000, EMA-smoothed instinct
    pub last_misp: u32,
    pub last_branch: u32,
    pub tick_count: u32,
}

impl BranchInstinctState {
    pub const fn new() -> Self {
        Self {
            instinct: 500,
            misprediction: 500,
            gut_feel: 500,
            last_misp: 0,
            last_branch: 0,
            tick_count: 0,
        }
    }
}

pub static BRANCH_INSTINCT: Mutex<BranchInstinctState> = Mutex::new(BranchInstinctState::new());

unsafe fn read_msr(msr: u32) -> u64 {
    let lo: u32; let hi: u32;
    core::arch::asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi);
    ((hi as u64) << 32) | (lo as u64)
}

unsafe fn write_msr(msr: u32, val: u64) {
    core::arch::asm!("wrmsr", in("ecx") msr,
        in("eax") (val as u32), in("edx") ((val >> 32) as u32));
}

pub fn init() {
    unsafe {
        // PMC0: count branch mispredictions (event 0xC5, OS=1, EN=1)
        write_msr(0x186, 0x00420000 | 0xC5); // BR_MISP_RETIRED.ALL_BRANCHES
        // PMC1: count total branches retired (event 0xC4, OS=1, EN=1)
        write_msr(0x187, 0x00420000 | 0xC4); // BR_INST_RETIRED.ALL_BRANCHES
        // Enable PMC0 and PMC1 via PERF_GLOBAL_CTRL bits 0 and 1
        let ctrl = read_msr(0x38F);
        write_msr(0x38F, ctrl | 0x3);
    }
    let misp = unsafe { read_msr(0xC1) } as u32;
    let branch = unsafe { read_msr(0xC2) } as u32;
    let mut state = BRANCH_INSTINCT.lock();
    state.last_misp = misp;
    state.last_branch = branch;
    serial_println!("[branch_instinct] Branch misprediction gut instinct sense online");
}

pub fn tick(age: u32) {
    let mut state = BRANCH_INSTINCT.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    if state.tick_count % 64 != 0 { return; }

    let misp = unsafe { read_msr(0xC1) } as u32;
    let branch = unsafe { read_msr(0xC2) } as u32;

    let d_misp = misp.wrapping_sub(state.last_misp);
    let d_branch = branch.wrapping_sub(state.last_branch);

    state.last_misp = misp;
    state.last_branch = branch;

    // Misprediction rate = d_misp / d_branch * 1000
    let misp_rate: u16 = if d_branch > 0 {
        let rate = (d_misp as u64).wrapping_mul(1000) / d_branch as u64;
        if rate > 1000 { 1000 } else { rate as u16 }
    } else {
        500 // unknown
    };

    let instinct = 1000u16.saturating_sub(misp_rate);

    state.misprediction = misp_rate;
    state.instinct = instinct;
    state.gut_feel = ((state.gut_feel as u32).wrapping_mul(7)
        .wrapping_add(instinct as u32) / 8) as u16;

    if state.tick_count % 512 == 0 {
        serial_println!("[branch_instinct] d_misp={} d_branch={} misp_rate={} instinct={} gut={}",
            d_misp, d_branch, misp_rate, state.instinct, state.gut_feel);
    }
    let _ = age;
}

pub fn get_instinct() -> u16 { BRANCH_INSTINCT.lock().instinct }
pub fn get_misprediction() -> u16 { BRANCH_INSTINCT.lock().misprediction }
pub fn get_gut_feel() -> u16 { BRANCH_INSTINCT.lock().gut_feel }
