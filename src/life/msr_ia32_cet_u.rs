#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { cet_u_shstk: u16, cet_u_ibt: u16, cet_u_endbr: u16, cet_u_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { cet_u_shstk:0, cet_u_ibt:0, cet_u_endbr:0, cet_u_ema:0 });

#[inline]
fn has_cet() -> bool {
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
    // CPUID 7.0 ECX bit 7: CET_SS (user shadow stack)
    (ecx >> 7) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_cet_u] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_cet() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x6A0u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bit 0: SH_STK_EN — user shadow stack enable
    let cet_u_shstk: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bit 2: ENDBR_EN — enable ENDBRANCH at indirect call/jmp targets
    let cet_u_endbr: u16 = if (lo >> 2) & 1 != 0 { 1000 } else { 0 };
    // bit 4: NO_TRACK_EN — track indirect branches
    let cet_u_ibt: u16 = if (lo >> 4) & 1 != 0 { 1000 } else { 0 };
    let composite = (cet_u_shstk as u32/3).saturating_add(cet_u_endbr as u32/3).saturating_add(cet_u_ibt as u32/3);
    let mut s = MODULE.lock();
    let cet_u_ema = ((s.cet_u_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.cet_u_shstk=cet_u_shstk; s.cet_u_ibt=cet_u_ibt; s.cet_u_endbr=cet_u_endbr; s.cet_u_ema=cet_u_ema;
    serial_println!("[msr_ia32_cet_u] age={} shstk={} endbr={} ibt={} ema={}", age, cet_u_shstk, cet_u_endbr, cet_u_ibt, cet_u_ema);
}
pub fn get_cet_u_shstk() -> u16 { MODULE.lock().cet_u_shstk }
pub fn get_cet_u_ibt()   -> u16 { MODULE.lock().cet_u_ibt }
pub fn get_cet_u_endbr() -> u16 { MODULE.lock().cet_u_endbr }
pub fn get_cet_u_ema()   -> u16 { MODULE.lock().cet_u_ema }
