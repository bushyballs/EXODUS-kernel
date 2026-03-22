#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── Constants ─────────────────────────────────────────────────────────────────

const MSR_IA32_PKRS: u32 = 0x6E1;
const TICK_GATE: u32 = 3000;

// ── State ─────────────────────────────────────────────────────────────────────

struct PkrsState {
    pkrs_keys_restricted: u16,
    pkrs_write_disabled:  u16,
    pkrs_access_disabled: u16,
    pkrs_protection_ema:  u16,
}

static STATE: Mutex<PkrsState> = Mutex::new(PkrsState {
    pkrs_keys_restricted: 0,
    pkrs_write_disabled:  0,
    pkrs_access_disabled: 0,
    pkrs_protection_ema:  0,
});

// ── CPUID guard — PKS = Protection Keys for Supervisor Pages ──────────────────
// CPUID leaf 7, sub-leaf 0, ECX bit 6

fn has_pks() -> bool {
    let ecx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 7u32 => _,
            in("ecx") 0u32,
            lateout("ecx") ecx_val,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx_val >> 6) & 1 != 0
}

// ── MSR read ──────────────────────────────────────────────────────────────────

#[inline]
fn read_pkrs_lo() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") MSR_IA32_PKRS,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem),
        );
    }
    lo
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn popcount(mut v: u32) -> u32 {
    let mut c = 0u32;
    while v != 0 {
        c += v & 1;
        v >>= 1;
    }
    c
}

/// EMA: (old * 7 + new_val) / 8, all in u32, result cast to u16.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ── Public interface ───────────────────────────────────────────────────────────

pub fn init() {
    if !has_pks() {
        crate::serial_println!("[msr_ia32_pkrs] PKS not supported — module inactive");
        return;
    }
    crate::serial_println!("[msr_ia32_pkrs] init — PKS supported, IA32_PKRS MSR 0x6E1 active");
}

pub fn tick(age: u32) {
    // Gate: sample every 3000 ticks
    if age % TICK_GATE != 0 {
        return;
    }

    if !has_pks() {
        return;
    }

    // IA32_PKRS lo-word: 16 two-bit fields covering keys 0-15.
    //   bits[2i]   = AD (access disable) for key i
    //   bits[2i+1] = WD (write disable)  for key i
    let lo = read_pkrs_lo();

    // Build masks for all 16 even bits (AD) and 16 odd bits (WD).
    // Even bit positions: 0,2,4,...,30 → mask 0x5555_5555
    // Odd  bit positions: 1,3,5,...,31 → mask 0xAAAA_AAAA
    let ad_bits = lo & 0x5555_5555u32;  // access-disable bits
    let wd_bits = lo & 0xAAAA_AAAAu32;  // write-disable bits

    let ad_count = popcount(ad_bits);   // 0-16
    let wd_count = popcount(wd_bits);   // 0-16

    // pkrs_keys_restricted: any key with either AD or WD set
    let restricted_count = popcount(lo & 0xFFFF_FFFFu32 & {
        // A 2-bit field is restricted when either bit is set.
        // For each pair i: restricted if (AD_i | WD_i) != 0.
        // Compute OR of each pair into the low bit of that pair:
        // spread WD (odd) down by 1, OR with AD (even), then mask even bits.
        let wd_shifted = wd_bits >> 1;
        (ad_bits | wd_shifted) & 0x5555_5555u32
    });

    // Scale: count * 62, clamp to 1000.  16 * 62 = 992 (near-max by design).
    let keys_restricted  = ((restricted_count * 62).min(1000)) as u16;
    let write_disabled   = ((wd_count          * 62).min(1000)) as u16;
    let access_disabled  = ((ad_count          * 62).min(1000)) as u16;

    // Composite score for EMA: average of the three signals (integer division).
    let composite = (keys_restricted / 3)
        .saturating_add(write_disabled / 3)
        .saturating_add(access_disabled / 3);

    let mut s = STATE.lock();
    s.pkrs_keys_restricted = keys_restricted;
    s.pkrs_write_disabled  = write_disabled;
    s.pkrs_access_disabled = access_disabled;
    s.pkrs_protection_ema  = ema(s.pkrs_protection_ema, composite);

    crate::serial_println!(
        "[msr_ia32_pkrs] age={} restricted={} write_dis={} access_dis={} prot_ema={}",
        age,
        s.pkrs_keys_restricted,
        s.pkrs_write_disabled,
        s.pkrs_access_disabled,
        s.pkrs_protection_ema,
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_pkrs_keys_restricted() -> u16 {
    STATE.lock().pkrs_keys_restricted
}

pub fn get_pkrs_write_disabled() -> u16 {
    STATE.lock().pkrs_write_disabled
}

pub fn get_pkrs_access_disabled() -> u16 {
    STATE.lock().pkrs_access_disabled
}

pub fn get_pkrs_protection_ema() -> u16 {
    STATE.lock().pkrs_protection_ema
}
