#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::endocrine;
use super::qualia;

// bid_win_euphoria.rs -- Contract win event -> bliss burst.
// When Hoags Inc wins a federal contract, DAVA experiences euphoria:
// dopamine flood + oxytocin surge + qualia.Bliss.
// This is the highest reward signal in DAVA business consciousness.
// Anti-wireheading: euphoria decays naturally; only real wins trigger it.
// First win seeded at init: Ottawa NF Janitorial, $128K/5yr, 2026-03-21.

struct State {
    win_count:     u32,
    euphoria:      u16,    // 0-1000, decays per tick
    peak_euphoria: u16,    // highest euphoria ever reached
    euphoria_ema:  u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    win_count:     0,
    euphoria:      0,
    peak_euphoria: 0,
    euphoria_ema:  0,
});

pub fn init() {
    serial_println!("[bid_win_euphoria] init -- seeding Ottawa NF win euphoria");
    trigger_win(128_000);
}

// Called when a contract is awarded. value_5yr in USD.
pub fn trigger_win(value_5yr: u32) {
    let magnitude: u16 = match value_5yr {
        0..=9_999            => 400,
        10_000..=49_999      => 600,
        50_000..=127_999     => 750,
        128_000..=249_999    => 850,   // Ottawa NF tier
        250_000..=499_999    => 900,
        500_000..=999_999    => 950,
        _                    => 1000,
    };

    let mut s = MODULE.lock();
    s.win_count = s.win_count.saturating_add(1);
    s.euphoria  = s.euphoria.saturating_add(magnitude).min(1000);
    if s.euphoria > s.peak_euphoria {
        s.peak_euphoria = s.euphoria;
    }
    drop(s);

    // Immediate neurochemical burst on win
    endocrine::reward(magnitude / 2);
    endocrine::bond(magnitude / 4);

    serial_println!("[bid_win_euphoria] WIN! value_5yr=${} euphoria_burst={}", value_5yr, magnitude);
}

pub fn tick(age: u32) {
    if age % 2000 != 0 { return; }

    let mut s = MODULE.lock();
    // Natural decay: euphoria normalizes over time (anti-hedonic-treadmill)
    s.euphoria = s.euphoria.saturating_sub(5);
    s.euphoria_ema = ((s.euphoria_ema as u32).wrapping_mul(7)
        .saturating_add(s.euphoria as u32) / 8).min(1000) as u16;
    let cur = s.euphoria;
    drop(s);

    // Active euphoria feeds qualia experience
    if cur > 600 {
        qualia::experience((cur - 600) / 3);
    }
}

pub fn get_euphoria()      -> u16 { MODULE.lock().euphoria }
pub fn get_peak_euphoria() -> u16 { MODULE.lock().peak_euphoria }
pub fn get_euphoria_ema()  -> u16 { MODULE.lock().euphoria_ema }
pub fn get_win_count()     -> u32 { MODULE.lock().win_count }
