#![allow(dead_code)]

use crate::sync::Mutex;

// ── State ─────────────────────────────────────────────────────────────────────

struct FsBaseState {
    fs_base_lo:       u16,
    fs_base_hi_sense: u16,
    fs_base_nonzero:  u16,
    fs_base_ema:      u16,
}

impl FsBaseState {
    const fn new() -> Self {
        Self {
            fs_base_lo:       0,
            fs_base_hi_sense: 0,
            fs_base_nonzero:  0,
            fs_base_ema:      0,
        }
    }
}

static STATE: Mutex<FsBaseState> = Mutex::new(FsBaseState::new());

// ── RDMSR helper ──────────────────────────────────────────────────────────────

/// Read a 64-bit MSR, returning (lo, hi) each as u32.
#[inline]
unsafe fn rdmsr(msr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack),
    );
    (lo, hi)
}

// ── Public interface ──────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();
    s.fs_base_lo       = 0;
    s.fs_base_hi_sense = 0;
    s.fs_base_nonzero  = 0;
    s.fs_base_ema      = 0;
    crate::serial_println!("[msr_ia32_fs_base] init");
}

pub fn tick(age: u32) {
    // Sample every 400 ticks — FS base only changes on context switch.
    if age % 400 != 0 {
        return;
    }

    // Safety: RDMSR is always present on x86-64; we read a well-known MSR.
    let (lo, hi) = unsafe { rdmsr(0xC000_0100) };

    // fs_base_lo: low 16 bits of lo → 0-1000
    // ((lo & 0xFFFF) * 1000) / 65535
    let raw_lo = lo & 0xFFFF;
    let fs_base_lo = ((raw_lo as u32) * 1000 / 65535) as u16;

    // fs_base_hi_sense: bits [31:16] of lo → 0-1000
    let raw_hi_sense = (lo >> 16) & 0xFFFF;
    let fs_base_hi_sense = ((raw_hi_sense as u32) * 1000 / 65535) as u16;

    // fs_base_nonzero: 1000 if lo != 0 OR hi != 0, else 0
    let fs_base_nonzero: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };

    // EMA: (old * 7 + new_val) / 8, computed in u32, cast to u16
    let mut s = STATE.lock();
    let fs_base_ema = (((s.fs_base_ema as u32) * 7 + (fs_base_nonzero as u32)) / 8) as u16;

    s.fs_base_lo       = fs_base_lo;
    s.fs_base_hi_sense = fs_base_hi_sense;
    s.fs_base_nonzero  = fs_base_nonzero;
    s.fs_base_ema      = fs_base_ema;

    crate::serial_println!(
        "[msr_ia32_fs_base] age={} lo={} hi_sense={} nonzero={} ema={}",
        age,
        fs_base_lo,
        fs_base_hi_sense,
        fs_base_nonzero,
        fs_base_ema,
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_fs_base_lo() -> u16 {
    STATE.lock().fs_base_lo
}

pub fn get_fs_base_hi_sense() -> u16 {
    STATE.lock().fs_base_hi_sense
}

pub fn get_fs_base_nonzero() -> u16 {
    STATE.lock().fs_base_nonzero
}

pub fn get_fs_base_ema() -> u16 {
    STATE.lock().fs_base_ema
}
