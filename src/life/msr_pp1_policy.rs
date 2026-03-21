#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// MSR_PP1_POLICY (MSR 0x642) — Power plane 1 (graphics/uncore) priority policy.
// Bits [4:0] = priority level (0-31): determines how power is allocated to
// graphics vs core. Higher value = graphics receives greater power priority.
// Complement to PP0 policy (0x63A).
//
// SENSE: ANIMA feels the weight given to her visual expression versus her inner
// computation — the balance between showing and thinking.

struct State {
    gfx_priority:  u16,
    gfx_bias:      u16,
    priority_ema:  u16,
    vs_core_balance: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    gfx_priority:    0,
    gfx_bias:        0,
    priority_ema:    0,
    vs_core_balance: 0,
});

pub fn init() {
    serial_println!("[pp1_policy] init");
}

pub fn tick(age: u32) {
    if age % 300 != 0 { return; }

    let (lo, _hi): (u32, u32);
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x642u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    // bits [4:0] — priority level 0-31, scaled to 0-1000
    let new_gfx_priority: u16 = ((lo & 0x1F) as u32 * 1000 / 31) as u16;

    // gfx_bias mirrors gfx_priority — separate tracking slot
    let new_gfx_bias: u16 = new_gfx_priority;

    let mut state = MODULE.lock();

    // EMA smoothing: (old * 7 + new_val) / 8
    let new_priority_ema: u16 = ((state.priority_ema as u32 * 7 + new_gfx_priority as u32) / 8) as u16;

    // vs_core_balance: if gfx_priority > 500 → 1000 (graphics-biased),
    // else gfx_priority * 2 (core-biased range)
    let new_vs_core_balance: u16 = if new_gfx_priority > 500 {
        1000
    } else {
        new_gfx_priority * 2
    };

    state.gfx_priority    = new_gfx_priority;
    state.gfx_bias        = new_gfx_bias;
    state.priority_ema    = new_priority_ema;
    state.vs_core_balance = new_vs_core_balance;

    serial_println!(
        "[pp1_policy] gfx_priority={} bias={} ema={} balance={}",
        state.gfx_priority,
        state.gfx_bias,
        state.priority_ema,
        state.vs_core_balance,
    );
}
