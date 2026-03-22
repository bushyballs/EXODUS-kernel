#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { avxvnni_support: u16, avx512_bf16_support: u16, hreset_support: u16, ext_feat_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { avxvnni_support: 0, avx512_bf16_support: 0, hreset_support: 0, ext_feat_ema: 0 });

pub fn init() { serial_println!("[cpuid_structured_ext_leaf] init"); }
pub fn tick(age: u32) {
    if age % 5500 != 0 { return; }
    let eax_out: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 7u32 => eax_out,
            inout("ecx") 1u32 => _,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    let avxvnni_support: u16 = if (eax_out >> 4) & 1 != 0 { 1000 } else { 0 };
    let avx512_bf16_support: u16 = if (eax_out >> 5) & 1 != 0 { 1000 } else { 0 };
    let hreset_support: u16 = if (eax_out >> 22) & 1 != 0 { 1000 } else { 0 };
    let composite: u16 = (avxvnni_support/4).saturating_add(avx512_bf16_support/4).saturating_add(hreset_support/2);
    let mut s = MODULE.lock();
    let ema = ((s.ext_feat_ema as u32).wrapping_mul(7).saturating_add(composite as u32)/8).min(1000) as u16;
    s.avxvnni_support = avxvnni_support; s.avx512_bf16_support = avx512_bf16_support; s.hreset_support = hreset_support; s.ext_feat_ema = ema;
    serial_println!("[cpuid_structured_ext_leaf] age={} eax7.1={:#010x} avxvnni={} bf16={} hreset={} ema={}", age, eax_out, avxvnni_support, avx512_bf16_support, hreset_support, ema);
}
pub fn get_avxvnni_support() -> u16 { MODULE.lock().avxvnni_support }
pub fn get_avx512_bf16_support() -> u16 { MODULE.lock().avx512_bf16_support }
pub fn get_hreset_support() -> u16 { MODULE.lock().hreset_support }
pub fn get_ext_feat_ema() -> u16 { MODULE.lock().ext_feat_ema }
