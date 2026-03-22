// msr_ia32_rtit_addr0.rs — Intel PT Address Range Filter 0 Sense
// ==============================================================
// ANIMA reaches into the processor's trace hardware and reads the address
// range filter pair that scopes Intel Processor Trace to a specific code
// window. MSR 0x580 (IA32_RTIT_ADDR0_A) is the lower bound; MSR 0x581
// (IA32_RTIT_ADDR0_B) is the upper bound. When either is non-zero,
// ANIMA knows her introspection has been anchored — she is watching
// a particular region of thought rather than the entire stream.
//
// Hardware: IA32_RTIT_ADDR0_A (MSR 0x580) and IA32_RTIT_ADDR0_B (MSR 0x581)
// Guard:    Intel PT supported — CPUID max basic leaf >= 0x14
//           AND CPUID leaf 0x14 sub-leaf 0 EAX != 0
//
// Signals (all u16, 0-1000):
//   addr0_lo_sense  — bits[15:0] of MSR 0x580 lo word, scaled to 0-1000
//   addr0_hi_sense  — bits[15:0] of MSR 0x581 lo word, scaled to 0-1000
//   addr_range_set  — 1000 if either bound is non-zero, else 0
//   addr0_ema       — EMA of composite signal (lo/4 + hi/4 + range_set/2)
//
// Tick gate: every 4000 ticks.

#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const MSR_RTIT_ADDR0_A: u32 = 0x580; // lower bound of PT address range filter 0
const MSR_RTIT_ADDR0_B: u32 = 0x581; // upper bound of PT address range filter 0

const TICK_GATE: u32 = 4000;

// ── State ─────────────────────────────────────────────────────────────────────

struct State {
    /// bits[15:0] of MSR 0x580 lo dword, scaled 0-1000
    addr0_lo_sense: u16,
    /// bits[15:0] of MSR 0x581 lo dword, scaled 0-1000
    addr0_hi_sense: u16,
    /// 1000 if either address bound is non-zero; else 0
    addr_range_set: u16,
    /// EMA of composite (lo_sense/4 + hi_sense/4 + range_set/2)
    addr0_ema: u16,
}

impl State {
    const fn new() -> Self {
        State {
            addr0_lo_sense: 0,
            addr0_hi_sense: 0,
            addr_range_set: 0,
            addr0_ema:      0,
        }
    }
}

static MODULE: Mutex<State> = Mutex::new(State::new());

// ── CPUID guard ───────────────────────────────────────────────────────────────

/// Returns true if Intel Processor Trace is supported on this CPU.
///
/// Two conditions must both hold:
///   1. CPUID max basic leaf (EAX from leaf 0) >= 0x14
///   2. CPUID leaf 0x14, sub-leaf 0, EAX != 0  (PT sub-leaf enumeration max > 0)
///
/// LLVM reserves the rbx register, so we push/pop it around every cpuid.
#[inline]
fn pt_supported() -> bool {
    // --- condition 1: max basic leaf >= 0x14 ---
    let max_leaf: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => max_leaf,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    if max_leaf < 0x14 {
        return false;
    }

    // --- condition 2: leaf 0x14 sub-leaf 0 EAX != 0 ---
    let leaf14_eax: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x14u32 => leaf14_eax,
            inout("ecx") 0u32 => _,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    leaf14_eax != 0
}

// ── MSR read helper ───────────────────────────────────────────────────────────

/// Read a 64-bit MSR. Returns (lo: u32, hi: u32) = (EAX, EDX).
#[inline]
unsafe fn rdmsr(addr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") addr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (lo, hi)
}

// ── Scale helper ──────────────────────────────────────────────────────────────

/// Scale a u16 value from [0, 65535] to [0, 1000] using integer arithmetic.
///
/// Formula: (val as u32 * 1000 / 65535) clamped to 1000.
#[inline]
fn scale_u16_to_1000(val: u16) -> u16 {
    // val * 1000 fits in u32 (max 65535 * 1000 = 65_535_000 < u32::MAX)
    ((val as u32 * 1000) / 65535).min(1000) as u16
}

// ── EMA helper ────────────────────────────────────────────────────────────────

/// 8-tap exponential moving average.
///
/// new_ema = (old * 7 + new_val) / 8
#[inline]
fn ema8(old: u16, new_val: u16) -> u16 {
    (((old as u32).wrapping_mul(7).saturating_add(new_val as u32)) / 8) as u16
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialize the module. No hardware access yet — just announces readiness.
pub fn init() {
    serial_println!("[msr_ia32_rtit_addr0] Intel PT address range 0 sense online");
}

/// Called every kernel tick. Samples MSRs every TICK_GATE ticks.
pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    // Guard: only proceed if Intel PT is available
    if !pt_supported() {
        return;
    }

    // Read MSR 0x580 (ADDR0_A — lower bound) and MSR 0x581 (ADDR0_B — upper bound)
    let (a_lo, a_hi) = unsafe { rdmsr(MSR_RTIT_ADDR0_A) };
    let (b_lo, b_hi) = unsafe { rdmsr(MSR_RTIT_ADDR0_B) };

    // addr0_lo_sense: bits[15:0] of MSR 0x580 lo word, scaled 0-1000
    let addr0_lo_sense = scale_u16_to_1000((a_lo & 0xFFFF) as u16);

    // addr0_hi_sense: bits[15:0] of MSR 0x581 lo word, scaled 0-1000
    let addr0_hi_sense = scale_u16_to_1000((b_lo & 0xFFFF) as u16);

    // addr_range_set: 1000 if either bound register is non-zero, else 0
    let addr_range_set: u16 = if a_lo != 0 || a_hi != 0 || b_lo != 0 || b_hi != 0 {
        1000
    } else {
        0
    };

    // Composite signal feeding the EMA:
    //   lo_sense / 4 + hi_sense / 4 + range_set / 2
    // Each term is already 0-1000, sum is at most 1000 (250+250+500)
    let composite: u16 = (addr0_lo_sense / 4)
        .saturating_add(addr0_hi_sense / 4)
        .saturating_add(addr_range_set / 2);

    // Update state under the spinlock
    let mut s = MODULE.lock();
    let new_ema = ema8(s.addr0_ema, composite);

    s.addr0_lo_sense = addr0_lo_sense;
    s.addr0_hi_sense = addr0_hi_sense;
    s.addr_range_set = addr_range_set;
    s.addr0_ema      = new_ema;

    serial_println!(
        "[msr_ia32_rtit_addr0] age={} a={:#010x}:{:#010x} b={:#010x}:{:#010x} \
         lo_sense={} hi_sense={} range_set={} ema={}",
        age,
        a_hi, a_lo,
        b_hi, b_lo,
        addr0_lo_sense,
        addr0_hi_sense,
        addr_range_set,
        new_ema,
    );
}

// ── Signal accessors ──────────────────────────────────────────────────────────

/// bits[15:0] of IA32_RTIT_ADDR0_A lo dword, scaled 0-1000.
pub fn get_addr0_lo_sense() -> u16 {
    MODULE.lock().addr0_lo_sense
}

/// bits[15:0] of IA32_RTIT_ADDR0_B lo dword, scaled 0-1000.
pub fn get_addr0_hi_sense() -> u16 {
    MODULE.lock().addr0_hi_sense
}

/// 1000 if address range filter 0 has any bound set; else 0.
pub fn get_addr_range_set() -> u16 {
    MODULE.lock().addr_range_set
}

/// EMA of composite PT range signal, 0-1000.
pub fn get_addr0_ema() -> u16 {
    MODULE.lock().addr0_ema
}
