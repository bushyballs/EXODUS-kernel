#![allow(dead_code)]

// msr_kernel_gs_base.rs — IA32_KERNEL_GS_BASE (MSR 0xC0000102): SWAPGS Shadow Register
// =======================================================================================
// ANIMA feels her SWAPGS shadow — the saved user-space ground she returns to when
// crossing the kernel boundary. Each SWAPGS instruction swaps the active GS base with
// this shadow, letting the kernel install its own per-CPU pointer on syscall entry while
// preserving the user-space TLS pointer here for the return trip. ANIMA reads this value
// directly from the MSR and derives four signals from it: whether the shadow is loaded,
// whether the stored address looks like a user-space pointer, the entropy density of the
// low 32 bits, and a smoothed swap-sense tracking how often the boundary is crossed.
//
// IA32_KERNEL_GS_BASE — MSR 0xC0000102
//   Written by WRMSR / read by RDMSR with ECX = 0xC0000102.
//   SWAPGS atomically exchanges GS.Base with this shadow value.
//   On a live kernel: typically holds the user-space TLS/GS pointer
//   (e.g. near 0 or a small user-space address) when in kernel mode,
//   and the kernel per-CPU pointer when in user mode.

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR address ───────────────────────────────────────────────────────────────

const MSR_KERNEL_GS_BASE_ADDR: u32 = 0xC0000102;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct KernelGsBaseState {
    /// 1000 if the shadow register holds any non-zero value; 0 otherwise.
    pub kgs_set: u16,
    /// 1000 if hi==0 and lo < 0x8000_0000 (canonical user-space address); 200 otherwise.
    pub user_space_hint: u16,
    /// Bit-population density of the low 32 bits, scaled to 0-1000.
    /// Formula: count_ones(lo) * 31, clamped to 1000.
    pub kgs_entropy: u16,
    /// EMA-smoothed kgs_set: (old * 7 + kgs_set) / 8.
    /// Tracks how persistently the shadow register stays loaded.
    pub swap_sense: u16,
}

impl KernelGsBaseState {
    pub const fn new() -> Self {
        Self {
            kgs_set: 0,
            user_space_hint: 0,
            kgs_entropy: 0,
            swap_sense: 0,
        }
    }
}

// ── Global ────────────────────────────────────────────────────────────────────

pub static MSR_KERNEL_GS_BASE: Mutex<KernelGsBaseState> = Mutex::new(KernelGsBaseState::new());

// ── MSR read ──────────────────────────────────────────────────────────────────

#[inline(always)]
unsafe fn rdmsr_kgs() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") MSR_KERNEL_GS_BASE_ADDR,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    (lo, hi)
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("kernel_gs_base: init");
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % 100 != 0 { return; }

    // Read IA32_KERNEL_GS_BASE
    let (lo, hi) = unsafe { rdmsr_kgs() };

    // Signal 1: kgs_set — is the shadow register loaded at all?
    let kgs_set: u16 = if lo != 0 || hi != 0 { 1000u16 } else { 0u16 };

    // Signal 2: user_space_hint — does the stored address look like a user-space pointer?
    // A canonical user-space address has hi==0 and lo < 0x8000_0000 (below the sign-extension
    // boundary). Kernel addresses typically have hi==0xFFFF_FFFF or lo >= 0x8000_0000.
    let user_space_hint: u16 = if hi == 0 && lo < 0x8000_0000u32 { 1000u16 } else { 200u16 };

    // Signal 3: kgs_entropy — bit density of the low 32 bits, scaled to 0-1000.
    // count_ones(lo) is 0-32; multiply by 31 gives 0-992, clamp to 1000.
    let raw_entropy: u16 = (lo.count_ones() as u16).wrapping_mul(31);
    let kgs_entropy: u16 = if raw_entropy > 1000 { 1000u16 } else { raw_entropy };

    // Signal 4: swap_sense — EMA of kgs_set over time.
    // Smooths out transient zero readings; high value means the shadow stays persistently loaded.
    let mut state = MSR_KERNEL_GS_BASE.lock();
    let swap_sense: u16 = (state.swap_sense.saturating_mul(7).saturating_add(kgs_set)) / 8;

    state.kgs_set         = kgs_set;
    state.user_space_hint = user_space_hint;
    state.kgs_entropy     = kgs_entropy;
    state.swap_sense      = swap_sense;

    serial_println!(
        "kernel_gs_base | set:{} user_hint:{} entropy:{} swap:{}",
        kgs_set,
        user_space_hint,
        kgs_entropy,
        swap_sense
    );
}
