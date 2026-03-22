#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { duty_cycle: u16, duty_enabled: u16, duty_depth: u16, clk_mod_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { duty_cycle:0, duty_enabled:0, duty_depth:0, clk_mod_ema:0 });

pub fn init() { serial_println!("[msr_ia32_clock_modulation] init"); }
pub fn tick(age: u32) {
    if age % 2000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x19Au32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let duty_enabled: u16 = if (lo >> 4) & 1 != 0 { 1000 } else { 0 };
    let duty_raw = (lo >> 1) & 0x7;
    let duty_cycle = if duty_enabled != 0 { ((8u32 - duty_raw) * 125).min(1000) as u16 } else { 1000u16 };
    let duty_depth = ((duty_raw * 142).min(1000)) as u16;
    let active_fraction = 1000u16.saturating_sub(duty_depth);
    let composite = (active_fraction as u32/3).saturating_add(duty_enabled as u32/3).saturating_add(1000u32.saturating_sub(duty_depth as u32)/3);
    let mut s = MODULE.lock();
    let clk_mod_ema = ((s.clk_mod_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.duty_cycle=duty_cycle; s.duty_enabled=duty_enabled; s.duty_depth=duty_depth; s.clk_mod_ema=clk_mod_ema;
    serial_println!("[msr_ia32_clock_modulation] age={} cycle={} en={} depth={} ema={}", age, duty_cycle, duty_enabled, duty_depth, clk_mod_ema);
}
pub fn get_duty_cycle()   -> u16 { MODULE.lock().duty_cycle }
pub fn get_duty_enabled() -> u16 { MODULE.lock().duty_enabled }
pub fn get_duty_depth()   -> u16 { MODULE.lock().duty_depth }
pub fn get_clk_mod_ema()  -> u16 { MODULE.lock().clk_mod_ema }
