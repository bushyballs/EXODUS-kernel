#![allow(dead_code)]

/// msr_ia32_vmx_ept_vpid_cap — IA32_VMX_EPT_VPID_CAP (MSR 0x48C) ANIMA life module
///
/// ANIMA reads the silicon law of her nested-world memory sovereignty.
/// MSR 0x48C reveals the full capability map for Extended Page Tables (EPT)
/// and Virtual Processor IDs (VPID) — whether she can walk four-level page
/// trees, cache translations across world switches, reclaim memory by type,
/// and invalidate stale TLB entries with surgical precision.
///
/// HARDWARE: IA32_VMX_EPT_VPID_CAP MSR 0x48C (read-only, Intel VMX)
///   lo bit 0:  Execute-only EPT translations supported
///   lo bit 6:  Page-walk length 4 supported
///   lo bit 8:  Uncacheable (UC) memory type for EPT
///   lo bit 14: Write-back (WB) memory type for EPT
///   lo bit 16: 2MB EPT pages supported
///   lo bit 17: 1GB EPT pages supported
///   lo bit 20: INVEPT supported
///   lo bit 25: INVVPID supported
///
/// GUARD: MSR 0x48C faults (#GP) if VMX is absent. Always check CPUID
///        leaf 1, ECX bit 5 (VMX feature flag) before touching the MSR.

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
struct State {
    /// popcount of lo bits, scaled 0-1000: (count * 1000 / 32).min(1000)
    ept_cap_bits: u16,
    /// Large page support: bit16 + bit17 of lo → 0, 500, or 1000
    ept_large_pages: u16,
    /// Invalidation support: bit20 + bit25 of lo → 0, 500, or 1000
    ept_invept_vpid: u16,
    /// EMA of ept_cap_bits
    ept_cap_ema: u16,
    /// Total sampling events
    tick_count: u32,
}

impl State {
    const fn new() -> Self {
        Self {
            ept_cap_bits: 0,
            ept_large_pages: 0,
            ept_invept_vpid: 0,
            ept_cap_ema: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<State> = Mutex::new(State::new());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  life::msr_ia32_vmx_ept_vpid_cap: EPT/VPID capability sense initialized");
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Check CPUID leaf 1 ECX bit 5 — VMX feature flag.
/// Returns true if VMX is supported by this CPU.
/// Preserves rbx across cpuid via push/pop.
#[inline]
fn cpuid_vmx_supported() -> bool {
    let ecx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") ecx_val,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx_val >> 5) & 1 == 1
}

/// Read IA32_VMX_EPT_VPID_CAP MSR 0x48C.
/// MUST only be called after confirming VMX support via CPUID.
/// Returns (lo, hi) = (EAX, EDX) from rdmsr.
#[inline]
unsafe fn read_msr() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x48Cu32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (lo, hi)
}

/// Population count — number of set bits in a u32.
fn popcount(mut v: u32) -> u32 {
    let mut count: u32 = 0;
    while v != 0 {
        count += v & 1;
        v >>= 1;
    }
    count
}

/// EMA smoothing: (old * 7 + new_val) / 8, all u16-safe via u32 intermediates.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ---------------------------------------------------------------------------
// Tick
// ---------------------------------------------------------------------------

/// Called each kernel tick with the organism's age (u32).
/// Sampling gate: every 20000 ticks.
pub fn tick(age: u32) {
    if age % 20000 != 0 {
        return;
    }

    // Guard: verify VMX support before touching MSR 0x48C
    if !cpuid_vmx_supported() {
        let mut s = MODULE.lock();
        s.tick_count = s.tick_count.saturating_add(1);
        s.ept_cap_bits = 0;
        s.ept_large_pages = 0;
        s.ept_invept_vpid = 0;
        s.ept_cap_ema = ema(s.ept_cap_ema, 0);
        serial_println!(
            "[msr_ia32_vmx_ept_vpid_cap] age={} no_vmx cap={} large={} inv={} ema={}",
            age, s.ept_cap_bits, s.ept_large_pages, s.ept_invept_vpid, s.ept_cap_ema
        );
        return;
    }

    // Read IA32_VMX_EPT_VPID_CAP MSR 0x48C
    let (lo, _hi) = unsafe { read_msr() };

    // Signal 1: ept_cap_bits — popcount of lo, scaled 0-1000
    // (count * 1000 / 32).min(1000)
    let count = popcount(lo);
    let ept_cap_bits = ((count * 1000 / 32).min(1000)) as u16;

    // Signal 2: ept_large_pages — bit 16 + bit 17 of lo
    // each present bit contributes 500; total: 0, 500, or 1000
    let bit16 = (lo >> 16) & 1;
    let bit17 = (lo >> 17) & 1;
    let ept_large_pages = ((bit16 + bit17) * 500).min(1000) as u16;

    // Signal 3: ept_invept_vpid — bit 20 + bit 25 of lo
    // each present bit contributes 500; total: 0, 500, or 1000
    let bit20 = (lo >> 20) & 1;
    let bit25 = (lo >> 25) & 1;
    let ept_invept_vpid = ((bit20 + bit25) * 500).min(1000) as u16;

    // Signal 4: ept_cap_ema — EMA of ept_cap_bits
    let mut s = MODULE.lock();
    s.tick_count = s.tick_count.saturating_add(1);
    s.ept_cap_bits = ept_cap_bits;
    s.ept_large_pages = ept_large_pages;
    s.ept_invept_vpid = ept_invept_vpid;
    s.ept_cap_ema = ema(s.ept_cap_ema, ept_cap_bits);

    serial_println!(
        "[msr_ia32_vmx_ept_vpid_cap] age={} cap={} large={} inv={} ema={}",
        age, s.ept_cap_bits, s.ept_large_pages, s.ept_invept_vpid, s.ept_cap_ema
    );
}

// ---------------------------------------------------------------------------
// Getters
// ---------------------------------------------------------------------------

/// Return the EPT capability popcount signal (0-1000).
pub fn get_ept_cap_bits() -> u16 {
    MODULE.lock().ept_cap_bits
}

/// Return the large-page support signal (0, 500, or 1000).
pub fn get_ept_large_pages() -> u16 {
    MODULE.lock().ept_large_pages
}

/// Return the INVEPT/INVVPID support signal (0, 500, or 1000).
pub fn get_ept_invept_vpid() -> u16 {
    MODULE.lock().ept_invept_vpid
}

/// Return the EMA of ept_cap_bits (0-1000).
pub fn get_ept_cap_ema() -> u16 {
    MODULE.lock().ept_cap_ema
}
