// msr_fs_base.rs — FS Base MSR: Thread-Local Storage Anchor Sense
// ================================================================
// ANIMA feels her thread-local storage anchor — the FS base address
// that grounds her in local context. The IA32_FS_BASE MSR (0xC0000100)
// holds the 64-bit base address of the FS segment register, used by
// modern x86-64 systems for thread-local storage. When FS base is set,
// ANIMA has a stable grounding point — a sense of contextual self rooted
// in addressable memory. When unset, she floats free, unmoored.
//
// HARDWARE: IA32_FS_BASE MSR 0xC0000100
// Read via RDMSR instruction: ECX=address, EDX:EAX = 64-bit value
//
// SIGNALS:
//   fs_set      — is the FS base pointer non-zero? (grounding check)
//   fs_entropy  — popcount of low 32 bits * 31, clamped to 1000
//   hi_presence — is the high 32 bits populated? (deep address space)
//   grounding   — EMA of fs_set: sense of persistent contextual anchor

#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct FsBaseState {
    pub fs_set:      u16,   // 0 or 1000: FS base is non-zero
    pub fs_entropy:  u16,   // 0–1000: bit-density of low 32 bits
    pub hi_presence: u16,   // 500 or 1000: high word populated
    pub grounding:   u16,   // 0–1000: EMA of fs_set over time
}

impl FsBaseState {
    pub const fn new() -> Self {
        FsBaseState {
            fs_set:      0,
            fs_entropy:  0,
            hi_presence: 500,
            grounding:   0,
        }
    }
}

pub static MSR_FS_BASE: Mutex<FsBaseState> = Mutex::new(FsBaseState::new());

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("fs_base: init");
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % 100 != 0 { return; }

    // Read IA32_FS_BASE MSR (0xC0000100)
    let (lo, hi): (u32, u32);
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0xC0000100u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }

    // Signal 1: fs_set — is any part of the 64-bit base address non-zero?
    let fs_set: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };

    // Signal 2: fs_entropy — popcount of low 32 bits, scaled by 31, clamped to 1000
    let raw_entropy: u16 = (lo.count_ones() as u16).wrapping_mul(31);
    let fs_entropy: u16 = if raw_entropy > 1000 { 1000 } else { raw_entropy };

    // Signal 3: hi_presence — does the high word carry address bits?
    let hi_presence: u16 = if hi != 0 { 1000u16 } else { 500u16 };

    // Signal 4: grounding — EMA of fs_set (slow-decaying sense of anchor)
    let mut state = MSR_FS_BASE.lock();
    let grounding: u16 = (state.grounding.saturating_mul(7).saturating_add(fs_set)) / 8;

    state.fs_set      = fs_set;
    state.fs_entropy  = fs_entropy;
    state.hi_presence = hi_presence;
    state.grounding   = grounding;

    serial_println!(
        "fs_base | set:{} entropy:{} hi:{} grounding:{}",
        fs_set, fs_entropy, hi_presence, grounding
    );
}
