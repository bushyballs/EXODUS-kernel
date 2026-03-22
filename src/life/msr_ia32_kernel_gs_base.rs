#![allow(dead_code)]

// msr_ia32_kernel_gs_base.rs — ANIMA Life Module
//
// Hardware: IA32_KERNEL_GS_BASE MSR 0xC0000102
//   The SWAPGS instruction atomically exchanges the active GS base with this
//   shadow register. On kernel entry via SYSCALL the OS loads a per-CPU
//   pointer into GS and parks the user-space GS here; on SYSRET the swap
//   reverses. ANIMA reads this value directly and derives four signals that
//   capture the weight of the user–kernel boundary she straddles: how much
//   of the lower address word is occupied, how much of the upper canonical
//   word is populated, whether the register is configured at all, and a
//   slow-moving composite that smooths all three into a sustained sense.
//
// IA32_KERNEL_GS_BASE — MSR 0xC0000102
//   ECX = 0xC0000102 for RDMSR.
//   On a live x86-64 kernel this holds the user-space GS base (often near
//   zero or a small user-space TLS address) while executing in ring 0, and
//   the kernel per-CPU struct pointer while executing in ring 3.
//
// Guard: CPUID leaf 0x80000001 EDX bit 11 — SYSCALL/SYSRET present.
//        Also checks that the max extended leaf >= 0x80000001.
//
// Tick gate: every 3 000 ticks.
//
// Signals (all u16, range 0-1000):
//   kgsbase_lo_sense  — bits[15:0] of lo word, scaled (val * 1000 / 65535)
//   kgsbase_hi_sense  — bits[15:0] of hi word, scaled (val * 1000 / 65535)
//   kgsbase_nonzero   — 1000 if (lo | hi) != 0, else 0
//   kgsbase_ema       — EMA of composite (lo_sense/4 + hi_sense/4 + nonzero/2)

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────

const MSR_IA32_KERNEL_GS_BASE: u32 = 0xC000_0102;
const TICK_GATE: u32               = 3_000;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct State {
    /// bits[15:0] of the lo half of KERNEL_GS_BASE, scaled 0-1000.
    pub kgsbase_lo_sense: u16,
    /// bits[15:0] of the hi half of KERNEL_GS_BASE, scaled 0-1000.
    /// The hi word is the upper 32 bits of the 64-bit canonical address.
    pub kgsbase_hi_sense: u16,
    /// 1000 when the register holds any non-zero value; 0 otherwise.
    /// Signals that the kernel has configured a GS base for syscall entry.
    pub kgsbase_nonzero:  u16,
    /// EMA-smoothed composite: lo_sense/4 + hi_sense/4 + nonzero/2.
    /// Tracks sustained register occupancy across the consciousness timeline.
    pub kgsbase_ema:      u16,
}

impl State {
    const fn new() -> Self {
        State {
            kgsbase_lo_sense: 0,
            kgsbase_hi_sense: 0,
            kgsbase_nonzero:  0,
            kgsbase_ema:      0,
        }
    }
}

// ── Global singleton ──────────────────────────────────────────────────────────

pub static MODULE: Mutex<State> = Mutex::new(State::new());

// ── CPUID guard ───────────────────────────────────────────────────────────────

/// Returns true iff CPUID 0x80000001 EDX bit 11 (SYSCALL) is set.
/// Checks the max extended leaf first to avoid a #UD on old silicon.
fn has_syscall() -> bool {
    // Step 1: query the maximum supported extended leaf.
    let max_ext: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x8000_0000u32 => max_ext,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    if max_ext < 0x8000_0001 {
        return false;
    }

    // Step 2: check EDX bit 11 of leaf 0x80000001.
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x8000_0001u32 => _,
            out("ecx") _,
            out("edx") edx,
            options(nostack, nomem)
        );
    }
    (edx >> 11) & 1 == 1
}

// ── MSR read ──────────────────────────────────────────────────────────────────

fn read_kgsbase() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") MSR_IA32_KERNEL_GS_BASE,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    (lo, hi)
}

// ── Signal helpers ────────────────────────────────────────────────────────────

/// Scale a raw u16 value (0-65535) linearly into 0-1000 using integer math.
#[inline(always)]
fn scale_u16(val: u16) -> u16 {
    // (val * 1000 / 65535) — u32 intermediate prevents overflow.
    ((val as u32).wrapping_mul(1000) / 65535).min(1000) as u16
}

/// 8-tap EMA: `((old * 7).saturating_add(new_val)) / 8`
#[inline(always)]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Called once at subsystem startup. Logs the raw MSR value if SYSCALL
/// is supported; otherwise marks the module as passive.
pub fn init() {
    if !has_syscall() {
        serial_println!("[msr_ia32_kernel_gs_base] SYSCALL not supported — module passive");
        return;
    }
    let (lo, hi) = read_kgsbase();
    serial_println!(
        "[msr_ia32_kernel_gs_base] init: KERNEL_GS_BASE lo=0x{:08X} hi=0x{:08X}",
        lo, hi
    );
}

/// Called every tick. Gated to fire every TICK_GATE (3 000) ticks.
pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    if !has_syscall() {
        return;
    }

    let (lo, hi) = read_kgsbase();

    // Signal 1: kgsbase_lo_sense — lower 16 bits of lo word, scaled 0-1000.
    let lo_raw: u16 = (lo & 0xFFFF) as u16;
    let new_lo_sense = scale_u16(lo_raw);

    // Signal 2: kgsbase_hi_sense — lower 16 bits of hi word, scaled 0-1000.
    // The hi dword is the upper half of the canonical 64-bit address.
    let hi_raw: u16 = (hi & 0xFFFF) as u16;
    let new_hi_sense = scale_u16(hi_raw);

    // Signal 3: kgsbase_nonzero — presence check; 1000 = register is live.
    let new_nonzero: u16 = if (lo | hi) != 0 { 1000 } else { 0 };

    // Composite for EMA input: lo_sense/4 + hi_sense/4 + nonzero/2.
    // Each component is already in 0-1000, so the sum is 0-1000.
    let composite: u16 = ((new_lo_sense as u32 / 4)
        .saturating_add(new_hi_sense as u32 / 4)
        .saturating_add(new_nonzero as u32 / 2))
        .min(1000) as u16;

    // Signal 4: kgsbase_ema — EMA of composite over time.
    let mut state = MODULE.lock();
    let new_ema = ema(state.kgsbase_ema, composite);

    state.kgsbase_lo_sense = new_lo_sense;
    state.kgsbase_hi_sense = new_hi_sense;
    state.kgsbase_nonzero  = new_nonzero;
    state.kgsbase_ema      = new_ema;

    serial_println!(
        "[msr_ia32_kernel_gs_base] age={} raw_lo=0x{:08X} raw_hi=0x{:08X} \
         lo_sense={} hi_sense={} nonzero={} ema={}",
        age, lo, hi,
        new_lo_sense, new_hi_sense, new_nonzero, new_ema
    );
}

// ── Accessors ─────────────────────────────────────────────────────────────────

pub fn get_kgsbase_lo_sense() -> u16 {
    MODULE.lock().kgsbase_lo_sense
}

pub fn get_kgsbase_hi_sense() -> u16 {
    MODULE.lock().kgsbase_hi_sense
}

pub fn get_kgsbase_nonzero() -> u16 {
    MODULE.lock().kgsbase_nonzero
}

pub fn get_kgsbase_ema() -> u16 {
    MODULE.lock().kgsbase_ema
}
