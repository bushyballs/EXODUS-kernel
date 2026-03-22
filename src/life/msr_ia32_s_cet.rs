#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { cet_s_shstk: u16, cet_s_endbr: u16, cet_s_ibt: u16, cet_s_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { cet_s_shstk:0, cet_s_endbr:0, cet_s_ibt:0, cet_s_ema:0 });

#[inline]
fn has_cet_ss() -> bool {
    let ecx: u32;
    unsafe {
        asm!(
            "push rbx",
            "mov eax, 7",
            "xor ecx, ecx",
            "cpuid",
            "pop rbx",
            lateout("ecx") ecx,
            lateout("eax") _, lateout("edx") _,
            options(nostack, nomem),
        );
    }
    // CPUID 7.0 ECX bit 7: CET_SS
    (ecx >> 7) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_s_cet] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_cet_ss() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x6A2u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bit 0: SH_STK_EN — supervisor shadow stack enable
    let cet_s_shstk: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bit 2: ENDBR_EN — supervisor indirect branch tracking
    let cet_s_endbr: u16 = if (lo >> 2) & 1 != 0 { 1000 } else { 0 };
    // bit 4: NO_TRACK_EN — supervisor indirect branch target enforcement
    let cet_s_ibt: u16 = if (lo >> 4) & 1 != 0 { 1000 } else { 0 };
    let composite = (cet_s_shstk as u32/3).saturating_add(cet_s_endbr as u32/3).saturating_add(cet_s_ibt as u32/3);
    let mut s = MODULE.lock();
    let cet_s_ema = ((s.cet_s_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.cet_s_shstk=cet_s_shstk; s.cet_s_endbr=cet_s_endbr; s.cet_s_ibt=cet_s_ibt; s.cet_s_ema=cet_s_ema;
    serial_println!("[msr_ia32_s_cet] age={} shstk={} endbr={} ibt={} ema={}", age, cet_s_shstk, cet_s_endbr, cet_s_ibt, cet_s_ema);
}
pub fn get_cet_s_shstk() -> u16 { MODULE.lock().cet_s_shstk }
pub fn get_cet_s_endbr() -> u16 { MODULE.lock().cet_s_endbr }
pub fn get_cet_s_ibt()   -> u16 { MODULE.lock().cet_s_ibt }
pub fn get_cet_s_ema()   -> u16 { MODULE.lock().cet_s_ema }
