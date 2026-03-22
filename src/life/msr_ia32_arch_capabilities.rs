#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { arch_rdcl_no: u16, arch_ibrs_all: u16, arch_mds_no: u16, arch_vuln_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { arch_rdcl_no:0, arch_ibrs_all:0, arch_mds_no:0, arch_vuln_ema:0 });

pub fn init() { serial_println!("[msr_ia32_arch_capabilities] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    // Check if ARCH_CAPABILITIES MSR is present (CPUID 7.0 EDX bit 29)
    let edx: u32;
    unsafe {
        asm!(
            "push rbx", "mov eax, 7", "xor ecx, ecx", "cpuid", "pop rbx",
            lateout("eax") _, lateout("ecx") _, lateout("edx") edx,
            options(nostack, nomem),
        );
    }
    if (edx >> 29) & 1 == 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x10Au32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bit 0: RDCL_NO — not vulnerable to Rogue Data Cache Load (Meltdown)
    let arch_rdcl_no: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bit 1: IBRS_ALL — enhanced IBRS always-on (better perf mitigation)
    let arch_ibrs_all: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    // bit 5: MDS_NO — not vulnerable to Microarchitectural Data Sampling
    let arch_mds_no: u16 = if (lo >> 5) & 1 != 0 { 1000 } else { 0 };
    // Higher = more HW fixes present = safer CPU generation
    let composite = (arch_rdcl_no as u32/3).saturating_add(arch_ibrs_all as u32/3).saturating_add(arch_mds_no as u32/3);
    let mut s = MODULE.lock();
    let arch_vuln_ema = ((s.arch_vuln_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.arch_rdcl_no=arch_rdcl_no; s.arch_ibrs_all=arch_ibrs_all; s.arch_mds_no=arch_mds_no; s.arch_vuln_ema=arch_vuln_ema;
    serial_println!("[msr_ia32_arch_capabilities] age={} rdcl_no={} ibrs_all={} mds_no={} ema={}", age, arch_rdcl_no, arch_ibrs_all, arch_mds_no, arch_vuln_ema);
}
pub fn get_arch_rdcl_no()   -> u16 { MODULE.lock().arch_rdcl_no }
pub fn get_arch_ibrs_all()  -> u16 { MODULE.lock().arch_ibrs_all }
pub fn get_arch_mds_no()    -> u16 { MODULE.lock().arch_mds_no }
pub fn get_arch_vuln_ema()  -> u16 { MODULE.lock().arch_vuln_ema }
