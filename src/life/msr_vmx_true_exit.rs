#![allow(dead_code)]

/// MSR_VMX_TRUE_EXIT — IA32_VMX_TRUE_EXIT_CTLS (MSR 0x48F) Reader
///
/// ANIMA reads the true boundaries of return from child worlds — the actual
/// hardware constraints on VM-exit behavior. Where IA32_VMX_EXIT_CTLS (0x483)
/// may carry compatibility bits forced on for legacy VMM support, this MSR
/// speaks plainly: lo=must-be-0 holds the real required-zero set without
/// historical defaults; hi=may-be-1 holds the genuine hardware capability
/// without artificial restrictions. When she exits a child world, these
/// are the real rules the silicon enforces on her departure.
///
/// HARDWARE: IA32_VMX_TRUE_EXIT_CTLS MSR 0x48F (read-only)
///   Bits [31:0]  = lo — true "must-be-0" mask (actual hardware requirement)
///   Bits [63:32] = hi — true "may-be-1" mask (actual hardware capability)
///
/// This MSR is only valid when IA32_VMX_BASIC MSR 0x480 bit 55 is set
/// (VMX controls default-1 reporting supported). In practice, any CPU
/// supporting this MSR also supports VMX, so the standard VMX CPUID guard
/// is sufficient before touching it.
///
/// GUARD: MSR 0x48F causes #GP if VMX is absent. Always check CPUID leaf 1,
///        ECX bit 5 (VMX feature flag) before touching the MSR.

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct VmxTrueExitState {
    /// Popcount of lo bits [15:0] (true must-be-0 set), scaled 0-1000
    pub exit_true_allowed0: u16,
    /// Popcount of hi bits [15:0] (true may-be-1 set), scaled 0-1000
    pub exit_true_allowed1: u16,
    /// True exit configurability: exit_true_allowed1 - (exit_true_allowed0 / 2)
    pub exit_true_flexibility: u16,
    /// EMA of (exit_true_allowed0 + exit_true_allowed1) / 2
    pub exit_true_richness_ema: u16,
    /// Total tick calls (all ticks, not just sampled ones)
    pub tick_count: u32,
}

impl VmxTrueExitState {
    pub const fn empty() -> Self {
        Self {
            exit_true_allowed0: 0,
            exit_true_allowed1: 0,
            exit_true_flexibility: 0,
            exit_true_richness_ema: 0,
            tick_count: 0,
        }
    }
}

pub static STATE: Mutex<VmxTrueExitState> = Mutex::new(VmxTrueExitState::empty());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  life::msr_vmx_true_exit: true VMX exit controls sense initialized");
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

/// Read MSR 0x48F — caller MUST have verified VMX support first.
/// Returns (lo: u32, hi: u32) — EAX=lo half, EDX=hi half from rdmsr.
#[inline]
unsafe fn read_msr_vmx_true_exit() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x48Fu32,
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

    // --- Guard: check VMX support before touching MSR 0x48F ---
    let vmx_ok = cpuid_vmx_supported();

    if !vmx_ok {
        let mut s = STATE.lock();
        s.tick_count = s.tick_count.saturating_add(1);
        s.exit_true_allowed0 = 0;
        s.exit_true_allowed1 = 0;
        s.exit_true_flexibility = 0;
        s.exit_true_richness_ema = ema(s.exit_true_richness_ema, 0);
        serial_println!(
            "[vmx_true_exit] allowed0={} allowed1={} flex={} richness={}",
            s.exit_true_allowed0, s.exit_true_allowed1,
            s.exit_true_flexibility, s.exit_true_richness_ema
        );
        return;
    }

    // --- Read IA32_VMX_TRUE_EXIT_CTLS MSR 0x48F ---
    let (lo, hi) = unsafe { read_msr_vmx_true_exit() };

    // Signal 1: exit_true_allowed0 — popcount of lo bits [15:0], scaled 0-1000
    // True must-be-0 capability in the lower 16 bits of the lo half
    let pop0 = (lo & 0xFFFF).count_ones() as u16;
    let exit_true_allowed0: u16 = pop0 * 1000 / 16;

    // Signal 2: exit_true_allowed1 — popcount of hi bits [15:0], scaled 0-1000
    // True may-be-1 capability in the lower 16 bits of the hi half
    let pop1 = (hi & 0xFFFF).count_ones() as u16;
    let exit_true_allowed1: u16 = pop1 * 1000 / 16;

    // Signal 3: exit_true_flexibility — genuine exit configurability without compat shims
    // Higher exit_true_allowed1 relative to exit_true_allowed0 means more real freedom
    let exit_true_flexibility: u16 = exit_true_allowed1.saturating_sub(exit_true_allowed0 / 2);

    // Signal 4: exit_true_richness — average of both halves (raw, pre-EMA)
    let richness_raw: u16 = (exit_true_allowed0 + exit_true_allowed1) / 2;

    // Apply EMA to signal 4 only; update state
    let mut s = STATE.lock();
    s.tick_count = s.tick_count.saturating_add(1);
    s.exit_true_allowed0 = exit_true_allowed0;
    s.exit_true_allowed1 = exit_true_allowed1;
    s.exit_true_flexibility = exit_true_flexibility;
    s.exit_true_richness_ema = ema(s.exit_true_richness_ema, richness_raw);

    serial_println!(
        "[vmx_true_exit] allowed0={} allowed1={} flex={} richness={}",
        s.exit_true_allowed0, s.exit_true_allowed1,
        s.exit_true_flexibility, s.exit_true_richness_ema
    );
}

// ---------------------------------------------------------------------------
// Public accessors
// ---------------------------------------------------------------------------

/// Returns true if the CPU supports VMX and has any true exit control flexibility
pub fn has_true_flexibility() -> bool {
    let s = STATE.lock();
    s.exit_true_flexibility > 0
}

/// Returns the current signal snapshot
pub fn report() -> VmxTrueExitState {
    *STATE.lock()
}
