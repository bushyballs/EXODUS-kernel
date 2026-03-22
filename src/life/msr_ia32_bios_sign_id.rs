#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { ucode_rev_lo: u16, ucode_rev_hi: u16, ucode_loaded: u16, ucode_rev_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { ucode_rev_lo:0, ucode_rev_hi:0, ucode_loaded:0, ucode_rev_ema:0 });

pub fn init() { serial_println!("[msr_ia32_bios_sign_id] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    // Write 0 first to latch the microcode revision (Intel SDM requirement)
    unsafe {
        asm!(
            "xor eax, eax", "xor edx, edx",
            "mov ecx, 0x8B",
            "wrmsr",
            options(nostack, nomem),
        );
        asm!("push rbx", "cpuid", "pop rbx",
            inout("eax") 1u32 => _,
            lateout("ecx") _, lateout("edx") _,
            options(nostack, nomem),
        );
    }
    let lo: u32;
    let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x8Bu32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    // hi = microcode update revision; lo = 0 after CPUID latch
    let ucode_rev_lo = ((lo & 0xFFFF) * 1000 / 65535) as u16;
    let ucode_rev_hi = ((hi & 0xFFFF) * 1000 / 65535) as u16;
    // Any nonzero hi means microcode is loaded
    let ucode_loaded: u16 = if hi != 0 { 1000 } else { 0 };
    let composite = (ucode_rev_lo as u32/3).saturating_add(ucode_rev_hi as u32/3).saturating_add(ucode_loaded as u32/3);
    let mut s = MODULE.lock();
    let ucode_rev_ema = ((s.ucode_rev_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.ucode_rev_lo=ucode_rev_lo; s.ucode_rev_hi=ucode_rev_hi; s.ucode_loaded=ucode_loaded; s.ucode_rev_ema=ucode_rev_ema;
    serial_println!("[msr_ia32_bios_sign_id] age={} rev_lo={} rev_hi={} loaded={} ema={}", age, ucode_rev_lo, ucode_rev_hi, ucode_loaded, ucode_rev_ema);
}
pub fn get_ucode_rev_lo()  -> u16 { MODULE.lock().ucode_rev_lo }
pub fn get_ucode_rev_hi()  -> u16 { MODULE.lock().ucode_rev_hi }
pub fn get_ucode_loaded()  -> u16 { MODULE.lock().ucode_loaded }
pub fn get_ucode_rev_ema() -> u16 { MODULE.lock().ucode_rev_ema }
