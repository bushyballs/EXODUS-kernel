#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const IA32_VMX_MISC: u32 = 0x485;
const TICK_GATE: u32 = 20000;

pub struct State {
    vmx_activity_states: u16,
    vmx_cr3_targets: u16,
    vmx_misc_features: u16,
    vmx_misc_ema: u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    vmx_activity_states: 0,
    vmx_cr3_targets: 0,
    vmx_misc_features: 0,
    vmx_misc_ema: 0,
});

fn popcount(mut v: u32) -> u32 {
    v = v - ((v >> 1) & 0x5555_5555);
    v = (v & 0x3333_3333) + ((v >> 2) & 0x3333_3333);
    v = (v + (v >> 4)) & 0x0f0f_0f0f;
    v = v.wrapping_mul(0x0101_0101) >> 24;
    v
}

fn vmx_supported() -> bool {
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "pop rbx",
            out("eax") _,
            out("ecx") ecx,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx >> 5) & 1 == 1
}

fn read_msr(addr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") addr,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

pub fn init() {
    serial_println!("[msr_ia32_vmx_misc] init");
    if !vmx_supported() {
        serial_println!("[msr_ia32_vmx_misc] VMX not supported — signals remain 0");
        return;
    }
    let raw = read_msr(IA32_VMX_MISC);
    let lo = raw as u32;

    let activity_bits = lo & 0x1f;
    let activity_count = popcount(activity_bits);
    let vmx_activity_states = ((activity_count * 1000) / 5).min(1000) as u16;

    let cr3_val = (lo >> 6) & 0xf;
    let vmx_cr3_targets = ((cr3_val * 1000) / 8).min(1000) as u16;

    let misc_bits = (lo >> 28) & 0x7;
    let misc_count = popcount(misc_bits);
    let vmx_misc_features = ((misc_count * 1000) / 3).min(1000) as u16;

    let vmx_misc_ema = vmx_activity_states;

    let mut s = MODULE.lock();
    s.vmx_activity_states = vmx_activity_states;
    s.vmx_cr3_targets = vmx_cr3_targets;
    s.vmx_misc_features = vmx_misc_features;
    s.vmx_misc_ema = vmx_misc_ema;

    serial_println!(
        "[msr_ia32_vmx_misc] init done: activity={} cr3={} features={} ema={}",
        vmx_activity_states,
        vmx_cr3_targets,
        vmx_misc_features,
        vmx_misc_ema,
    );
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    if !vmx_supported() {
        return;
    }

    let raw = read_msr(IA32_VMX_MISC);
    let lo = raw as u32;

    let activity_bits = lo & 0x1f;
    let activity_count = popcount(activity_bits);
    let new_activity_states = ((activity_count * 1000) / 5).min(1000) as u16;

    let cr3_val = (lo >> 6) & 0xf;
    let new_cr3_targets = ((cr3_val * 1000) / 8).min(1000) as u16;

    let misc_bits = (lo >> 28) & 0x7;
    let misc_count = popcount(misc_bits);
    let new_misc_features = ((misc_count * 1000) / 3).min(1000) as u16;

    let mut s = MODULE.lock();
    let new_ema = ema(s.vmx_misc_ema, new_activity_states);

    s.vmx_activity_states = new_activity_states;
    s.vmx_cr3_targets = new_cr3_targets;
    s.vmx_misc_features = new_misc_features;
    s.vmx_misc_ema = new_ema;

    serial_println!(
        "[msr_ia32_vmx_misc] tick {}: activity={} cr3={} features={} ema={}",
        age,
        new_activity_states,
        new_cr3_targets,
        new_misc_features,
        new_ema,
    );
}

pub fn get_vmx_activity_states() -> u16 {
    MODULE.lock().vmx_activity_states
}

pub fn get_vmx_cr3_targets() -> u16 {
    MODULE.lock().vmx_cr3_targets
}

pub fn get_vmx_misc_features() -> u16 {
    MODULE.lock().vmx_misc_features
}

pub fn get_vmx_misc_ema() -> u16 {
    MODULE.lock().vmx_misc_ema
}
