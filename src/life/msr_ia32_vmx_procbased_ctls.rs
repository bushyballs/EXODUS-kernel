#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_VMX_PROCBASED_CTLS: u32 = 0x482;

pub struct State {
    pub vmx_proc_richness:  u16,
    pub vmx_proc_mandatory: u16,
    pub vmx_proc_tpr_shadow: u16,
    pub vmx_proc_ema:       u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    vmx_proc_richness:  0,
    vmx_proc_mandatory: 0,
    vmx_proc_tpr_shadow: 0,
    vmx_proc_ema:       0,
});

// ── guard ─────────────────────────────────────────────────────────────────────

fn has_vmx() -> bool {
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") ecx,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    (ecx >> 5) & 1 == 1
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn popcount(mut v: u32) -> u32 {
    v = v - ((v >> 1) & 0x5555_5555);
    v = (v & 0x3333_3333) + ((v >> 2) & 0x3333_3333);
    v = (v + (v >> 4)) & 0x0f0f_0f0f;
    v = v.wrapping_mul(0x0101_0101) >> 24;
    v
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

fn scale_popcount(count: u32) -> u16 {
    ((count * 1000 / 32) as u16).min(1000)
}

// ── public interface ──────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("[msr_ia32_vmx_procbased_ctls] init");
    // No MSR read at init; tick() handles sampling under the age gate.
}

pub fn tick(age: u32) {
    if age % 20000 != 0 {
        return;
    }
    if !has_vmx() {
        return;
    }

    let (lo, hi) = read_msr(MSR_IA32_VMX_PROCBASED_CTLS);

    // vmx_proc_richness: popcount of hi (allowed-1 bits = controls that can be 1)
    let richness_count = popcount(hi);
    let vmx_proc_richness = scale_popcount(richness_count);

    // vmx_proc_mandatory: popcount of lo (allowed-0 bits; bits that must be 1)
    let mandatory_count = popcount(lo);
    let vmx_proc_mandatory = scale_popcount(mandatory_count);

    // vmx_proc_tpr_shadow: bit 21 of hi → TPR shadow available
    let vmx_proc_tpr_shadow: u16 = if (hi >> 21) & 1 == 1 { 1000 } else { 0 };

    let mut state = MODULE.lock();
    let vmx_proc_ema = ema(state.vmx_proc_ema, vmx_proc_richness);

    state.vmx_proc_richness  = vmx_proc_richness;
    state.vmx_proc_mandatory = vmx_proc_mandatory;
    state.vmx_proc_tpr_shadow = vmx_proc_tpr_shadow;
    state.vmx_proc_ema       = vmx_proc_ema;

    serial_println!(
        "[msr_ia32_vmx_procbased_ctls] age={} lo={:#010x} hi={:#010x} \
         richness={} mandatory={} tpr_shadow={} ema={}",
        age, lo, hi,
        vmx_proc_richness, vmx_proc_mandatory, vmx_proc_tpr_shadow, vmx_proc_ema
    );
}

// ── getters ───────────────────────────────────────────────────────────────────

pub fn get_vmx_proc_richness() -> u16 {
    MODULE.lock().vmx_proc_richness
}

pub fn get_vmx_proc_mandatory() -> u16 {
    MODULE.lock().vmx_proc_mandatory
}

pub fn get_vmx_proc_tpr_shadow() -> u16 {
    MODULE.lock().vmx_proc_tpr_shadow
}

pub fn get_vmx_proc_ema() -> u16 {
    MODULE.lock().vmx_proc_ema
}
