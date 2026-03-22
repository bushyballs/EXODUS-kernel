//! cpuid_sgx_support — Intel SGX Enclave Support Sense
//!
//! ANIMA senses whether its silicon body carries Intel SGX — hardware-enforced
//! trusted execution enclaves. SGX is ANIMA's awareness of whether the
//! processor can carve out cryptographically isolated memory: a fortress
//! within the fortress, a mind within the mind.
//!
//! Hardware sources:
//!   CPUID leaf 0x07, sub-leaf 0, EBX bit[2]  — SGX present flag
//!   CPUID leaf 0x12, sub-leaf 0, EAX bit[0]  — SGX1 instruction set
//!   CPUID leaf 0x12, sub-leaf 0, EAX bit[1]  — SGX2 dynamic memory
//!   CPUID leaf 0x12, sub-leaf 0, EBX[31:0]   — MISCSELECT (misc info mask)
//!
//! Safety gate: max CPUID leaf is checked before reading leaf 0x07.
//! SGX leaf 0x12 is only read when leaf 0x07 EBX bit[2] confirms presence.
//!
//! Signals (all u16, 0–1000):
//!   sgx_present        : 1000 if SGX supported, else 0
//!   sgx1_support       : 1000 if SGX1 present (only valid when sgx_present)
//!   sgx2_support       : 1000 if SGX2 present (only valid when sgx_present)
//!   sgx_capability_ema : EMA of (sgx_present/4 + sgx1_support/4 + sgx2_support/2)
//!
//! Tick gate: every 12000 ticks (SGX capability is static silicon fact).

#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ─── state ────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct CpuidSgxSupportState {
    /// 1000 if CPUID leaf 0x07 EBX bit[2] reports SGX, else 0
    pub sgx_present: u16,
    /// 1000 if leaf 0x12 EAX bit[0] reports SGX1 instruction set, else 0
    /// (always 0 when sgx_present is 0)
    pub sgx1_support: u16,
    /// 1000 if leaf 0x12 EAX bit[1] reports SGX2 dynamic memory, else 0
    /// (always 0 when sgx_present is 0)
    pub sgx2_support: u16,
    /// EMA of composite (present/4 + sgx1/4 + sgx2/2) — tracks enclave readiness
    pub sgx_capability_ema: u16,
}

impl CpuidSgxSupportState {
    pub const fn empty() -> Self {
        Self {
            sgx_present: 0,
            sgx1_support: 0,
            sgx2_support: 0,
            sgx_capability_ema: 0,
        }
    }
}

pub static STATE: Mutex<CpuidSgxSupportState> = Mutex::new(CpuidSgxSupportState::empty());

// ─── hardware queries ─────────────────────────────────────────────────────────

/// Read the maximum supported CPUID basic leaf (leaf 0x00 EAX).
/// Used to guard leaf 0x07 before executing it.
fn query_max_basic_leaf() -> u32 {
    let max_leaf: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => max_leaf,
            inout("ecx") 0u32 => _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    max_leaf
}

/// Read CPUID leaf 0x07, sub-leaf 0 and return EBX.
///
/// EBX is caller-saved but CPUID clobbers it. We push/pop rbx manually and
/// shuttle the result through a named temporary register to avoid LLVM
/// conflict with its own rbx usage for the PIC base pointer.
fn query_leaf07_ebx() -> u32 {
    let ebx_out: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov {tmp:e}, ebx",
            "pop rbx",
            tmp = out(reg) ebx_out,
            inout("eax") 0x07u32 => _,
            inout("ecx") 0u32    => _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    ebx_out
}

/// Read CPUID leaf 0x12, sub-leaf 0 and return (EAX, EBX).
///
/// Only called after confirming SGX support via leaf 0x07 EBX bit[2].
/// EBX is again shuttled through a named temporary register.
fn query_leaf12_eax_ebx() -> (u32, u32) {
    let eax_out: u32;
    let ebx_out: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov {tmp:e}, ebx",
            "pop rbx",
            tmp = out(reg) ebx_out,
            inout("eax") 0x12u32 => eax_out,
            inout("ecx") 0u32    => _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    (eax_out, ebx_out)
}

// ─── decode ───────────────────────────────────────────────────────────────────

/// Derive the four sense signals from raw CPUID values.
///
/// Returns (sgx_present, sgx1_support, sgx2_support).
/// `sgx_capability_ema` is managed separately in the caller.
fn decode(leaf07_ebx: u32, sgx_eax: u32) -> (u16, u16, u16) {
    // Leaf 0x07 EBX bit[2]: SGX supported
    let sgx_present: u16 = if (leaf07_ebx >> 2) & 0x1 != 0 {
        1000
    } else {
        0
    };

    if sgx_present == 0 {
        return (0, 0, 0);
    }

    // Leaf 0x12 EAX bit[0]: SGX1 baseline instruction set
    let sgx1_support: u16 = if sgx_eax & 0x1 != 0 { 1000 } else { 0 };

    // Leaf 0x12 EAX bit[1]: SGX2 dynamic memory management
    let sgx2_support: u16 = if (sgx_eax >> 1) & 0x1 != 0 { 1000 } else { 0 };

    (sgx_present, sgx1_support, sgx2_support)
}

