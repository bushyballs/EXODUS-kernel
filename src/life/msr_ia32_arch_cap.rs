#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { rdcl_no: u16, ibrs_all: u16, mds_no: u16, arch_cap_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { rdcl_no: 0, ibrs_all: 0, mds_no: 0, arch_cap_ema: 0 });

fn has_arch_cap() -> bool {
    let edx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 7u32 => _, inout("ecx") 0u32 => _, lateout("edx") edx, options(nostack,nomem)); }
    (edx >> 29) & 1 == 1
}
pub fn init() { serial_println!("[msr_ia32_arch_cap] init"); }
pub fn tick(age: u32) {
    if age % 6000 != 0 { return; }
    if !has_arch_cap() { return; }
    let lo: u32; let _hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x10Au32, out("eax") lo, out("edx") _hi, options(nostack,nomem)); }
    let rdcl_no: u16 = if lo & 1 != 0 { 1000 } else { 0 };
    let ibrs_all: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    let mds_no: u16 = if (lo >> 5) & 1 != 0 { 1000 } else { 0 };
    let composite: u16 = (rdcl_no/4).saturating_add(ibrs_all/4).saturating_add(mds_no/2);
    let mut s = MODULE.lock();
    let ema = ((s.arch_cap_ema as u32).wrapping_mul(7).saturating_add(composite as u32)/8).min(1000) as u16;
    s.rdcl_no = rdcl_no; s.ibrs_all = ibrs_all; s.mds_no = mds_no; s.arch_cap_ema = ema;
    serial_println!("[msr_ia32_arch_cap] age={} lo={:#010x} rdcl_no={} ibrs_all={} mds_no={} ema={}", age, lo, rdcl_no, ibrs_all, mds_no, ema);
}
pub fn get_rdcl_no() -> u16 { MODULE.lock().rdcl_no }
pub fn get_ibrs_all() -> u16 { MODULE.lock().ibrs_all }
pub fn get_mds_no() -> u16 { MODULE.lock().mds_no }
pub fn get_arch_cap_ema() -> u16 { MODULE.lock().arch_cap_ema }
