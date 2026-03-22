#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { vmx_supported: u16, vmx_memory_type: u16, vmx_ins_outs: u16, vmx_cap_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { vmx_supported:0, vmx_memory_type:0, vmx_ins_outs:0, vmx_cap_ema:0 });

#[inline]
fn has_vmx() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 5) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_vmx_basic] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_vmx() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x480u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let vmx_supported: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let mem_type = (hi >> 18) & 0xF;
    let vmx_memory_type = ((mem_type * 166).min(1000)) as u16;
    let vmx_ins_outs: u16 = if (hi >> 22) & 1 != 0 { 1000 } else { 0 };
    let composite = (vmx_supported as u32/3).saturating_add(vmx_memory_type as u32/3).saturating_add(vmx_ins_outs as u32/3);
    let mut s = MODULE.lock();
    let vmx_cap_ema = ((s.vmx_cap_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.vmx_supported=vmx_supported; s.vmx_memory_type=vmx_memory_type; s.vmx_ins_outs=vmx_ins_outs; s.vmx_cap_ema=vmx_cap_ema;
    serial_println!("[msr_ia32_vmx_basic] age={} supported={} memtype={} ins_outs={} ema={}", age, vmx_supported, vmx_memory_type, vmx_ins_outs, vmx_cap_ema);
}
pub fn get_vmx_supported()    -> u16 { MODULE.lock().vmx_supported }
pub fn get_vmx_memory_type()  -> u16 { MODULE.lock().vmx_memory_type }
pub fn get_vmx_ins_outs()     -> u16 { MODULE.lock().vmx_ins_outs }
pub fn get_vmx_cap_ema()      -> u16 { MODULE.lock().vmx_cap_ema }
