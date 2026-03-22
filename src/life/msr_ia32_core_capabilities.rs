#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { core_split_lock: u16, core_uc_lock_dis: u16, core_cap_ema: u16, core_pad: u16 }
static MODULE: Mutex<State> = Mutex::new(State { core_split_lock:0, core_uc_lock_dis:0, core_cap_ema:0, core_pad:0 });

pub fn init() { serial_println!("[msr_ia32_core_capabilities] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    // Check CORE_CAPABILITIES MSR presence (CPUID 7.0 EDX bit 30)
    let edx: u32;
    unsafe {
        asm!(
            "push rbx", "mov eax, 7", "xor ecx, ecx", "cpuid", "pop rbx",
            lateout("eax") _, lateout("ecx") _, lateout("edx") edx,
            options(nostack, nomem),
        );
    }
    if (edx >> 30) & 1 == 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0xCFu32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bit 5: SPLIT_LOCK_DISABLE — split lock detection/disable capability
    let core_split_lock: u16 = if (lo >> 5) & 1 != 0 { 1000 } else { 0 };
    // bit 6: UC_LOCK_DISABLE — UC-lock disable capability (bus-lock detection)
    let core_uc_lock_dis: u16 = if (lo >> 6) & 1 != 0 { 1000 } else { 0 };
    let composite = (core_split_lock as u32/2).saturating_add(core_uc_lock_dis as u32/2);
    let mut s = MODULE.lock();
    let core_cap_ema = ((s.core_cap_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.core_split_lock=core_split_lock; s.core_uc_lock_dis=core_uc_lock_dis; s.core_cap_ema=core_cap_ema;
    serial_println!("[msr_ia32_core_capabilities] age={} split_lock={} uc_lock={} ema={}", age, core_split_lock, core_uc_lock_dis, core_cap_ema);
}
pub fn get_core_split_lock() -> u16 { MODULE.lock().core_split_lock }
pub fn get_core_uc_lock_dis() -> u16 { MODULE.lock().core_uc_lock_dis }
pub fn get_core_cap_ema()    -> u16 { MODULE.lock().core_cap_ema }
