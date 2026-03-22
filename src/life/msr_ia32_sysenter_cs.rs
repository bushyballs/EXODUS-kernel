#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    sysenter_cs_valid: u16,
    sysenter_privilege_ring: u16,
    sysenter_table_indicator: u16,
    syscall_topology_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    sysenter_cs_valid: 0,
    sysenter_privilege_ring: 0,
    sysenter_table_indicator: 0,
    syscall_topology_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_sysenter_cs] init"); }

pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x174u32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    // SYSENTER_CS_MSR: bits[15:0] = target CS selector
    let cs = lo & 0xFFFF;
    let sysenter_cs_valid: u16 = if cs != 0 { 1000 } else { 0 };

    // bits[1:0]: RPL (requested privilege level) — should be 0 (kernel)
    let rpl = cs & 0x3;
    let sysenter_privilege_ring: u16 = ((rpl * 333).min(1000)) as u16;

    // bit 2: TI (table indicator — 0=GDT, 1=LDT)
    let ti = (cs >> 2) & 1;
    let sysenter_table_indicator: u16 = if ti != 0 { 1000 } else { 0 };

    let composite = (sysenter_cs_valid as u32 / 2)
        .saturating_add(1000u32.saturating_sub(sysenter_privilege_ring as u32) / 4)
        .saturating_add(1000u32.saturating_sub(sysenter_table_indicator as u32) / 4);

    let mut s = MODULE.lock();
    let syscall_topology_ema = ((s.syscall_topology_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.sysenter_cs_valid = sysenter_cs_valid;
    s.sysenter_privilege_ring = sysenter_privilege_ring;
    s.sysenter_table_indicator = sysenter_table_indicator;
    s.syscall_topology_ema = syscall_topology_ema;

    serial_println!("[msr_ia32_sysenter_cs] age={} valid={} ring={} ti={} ema={}",
        age, sysenter_cs_valid, sysenter_privilege_ring, sysenter_table_indicator, syscall_topology_ema);
}

pub fn get_sysenter_cs_valid()          -> u16 { MODULE.lock().sysenter_cs_valid }
pub fn get_sysenter_privilege_ring()    -> u16 { MODULE.lock().sysenter_privilege_ring }
pub fn get_sysenter_table_indicator()   -> u16 { MODULE.lock().sysenter_table_indicator }
pub fn get_syscall_topology_ema()       -> u16 { MODULE.lock().syscall_topology_ema }
