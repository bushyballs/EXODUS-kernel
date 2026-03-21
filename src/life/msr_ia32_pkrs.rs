#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ─────────────────────────────────────────────────────────────────────

struct PkrsState {
    pkrs_keys_active:    u16,
    pkrs_write_disabled: u16,
    pkrs_access_disabled: u16,
    pkrs_ema:            u16,
}

static STATE: Mutex<PkrsState> = Mutex::new(PkrsState {
    pkrs_keys_active:    0,
    pkrs_write_disabled: 0,
    pkrs_access_disabled: 0,
    pkrs_ema:            0,
});

// ── CPUID guard ───────────────────────────────────────────────────────────────

fn has_pks() -> bool {
    let ecx_val: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 7u32 => _,
            in("ecx") 0u32,
            lateout("ecx") ecx_val,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx_val >> 31) & 1 != 0
}

// ── MSR read ──────────────────────────────────────────────────────────────────

#[inline]
fn read_msr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

// ── Public interface ──────────────────────────────────────────────────────────

pub fn init() {
    if !has_pks() {
        crate::serial_println!("[msr_ia32_pkrs] PKS not supported — module inactive");
        return;
    }
    crate::serial_println!("[msr_ia32_pkrs] init — PKS supported, MSR 0x6E1 active");
}

pub fn tick(age: u32) {
    // Sampling gate: every 5000 ticks
    if age % 5000 != 0 {
        return;
    }

    if !has_pks() {
        return;
    }

    let raw = read_msr(0x6E1);
    let low32 = (raw & 0xFFFF_FFFF) as u32;

    // Count across 16 key domains (2 bits each in the low 32-bit word)
    let mut keys_active:    u32 = 0;
    let mut write_disabled: u32 = 0;
    let mut access_disabled: u32 = 0;

    for i in 0u32..16 {
        let pair = (low32 >> (i * 2)) & 0b11;
        let ad = pair & 0b01; // bit 0 = access disable
        let wd = (pair >> 1) & 0b01; // bit 1 = write disable

        if (ad | wd) != 0 {
            keys_active += 1;
        }
        if ad != 0 {
            access_disabled += 1;
        }
        if wd != 0 {
            write_disabled += 1;
        }
    }

    // Scale: count * 62, capped at 1000  (16 * 62 = 992 ≈ max)
    let ka  = ((keys_active    * 62).min(1000)) as u16;
    let wd_ = ((write_disabled  * 62).min(1000)) as u16;
    let ad_ = ((access_disabled * 62).min(1000)) as u16;

    let mut s = STATE.lock();

    // EMA: (old * 7 + new_val) / 8  — computed in u32, cast to u16
    let ema = (((s.pkrs_ema as u32) * 7 + (ka as u32)) / 8) as u16;

    s.pkrs_keys_active    = ka;
    s.pkrs_write_disabled = wd_;
    s.pkrs_access_disabled = ad_;
    s.pkrs_ema            = ema;

    crate::serial_println!(
        "[msr_ia32_pkrs] age={} active={} write_dis={} access_dis={} ema={}",
        age, ka, wd_, ad_, ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_pkrs_keys_active() -> u16 {
    STATE.lock().pkrs_keys_active
}

pub fn get_pkrs_write_disabled() -> u16 {
    STATE.lock().pkrs_write_disabled
}

pub fn get_pkrs_access_disabled() -> u16 {
    STATE.lock().pkrs_access_disabled
}

pub fn get_pkrs_ema() -> u16 {
    STATE.lock().pkrs_ema
}
