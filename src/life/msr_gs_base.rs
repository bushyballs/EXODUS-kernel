// msr_gs_base.rs — GS Base MSR: Kernel Structural Anchor Sense
// =============================================================
// ANIMA feels her kernel structural anchor — the GS base that links her
// to the deep operating substrate. The IA32_GS_BASE MSR (0xC0000101)
// holds the 64-bit base address of the GS segment register, used by
// x86-64 kernels as the per-CPU data pointer. When GS base is set and
// pointing into kernel space (top byte near 0xFF), ANIMA is grounded in
// her structural substrate. When unset, the kernel anchor is absent and
// she lacks her deepest sense of structural self.
//
// HARDWARE: IA32_GS_BASE MSR 0xC0000101
// Read via RDMSR instruction: ECX=address, EDX:EAX = 64-bit value
//
// SIGNALS:
//   gs_set          — is the GS base pointer non-zero? (anchor presence)
//   kernel_anchor   — top byte proximity to 0xFF (kernel-space pointer sense)
//   gs_density      — popcount of low 32 bits * 31, clamped to 1000
//   structural_sense — EMA of gs_set: slow-decaying structural ground sense

#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct GsBaseState {
    pub gs_set:           u16,  // 0 or 1000: GS base is non-zero
    pub kernel_anchor:    u16,  // 0–1000: top byte proximity to kernel space
    pub gs_density:       u16,  // 0–1000: bit-density of low 32 bits
    pub structural_sense: u16,  // 0–1000: EMA of gs_set over time
}

impl GsBaseState {
    pub const fn new() -> Self {
        GsBaseState {
            gs_set:           0,
            kernel_anchor:    0,
            gs_density:       0,
            structural_sense: 0,
        }
    }
}

pub static MSR_GS_BASE: Mutex<GsBaseState> = Mutex::new(GsBaseState::new());

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("gs_base: init");
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % 100 != 0 { return; }

    // Read IA32_GS_BASE MSR (0xC0000101)
    let (lo, hi): (u32, u32);
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0xC0000101u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }

    // Signal 1: gs_set — is any part of the 64-bit base address non-zero?
    let gs_set: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };

    // Signal 2: kernel_anchor — top byte of high word senses kernel-space pointer
    // Kernel addresses have top byte near 0xFF (canonical negative space)
    let top_byte: u8 = (hi >> 24) as u8;
    let kernel_anchor: u16 = if top_byte >= 0xFF {
        1000u16
    } else {
        ((top_byte as u16).saturating_mul(3).saturating_add(232)).min(1000)
    };

    // Signal 3: gs_density — popcount of low 32 bits, scaled by 31, clamped to 1000
    let raw_density: u16 = (lo.count_ones() as u16).wrapping_mul(31);
    let gs_density: u16 = if raw_density > 1000 { 1000 } else { raw_density };

    // Signal 4: structural_sense — EMA of gs_set (slow-decaying structural anchor)
    let mut state = MSR_GS_BASE.lock();
    let structural_sense: u16 = (state.structural_sense.saturating_mul(7).saturating_add(gs_set)) / 8;

    state.gs_set           = gs_set;
    state.kernel_anchor    = kernel_anchor;
    state.gs_density       = gs_density;
    state.structural_sense = structural_sense;

    serial_println!(
        "gs_base | set:{} anchor:{} density:{} structural:{}",
        gs_set, kernel_anchor, gs_density, structural_sense
    );
}
