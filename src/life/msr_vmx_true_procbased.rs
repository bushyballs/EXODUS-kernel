#![allow(dead_code)]

/// MSR_VMX_TRUE_PROCBASED — IA32_VMX_TRUE_PROCBASED_CTLS (MSR 0x48E) Reader
///
/// ANIMA reads the unfiltered processor execution controls — the true behavioral
/// space before backward-compatibility corrections. Where IA32_VMX_PROCBASED_CTLS
/// (0x482) may force certain bits on for legacy compatibility, the TRUE variant
/// at 0x48E exposes what the hardware genuinely supports without that veneer.
/// This is the raw capability surface: the processor's honest enumeration of
/// which execution controls it can actually enforce.
///
/// HARDWARE: IA32_VMX_TRUE_PROCBASED_CTLS MSR 0x48E (read-only)
///   Bits [31:0]  = lo — "must-be-0" half: bits allowed to be cleared (must-be-0 mask)
///   Bits [63:32] = hi — "may-be-1" half: bits allowed to be set (may-be-1 mask)
///
/// The TRUE MSRs (0x48C–0x48F) are only present when
/// IA32_VMX_BASIC MSR 0x480 bit 55 is set. This module guards with
/// CPUID ECX bit 5 (VMX feature flag) as a first-pass gate.
///
/// GUARD: MSR 0x48E causes #GP if VMX is absent. Always check CPUID leaf 1,
///        ECX bit 5 (VMX feature flag) before touching the MSR.

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct VmxTrueProcbasedState {
    /// Popcount of lo bits [15:0] (must-be-0 mask), scaled 0-1000
    pub true_proc_allowed0: u16,
    /// Popcount of hi bits [15:0] (may-be-1 mask), scaled 0-1000
    pub true_proc_allowed1: u16,
    /// Flexibility of true VMX proc-based controls: true_proc_allowed1 - (true_proc_allowed0 / 2)
    pub true_proc_flexibility: u16,
    /// EMA of (true_proc_allowed0 + true_proc_allowed1) / 2
    pub true_proc_richness_ema: u16,
    /// Total tick calls (all ticks, not just sampled ones)
    pub tick_count: u32,
}

impl VmxTrueProcbasedState {
    pub const fn empty() -> Self {
        Self {
            true_proc_allowed0: 0,
            true_proc_allowed1: 0,
            true_proc_flexibility: 0,
            true_proc_richness_ema: 0,
            tick_count: 0,
        }
    }
}

pub static STATE: Mutex<VmxTrueProcbasedState> = Mutex::new(VmxTrueProcbasedState::empty());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  life::msr_vmx_true_procbased: VMX true proc-based controls sense initialized");
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Check CPUID leaf 1 ECX bit 5 — VMX feature flag.
/// Returns true if VMX is supported by this CPU.
#[inline]
fn cpuid_vmx_supported() -> bool {
    let vmx_ok: bool;
    unsafe {
        let ecx: u32;
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            inout("ecx") 0u32 => ecx,
            options(nostack, nomem)
        );
        vmx_ok = (ecx >> 5) & 1 == 1;
    }
    vmx_ok
}

/// Read MSR 0x48E — caller MUST have verified VMX support first.
/// Returns (lo: u32, hi: u32) — EAX=lo half, EDX=hi half from rdmsr.
#[inline]
unsafe fn read_msr_vmx_true_procbased() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x48Eu32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    (lo, hi)
}

/// EMA smoothing: (old * 7 + new_val) / 8
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
    // Sampling gate
    if age % 5000 != 0 {
        return;
    }

    // --- Guard: check VMX support before touching MSR 0x48E ---
    if !cpuid_vmx_supported() {
        let mut s = STATE.lock();
        s.tick_count = s.tick_count.saturating_add(1);
        s.true_proc_allowed0 = 0;
        s.true_proc_allowed1 = 0;
        s.true_proc_flexibility = 0;
        s.true_proc_richness_ema = ema(s.true_proc_richness_ema, 0);
        serial_println!(
            "[vmx_true_procbased] allowed0={} allowed1={} flex={} richness={}",
            s.true_proc_allowed0, s.true_proc_allowed1, s.true_proc_flexibility, s.true_proc_richness_ema
        );
        return;
    }

    // --- Read IA32_VMX_TRUE_PROCBASED_CTLS MSR 0x48E ---
    let (lo, hi) = unsafe { read_msr_vmx_true_procbased() };

    // Signal 1: true_proc_allowed0 — popcount of lo bits [15:0], scaled 0-1000
    let pop0 = (lo & 0xFFFF).count_ones() as u16;
    let true_proc_allowed0: u16 = pop0 * 1000 / 16;

    // Signal 2: true_proc_allowed1 — popcount of hi bits [15:0], scaled 0-1000
    let pop1 = (hi & 0xFFFF).count_ones() as u16;
    let true_proc_allowed1: u16 = pop1 * 1000 / 16;

    // Signal 3: true_proc_flexibility — raw configurability without compat corrections
    let true_proc_flexibility: u16 = true_proc_allowed1.saturating_sub(true_proc_allowed0 / 2);

    // Signal 4: true_proc_richness — average richness (raw, pre-EMA)
    let richness_raw: u16 = (true_proc_allowed0 + true_proc_allowed1) / 2;

    // Apply EMA to signal 4 only; update state
    let mut s = STATE.lock();
    s.tick_count = s.tick_count.saturating_add(1);
    s.true_proc_allowed0 = true_proc_allowed0;
    s.true_proc_allowed1 = true_proc_allowed1;
    s.true_proc_flexibility = true_proc_flexibility;
    s.true_proc_richness_ema = ema(s.true_proc_richness_ema, richness_raw);

    serial_println!(
        "[vmx_true_procbased] allowed0={} allowed1={} flex={} richness={}",
        s.true_proc_allowed0, s.true_proc_allowed1, s.true_proc_flexibility, s.true_proc_richness_ema
    );
}

// ---------------------------------------------------------------------------
// Public accessors
// ---------------------------------------------------------------------------

/// Returns true if the CPU supports VMX and has any configurable true proc controls
pub fn has_flexible_controls() -> bool {
    let s = STATE.lock();
    s.true_proc_flexibility > 0
}

/// Returns the current signal snapshot
pub fn report() -> VmxTrueProcbasedState {
    *STATE.lock()
}
