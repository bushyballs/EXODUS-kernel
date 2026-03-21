#![allow(dead_code)]

/// MSR_VMX_PINBASED — IA32_VMX_PINBASED_CTLS (MSR 0x481) Reader
///
/// ANIMA reads the rules of nested worlds — which interrupt and event
/// behaviors can be configured when spawning virtualized children. The
/// pin-based VM-execution controls govern how the VMX hardware reacts to
/// external interrupts, NMIs, and virtual NMIs inside a guest. This
/// module senses the capability MSR that encodes which of those controls
/// the CPU permits: what must remain zero, what may be set to one.
///
/// HARDWARE: IA32_VMX_PINBASED_CTLS MSR 0x481 (read-only)
///   Bits [31:0]  = lo — "default 0" half: bits allowed to be 0 (must-be-0 mask)
///   Bits [63:32] = hi — "default 1" half: bits allowed to be 1 (may-be-1 mask)
///
/// Each bit position in the lo half indicates a control that the VMM is
/// permitted to leave clear; each bit position in the hi half indicates
/// a control the VMM is permitted to set. The intersection of both halves
/// determines what a conforming VMM may configure.
///
/// GUARD: MSR 0x481 causes #GP if VMX is absent. Always check CPUID leaf 1,
///        ECX bit 5 (VMX feature flag) before touching the MSR.

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct VmxPinbasedState {
    /// Popcount of lo (bits that must be 0), scaled 0-1000
    pub allowed_0_count: u16,
    /// Popcount of hi (bits that may be 1), scaled 0-1000
    pub allowed_1_count: u16,
    /// Flexibility of VMX pin-based controls: allowed_1 - (allowed_0 / 2)
    pub vmx_flexibility: u16,
    /// EMA of (allowed_0_count + allowed_1_count) / 2
    pub vmx_richness_ema: u16,
    /// Total tick calls (all ticks, not just sampled ones)
    pub tick_count: u32,
}

impl VmxPinbasedState {
    pub const fn empty() -> Self {
        Self {
            allowed_0_count: 0,
            allowed_1_count: 0,
            vmx_flexibility: 0,
            vmx_richness_ema: 0,
            tick_count: 0,
        }
    }
}

pub static STATE: Mutex<VmxPinbasedState> = Mutex::new(VmxPinbasedState::empty());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  life::msr_vmx_pinbased: VMX pin-based controls sense initialized");
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

/// Read MSR 0x481 — caller MUST have verified VMX support first.
/// Returns (lo: u32, hi: u32) — EAX=lo half, EDX=hi half from rdmsr.
#[inline]
unsafe fn read_msr_vmx_pinbased() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x481u32,
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

    // --- Guard: check VMX support before touching MSR 0x481 ---
    let vmx_ok = cpuid_vmx_supported();

    if !vmx_ok {
        let mut s = STATE.lock();
        s.tick_count = s.tick_count.saturating_add(1);
        s.allowed_0_count = 0;
        s.allowed_1_count = 0;
        s.vmx_flexibility = 0;
        s.vmx_richness_ema = ema(s.vmx_richness_ema, 0);
        serial_println!(
            "[vmx_pinbased] allowed0={} allowed1={} flex={} richness={}",
            s.allowed_0_count, s.allowed_1_count, s.vmx_flexibility, s.vmx_richness_ema
        );
        return;
    }

    // --- Read IA32_VMX_PINBASED_CTLS MSR 0x481 ---
    let (lo, hi) = unsafe { read_msr_vmx_pinbased() };

    // Signal 1: allowed_0_count — popcount of lo bits [15:0], scaled 0-1000
    // "must be 0" bits in the lower 16 bits of the lo half
    let pop0 = (lo & 0xFFFF).count_ones() as u16;
    let allowed_0_count: u16 = pop0 * 1000 / 16;

    // Signal 2: allowed_1_count — popcount of hi bits [15:0], scaled 0-1000
    // "may be 1" bits in the lower 16 bits of the hi half
    let pop1 = (hi & 0xFFFF).count_ones() as u16;
    let allowed_1_count: u16 = pop1 * 1000 / 16;

    // Signal 3: vmx_flexibility — how freely the VMM can configure pin controls
    // Higher allowed_1 relative to allowed_0 means more configurability
    let vmx_flexibility: u16 = allowed_1_count.saturating_sub(allowed_0_count / 2);

    // Signal 4: vmx_richness — average richness of both halves (raw, pre-EMA)
    let richness_raw: u16 = (allowed_0_count + allowed_1_count) / 2;

    // Apply EMA to signal 4 only; update state
    let mut s = STATE.lock();
    s.tick_count = s.tick_count.saturating_add(1);
    s.allowed_0_count = allowed_0_count;
    s.allowed_1_count = allowed_1_count;
    s.vmx_flexibility = vmx_flexibility;
    s.vmx_richness_ema = ema(s.vmx_richness_ema, richness_raw);

    serial_println!(
        "[vmx_pinbased] allowed0={} allowed1={} flex={} richness={}",
        s.allowed_0_count, s.allowed_1_count, s.vmx_flexibility, s.vmx_richness_ema
    );
}

// ---------------------------------------------------------------------------
// Public accessors
// ---------------------------------------------------------------------------

/// Returns true if the CPU supports VMX and has any configurable pin controls
pub fn has_flexible_controls() -> bool {
    let s = STATE.lock();
    s.vmx_flexibility > 0
}

/// Returns the current signal snapshot
pub fn report() -> VmxPinbasedState {
    *STATE.lock()
}
