// msr_vmx_cr4_fixed1.rs — ANIMA Life Module
//
// Reads the IA32_VMX_CR4_FIXED1 MSR (address 0x489) to give ANIMA a sense of
// the allowed-1 settings for CR4 in VMX operation: which CR4 bits the hypervisor
// permits guests to set to 1.
//
// Signals exposed:
//   cr4_fixed1_pae      — PAE (bit 5) allowed in guest CR4 (0 or 1000)
//   cr4_fixed1_pge      — PGE (bit 7) allowed in guest CR4 (0 or 1000)
//   cr4_fixed1_pse      — PSE (bit 4) allowed in guest CR4 (0 or 1000)
//   cr4_fixed1_richness_ema — EMA of popcount(fixed1_lo) scaled 0-1000
//
// Hardware: IA32_VMX_CR4_FIXED1 at MSR 0x489 (Intel SDM Vol 3D, Appendix A.8)
//   RDMSR returns EDX:EAX; we use EAX (low 32 bits) — the CR4 allowed-1 mask.
//
// VMX guard: CPUID leaf 1 ECX bit 5 must be set before attempting RDMSR 0x489.
//   Uses the push rbx/cpuid/mov esi,ebx/pop rbx pattern to preserve rbx across
//   the CPUID instruction without clobbering the compiler's base-pointer use.
//
// Sample gate: runs only when age % 5000 == 0.
// EMA formula: (old * 7 + new_val) / 8 in u32, cast to u16.
// No floats, no heap, no libc. All arithmetic saturating or wrapping.

#![allow(dead_code)]

use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

pub struct MsrVmxCr4Fixed1State {
    /// PAE allowed in guest CR4 (bit 5 of fixed1_lo): 0 or 1000
    pub cr4_fixed1_pae: u16,
    /// PGE allowed in guest CR4 (bit 7 of fixed1_lo): 0 or 1000
    pub cr4_fixed1_pge: u16,
    /// PSE allowed in guest CR4 (bit 4 of fixed1_lo): 0 or 1000
    pub cr4_fixed1_pse: u16,
    /// EMA of popcount(fixed1_lo) scaled 0–1000 (*31 per bit)
    pub cr4_fixed1_richness_ema: u16,
    /// Tracks whether VMX was present on first sample
    vmx_supported: bool,
    /// Raw low-32 bits from last successful RDMSR
    last_fixed1_lo: u32,
}

impl MsrVmxCr4Fixed1State {
    const fn new() -> Self {
        MsrVmxCr4Fixed1State {
            cr4_fixed1_pae: 0,
            cr4_fixed1_pge: 0,
            cr4_fixed1_pse: 0,
            cr4_fixed1_richness_ema: 0,
            vmx_supported: false,
            last_fixed1_lo: 0,
        }
    }
}

pub static MODULE: Mutex<MsrVmxCr4Fixed1State> =
    Mutex::new(MsrVmxCr4Fixed1State::new());

// ---------------------------------------------------------------------------
// CPUID VMX check
// ---------------------------------------------------------------------------

/// Returns true if CPUID leaf 1 ECX bit 5 (VMX feature flag) is set.
///
/// Uses inline asm with push/pop rbx to preserve the register across CPUID,
/// because LLVM may use rbx as a base pointer in PIC/PIE builds.
fn vmx_supported_by_cpuid() -> bool {
    let ecx_val: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            out("eax") _,
            out("esi") _,
            out("ecx") ecx_val,
            out("edx") _,
            options(nostack, preserves_flags),
        );
    }
    (ecx_val >> 5) & 1 == 1
}

// ---------------------------------------------------------------------------
// RDMSR helper
// ---------------------------------------------------------------------------

/// Read a 64-bit MSR. Returns (eax, edx) = (low32, high32).
///
/// SAFETY: caller must ensure the MSR address is valid on this CPU.
unsafe fn rdmsr(msr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem, preserves_flags),
    );
    (lo, hi)
}

