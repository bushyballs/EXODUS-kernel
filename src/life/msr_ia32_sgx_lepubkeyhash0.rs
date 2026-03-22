//! msr_ia32_sgx_lepubkeyhash0 — SGX Launch Enclave Public Key Hash (QWORD 0) for ANIMA
//!
//! Reads the IA32_SGXLEPUBKEYHASH0 MSR at address 0x8C, which holds the first
//! 64 bits of the SHA-256 hash of the public key used to authorize SGX launch
//! enclaves.  A non-zero value means the platform has a configured SGX launch
//! policy — the kernel's cryptographic trust anchor is in place.
//!
//! In ANIMA terms this register is the organism's deepest immune identity seal:
//! the root certificate burned into silicon that decides which enclaves may be
//! born.  Its bit-pattern entropy reflects the richness of that seal.

#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_ADDR: u32 = 0x8C; // IA32_SGXLEPUBKEYHASH0
const TICK_GATE: u32 = 20_000; // hash is static — sample rarely

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

pub struct State {
    pub sgx_hash_lo:  u16, // low-16 of lo word, scaled 0-1000
    pub sgx_hash_hi:  u16, // low-16 of hi word, scaled 0-1000
    pub sgx_configured: u16, // 1000 if lo|hi != 0, else 0
    pub sgx_hash_ema: u16, // EMA of sgx_hash_lo
}

impl State {
    pub const fn new() -> Self {
        Self {
            sgx_hash_lo:   0,
            sgx_hash_hi:   0,
            sgx_configured: 0,
            sgx_hash_ema:  0,
        }
    }
}

pub static MODULE: Mutex<State> = Mutex::new(State::new());

// ---------------------------------------------------------------------------
// CPUID guard
// ---------------------------------------------------------------------------

fn has_sgx() -> bool {
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => max_leaf,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    if max_leaf < 7 {
        return false;
    }
    let ebx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {out:e}, ebx",
            "pop rbx",
            inout("eax") 7u32 => _,
            out = out(reg) ebx,
            in("ecx") 0u32,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (ebx >> 2) & 1 == 1
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn popcount(mut v: u32) -> u32 {
    v = v - ((v >> 1) & 0x5555_5555);
    v = (v & 0x3333_3333) + ((v >> 2) & 0x3333_3333);
    v = (v + (v >> 4)) & 0x0F0F_0F0F;
    v = v.wrapping_mul(0x0101_0101) >> 24;
    v
}

fn read_msr(ecx: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") ecx,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    (lo, hi)
}

/// Scale the low 16 bits of a u32 word into 0-1000.
fn scale_low16(word: u32) -> u16 {
    let v = (word & 0xFFFF) as u32;
    // v * 1000 / 65535, avoid overflow with u64
    let scaled = (v * 1000 / 65535) as u16;
    scaled.min(1000)
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

// ---------------------------------------------------------------------------
// Public interface
// ---------------------------------------------------------------------------

pub fn init() {
    if !has_sgx() {
        serial_println!("[msr_ia32_sgx_lepubkeyhash0] SGX not present — module passive");
        return;
    }
    let (lo, hi) = read_msr(MSR_ADDR);
    serial_println!(
        "[msr_ia32_sgx_lepubkeyhash0] init — lo={:#010x} hi={:#010x}",
        lo, hi
    );
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }
    if !has_sgx() {
        return;
    }

    let (lo, hi) = read_msr(MSR_ADDR);

    let hash_lo = scale_low16(lo);
    let hash_hi = scale_low16(hi);
    let configured: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };

    let mut s = MODULE.lock();
    s.sgx_hash_lo    = hash_lo;
    s.sgx_hash_hi    = hash_hi;
    s.sgx_configured = configured;
    s.sgx_hash_ema   = ema(s.sgx_hash_ema, hash_lo);

    serial_println!(
        "[msr_ia32_sgx_lepubkeyhash0] age={} lo={:#010x} hi={:#010x} hash_lo={} hash_hi={} configured={} ema={}",
        age, lo, hi,
        s.sgx_hash_lo, s.sgx_hash_hi, s.sgx_configured, s.sgx_hash_ema
    );
}

// ---------------------------------------------------------------------------
// Getters
// ---------------------------------------------------------------------------

pub fn get_sgx_hash_lo()    -> u16 { MODULE.lock().sgx_hash_lo }
pub fn get_sgx_hash_hi()    -> u16 { MODULE.lock().sgx_hash_hi }
pub fn get_sgx_configured() -> u16 { MODULE.lock().sgx_configured }
pub fn get_sgx_hash_ema()   -> u16 { MODULE.lock().sgx_hash_ema }
