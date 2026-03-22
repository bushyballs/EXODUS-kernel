#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    evtsel_rmid: u16,
    evtsel_event_id: u16,
    evtsel_active: u16,
    evtsel_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    evtsel_rmid: 0,
    evtsel_event_id: 0,
    evtsel_active: 0,
    evtsel_ema: 0,
});

fn has_rdt() -> bool {
    let max_leaf: u32;
    unsafe { asm!("push rbx", "cpuid", "pop rbx", inout("eax") 0u32 => max_leaf, lateout("ecx") _, lateout("edx") _, options(nostack, nomem)); }
    if max_leaf < 0x0F { return false; }
    let edx_0f: u32;
    unsafe { asm!("push rbx", "cpuid", "pop rbx", inout("eax") 0x0Fu32 => _, in("ecx") 0u32, lateout("ecx") _, lateout("edx") edx_0f, options(nostack, nomem)); }
    (edx_0f >> 1) & 1 != 0
}

pub fn init() { serial_println!("[msr_ia32_qm_evtsel] init"); }

pub fn tick(age: u32) {
    if age % 2000 != 0 { return; }
    if !has_rdt() { return; }

    // IA32_QM_EVTSEL MSR 0xC8F: bits[9:0]=RMID, bits[15:8]=EventID in hi nibble
    // Actually: lo bits[9:0]=RMID, bits[31:16]=reserved; EventID in bits[7:0] of separate field
    // Intel SDM: bits[9:0]=RMID, bits[63:32] contains EventID type
    let lo: u32; let hi: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0xC8Fu32, out("eax") lo, out("edx") hi, options(nostack, nomem));
    }

    let rmid = lo & 0x3FF;
    let event_id = hi & 0xFF;

    let evtsel_rmid: u16 = ((rmid * 1000 / 1024) as u16).min(1000);
    let evtsel_event_id: u16 = ((event_id * 4) as u16).min(1000);
    let evtsel_active: u16 = if rmid != 0 || event_id != 0 { 1000 } else { 0 };

    let composite = (evtsel_rmid as u32 / 4)
        .saturating_add(evtsel_event_id as u32 / 4)
        .saturating_add(evtsel_active as u32 / 2);

    let mut s = MODULE.lock();
    let evtsel_ema = (((s.evtsel_ema as u32).wrapping_mul(7)).wrapping_add(composite) / 8) as u16;

    s.evtsel_rmid = evtsel_rmid;
    s.evtsel_event_id = evtsel_event_id;
    s.evtsel_active = evtsel_active;
    s.evtsel_ema = evtsel_ema;

    serial_println!("[msr_ia32_qm_evtsel] age={} rmid={} event={} active={} ema={}",
        age, evtsel_rmid, evtsel_event_id, evtsel_active, evtsel_ema);
}

pub fn get_evtsel_rmid() -> u16 { MODULE.lock().evtsel_rmid }
pub fn get_evtsel_event_id() -> u16 { MODULE.lock().evtsel_event_id }
pub fn get_evtsel_active() -> u16 { MODULE.lock().evtsel_active }
pub fn get_evtsel_ema() -> u16 { MODULE.lock().evtsel_ema }
