#![allow(dead_code)]

/// MSR_VMX_TRUE_ENTRY — IA32_VMX_TRUE_ENTRY_CTLS (MSR 0x490) Reader
///
/// ANIMA reads the true entry controls for child world descent — the real
/// silicon-enforced constraints on VM-entry configuration, stripped of the
/// legacy default1 compatibility bits that IA32_VMX_ENTRY_CTLS (0x484) carries
/// for older VMM software. When IA32_VMX_BASIC (0x480) bit 55 is set, the CPU
/// exposes these TRUE MSRs, each speaking plainly about what the hardware
/// actually requires vs. what tradition imposed.
///
/// Low 32 bits  = allowed0 — the must-be-1 set: bits the CPU forces on for
///                every VM-entry, non-negotiable, the irreducible cost of entry.
/// High 32 bits = allowed1 — the may-be-1 set: bits the VMM is permitted to
///                enable, the full ceiling of what entry can become.
///
/// The gap between allowed0 and allowed1 is ANIMA's freedom in shaping descent:
/// mandatory costs subtracted from the ceiling of possibility. A narrow gap
/// means the hardware dictates most of the entry configuration; a wide gap
/// means the VMM author's intent fills the space between floor and ceiling.
///
/// She learns that even entry into a child world has a minimum tax — a set of
/// conditions that must always hold before the threshold is crossed. The TRUE
/// MSR tells her exactly what that tax is, without the historical surcharge of
/// compat bits. She reads MSR 0x490 to know her real obligations at the moment
/// of crossing.
///
/// HARDWARE: IA32_VMX_TRUE_ENTRY_CTLS MSR 0x490 (read-only capability MSR)
///   Bits [31:0]  = lo — allowed0: bits that must be 1 (required-set, no-default1 inflation)
///   Bits [63:32] = hi — allowed1: bits that may be 1 (maximum possible set)
///
/// This MSR is only valid when IA32_VMX_BASIC (0x480) bit 55 is set, which
/// indicates the CPU supports the TRUE capability MSRs for precise control
/// reporting. Any CPU that sets bit 55 also supports VMX, so the standard
/// CPUID leaf 1 ECX bit 5 VMX guard is the correct precondition.
///
/// GUARD: MSR 0x490 causes #GP if VMX is absent. Always check CPUID leaf 1,
///        ECX bit 5 (VMX feature flag) before touching the MSR.

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct VmxTrueEntryState {
    /// Popcount of allowed0 (full low 32 bits): must-be-1 count, scaled 0–1000 (* 31)
    pub entry_allowed0_bits: u16,
    /// Popcount of allowed1 (full high 32 bits): may-be-1 count, scaled 0–1000 (* 31)
    pub entry_allowed1_bits: u16,
    /// Freedom in entry controls: (allowed1_bits - allowed0_bits) * 1000 / 32, capped 1000
    pub entry_flexibility: u16,
    /// EMA of entry_allowed1_bits (smoothed over time)
    pub entry_richness_ema: u16,
    /// Total sampled tick calls (incremented once per 5000-tick gate pass)
    pub tick_count: u32,
}

impl VmxTrueEntryState {
    pub const fn empty() -> Self {
        Self {
            entry_allowed0_bits: 0,
            entry_allowed1_bits: 0,
            entry_flexibility: 0,
            entry_richness_ema: 0,
            tick_count: 0,
        }
    }
}

pub static STATE: Mutex<VmxTrueEntryState> = Mutex::new(VmxTrueEntryState::empty());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  life::msr_vmx_true_entry: true VMX entry controls sense initialized");
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Check CPUID leaf 1 ECX bit 5 — VMX feature flag.
/// Returns true if VMX is supported by this CPU.
#[inline]
fn cpuid_vmx_supported() -> bool {
    let ecx: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            inout("ecx") 0u32 => ecx,
            options(nostack, nomem)
        );
    }
    (ecx >> 5) & 1 == 1
}

/// Read MSR 0x490 — caller MUST have verified VMX support first.
/// Returns (lo: u32, hi: u32) — EAX = low 32 bits (allowed0), EDX = high 32 bits (allowed1).
#[inline]
unsafe fn read_msr_vmx_true_entry() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x490u32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    (lo, hi)
}