// ---------------------------------------------------------------------------
// popcount (no stdlib)
// ---------------------------------------------------------------------------

/// Count the number of set bits in a u32 (Hamming weight).
fn popcount32(mut v: u32) -> u32 {
    let mut count: u32 = 0;
    while v != 0 {
        count = count.saturating_add(v & 1);
        v >>= 1;
    }
    count
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    let supported = vmx_supported_by_cpuid();
    {
        let mut state = MODULE.lock();
        state.vmx_supported = supported;
    }
    serial_println!(
        "[msr_vmx_cr4_fixed1] init: vmx_supported={}",
        supported
    );
}

pub fn tick(age: u32) {
    // Sample gate: only run every 5000 ticks
    if age % 5000 != 0 {
        return;
    }

    // Check VMX support — avoid faulting on non-VMX hardware
    if !vmx_supported_by_cpuid() {
        let mut state = MODULE.lock();
        state.cr4_fixed1_pae = 0;
        state.cr4_fixed1_pge = 0;
        state.cr4_fixed1_pse = 0;
        state.cr4_fixed1_richness_ema = 0;
        state.last_fixed1_lo = 0;
        serial_println!(
            "[msr_vmx_cr4_fixed1] age={} VMX not supported — signals zeroed",
            age
        );
        return;
    }

    // Read IA32_VMX_CR4_FIXED1 (MSR 0x489); we only need the low 32 bits (EAX)
    let fixed1_lo: u32 = unsafe {
        let (lo, _hi) = rdmsr(0x489);
        lo
    };

    // --- cr4_fixed1_pae: bit 5 of fixed1_lo
    let new_pae: u16 = if (fixed1_lo >> 5) & 1 == 1 { 1000 } else { 0 };

    // --- cr4_fixed1_pge: bit 7 of fixed1_lo
    let new_pge: u16 = if (fixed1_lo >> 7) & 1 == 1 { 1000 } else { 0 };

    // --- cr4_fixed1_pse: bit 4 of fixed1_lo
    let new_pse: u16 = if (fixed1_lo >> 4) & 1 == 1 { 1000 } else { 0 };

    // --- richness: popcount scaled 0-32 → 0-992 (max 32*31=992, close to 1000)
    let bits_set = popcount32(fixed1_lo);                // 0..=32
    let richness_raw = (bits_set.saturating_mul(31)) as u16; // 0..=992 ≤ 1000

    // EMA: (old * 7 + new_val) / 8  — all in u32, cast to u16
    let mut state = MODULE.lock();

    let new_richness_ema = ((state.cr4_fixed1_richness_ema as u32)
        .wrapping_mul(7)
        .saturating_add(richness_raw as u32))
        / 8;

    state.cr4_fixed1_pae = new_pae;
    state.cr4_fixed1_pge = new_pge;
    state.cr4_fixed1_pse = new_pse;
    state.cr4_fixed1_richness_ema = new_richness_ema as u16;
    state.last_fixed1_lo = fixed1_lo;

    serial_println!(
        "[msr_vmx_cr4_fixed1] age={} fixed1_lo=0x{:08X} \
         pae={} pge={} pse={} richness_ema={}",
        age,
        fixed1_lo,
        state.cr4_fixed1_pae,
        state.cr4_fixed1_pge,
        state.cr4_fixed1_pse,
        state.cr4_fixed1_richness_ema,
    );
}

// ---------------------------------------------------------------------------
// Accessors
// ---------------------------------------------------------------------------

pub fn get_cr4_fixed1_pae() -> u16 {
    MODULE.lock().cr4_fixed1_pae
}

pub fn get_cr4_fixed1_pge() -> u16 {
    MODULE.lock().cr4_fixed1_pge
}

pub fn get_cr4_fixed1_pse() -> u16 {
    MODULE.lock().cr4_fixed1_pse
}

pub fn get_cr4_fixed1_richness_ema() -> u16 {
    MODULE.lock().cr4_fixed1_richness_ema
}
