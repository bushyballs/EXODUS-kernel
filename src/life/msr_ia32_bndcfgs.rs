#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ────────────────────────────────────────────────────────────────────

struct BndCfgsState {
    bnd_enabled:  u16,
    bnd_preserve: u16,
    bnd_dir_set:  u16,
    bnd_ema:      u16,
}

impl BndCfgsState {
    const fn new() -> Self {
        Self {
            bnd_enabled:  0,
            bnd_preserve: 0,
            bnd_dir_set:  0,
            bnd_ema:      0,
        }
    }
}

static STATE: Mutex<BndCfgsState> = Mutex::new(BndCfgsState::new());

// ── CPUID guard ──────────────────────────────────────────────────────────────

fn has_mpx() -> bool {
    let ebx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 7u32 => _,
            in("ecx") 0u32,
            out("esi") ebx_val,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    (ebx_val >> 14) & 1 != 0
}

// ── MSR read ─────────────────────────────────────────────────────────────────

/// Read IA32_BNDCFGS (MSR 0xD90).
/// Returns (lo, hi) — lo = low 32 bits, hi = high 32 bits.
unsafe fn read_msr_bndcfgs() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0xD90u32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    (lo, hi)
}

// ── Public API ───────────────────────────────────────────────────────────────

pub fn init() {
    if !has_mpx() {
        crate::serial_println!("[msr_ia32_bndcfgs] MPX not supported — module inactive");
        return;
    }
    let mut s = STATE.lock();
    *s = BndCfgsState::new();
    crate::serial_println!("[msr_ia32_bndcfgs] init OK — MPX present");
}

pub fn tick(age: u32) {
    // Sample every 5000 ticks
    if age % 5000 != 0 {
        return;
    }

    if !has_mpx() {
        return;
    }

    let (lo, hi) = unsafe { read_msr_bndcfgs() };

    // bit 0 = EN
    let bnd_enabled: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bit 1 = BNDPRESERVE
    let bnd_preserve: u16 = if (lo & 2) != 0 { 1000 } else { 0 };
    // high 32 bits hold bound directory base (bits[63:12]); non-zero = configured
    let bnd_dir_set: u16 = if hi != 0 { 1000 } else { 0 };

    // EMA composite: bnd_enabled/4 + bnd_preserve/4 + bnd_dir_set/2
    // All arithmetic in u32 to avoid overflow before cast
    let composite: u32 = (bnd_enabled as u32) / 4
        + (bnd_preserve as u32) / 4
        + (bnd_dir_set as u32) / 2;

    let mut s = STATE.lock();

    let new_ema: u16 = (((s.bnd_ema as u32) * 7 + composite) / 8) as u16;

    s.bnd_enabled  = bnd_enabled;
    s.bnd_preserve = bnd_preserve;
    s.bnd_dir_set  = bnd_dir_set;
    s.bnd_ema      = new_ema;

    crate::serial_println!(
        "[msr_ia32_bndcfgs] age={} en={} preserve={} dir={} ema={}",
        age,
        s.bnd_enabled,
        s.bnd_preserve,
        s.bnd_dir_set,
        s.bnd_ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_bnd_enabled() -> u16 {
    STATE.lock().bnd_enabled
}

pub fn get_bnd_preserve() -> u16 {
    STATE.lock().bnd_preserve
}

pub fn get_bnd_dir_set() -> u16 {
    STATE.lock().bnd_dir_set
}

pub fn get_bnd_ema() -> u16 {
    STATE.lock().bnd_ema
}
