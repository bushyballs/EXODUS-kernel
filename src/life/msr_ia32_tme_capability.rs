#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// IA32_TME_CAPABILITY MSR 0x981
// Guard: CPUID leaf 7 ECX bit 13 (TME supported)
//
// lo bit 0 = AES-XTS-128 supported
// lo bit 1 = AES-XTS-128-WITH-INTEGRITY supported
// lo bit 2 = AES-XTS-256 supported
// hi bits[3:0] (= original bits[35:32]) = number of TME encryption keys supported (minus 1)
//
// Signals (all u16, 0-1000):
//   tme_aes128      — bit 0 of lo: 0 or 1000
//   tme_aes256      — bit 2 of lo: 0 or 1000
//   tme_algo_count  — popcount(lo & 0x7) * 333, clamped to 1000
//   tme_capability_ema — EMA of (aes128/4 + aes256/4 + algo_count/2)
//
// Tick gate: every 8000 ticks

struct State {
    tme_aes128:         u16,
    tme_aes256:         u16,
    tme_algo_count:     u16,
    tme_capability_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    tme_aes128:         0,
    tme_aes256:         0,
    tme_algo_count:     0,
    tme_capability_ema: 0,
});

fn popcount(mut v: u32) -> u32 {
    let mut c = 0u32;
    while v != 0 {
        c += v & 1;
        v >>= 1;
    }
    c
}

fn has_tme() -> bool {
    let ecx: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 7u32 => _,
            inout("ecx") 0u32 => ecx,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    (ecx >> 13) & 1 == 1
}

pub fn init() {
    serial_println!("[msr_ia32_tme_capability] init");
}

pub fn tick(age: u32) {
    if age % 8000 != 0 {
        return;
    }
    if !has_tme() {
        serial_println!("[msr_ia32_tme_capability] age={} TME not supported by CPUID", age);
        return;
    }

    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x981u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    // bit 0 — AES-XTS-128
    let tme_aes128: u16 = if lo & 1 != 0 { 1000 } else { 0 };

    // bit 2 — AES-XTS-256
    let tme_aes256: u16 = if (lo >> 2) & 1 != 0 { 1000 } else { 0 };

    // popcount of lo & 0x7 (bits 0-2: three algorithm flags), * 333, clamped 1000
    let algo_bits = lo & 0x7;
    let pc = popcount(algo_bits);
    let tme_algo_count: u16 = (pc.saturating_mul(333)).min(1000) as u16;

    // composite for EMA: aes128/4 + aes256/4 + algo_count/2
    let composite: u16 = (tme_aes128 / 4)
        .saturating_add(tme_aes256 / 4)
        .saturating_add(tme_algo_count / 2);

    let mut s = MODULE.lock();
    let ema: u16 = ((s.tme_capability_ema as u32)
        .wrapping_mul(7)
        .saturating_add(composite as u32)
        / 8) as u16;

    s.tme_aes128         = tme_aes128;
    s.tme_aes256         = tme_aes256;
    s.tme_algo_count     = tme_algo_count;
    s.tme_capability_ema = ema;

    serial_println!(
        "[msr_ia32_tme_capability] age={} lo={:#010x} aes128={} aes256={} algo_count={} ema={}",
        age, lo, tme_aes128, tme_aes256, tme_algo_count, ema
    );
}

pub fn get_tme_aes128()         -> u16 { MODULE.lock().tme_aes128 }
pub fn get_tme_aes256()         -> u16 { MODULE.lock().tme_aes256 }
pub fn get_tme_algo_count()     -> u16 { MODULE.lock().tme_algo_count }
pub fn get_tme_capability_ema() -> u16 { MODULE.lock().tme_capability_ema }
