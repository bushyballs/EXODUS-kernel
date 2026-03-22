#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_VMX_EXIT_CTLS: u32 = 0x483;

pub struct State {
    pub vmx_exit_richness:   u16,
    pub vmx_exit_64bit_host: u16,
    pub vmx_exit_ack_irq:    u16,
    pub vmx_exit_ema:        u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    vmx_exit_richness:   0,
    vmx_exit_64bit_host: 0,
    vmx_exit_ack_irq:    0,
    vmx_exit_ema:        0,
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
    serial_println!("[msr_ia32_vmx_exit_ctls] init");
    // No MSR read at init; tick() handles sampling under the age gate.
}

pub fn tick(age: u32) {
    if age % 20000 != 0 {
        return;
    }
    if !has_vmx() {
        return;
    }

    let (_lo, hi) = read_msr(MSR_IA32_VMX_EXIT_CTLS);

    // vmx_exit_richness: popcount of hi (allowed-1 bits), scaled 0-1000
    let richness_count = popcount(hi);
    let vmx_exit_richness = scale_popcount(richness_count);

    // vmx_exit_64bit_host: bit 15 of hi — host address-space size (64-bit host)
    let vmx_exit_64bit_host: u16 = if (hi >> 15) & 1 == 1 { 1000 } else { 0 };

    // vmx_exit_ack_irq: bit 21 of hi — acknowledge interrupt on exit
    let vmx_exit_ack_irq: u16 = if (hi >> 21) & 1 == 1 { 1000 } else { 0 };

    let mut state = MODULE.lock();
    let vmx_exit_ema = ema(state.vmx_exit_ema, vmx_exit_richness);

    state.vmx_exit_richness   = vmx_exit_richness;
    state.vmx_exit_64bit_host = vmx_exit_64bit_host;
    state.vmx_exit_ack_irq    = vmx_exit_ack_irq;
    state.vmx_exit_ema        = vmx_exit_ema;

    serial_println!(
        "[msr_ia32_vmx_exit_ctls] age={} hi={:#010x} \
         richness={} 64bit_host={} ack_irq={} ema={}",
        age, hi,
        vmx_exit_richness, vmx_exit_64bit_host, vmx_exit_ack_irq, vmx_exit_ema
    );
}

// ── getters ───────────────────────────────────────────────────────────────────

pub fn get_vmx_exit_richness() -> u16 {
    MODULE.lock().vmx_exit_richness
}

pub fn get_vmx_exit_64bit_host() -> u16 {
    MODULE.lock().vmx_exit_64bit_host
}

pub fn get_vmx_exit_ack_irq() -> u16 {
    MODULE.lock().vmx_exit_ack_irq
}

pub fn get_vmx_exit_ema() -> u16 {
    MODULE.lock().vmx_exit_ema
}
