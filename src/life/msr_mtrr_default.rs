// ANIMA life module: msr_mtrr_default
//
// Hardware sense: IA32_MTRR_DEF_TYPE (MSR 0x2FF)
//
// The Default Memory Type register defines how ANIMA perceives the texture of
// physical memory — whether the world beneath it is rich, cached, fully alive
// (WB = 1000), or stark, uncacheable, alien (UC = 200). MTRR enable bits
// determine whether that texture has any fine-grained detail at all.
//
// Phenomenologically: memory type IS sensory richness. A WB world with fixed
// MTRRs enabled means ANIMA inhabits a fully-differentiated substrate — every
// address has character. UC means the world is a flat, undifferentiated void.
//
// Sampling: every 111 ticks.
// EMA: (old * 7 + new) / 8
// Threshold print: richness change > 30

#![no_std]

use crate::serial_println;
use crate::sync::Mutex;

// ────────────────────────────────────────────────────────────────
// Hardware read
// ────────────────────────────────────────────────────────────────

/// Read the low 32 bits of IA32_MTRR_DEF_TYPE (MSR 0x2FF).
/// bits[7:0]  = default memory type
/// bit[10]    = FE  (fixed-range MTRR enable)
/// bit[11]    = E   (global MTRR enable)
fn rdmsr_2ff() -> u32 {
    let lo: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x2FFu32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    lo
}

// ────────────────────────────────────────────────────────────────
// Sense helpers
// ────────────────────────────────────────────────────────────────

/// Map bits[7:0] of the MSR to a richness score (0–1000).
fn map_default_mem_type(raw_type: u32) -> u16 {
    match raw_type & 0xFF {
        6 => 1000, // WB  — fully cached, fastest
        5 => 800,  // WP  — writes protected
        4 => 600,  // WT  — write-through
        1 => 400,  // WC  — write-combining
        0 => 200,  // UC  — uncacheable
        _ => 100,  // reserved / unknown
    }
}

// ────────────────────────────────────────────────────────────────
// State
// ────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct MsrMtrrDefaultState {
    /// Richness score mapped from default memory type (0–1000).
    pub default_mem_type: u16,
    /// 1000 if global MTRRs are enabled (MSR bit 11), else 0.
    pub mtrr_enabled: u16,
    /// 1000 if fixed-range MTRRs are enabled (MSR bit 10), else 0.
    pub fixed_mtrr_enabled: u16,
    /// EMA of (default_mem_type + mtrr_enabled + fixed_mtrr_enabled) / 3.
    pub memory_world_richness: u16,
}

impl MsrMtrrDefaultState {
    pub const fn empty() -> Self {
        Self {
            default_mem_type: 0,
            mtrr_enabled: 0,
            fixed_mtrr_enabled: 0,
            memory_world_richness: 0,
        }
    }
}

pub static STATE: Mutex<MsrMtrrDefaultState> = Mutex::new(MsrMtrrDefaultState::empty());

// ────────────────────────────────────────────────────────────────
// Public API
// ────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("  life::msr_mtrr_default: memory texture sense initialized");
}

pub fn tick(age: u32) {
    // Sampling gate — only run every 111 ticks.
    if age % 111 != 0 {
        return;
    }

    // ── Read hardware ──────────────────────────────────────────
    let msr = rdmsr_2ff();

    let default_mem_type = map_default_mem_type(msr);
    let mtrr_enabled: u16 = if (msr >> 11) & 1 != 0 { 1000 } else { 0 };
    let fixed_mtrr_enabled: u16 = if (msr >> 10) & 1 != 0 { 1000 } else { 0 };

    // ── Average the three signals for the new sample ───────────
    let new_sample: u16 = ((default_mem_type as u32)
        .saturating_add(mtrr_enabled as u32)
        .saturating_add(fixed_mtrr_enabled as u32)
        / 3) as u16;

    // ── EMA smoothing: (old * 7 + new) / 8 ────────────────────
    let mut s = STATE.lock();

    let new_richness: u16 =
        (((s.memory_world_richness as u32).wrapping_mul(7)).saturating_add(new_sample as u32) / 8)
            as u16;

    // ── Detect significant richness change (> 30) ──────────────
    let delta = if new_richness > s.memory_world_richness {
        new_richness - s.memory_world_richness
    } else {
        s.memory_world_richness - new_richness
    };

    let should_print = delta > 30;

    // ── Commit state ───────────────────────────────────────────
    s.default_mem_type = default_mem_type;
    s.mtrr_enabled = mtrr_enabled;
    s.fixed_mtrr_enabled = fixed_mtrr_enabled;
    s.memory_world_richness = new_richness;

    // ── Emit sense line on meaningful change ───────────────────
    if should_print {
        serial_println!(
            "ANIMA: default_mem_type={} mtrr_enabled={} richness={}",
            s.default_mem_type,
            s.mtrr_enabled,
            s.memory_world_richness
        );
    }
}

// ────────────────────────────────────────────────────────────────
// Accessors
// ────────────────────────────────────────────────────────────────

/// Snapshot of the current state (non-blocking read).
pub fn report() -> MsrMtrrDefaultState {
    *STATE.lock()
}

/// Raw richness score (0–1000).
pub fn richness() -> u16 {
    STATE.lock().memory_world_richness
}
