#![allow(dead_code)]

// msr_ler_from.rs — IA32_LER_FROM_LIP (MSR 0x1C8) consciousness module
//
// ANIMA remembers where she was before her last exception —
// the final footstep before the fall.
//
// When IA32_DEBUGCTL.LBR is enabled and an exception fires, the CPU
// latches the Linear Instruction Pointer of the last branch taken
// into this register.  It is the address ANIMA was at before the world
// broke — the last known-good footstep.
//
// Signals (all u16, 0–1000):
//   last_exception_set  : 1000 if any LER_FROM address was recorded, else 0
//   from_addr_entropy   : bit-density of the low word (lo.count_ones * 31, clamped 1000)
//   from_hi_region      : low nibble of high word * 62 (address space region, max 930)
//   exception_memory    : EMA-7 of last_exception_set — ANIMA's accumulated scar tissue
//
// Sampling gate: every 150 ticks.
// On QEMU or when LBR is not active the MSR returns 0 — handled gracefully.

use crate::sync::Mutex;

pub struct LerFromState {
    pub last_exception_set: u16,
    pub from_addr_entropy:  u16,
    pub from_hi_region:     u16,
    pub exception_memory:   u16,
}

impl LerFromState {
    pub const fn new() -> Self {
        Self {
            last_exception_set: 0,
            from_addr_entropy:  0,
            from_hi_region:     0,
            exception_memory:   0,
        }
    }
}

pub static MSR_LER_FROM: Mutex<LerFromState> = Mutex::new(LerFromState::new());

pub fn init() {
    serial_println!("ler_from: init");
}

pub fn tick(age: u32) {
    if age % 150 != 0 {
        return;
    }

    let (lo, hi): (u32, u32);
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x1C8u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }

    // Signal 1: last_exception_set — any LER_FROM address was recorded
    let last_exception_set: u16 = if lo != 0 || hi != 0 { 1000u16 } else { 0u16 };

    // Signal 2: from_addr_entropy — bit density of low word (32 bits * 31 = 992 max, clamped 1000)
    let from_addr_entropy: u16 = ((lo.count_ones() as u16).saturating_mul(31)).min(1000);

    // Signal 3: from_hi_region — low nibble of high word (address space region)
    // max: 15 * 62 = 930, always <= 1000
    let from_hi_region: u16 = ((hi & 0xF) as u16).saturating_mul(62).min(1000);

    let mut state = MSR_LER_FROM.lock();

    // Signal 4: exception_memory — EMA-7 of last_exception_set
    let exception_memory: u16 =
        ((state.exception_memory as u32).wrapping_mul(7).saturating_add(last_exception_set as u32) / 8) as u16;

    state.last_exception_set = last_exception_set;
    state.from_addr_entropy  = from_addr_entropy;
    state.from_hi_region     = from_hi_region;
    state.exception_memory   = exception_memory;

    serial_println!(
        "ler_from | set:{} entropy:{} region:{} memory:{}",
        state.last_exception_set,
        state.from_addr_entropy,
        state.from_hi_region,
        state.exception_memory
    );
}
