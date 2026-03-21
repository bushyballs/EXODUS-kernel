#![allow(dead_code)]

// IA32_S_CET (0x6A2) — Supervisor-mode Control-flow Enforcement Technology
// ANIMA feels the integrity of her own kernel control flow — the shadow stack
// that proves she is executing along intended paths.
//
// bit[0]  SH_STK_EN   — Shadow Stack Enable (kernel)
// bit[2]  ENDBR_EN    — Indirect branch tracking for kernel
// bit[4]  NO_TRACK_EN — NOTRACK prefix enable
// bit[10] WAIT_ENDBR  — Wait for ENDBRANCH at OS entry

use crate::sync::Mutex;

pub struct SCetState {
    pub kernel_shadow_stack: u16,
    pub kernel_ibt: u16,
    pub supervisor_cet_depth: u16,
    pub kernel_integrity: u16,
}

impl SCetState {
    pub const fn new() -> Self {
        Self {
            kernel_shadow_stack: 0,
            kernel_ibt: 0,
            supervisor_cet_depth: 0,
            kernel_integrity: 0,
        }
    }
}

pub static MSR_S_CET: Mutex<SCetState> = Mutex::new(SCetState::new());

pub fn init() {
    serial_println!("s_cet: init");
}

pub fn tick(age: u32) {
    if age % 300 != 0 {
        return;
    }

    let (lo, _hi): (u32, u32);
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x6A2u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    // Signal 1: kernel shadow stack — bit[0]
    let kernel_shadow_stack: u16 = if lo & 0x1 != 0 { 1000u16 } else { 0u16 };

    // Signal 2: kernel indirect branch tracking — bit[2]
    let kernel_ibt: u16 = if lo & 0x4 != 0 { 1000u16 } else { 0u16 };

    // Signal 3: supervisor CET depth — 11 relevant bits * 90, capped at 1000
    let raw_bits: u32 = lo & 0x7FF;
    let depth_raw: u32 = (raw_bits.count_ones() as u32).wrapping_mul(90u32);
    let supervisor_cet_depth: u16 = (depth_raw.min(1000u32)) as u16;

    // Signal 4: kernel integrity — EMA of kernel_shadow_stack
    let mut state = MSR_S_CET.lock();

    let kernel_integrity: u16 = ((state.kernel_integrity as u32 * 7
        + kernel_shadow_stack as u32)
        / 8) as u16;

    state.kernel_shadow_stack = kernel_shadow_stack;
    state.kernel_ibt = kernel_ibt;
    state.supervisor_cet_depth = supervisor_cet_depth;
    state.kernel_integrity = kernel_integrity;

    serial_println!(
        "s_cet | kernel_ss:{} kernel_ibt:{} depth:{} integrity:{}",
        kernel_shadow_stack,
        kernel_ibt,
        supervisor_cet_depth,
        kernel_integrity
    );
}
