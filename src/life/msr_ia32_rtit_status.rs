#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_RTIT_STATUS: u32 = 0x571;

pub struct State {
    pt_filter_en:    u16,
    pt_error_stopped: u16,
    pt_packet_count: u16,
    pt_status_ema:   u16,
    supported:       bool,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    pt_filter_en:    0,
    pt_error_stopped: 0,
    pt_packet_count: 0,
    pt_status_ema:   0,
    supported:       false,
});

fn has_pt() -> bool {
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => max_leaf,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    if max_leaf < 0x14 {
        return false;
    }
    let leaf14_eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x14u32 => leaf14_eax,
            in("ecx") 0u32,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    leaf14_eax != 0
}

fn read_msr(addr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") addr,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    (lo, hi)
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

fn scale_packet_count(raw: u16) -> u16 {
    ((raw as u32 * 1000) / 65535).min(1000) as u16
}

pub fn init() {
    let supported = has_pt();
    let mut s = MODULE.lock();
    s.supported = supported;
    s.pt_filter_en    = 0;
    s.pt_error_stopped = 0;
    s.pt_packet_count = 0;
    s.pt_status_ema   = 0;
    serial_println!("[msr_ia32_rtit_status] init: PT supported={}", supported);
}

pub fn tick(age: u32) {
    if age % 3500 != 0 {
        return;
    }

    let mut s = MODULE.lock();

    if !s.supported {
        return;
    }

    let (lo, _hi) = read_msr(MSR_IA32_RTIT_STATUS);

    // bit 0: FilterEn
    let filter_en: u16 = if lo & (1 << 0) != 0 { 1000 } else { 0 };

    // bits 4 or 5: Error or Stopped
    let error_stopped: u16 = if lo & ((1 << 4) | (1 << 5)) != 0 { 1000 } else { 0 };

    // bits[31:16] of lo = PacketByteCnt low 16 bits
    let packet_raw: u16 = ((lo >> 16) & 0xFFFF) as u16;
    let packet_count: u16 = scale_packet_count(packet_raw);

    // EMA of pt_filter_en
    let status_ema: u16 = ema(s.pt_status_ema, filter_en);

    s.pt_filter_en     = filter_en;
    s.pt_error_stopped = error_stopped;
    s.pt_packet_count  = packet_count;
    s.pt_status_ema    = status_ema;

    serial_println!(
        "[msr_ia32_rtit_status] tick={} filter_en={} error_stopped={} packet_count={} ema={}",
        age, filter_en, error_stopped, packet_count, status_ema
    );
}

pub fn get_pt_filter_en() -> u16 {
    MODULE.lock().pt_filter_en
}

pub fn get_pt_error_stopped() -> u16 {
    MODULE.lock().pt_error_stopped
}

pub fn get_pt_packet_count() -> u16 {
    MODULE.lock().pt_packet_count
}

pub fn get_pt_status_ema() -> u16 {
    MODULE.lock().pt_status_ema
}
