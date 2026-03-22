#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_VMX_PINBASED_CTLS: u32 = 0x481;

pub struct State {
    pub vmx_pin_allowed1: u16,
    pub vmx_pin_must_be1: u16,
    pub vmx_pin_nmi_ctrl: u16,
    pub vmx_pin_ema:      u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    vmx_pin_allowed1: 0,
    vmx_pin_must_be1: 0,
    vmx_pin_nmi_ctrl: 0,
    vmx_pin_ema:      0,
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
    let mut count: u32 = 0;
    while v != 0 {
        count += v & 1;
        v >>= 1;
    }
    count
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
    ((count * 1000 / 8).min(1000)) as u16
}

fn compute_nmi_ctrl(hi: u32) -> u16 {
    let nmi_exit = (hi >> 3) & 1;
    let virt_nmi = (hi >> 5) & 1;
    match nmi_exit + virt_nmi {
        2 => 1000,
        1 => 500,
        _ => 0,
    }
}

fn sample() {
    let (lo, hi) = read_msr(MSR_IA32_VMX_PINBASED_CTLS);

    let vmx_pin_allowed1 = scale_popcount(popcount(hi));
    let vmx_pin_must_be1 = scale_popcount(popcount(lo & hi));
    let vmx_pin_nmi_ctrl = compute_nmi_ctrl(hi);

    let mut state = MODULE.lock();
    let vmx_pin_ema = ema(state.vmx_pin_ema, vmx_pin_allowed1);

    state.vmx_pin_allowed1 = vmx_pin_allowed1;
    state.vmx_pin_must_be1 = vmx_pin_must_be1;
    state.vmx_pin_nmi_ctrl = vmx_pin_nmi_ctrl;
    state.vmx_pin_ema      = vmx_pin_ema;
}

// ── public interface ──────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("[msr_ia32_vmx_pinbased_ctls] init");
    if !has_vmx() {
        serial_println!("[msr_ia32_vmx_pinbased_ctls] VMX not supported, skipping");
        return;
    }
    sample();
    let state = MODULE.lock();
    serial_println!(
        "[msr_ia32_vmx_pinbased_ctls] allowed1={} must_be1={} nmi_ctrl={} ema={}",
        state.vmx_pin_allowed1,
        state.vmx_pin_must_be1,
        state.vmx_pin_nmi_ctrl,
        state.vmx_pin_ema
    );
}

pub fn tick(age: u32) {
    if age % 20000 != 0 {
        return;
    }
    if !has_vmx() {
        return;
    }
    sample();
    let state = MODULE.lock();
    serial_println!(
        "[msr_ia32_vmx_pinbased_ctls] age={} allowed1={} must_be1={} nmi_ctrl={} ema={}",
        age,
        state.vmx_pin_allowed1,
        state.vmx_pin_must_be1,
        state.vmx_pin_nmi_ctrl,
        state.vmx_pin_ema
    );
}

// ── getters ───────────────────────────────────────────────────────────────────

pub fn get_vmx_pin_allowed1() -> u16 {
    MODULE.lock().vmx_pin_allowed1
}

pub fn get_vmx_pin_must_be1() -> u16 {
    MODULE.lock().vmx_pin_must_be1
}

pub fn get_vmx_pin_nmi_ctrl() -> u16 {
    MODULE.lock().vmx_pin_nmi_ctrl
}

pub fn get_vmx_pin_ema() -> u16 {
    MODULE.lock().vmx_pin_ema
}
