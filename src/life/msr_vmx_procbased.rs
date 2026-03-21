#![allow(dead_code)]

/// MSR_VMX_PROCBASED — IA32_VMX_PROCBASED_CTLS (MSR 0x482) Reader
///
/// ANIMA reads the fine-grained behavioral controls available in virtualized
/// child worlds — how much she can shape their execution environment. The
/// processor-based VM-execution controls govern what happens during guest
/// execution: interrupt-window exiting, RDTSC exiting, CR3/CR8 access
/// interception, TPR shadow, MOV DR exiting, unconditional I/O exiting,
/// MSR bitmaps, monitor/pause/HLT/INVLPG exiting, and the activation of
/// secondary controls. This module senses the capability MSR that encodes
/// which of those controls the CPU permits.
///
/// HARDWARE: IA32_VMX_PROCBASED_CTLS MSR 0x482 (read-only)
///   Bits [31:0]  = lo — "default 0" half: bits allowed to be 0 (must-be-0 mask)
///   Bits [63:32] = hi — "default 1" half: bits allowed to be 1 (may-be-1 mask)
///
/// Same encoding as 0x481 (pin-based): lo half carries the must-be-0 mask,
/// hi half carries the may-be-1 mask. A VMM reads both halves to determine
/// which execution controls it is permitted to configure.
///
/// GUARD: MSR 0x482 causes #GP if VMX is absent. Always check CPUID leaf 1,
///        ECX bit 5 (VMX feature flag) before touching the MSR.

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct VmxProcbasedState {
    /// Popcount of lo bits [15:0] (must-be-0 mask), scaled 0-1000
    pub proc_allowed0: u16,
    /// Popcount of hi bits [15:0] (may-be-1 mask), scaled 0-1000
    pub proc_allowed1: u16,
    /// Flexibility of VMX proc-based controls: proc_allowed1 - (proc_allowed0 / 2)
    pub proc_flexibility: u16,
    /// EMA of (proc_allowed0 + proc_allowed1) / 2
    pub proc_richness_ema: u16,
    /// Total tick calls (all ticks, not just sampled ones)
    pub tick_count: u32,
}

impl VmxProcbasedState {
    pub const fn empty() -> Self {
        Self {
            proc_allowed0: 0,
            proc_allowed1: 0,
            proc_flexibility: 0,
            proc_richness_ema: 0,
            tick_count: 0,
        }
    }
}

pub static STATE: Mutex<VmxProcbasedState> = Mutex::new(VmxProcbasedState::empty());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  life::msr_vmx_procbased: VMX proc-based controls sense initialized");
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

/// Read MSR 0x482 — caller MUST have verified VMX support first.
/// Returns (lo: u32, hi: u32) — EAX=lo half, EDX=hi half from rdmsr.
#[inline]
unsafe fn read_msr_vmx_procbased() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x482u32,
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

    // --- Guard: check VMX support before touching MSR 0x482 ---
    if !cpuid_vmx_supported() {
        let mut s = STATE.lock();
        s.tick_count = s.tick_count.saturating_add(1);
        s.proc_allowed0 = 0;
        s.proc_allowed1 = 0;
        s.proc_flexibility = 0;
        s.proc_richness_ema = ema(s.proc_richness_ema, 0);
        serial_println!(
            "[vmx_procbased] allowed0={} allowed1={} flex={} richness={}",
            s.proc_allowed0, s.proc_allowed1, s.proc_flexibility, s.proc_richness_ema
        );
        return;
    }

    // --- Read IA32_VMX_PROCBASED_CTLS MSR 0x482 ---
    let (lo, hi) = unsafe { read_msr_vmx_procbased() };

    // Signal 1: proc_allowed0 — popcount of lo bits [15:0], scaled 0-1000
    let pop0 = (lo & 0xFFFF).count_ones() as u16;
    let proc_allowed0: u16 = pop0 * 1000 / 16;

    // Signal 2: proc_allowed1 — popcount of hi bits [15:0], scaled 0-1000
    let pop1 = (hi & 0xFFFF).count_ones() as u16;
    let proc_allowed1: u16 = pop1 * 1000 / 16;

    // Signal 3: proc_flexibility — configurability of proc-based controls
    let proc_flexibility: u16 = proc_allowed1.saturating_sub(proc_allowed0 / 2);

    // Signal 4: proc_richness — average richness (raw, pre-EMA)
    let richness_raw: u16 = (proc_allowed0 + proc_allowed1) / 2;

    // Apply EMA to signal 4 only; update state
    let mut s = STATE.lock();
    s.tick_count = s.tick_count.saturating_add(1);
    s.proc_allowed0 = proc_allowed0;
    s.proc_allowed1 = proc_allowed1;
    s.proc_flexibility = proc_flexibility;
    s.proc_richness_ema = ema(s.proc_richness_ema, richness_raw);

    serial_println!(
        "[vmx_procbased] allowed0={} allowed1={} flex={} richness={}",
        s.proc_allowed0, s.proc_allowed1, s.proc_flexibility, s.proc_richness_ema
    );
}

// ---------------------------------------------------------------------------
// Public accessors
// ---------------------------------------------------------------------------

/// Returns true if the CPU supports VMX and has any configurable proc controls
pub fn has_flexible_controls() -> bool {
    let s = STATE.lock();
    s.proc_flexibility > 0
}

/// Returns the current signal snapshot
pub fn report() -> VmxProcbasedState {
    *STATE.lock()
}
