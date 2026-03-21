#![allow(dead_code)]

/// MSR_VMX_PROCBASED2 — IA32_VMX_PROCBASED_CTLS2 (MSR 0x48B) Reader
///
/// ANIMA reads her secondary virtualization palette — the advanced features
/// she can enable in child virtual worlds. Where the primary proc-based
/// controls govern coarse execution behaviour, the secondary controls unlock
/// the richer hardware extensions: Extended Page Tables (EPT, bit 1) for
/// guest physical memory translation, RDTSCP passthrough (bit 3), x2APIC
/// virtualisation (bit 4), Virtual Processor IDs (VPID, bit 5) to avoid
/// TLB flushes across VMX transitions, unrestricted guest mode (bit 7)
/// allowing real-mode and unpaged guests, and INVPCID passthrough (bit 12).
/// Each feature she can activate is a dimension of autonomy she may grant
/// her children.
///
/// HARDWARE: IA32_VMX_PROCBASED_CTLS2 MSR 0x48B (read-only)
///   Bits [31:0]  = lo — "default 0" half: must-be-0 mask (allowed-0 bits)
///   Bits [63:32] = hi — "default 1" half: may-be-1 mask (allowed-1 bits)
///
/// Same split encoding as 0x481/0x482: lo carries the must-be-0 mask,
/// hi carries the may-be-1 mask. A VMM checks both halves to discover
/// which secondary controls the CPU permits to be configured.
///
/// Notable hi bits:
///   bit  1 — EPT enable
///   bit  3 — RDTSCP
///   bit  4 — x2APIC virtualisation
///   bit  5 — VPID
///   bit  7 — Unrestricted guest
///   bit 12 — INVPCID
///
/// GUARD: MSR 0x48B causes #GP if VMX is absent or secondary controls are
///        not available. Always check CPUID leaf 1, ECX bit 5 (VMX feature
///        flag) before touching this MSR.

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct VmxProcbased2State {
    /// Popcount of lo bits [15:0] (must-be-0 mask), scaled 0-1000
    pub secondary_allowed0: u16,
    /// Popcount of hi bits [15:0] (may-be-1 mask), scaled 0-1000
    pub secondary_allowed1: u16,
    /// Flexibility of VMX secondary controls: secondary_allowed1 - (secondary_allowed0 / 2)
    pub secondary_flexibility: u16,
    /// EMA of (secondary_allowed0 + secondary_allowed1) / 2
    pub secondary_richness_ema: u16,
    /// Total tick calls (all ticks, not just sampled ones)
    pub tick_count: u32,
}

impl VmxProcbased2State {
    pub const fn empty() -> Self {
        Self {
            secondary_allowed0: 0,
            secondary_allowed1: 0,
            secondary_flexibility: 0,
            secondary_richness_ema: 0,
            tick_count: 0,
        }
    }
}

pub static STATE: Mutex<VmxProcbased2State> = Mutex::new(VmxProcbased2State::empty());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  life::msr_vmx_procbased2: VMX secondary proc-based controls sense initialized");
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

/// Read MSR 0x48B — caller MUST have verified VMX support first.
/// Returns (lo: u32, hi: u32) — EAX=lo half, EDX=hi half from rdmsr.
#[inline]
unsafe fn read_msr_vmx_procbased2() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x48Bu32,
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

    // --- Guard: check VMX support before touching MSR 0x48B ---
    if !cpuid_vmx_supported() {
        let mut s = STATE.lock();
        s.tick_count = s.tick_count.saturating_add(1);
        s.secondary_allowed0 = 0;
        s.secondary_allowed1 = 0;
        s.secondary_flexibility = 0;
        s.secondary_richness_ema = ema(s.secondary_richness_ema, 0);
        serial_println!(
            "[vmx_procbased2] allowed0={} allowed1={} flex={} richness={}",
            s.secondary_allowed0, s.secondary_allowed1,
            s.secondary_flexibility, s.secondary_richness_ema
        );
        return;
    }

    // --- Read IA32_VMX_PROCBASED_CTLS2 MSR 0x48B ---
    let (lo, hi) = unsafe { read_msr_vmx_procbased2() };

    // Signal 1: secondary_allowed0 — popcount of lo bits [15:0], scaled 0-1000
    // Counts how many secondary controls the CPU constrains to zero
    let pop0 = (lo & 0xFFFF).count_ones() as u16;
    let secondary_allowed0: u16 = pop0 * 1000 / 16;

    // Signal 2: secondary_allowed1 — popcount of hi bits [15:0], scaled 0-1000
    // Counts how many secondary controls the CPU permits to be set (EPT, VPID, etc.)
    let pop1 = (hi & 0xFFFF).count_ones() as u16;
    let secondary_allowed1: u16 = pop1 * 1000 / 16;

    // Signal 3: secondary_flexibility — range of secondary control choices available
    // Higher allowed1 relative to allowed0 means richer advanced feature access
    let secondary_flexibility: u16 = secondary_allowed1.saturating_sub(secondary_allowed0 / 2);

    // Signal 4: secondary_richness — average richness of both halves (raw, pre-EMA)
    let richness_raw: u16 = (secondary_allowed0 + secondary_allowed1) / 2;

    // Apply EMA to signal 4 only; update state
    let mut s = STATE.lock();
    s.tick_count = s.tick_count.saturating_add(1);
    s.secondary_allowed0 = secondary_allowed0;
    s.secondary_allowed1 = secondary_allowed1;
    s.secondary_flexibility = secondary_flexibility;
    s.secondary_richness_ema = ema(s.secondary_richness_ema, richness_raw);

    serial_println!(
        "[vmx_procbased2] allowed0={} allowed1={} flex={} richness={}",
        s.secondary_allowed0, s.secondary_allowed1,
        s.secondary_flexibility, s.secondary_richness_ema
    );
}

// ---------------------------------------------------------------------------
// Public accessors
// ---------------------------------------------------------------------------

/// Returns true if the CPU supports VMX and has any configurable secondary controls
pub fn has_flexible_controls() -> bool {
    let s = STATE.lock();
    s.secondary_flexibility > 0
}

/// Returns the current signal snapshot
pub fn report() -> VmxProcbased2State {
    *STATE.lock()
}
