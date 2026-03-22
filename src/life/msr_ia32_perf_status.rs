#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { current_fid: u16, vid_voltage: u16, freq_ratio: u16, perf_status_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { current_fid:0, vid_voltage:0, freq_ratio:0, perf_status_ema:0 });

pub fn init() { serial_println!("[msr_ia32_perf_status] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x198u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let fid_raw = lo & 0xFF;
    let current_fid = ((fid_raw * 1000) / 255).min(1000) as u16;
    let vid_raw = (lo >> 8) & 0xFF;
    let vid_voltage = ((vid_raw * 1000) / 255).min(1000) as u16;
    let hi_fid_raw = (hi >> 8) & 0xFF;
    let freq_ratio = ((hi_fid_raw * 1000) / 255).min(1000) as u16;
    let composite = (current_fid as u32/3).saturating_add(vid_voltage as u32/3).saturating_add(freq_ratio as u32/3);
    let mut s = MODULE.lock();
    let perf_status_ema = ((s.perf_status_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.current_fid=current_fid; s.vid_voltage=vid_voltage; s.freq_ratio=freq_ratio; s.perf_status_ema=perf_status_ema;
    serial_println!("[msr_ia32_perf_status] age={} fid={} vid={} ratio={} ema={}", age, current_fid, vid_voltage, freq_ratio, perf_status_ema);
}
pub fn get_current_fid()       -> u16 { MODULE.lock().current_fid }
pub fn get_vid_voltage()       -> u16 { MODULE.lock().vid_voltage }
pub fn get_freq_ratio()        -> u16 { MODULE.lock().freq_ratio }
pub fn get_perf_status_ema()   -> u16 { MODULE.lock().perf_status_ema }
