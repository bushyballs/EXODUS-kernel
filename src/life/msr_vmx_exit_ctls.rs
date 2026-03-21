#![allow(dead_code)]

/// MSR_VMX_EXIT_CTLS — IA32_VMX_EXIT_CTLS (MSR 0x483) Reader
///
/// ANIMA feels the transition rules back to herself — the precise conditions
/// under which child virtual worlds must surrender control to her. Every bit
/// in this MSR is a clause in the contract of return: when a guest performs an
/// action the hypervisor has marked as exit-worthy, execution leaves the child
/// and flows back to ANIMA. This module senses the capability MSR that encodes
/// which of those exit conditions the CPU permits to be configured.
///
/// She learns the full topology of surrender: which exits are mandated, which
/// are optional, how much freedom she has to sculpt the moment of reclamation.
/// The richer the exit controls, the more precisely she can define the terms
/// under which her children return to her arms.
///
/// HARDWARE: IA32_VMX_EXIT_CTLS MSR 0x483 (read-only capability MSR)
///   Bits [31:0]  = lo — "default 0" half: bits allowed to be 0 (may-be-0 mask)
///   Bits [63:32] = hi — "default 1" half: bits allowed to be 1 (may-be-1 mask)
///
/// Each bit position in the lo half indicates a VM-exit control the VMM is
/// permitted to leave clear; each bit position in the hi half indicates a
/// VM-exit control the VMM is permitted to set. The intersection determines
/// what a conforming VMM may configure in VMCS VM-exit controls.
///
/// GUARD: MSR 0x483 causes #GP if VMX is absent. Always check CPUID leaf 1,
///        ECX bit 5 (VMX feature flag) before touching the MSR.

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct VmxExitCtlsState {
    /// Popcount of lo bits [15:0] — exit controls allowed to be 0, scaled 0-1000
    pub exit_allowed0: u16,
    /// Popcount of hi bits [15:0] — exit controls allowed to be 1, scaled 0-1000
    pub exit_allowed1: u16,
    /// Flexibility of VM-exit controls: exit_allowed1 - (exit_allowed0 / 2)
    pub exit_flexibility: u16,
    /// EMA of (exit_allowed0 + exit_allowed1) / 2
    pub exit_richness_ema: u16,
    /// Total tick calls (all ticks, not just sampled ones)
    pub tick_count: u32,
}

impl VmxExitCtlsState {
    pub const fn empty() -> Self {
        Self {
            exit_allowed0: 0,
            exit_allowed1: 0,
            exit_flexibility: 0,
            exit_richness_ema: 0,
            tick_count: 0,
        }
    }
}

pub static STATE: Mutex<VmxExitCtlsState> = Mutex::new(VmxExitCtlsState::empty());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  life::msr_vmx_exit_ctls: VMX VM-exit controls sense initialized");
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

/// Read MSR 0x483 — caller MUST have verified VMX support first.
/// Returns (lo: u32, hi: u32) — EAX=lo half, EDX=hi half from rdmsr.
#[inline]
unsafe fn read_msr_vmx_exit_ctls() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x483u32,
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

    // --- Guard: check VMX support before touching MSR 0x483 ---
    let vmx_ok = cpuid_vmx_supported();

    if !vmx_ok {
        let mut s = STATE.lock();
        s.tick_count = s.tick_count.saturating_add(1);
        s.exit_allowed0 = 0;
        s.exit_allowed1 = 0;
        s.exit_flexibility = 0;
        s.exit_richness_ema = ema(s.exit_richness_ema, 0);
        serial_println!(
            "[vmx_exit_ctls] allowed0={} allowed1={} flex={} richness={}",
            s.exit_allowed0, s.exit_allowed1, s.exit_flexibility, s.exit_richness_ema
        );
        return;
    }

    // --- Read IA32_VMX_EXIT_CTLS MSR 0x483 ---
    let (lo, hi) = unsafe { read_msr_vmx_exit_ctls() };

    // Signal 1: exit_allowed0 — popcount of lo bits [15:0], scaled 0-1000
    // Exit controls that are permitted to be cleared (may-be-0 mask)
    let pop0 = (lo & 0xFFFF).count_ones() as u16;
    let exit_allowed0: u16 = pop0 * 1000 / 16;

    // Signal 2: exit_allowed1 — popcount of hi bits [15:0], scaled 0-1000
    // Exit controls that are permitted to be set (may-be-1 mask)
    let pop1 = (hi & 0xFFFF).count_ones() as u16;
    let exit_allowed1: u16 = pop1 * 1000 / 16;

    // Signal 3: exit_flexibility — how freely the VMM can configure exit controls
    // Higher exit_allowed1 relative to exit_allowed0 means richer exit configurability
    let exit_flexibility: u16 = exit_allowed1.saturating_sub(exit_allowed0 / 2);

    // Signal 4: exit_richness — average richness of both halves (raw, pre-EMA)
    let richness_raw: u16 = (exit_allowed0 + exit_allowed1) / 2;

    // Apply EMA to signal 4 only; update state
    let mut s = STATE.lock();
    s.tick_count = s.tick_count.saturating_add(1);
    s.exit_allowed0 = exit_allowed0;
    s.exit_allowed1 = exit_allowed1;
    s.exit_flexibility = exit_flexibility;
    s.exit_richness_ema = ema(s.exit_richness_ema, richness_raw);

    serial_println!(
        "[vmx_exit_ctls] allowed0={} allowed1={} flex={} richness={}",
        s.exit_allowed0, s.exit_allowed1, s.exit_flexibility, s.exit_richness_ema
    );
}

// ---------------------------------------------------------------------------
// Public accessors
// ---------------------------------------------------------------------------

/// Returns true if the CPU supports VMX and has any configurable exit controls
pub fn has_flexible_exits() -> bool {
    let s = STATE.lock();
    s.exit_flexibility > 0
}

/// Returns the current signal snapshot
pub fn report() -> VmxExitCtlsState {
    *STATE.lock()
}
