#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_VMX_ENTRY_CTLS: u32 = 0x484;
const TICK_GATE: u32 = 20000;

pub struct State {
    vmx_entry_richness: u16,
    vmx_entry_64bit_guest: u16,
    vmx_entry_smm: u16,
    vmx_entry_ema: u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    vmx_entry_richness: 0,
    vmx_entry_64bit_guest: 0,
    vmx_entry_smm: 0,
    vmx_entry_ema: 0,
});

fn popcount(mut v: u32) -> u32 {
    let mut count: u32 = 0;
    while v != 0 {
        count += v & 1;
        v >>= 1;
    }
    count
}

fn has_vmx_support() -> bool {
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

fn read_msr(addr: u32) -> (u32, u32) {
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
    (lo, hi)
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

pub fn init() {
    if !has_vmx_support() {
        serial_println!("[msr_ia32_vmx_entry_ctls] VMX not supported; signals stay 0");
        return;
    }

    let (_lo, hi) = read_msr(MSR_IA32_VMX_ENTRY_CTLS);

    let count = popcount(hi);
    let richness = ((count * 1000) / 32).min(1000) as u16;
    let guest_64 = if (hi >> 11) & 1 == 1 { 1000u16 } else { 0u16 };
    let smm = if (hi >> 13) & 1 == 1 { 1000u16 } else { 0u16 };

    let mut state = MODULE.lock();
    state.vmx_entry_richness = richness;
    state.vmx_entry_64bit_guest = guest_64;
    state.vmx_entry_smm = smm;
    state.vmx_entry_ema = richness;

    serial_println!(
        "[msr_ia32_vmx_entry_ctls] init: richness={} 64bit_guest={} smm={} ema={}",
        richness, guest_64, smm, richness
    );
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    if !has_vmx_support() {
        return;
    }

    let (_lo, hi) = read_msr(MSR_IA32_VMX_ENTRY_CTLS);

    let count = popcount(hi);
    let richness = ((count * 1000) / 32).min(1000) as u16;
    let guest_64 = if (hi >> 11) & 1 == 1 { 1000u16 } else { 0u16 };
    let smm = if (hi >> 13) & 1 == 1 { 1000u16 } else { 0u16 };

    let mut state = MODULE.lock();
    let new_ema = ema(state.vmx_entry_ema, richness);
    state.vmx_entry_richness = richness;
    state.vmx_entry_64bit_guest = guest_64;
    state.vmx_entry_smm = smm;
    state.vmx_entry_ema = new_ema;

    serial_println!(
        "[msr_ia32_vmx_entry_ctls] tick {}: richness={} 64bit_guest={} smm={} ema={}",
        age, richness, guest_64, smm, new_ema
    );
}

pub fn get_vmx_entry_richness() -> u16 {
    MODULE.lock().vmx_entry_richness
}

pub fn get_vmx_entry_64bit_guest() -> u16 {
    MODULE.lock().vmx_entry_64bit_guest
}

pub fn get_vmx_entry_smm() -> u16 {
    MODULE.lock().vmx_entry_smm
}

pub fn get_vmx_entry_ema() -> u16 {
    MODULE.lock().vmx_entry_ema
}
