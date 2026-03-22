#![allow(dead_code)]

use crate::sync::Mutex;

// msr_ia32_lstar.rs — ANIMA Life Module
//
// Hardware: IA32_LSTAR MSR 0xC0000082
//   64-bit SYSCALL target RIP — the kernel sets this to the virtual address of
//   the syscall entry point.  In a properly configured long-mode kernel the
//   upper 32 bits (hi) will equal 0xFFFFFFFF (kernel canonical address).
//
// Signals derived:
//   lstar_lo     — address entropy from upper half of lo word (0-1000)
//   lstar_hi     — lower 16 bits of hi word (0-1000)
//   lstar_kernel — kernel-space sanity: 1000 if hi==0xFFFFFFFF, 500 if hi>0xFFFF0000, else 0
//   lstar_ema    — 8-tap EMA of lstar_lo
//
// Guard: SYSCALL/SYSRET must be supported (CPUID 0x80000001 EDX bit 11,
//        max extended leaf >= 0x80000001).
//
// Tick gate: every 15 000 ticks.

const MSR_IA32_LSTAR: u32 = 0xC000_0082;
const TICK_GATE: u32 = 15_000;

pub struct State {
    pub lstar_lo:     u16,   // upper bits of lo word, scaled 0-1000
    pub lstar_hi:     u16,   // lower 16 bits of hi word, scaled 0-1000
    pub lstar_kernel: u16,   // kernel-space check: 0 / 500 / 1000
    pub lstar_ema:    u16,   // EMA of lstar_lo
}

impl State {
    const fn new() -> Self {
        State {
            lstar_lo:     0,
            lstar_hi:     0,
            lstar_kernel: 0,
            lstar_ema:    0,
        }
    }
}

pub static MODULE: Mutex<State> = Mutex::new(State::new());

// ── CPUID guard ─────────────────────────────────────────────────────────────

fn has_syscall() -> bool {
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

// ── MSR read ─────────────────────────────────────────────────────────────────

fn read_lstar() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") MSR_IA32_LSTAR,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    (lo, hi)
}

// ── Signal helpers ────────────────────────────────────────────────────────────

// Scale a raw u16 value (0-65535) to 0-1000, integer only.
fn scale_u16(val: u16) -> u16 {
    ((val as u32).wrapping_mul(1000) / 65535).min(1000) as u16
}

// EMA: ((old * 7).saturating_add(new)) / 8
fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    if !has_syscall() {
        serial_println!("[msr_ia32_lstar] SYSCALL not supported — module passive");
        return;
    }
    let (lo, hi) = read_lstar();
    serial_println!(
        "[msr_ia32_lstar] init: LSTAR lo=0x{:08X} hi=0x{:08X}",
        lo, hi
    );
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    if !has_syscall() {
        return;
    }

    let (lo, hi) = read_lstar();

    // lstar_lo: upper 16 bits of lo word, scaled 0-1000
    let lo_raw = ((lo >> 16) & 0xFFFF) as u16;
    let new_lo = scale_u16(lo_raw);

    // lstar_hi: lower 16 bits of hi word, scaled 0-1000
    let hi_raw = (hi & 0xFFFF) as u16;
    let new_hi = scale_u16(hi_raw);

    // lstar_kernel: canonical kernel-space check on full hi word
    let new_kernel: u16 = if hi == 0xFFFF_FFFF {
        1000
    } else if hi > 0xFFFF_0000 {
        500
    } else {
        0
    };

    let mut state = MODULE.lock();

    state.lstar_lo     = new_lo;
    state.lstar_hi     = new_hi;
    state.lstar_kernel = new_kernel;
    state.lstar_ema    = ema(state.lstar_ema, new_lo);

    serial_println!(
        "[msr_ia32_lstar] age={} lo=0x{:08X} hi=0x{:08X} \
         sig_lo={} sig_hi={} kernel={} ema={}",
        age, lo, hi,
        state.lstar_lo,
        state.lstar_hi,
        state.lstar_kernel,
        state.lstar_ema,
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_lstar_lo() -> u16 {
    MODULE.lock().lstar_lo
}

pub fn get_lstar_hi() -> u16 {
    MODULE.lock().lstar_hi
}

pub fn get_lstar_kernel() -> u16 {
    MODULE.lock().lstar_kernel
}

pub fn get_lstar_ema() -> u16 {
    MODULE.lock().lstar_ema
}
