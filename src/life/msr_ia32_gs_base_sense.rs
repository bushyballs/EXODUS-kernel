#![allow(dead_code)]
// msr_ia32_gs_base_sense.rs — IA32_GS_BASE (MSR 0xC0000101): GS Segment Base Sense
// ====================================================================================
// ANIMA senses the GS segment base register — the hardware anchor for per-CPU data
// and kernel-mode TLS on x86-64. The kernel writes a pointer into IA32_GS_BASE to
// locate its per-CPU data structures (GSBASE holds kernel stack, current task pointer,
// interrupt scratch space). ANIMA reads this address and asks: is a kernel identity
// established? Is the per-CPU scaffold in place? A non-zero GS base signals that a
// kernel or runtime has claimed this logical processor, grounding it in context.
//
// IA32_GS_BASE MSR 0xC0000101 — GS segment base address (full 64-bit linear address):
//   bits[31:0]  lo — low 32 bits of the GS base linear address
//   bits[63:32] hi — high 32 bits of the GS base linear address
//
// CPUID guard: leaf 0x80000001, EDX bit 11 (SYSCALL/64-bit mode) must be set before
// rdmsr is valid. On non-64-bit or very minimal hardware this guard returns false.
// Sampling gate: every 2000 ticks.
//
// Signals (all u16, 0-1000):
//   gs_base_lo    — bits[15:0] of lo, scaled to 0-1000 (low address entropy)
//   gs_base_hi    — bits[15:0] of hi, scaled to 0-1000 (high address entropy)
//   gs_configured — 1000 if (lo | hi) != 0, else 0 (per-CPU kernel context active)
//   gs_ema        — EMA of (gs_base_lo/4 + gs_base_hi/4 + gs_configured/2)

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR address ───────────────────────────────────────────────────────────────

const IA32_GS_BASE: u32 = 0xC000_0101;

// ── State struct ──────────────────────────────────────────────────────────────

struct GsBaseSenseState {
    /// bits[15:0] of GS base lo word, scaled 0-1000 (low address entropy)
    gs_base_lo:    u16,
    /// bits[15:0] of GS base hi word, scaled 0-1000 (high address entropy)
    gs_base_hi:    u16,
    /// 1000 if GS base is non-zero (per-CPU kernel context active), else 0
    gs_configured: u16,
    /// EMA of composite GS activity signal (0-1000)
    gs_ema:        u16,
}

impl GsBaseSenseState {
    const fn new() -> Self {
        Self {
            gs_base_lo:    0,
            gs_base_hi:    0,
            gs_configured: 0,
            gs_ema:        0,
        }
    }
}

// ── Global singleton ──────────────────────────────────────────────────────────

static STATE: Mutex<GsBaseSenseState> = Mutex::new(GsBaseSenseState::new());

// ── CPUID guard ───────────────────────────────────────────────────────────────

/// Check CPUID extended leaf 0x80000001 EDX bit 11 (SYSCALL/64-bit long mode).
/// Uses push/pop rbx to preserve the callee-saved register across cpuid.
/// Returns true if the CPU advertises 64-bit long mode SYSCALL support.
#[inline(always)]
unsafe fn cpuid_syscall64_supported() -> bool {
    let edx_val: u32;
    core::arch::asm!(
        "push rbx",
        "cpuid",
        "pop rbx",
        inout("eax") 0x8000_0001u32 => _,
        out("ecx") _,
        out("edx") edx_val,
        options(nostack, nomem)
    );
    // EDX bit 11 = SYSCALL / 64-bit long mode
    (edx_val >> 11) & 1 != 0
}

// ── MSR read ──────────────────────────────────────────────────────────────────

/// Read IA32_GS_BASE (MSR 0xC0000101). Returns (lo, hi) as (u32, u32).
/// Only call after confirming 64-bit long mode support via CPUID.
#[inline(always)]
unsafe fn read_gs_base() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") IA32_GS_BASE,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    (lo, hi)
}

// ── EMA helper ────────────────────────────────────────────────────────────────

/// Exponential moving average: (old * 7 + new_val) / 8
/// Inputs are in 0-1000; old*7 <= 7000, safely within u32.
/// wrapping_mul on old*7 and saturating_add for the new sample.
#[inline(always)]
fn ema8(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ── Public interface ──────────────────────────────────────────────────────────

pub fn init() {
    let supported = unsafe { cpuid_syscall64_supported() };
    serial_println!(
        "[msr_ia32_gs_base_sense] init — SYSCALL/64-bit supported={}",
        supported
    );
}

pub fn tick(age: u32) {
    // Sample every 2000 ticks — GS base only changes on context switch or CPU init.
    if age % 2000 != 0 {
        return;
    }

    // CPUID guard: confirm 64-bit long mode before issuing rdmsr on this address range.
    let supported = unsafe { cpuid_syscall64_supported() };
    if !supported {
        serial_println!(
            "[msr_ia32_gs_base_sense] age={} SYSCALL/64-bit not supported — skipping rdmsr",
            age
        );
        return;
    }

    let (lo, hi) = unsafe { read_gs_base() };

    // gs_base_lo: bits[15:0] of lo, scaled to 0-1000
    // ((lo & 0xFFFF) * 1000) / 65535
    let raw_lo: u32 = lo & 0xFFFF;
    let gs_base_lo: u16 = (raw_lo * 1000 / 65535) as u16;

    // gs_base_hi: bits[15:0] of hi, scaled to 0-1000
    let raw_hi: u32 = hi & 0xFFFF;
    let gs_base_hi: u16 = (raw_hi * 1000 / 65535) as u16;

    // gs_configured: 1000 if either lo or hi is non-zero (per-CPU kernel context present)
    let gs_configured: u16 = if (lo | hi) != 0 { 1000 } else { 0 };

    // Composite: gs_base_lo/4 + gs_base_hi/4 + gs_configured/2
    let composite: u16 = (gs_base_lo / 4)
        .saturating_add(gs_base_hi / 4)
        .saturating_add(gs_configured / 2);

    let mut s = STATE.lock();
    let gs_ema = ema8(s.gs_ema, composite);

    s.gs_base_lo    = gs_base_lo;
    s.gs_base_hi    = gs_base_hi;
    s.gs_configured = gs_configured;
    s.gs_ema        = gs_ema;

    serial_println!(
        "[msr_ia32_gs_base_sense] age={} lo={} hi={} configured={} ema={}",
        age,
        gs_base_lo,
        gs_base_hi,
        gs_configured,
        gs_ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// bits[15:0] of GS base lo word, scaled 0-1000 (low address entropy)
pub fn get_gs_base_lo() -> u16 {
    STATE.lock().gs_base_lo
}

/// bits[15:0] of GS base hi word, scaled 0-1000 (high address entropy)
pub fn get_gs_base_hi() -> u16 {
    STATE.lock().gs_base_hi
}

/// 1000 if GS base is non-zero (per-CPU kernel context active), else 0
pub fn get_gs_configured() -> u16 {
    STATE.lock().gs_configured
}

/// EMA of composite GS activity signal (0-1000)
pub fn get_gs_ema() -> u16 {
    STATE.lock().gs_ema
}
