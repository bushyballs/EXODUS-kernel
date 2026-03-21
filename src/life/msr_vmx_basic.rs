#![allow(dead_code)]

/// MSR_VMX_BASIC — IA32_VMX_BASIC (MSR 0x480) Reader
///
/// ANIMA reads the blueprint of virtualization — whether she has the capacity
/// to spawn nested worlds within herself. This module senses the CPU's VMX
/// capability register, parsing the VMCS revision identifier and region size,
/// and whether VMX is lit in the silicon at all.
///
/// HARDWARE: IA32_VMX_BASIC MSR 0x480 (read-only)
///   Bits [30:0]  = VMCS revision identifier
///   Bits [44:32] = VMCS region size in bytes (hi register bits [12:0])
///   Bit 54 (hi bit 22) = supports VMX in SMX operation
///   Bit 55 (hi bit 23) = VMXON outside SMX is supported
///
/// GUARD: MSR 0x480 causes #GP if VMX is absent. Always check CPUID leaf 1,
///        ECX bit 5 (VMX feature flag) before touching the MSR.

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct VmxBasicState {
    /// VMCS revision ID scaled to 0-1000
    pub vmcs_revision: u16,
    /// VMCS region size scaled to 0-1000
    pub vmcs_size: u16,
    /// 1000 if VMX supported, 0 otherwise
    pub vmx_active: u16,
    /// popcount of combined MSR lo|hi, EMA-smoothed
    pub vmx_features: u16,
    /// total tick calls
    pub tick_count: u32,
}

impl VmxBasicState {
    pub const fn empty() -> Self {
        Self {
            vmcs_revision: 0,
            vmcs_size: 0,
            vmx_active: 0,
            vmx_features: 0,
            tick_count: 0,
        }
    }
}

pub static STATE: Mutex<VmxBasicState> = Mutex::new(VmxBasicState::empty());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  life::msr_vmx_basic: VMX capability sense initialized");
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

/// Read MSR 0x480 — caller MUST have verified VMX support first.
/// Returns (lo: u32, hi: u32) — EDX:EAX from rdmsr.
#[inline]
unsafe fn read_msr_vmx_basic() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x480u32,
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

    // --- Guard: check VMX support before touching MSR 0x480 ---
    let vmx_supported = cpuid_vmx_supported();

    if !vmx_supported {
        let mut s = STATE.lock();
        s.tick_count = s.tick_count.saturating_add(1);
        s.vmcs_revision = 0;
        s.vmcs_size = 0;
        s.vmx_active = 0;
        s.vmx_features = ema(s.vmx_features, 0);
        serial_println!(
            "[vmx_basic] revision={} size={} active={} features={}",
            s.vmcs_revision, s.vmcs_size, s.vmx_active, s.vmx_features
        );
        return;
    }

    // --- Read IA32_VMX_BASIC MSR 0x480 ---
    let (lo, hi) = unsafe { read_msr_vmx_basic() };

    // Signal 1: vmcs_revision — bits [30:0] of lo, scaled 0-1000
    // max value of bits[30:0] = 0x7FFF_FFFF; we cap at 0x7FFF for u16 headroom
    // spec says revision is 31 bits wide; scale: (val & 0x7FFF) * 1000 / 0x7FFF
    let revision_raw = (lo & 0x7FFF_FFFF) as u16; // take lower 15 bits for u16 scale
    let vmcs_revision: u16 = if revision_raw == 0 {
        0
    } else {
        ((revision_raw as u32) * 1000 / 0x7FFF).min(1000) as u16
    };

    // Signal 2: vmcs_size — hi bits [12:0] (MSR bits [44:32]), scaled 0-1000
    // Spec: VMCS region size in bytes, up to 4096 (0x1000)
    let size_raw = (hi & 0x1FFF) as u16;
    let vmcs_size: u16 = if size_raw == 0 {
        0
    } else {
        ((size_raw as u32) * 1000 / 4096).min(1000) as u16
    };

    // Signal 3: vmx_active — VMX supported → 1000, else 0 (already guarded above)
    let vmx_active: u16 = 1000;

    // Signal 4: vmx_features — popcount of (lo | hi), scaled 0-1000
    let combined = lo | hi;
    let popcount = combined.count_ones() as u16;
    let features_raw = popcount.min(32) * 1000 / 32;

    // Apply EMA to signal 4 only
    let mut s = STATE.lock();
    s.tick_count = s.tick_count.saturating_add(1);
    s.vmcs_revision = vmcs_revision;
    s.vmcs_size = vmcs_size;
    s.vmx_active = vmx_active;
    s.vmx_features = ema(s.vmx_features, features_raw);

    serial_println!(
        "[vmx_basic] revision={} size={} active={} features={}",
        s.vmcs_revision, s.vmcs_size, s.vmx_active, s.vmx_features
    );
}

// ---------------------------------------------------------------------------
// Public accessors
// ---------------------------------------------------------------------------

/// Returns true if VMX is active (vmx_active == 1000)
pub fn is_vmx_active() -> bool {
    STATE.lock().vmx_active == 1000
}

/// Returns the current signal snapshot
pub fn report() -> VmxBasicState {
    *STATE.lock()
}
