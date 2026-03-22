#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    vmcs_revision: u16,
    vmx_dual_monitor: u16,
    vmx_true_ctls: u16,
    msr_ia32_vmx_basic_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    vmcs_revision: 0,
    vmx_dual_monitor: 0,
    vmx_true_ctls: 0,
    msr_ia32_vmx_basic_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_vmx_basic] init"); }

pub fn tick(age: u32) {
    if age % 10000 != 0 { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x480u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    // VMX basic capability: VMCS revision, dual-monitor, memory type
    let vmcs_revision = ((lo & 0x7FFF_FFFF as u32) * 1000 / 2147483647).min(1000) as u16;
    let vmx_dual_monitor: u16 = if ((hi >> 14) & 1) != 0 { 1000 } else { 0 };
    let vmx_true_ctls: u16 = if ((hi >> 23) & 1) != 0 { 1000 } else { 0 };

    let composite = (vmcs_revision as u32 / 3)
        .saturating_add(vmx_dual_monitor as u32 / 3)
        .saturating_add(vmx_true_ctls as u32 / 3);

    let mut s = MODULE.lock();
    let msr_ia32_vmx_basic_ema = ((s.msr_ia32_vmx_basic_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.vmcs_revision = vmcs_revision;
    s.vmx_dual_monitor = vmx_dual_monitor;
    s.vmx_true_ctls = vmx_true_ctls;
    s.msr_ia32_vmx_basic_ema = msr_ia32_vmx_basic_ema;

    serial_println!("[msr_ia32_vmx_basic] age={} vmcs_revision={} vmx_dual_monitor={} vmx_true_ctls={} ema={}",
        age, vmcs_revision, vmx_dual_monitor, vmx_true_ctls, msr_ia32_vmx_basic_ema);
}

pub fn get_vmcs_revision() -> u16 { MODULE.lock().vmcs_revision }
pub fn get_vmx_dual_monitor() -> u16 { MODULE.lock().vmx_dual_monitor }
pub fn get_vmx_true_ctls() -> u16 { MODULE.lock().vmx_true_ctls }
pub fn get_msr_ia32_vmx_basic_ema() -> u16 { MODULE.lock().msr_ia32_vmx_basic_ema }
