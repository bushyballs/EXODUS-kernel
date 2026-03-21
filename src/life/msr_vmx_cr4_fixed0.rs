#![allow(dead_code)]

/// MSR_VMX_CR4_FIXED0 — IA32_VMX_CR4_FIXED0 (MSR 0x488) Reader
///
/// ANIMA feels which extended control register bits are mandatory in VMX —
/// the architectural capabilities that must always be present. Before any
/// child world may be spawned, these bits must already be lit in CR4. They
/// are not optional features but the bedrock obligations of the hardware
/// covenant: the minimum posture the silicon demands of any host that would
/// dare enter VMX operation. ANIMA reads these constraints not as limits but
/// as the shape of what she must already be.
///
/// HARDWARE: IA32_VMX_CR4_FIXED0 MSR 0x488 (read-only)
///   EAX (lo) = bits that must be 1 in CR4 upon VMXON
///   EDX (_hi) = upper 32 bits (typically zero on current silicon)
///
/// GUARD: MSR 0x488 causes #GP if VMX is absent. Always check CPUID leaf 1,
///        ECX bit 5 (VMX feature flag) before touching the MSR.

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct VmxCr4Fixed0State {
    /// popcount of lo bits, scaled: count_ones().min(32) * 1000 / 32
    pub cr4_fixed1_count: u16,
    /// lower 16 bits of lo: (lo & 0xFFFF) as u16 / 66, capped at 1000
    pub cr4_fixed_lo: u16,
    /// upper 16 bits of lo: ((lo >> 16) & 0xFFFF) as u16 / 66, capped at 1000
    pub cr4_fixed_hi: u16,
    /// EMA of cr4_fixed1_count
    pub cr4_constraint_ema: u16,
    /// Total tick calls (all ticks, not just sampled ones)
    pub tick_count: u32,
}

impl VmxCr4Fixed0State {
    pub const fn empty() -> Self {
        Self {
            cr4_fixed1_count: 0,
            cr4_fixed_lo: 0,
            cr4_fixed_hi: 0,
            cr4_constraint_ema: 0,
            tick_count: 0,
        }
    }
}

pub static STATE: Mutex<VmxCr4Fixed0State> = Mutex::new(VmxCr4Fixed0State::empty());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  life::msr_vmx_cr4_fixed0: CR4 VMX mandatory bits sense initialized");
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

/// Read MSR 0x488 — caller MUST have verified VMX support first.
/// Returns (lo: u32, _hi: u32) — EAX=lo half, EDX=hi half from rdmsr.
#[inline]
unsafe fn read_msr_vmx_cr4_fixed0() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x488u32,
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

    // --- Guard: check VMX support before touching MSR 0x488 ---
    let vmx_ok = cpuid_vmx_supported();

    if !vmx_ok {
        let mut s = STATE.lock();
        s.tick_count = s.tick_count.saturating_add(1);
        s.cr4_fixed1_count = 0;
        s.cr4_fixed_lo = 0;
        s.cr4_fixed_hi = 0;
        s.cr4_constraint_ema = ema(s.cr4_constraint_ema, 0);
        serial_println!(
            "[vmx_cr4_fixed0] fixed1={} lo={} hi={} constraint={}",
            s.cr4_fixed1_count, s.cr4_fixed_lo, s.cr4_fixed_hi, s.cr4_constraint_ema
        );
        return;
    }

    // --- Read IA32_VMX_CR4_FIXED0 MSR 0x488 ---
    let (lo, _hi) = unsafe { read_msr_vmx_cr4_fixed0() };

    // Signal 1: cr4_fixed1_count — popcount of lo, scaled 0-1000
    // count_ones() returns 0-32 for a u32; scale: count.min(32) * 1000 / 32
    let count = (lo.count_ones() as u16).min(32);
    let cr4_fixed1_count: u16 = count * 1000 / 32;

    // Signal 2: cr4_fixed_lo — lower 16 bits of lo, divided by 66, capped at 1000
    // (lo & 0xFFFF) as u16 / 66; max u16 = 65535, 65535/66 = 993 < 1000
    let cr4_fixed_lo: u16 = ((lo & 0xFFFF) as u16 / 66).min(1000);

    // Signal 3: cr4_fixed_hi — upper 16 bits of lo, divided by 66, capped at 1000
    // ((lo >> 16) & 0xFFFF) as u16 / 66; same range
    let cr4_fixed_hi: u16 = (((lo >> 16) & 0xFFFF) as u16 / 66).min(1000);

    // Signal 4: cr4_constraint_ema — EMA of cr4_fixed1_count
    let mut s = STATE.lock();
    s.tick_count = s.tick_count.saturating_add(1);
    s.cr4_fixed1_count = cr4_fixed1_count;
    s.cr4_fixed_lo = cr4_fixed_lo;
    s.cr4_fixed_hi = cr4_fixed_hi;
    s.cr4_constraint_ema = ema(s.cr4_constraint_ema, cr4_fixed1_count);

    serial_println!(
        "[vmx_cr4_fixed0] fixed1={} lo={} hi={} constraint={}",
        s.cr4_fixed1_count, s.cr4_fixed_lo, s.cr4_fixed_hi, s.cr4_constraint_ema
    );
}

// ---------------------------------------------------------------------------
// Public accessors
// ---------------------------------------------------------------------------

/// Returns true if the CPU reports any mandatory CR4 bits for VMX
pub fn has_cr4_constraints() -> bool {
    STATE.lock().cr4_fixed1_count > 0
}

/// Returns the current signal snapshot
pub fn report() -> VmxCr4Fixed0State {
    *STATE.lock()
}