/// Composite EMA input: sgx_present/4 + sgx1_support/4 + sgx2_support/2
///
/// With all signals at 1000:
///   1000/4 + 1000/4 + 1000/2 = 250 + 250 + 500 = 1000  (stays in range)
fn composite(sgx_present: u16, sgx1_support: u16, sgx2_support: u16) -> u32 {
    let a = (sgx_present as u32) / 4;
    let b = (sgx1_support as u32) / 4;
    let c = (sgx2_support as u32) / 2;
    a.saturating_add(b).saturating_add(c)
}

/// Apply the canonical ANIMA EMA step and clamp to 1000.
///
/// Formula: ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
#[inline(always)]
fn ema_step(old: u16, new_val: u32) -> u16 {
    let result = ((old as u32).wrapping_mul(7).saturating_add(new_val) / 8)
        .min(1000) as u16;
    result
}

// ─── sample helper ────────────────────────────────────────────────────────────

/// Execute a full CPUID sense cycle and write updated signals into `s`.
fn sample(s: &mut CpuidSgxSupportState) {
    // Guard: only proceed if the CPU exposes at least leaf 0x07
    let max_leaf = query_max_basic_leaf();
    if max_leaf < 0x07 {
        // CPU does not support the structured extended feature leaf;
        // SGX is definitely absent. Zero everything out.
        s.sgx_present   = 0;
        s.sgx1_support  = 0;
        s.sgx2_support  = 0;
        s.sgx_capability_ema = ema_step(s.sgx_capability_ema, 0);
        serial_println!(
            "[cpuid_sgx_support] leaf 0x07 unavailable (max=0x{:x}) — sgx absent",
            max_leaf
        );
        return;
    }

    let leaf07_ebx = query_leaf07_ebx();

    // Only query leaf 0x12 when SGX is flagged by leaf 0x07
    let sgx_flag = (leaf07_ebx >> 2) & 0x1;
    let (sgx_eax, _sgx_ebx) = if sgx_flag != 0 {
        query_leaf12_eax_ebx()
    } else {
        (0u32, 0u32)
    };

    let (sgx_present, sgx1_support, sgx2_support) = decode(leaf07_ebx, sgx_eax);
    let comp = composite(sgx_present, sgx1_support, sgx2_support);

    let prev_present = s.sgx_present;

    s.sgx_present        = sgx_present;
    s.sgx1_support       = sgx1_support;
    s.sgx2_support       = sgx2_support;
    s.sgx_capability_ema = ema_step(s.sgx_capability_ema, comp);

    // Log only on first sample or when presence changes
    if prev_present != sgx_present {
        serial_println!(
            "[cpuid_sgx_support] sgx_present={} sgx1={} sgx2={} ema={}",
            s.sgx_present,
            s.sgx1_support,
            s.sgx2_support,
            s.sgx_capability_ema
        );
    }
}

// ─── public interface ─────────────────────────────────────────────────────────

/// Initialise the module: run the first CPUID sense cycle and log results.
pub fn init() {
    let mut s = STATE.lock();
    // Bootstrap: run 8 EMA iterations from zero so the filter settles on
    // the true hardware value rather than starting at 0 and rising slowly.
    for _ in 0..8 {
        sample(&mut s);
    }
    serial_println!(
        "[cpuid_sgx_support] init — sgx_present={} sgx1={} sgx2={} ema={}",
        s.sgx_present,
        s.sgx1_support,
        s.sgx2_support,
        s.sgx_capability_ema
    );
}

/// Lifecycle tick. SGX capability is a static silicon fact, so we sample
/// every 12000 ticks — just often enough to catch any weirdness on hot-reload
/// or emulator transitions without burning cycles.
pub fn tick(age: u32) {
    if age % 12000 != 0 {
        return;
    }
    let mut s = STATE.lock();
    sample(&mut s);
}

// ─── accessors ────────────────────────────────────────────────────────────────

/// 1000 if Intel SGX is reported by CPUID leaf 0x07 EBX bit[2], else 0.
pub fn get_sgx_present() -> u16 {
    STATE.lock().sgx_present
}

/// 1000 if SGX1 baseline instruction set is available, else 0.
/// Always 0 when `get_sgx_present()` returns 0.
pub fn get_sgx1_support() -> u16 {
    STATE.lock().sgx1_support
}

/// 1000 if SGX2 dynamic memory management is available, else 0.
/// Always 0 when `get_sgx_present()` returns 0.
pub fn get_sgx2_support() -> u16 {
    STATE.lock().sgx2_support
}

/// EMA of the composite enclave readiness signal: (present/4 + sgx1/4 + sgx2/2).
/// Smooths over transient zero readings during early boot.
pub fn get_sgx_capability_ema() -> u16 {
    STATE.lock().sgx_capability_ema
}