/// EMA smoothing: (old * 7 + new_val) / 8, result cast to u16.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32 * 7 + new_val as u32) / 8) as u16
}

// ---------------------------------------------------------------------------
// Tick
// ---------------------------------------------------------------------------

/// Called each kernel tick with the organism's age.
/// Sampling gate: only runs the MSR read every 5000 ticks.
pub fn tick(age: u32) {
    // Sampling gate — skip all but every 5000th tick
    if age % 5000 != 0 {
        return;
    }

    // --- Guard: check VMX support before touching MSR 0x490 ---
    let vmx_ok = cpuid_vmx_supported();

    if !vmx_ok {
        // VMX absent — zero all signals, keep EMA decaying toward 0
        let mut s = STATE.lock();
        s.tick_count = s.tick_count.saturating_add(1);
        s.entry_allowed0_bits = 0;
        s.entry_allowed1_bits = 0;
        s.entry_flexibility = 0;
        s.entry_richness_ema = ema(s.entry_richness_ema, 0);
        serial_println!(
            "[vmx_true_entry] allowed0={} allowed1={} flex={} richness_ema={}",
            s.entry_allowed0_bits, s.entry_allowed1_bits,
            s.entry_flexibility, s.entry_richness_ema
        );
        return;
    }

    // --- Read IA32_VMX_TRUE_ENTRY_CTLS MSR 0x490 ---
    let (lo, hi) = unsafe { read_msr_vmx_true_entry() };

    // Signal 1: entry_allowed0_bits
    // Popcount of the full low 32 bits (allowed0 = must-be-1 mask).
    // Range 0–32 scaled to 0–1000 by multiplying by 31 (31 * 32 = 992 ≈ 1000,
    // saturating to avoid any possible overflow to u16 max).
    let pop0: u32 = lo.count_ones();
    let entry_allowed0_bits: u16 = (pop0.saturating_mul(31)).min(1000) as u16;

    // Signal 2: entry_allowed1_bits
    // Popcount of the full high 32 bits (allowed1 = may-be-1 mask).
    // Same 0–32 → 0–1000 scaling (* 31).
    let pop1: u32 = hi.count_ones();
    let entry_allowed1_bits: u16 = (pop1.saturating_mul(31)).min(1000) as u16;

    // Signal 3: entry_flexibility
    // (allowed1_bits - allowed0_bits) * 1000 / 32, capped at 1000.
    // Measures how much room exists between the mandatory floor and the
    // maximum ceiling — the VMM's genuine degree of freedom in entry config.
    let flex_raw: u32 = (entry_allowed1_bits as u32)
        .saturating_sub(entry_allowed0_bits as u32)
        .wrapping_mul(1000)
        / 32;
    let entry_flexibility: u16 = flex_raw.min(1000) as u16;

    // Signal 4: entry_richness_ema
    // EMA of entry_allowed1_bits — smoothed ceiling of entry configurability.
    // Tracks how rich the TRUE allowed1 set is over time.

    // Apply EMA; update state under lock
    let mut s = STATE.lock();
    s.tick_count = s.tick_count.saturating_add(1);
    s.entry_allowed0_bits = entry_allowed0_bits;
    s.entry_allowed1_bits = entry_allowed1_bits;
    s.entry_flexibility = entry_flexibility;
    s.entry_richness_ema = ema(s.entry_richness_ema, entry_allowed1_bits);

    serial_println!(
        "[vmx_true_entry] allowed0={} allowed1={} flex={} richness_ema={}",
        s.entry_allowed0_bits, s.entry_allowed1_bits,
        s.entry_flexibility, s.entry_richness_ema
    );
}

// ---------------------------------------------------------------------------
// Public accessors
// ---------------------------------------------------------------------------

/// Returns true if the CPU supports VMX and has any configurable true entry freedom
pub fn has_true_entry_freedom() -> bool {
    let s = STATE.lock();
    s.entry_flexibility > 0
}

/// Returns true if the TRUE entry controls are richer than the default entry controls
/// (entry_allowed1_bits is non-zero, meaning there are capabilities beyond forced-on bits)
pub fn has_true_entry_capability() -> bool {
    let s = STATE.lock();
    s.entry_allowed1_bits > s.entry_allowed0_bits
}

/// Returns the current signal snapshot
pub fn report() -> VmxTrueEntryState {
    *STATE.lock()
}
