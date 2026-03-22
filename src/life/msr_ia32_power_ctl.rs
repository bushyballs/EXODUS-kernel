#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { turbo_disabled: u16, c_state_req_enable: u16, race_to_halt: u16, power_ctl_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { turbo_disabled:0, c_state_req_enable:0, race_to_halt:0, power_ctl_ema:0 });

pub fn init() { serial_println!("[msr_ia32_power_ctl] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x1FCu32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let turbo_disabled: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    let c_state_req_enable: u16 = if (lo >> 3) & 1 != 0 { 1000 } else { 0 };
    let race_to_halt: u16 = if (lo >> 20) & 1 != 0 { 1000 } else { 0 };
    let perf_potential = 1000u32.saturating_sub(turbo_disabled as u32/2);
    let composite = (perf_potential/3).saturating_add(c_state_req_enable as u32/3).saturating_add(race_to_halt as u32/3);
    let mut s = MODULE.lock();
    let power_ctl_ema = ((s.power_ctl_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.turbo_disabled=turbo_disabled; s.c_state_req_enable=c_state_req_enable; s.race_to_halt=race_to_halt; s.power_ctl_ema=power_ctl_ema;
    serial_println!("[msr_ia32_power_ctl] age={} turbo_dis={} c_state={} rth={} ema={}", age, turbo_disabled, c_state_req_enable, race_to_halt, power_ctl_ema);
}
pub fn get_turbo_disabled()      -> u16 { MODULE.lock().turbo_disabled }
pub fn get_c_state_req_enable()  -> u16 { MODULE.lock().c_state_req_enable }
pub fn get_race_to_halt()        -> u16 { MODULE.lock().race_to_halt }
pub fn get_power_ctl_ema()       -> u16 { MODULE.lock().power_ctl_ema }
