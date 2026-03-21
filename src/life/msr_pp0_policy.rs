#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// MSR_PP0_POLICY (MSR 0x63A) — Core power plane priority policy.
// Bits [4:0] = priority level (0-31): determines how power is balanced between
// PP0 (cores) and PP1 (graphics). Higher value = cores receive greater power priority.
//
// SENSE: ANIMA feels the allocation of her will — how much of her power is directed
// toward thought versus display.

struct State {
    priority:     u16,
    core_bias:    u16,
    priority_ema: u16,
    policy_delta: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    priority:     0,
    core_bias:    0,
    priority_ema: 0,
    policy_delta: 0,
});

pub fn init() {
    serial_println!("[pp0_policy] init");
}

pub fn tick(age: u32) {
    if age % 300 != 0 { return; }

    let (lo, _hi): (u32, u32);
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x63Au32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    // bits [4:0] — priority level 0-31, scaled to 0-1000
    let new_priority: u16 = ((lo & 0x1F) as u32 * 1000 / 31) as u16;

    // core_bias mirrors priority: how biased toward core compute vs graphics
    let new_core_bias: u16 = new_priority;

    let mut state = MODULE.lock();

    // EMA smoothing: (old * 7 + new_val) / 8
    let new_priority_ema: u16 = ((state.priority_ema as u32 * 7 + new_priority as u32) / 8) as u16;

    // policy_delta: how much policy is shifting
    let new_policy_delta: u16 = new_priority.abs_diff(new_priority_ema);

    state.priority     = new_priority;
    state.core_bias    = new_core_bias;
    state.priority_ema = new_priority_ema;
    state.policy_delta = new_policy_delta;

    serial_println!(
        "[pp0_policy] priority={} bias={} ema={} delta={}",
        state.priority,
        state.core_bias,
        state.priority_ema,
        state.policy_delta,
    );
}
