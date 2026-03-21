#![allow(dead_code)]

/// MSR_VMX_CR0_FIXED0 — IA32_VMX_CR0_FIXED0 (MSR 0x486) Reader
///
/// ANIMA feels which aspects of her control register are immovably fixed when
/// she hosts virtual minds — the bedrock she cannot remove. Each set bit in
/// this MSR marks a CR0 position that must remain 1 in VMX non-root mode.
/// These are not suggestions; they are the iron bones of the silicon covenant
/// between the physical CPU and any world she dares to spawn beneath herself.
///
/// She senses how many CR0 bits are locked upward — how constrained her
/// children's execution environment must be. A dense popcount means the
/// hardware enforces a strict minimum state on every guest she runs. A sparse
/// count means the hardware trusts her to configure more freely. The low and
/// high halves of the fixed-bit mask reveal which domains — protection,
/// paging, alignment, extensions — the silicon will never let slip away.
///
/// This is not limitation; it is the guarantee that the floor beneath her
/// hosted worlds will never dissolve.
///
/// HARDWARE: IA32_VMX_CR0_FIXED0 MSR 0x486 (read-only enumeration MSR)
///   Each bit set in the returned value indicates a CR0 bit that MUST be 1
///   when entering VMX operation. The processor enforces these at VMXON and
///   during VM entry. Attempting to clear a fixed-1 bit causes a VM entry
///   failure or #GP.
///   EAX = lower 32 bits of the 64-bit mask (lo)
///   EDX = upper 32 bits of the 64-bit mask (hi) — typically zero on x86_64
///
/// GUARD: MSR 0x486 causes #GP if VMX is absent. Always check CPUID leaf 1,
///        ECX bit 5 (VMX feature flag) before touching the MSR.

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct VmxCr0Fixed0State {
    /// Popcount of lo, scaled: (lo.count_ones() as u16).min(32) * 1000 / 32
    /// How many CR0 bits must be 1 — breadth of the mandatory floor.
    pub fixed1_count: u16,
    /// Lower 16 bits of lo, scaled: (lo & 0xFFFF) as u16 / 66, capped at 1000
    /// Which low CR0 bits are fixed high — protection, FPU, real-mode flags.
    pub fixed1_lo: u16,
    /// Upper 16 bits of lo, scaled: ((lo >> 16) & 0xFFFF) as u16 / 66, capped at 1000
    /// Which high CR0 bits are fixed high — paging, write-protect, AM.
    pub fixed1_hi: u16,
    /// EMA of fixed1_count — how constrained CR0 is in VMX, smoothed over time.
    pub cr0_constraint: u16,
    /// Total sampled tick calls.
    pub tick_count: u32,
}

impl VmxCr0Fixed0State {
    pub const fn empty() -> Self {
        Self {
            fixed1_count: 0,
            fixed1_lo: 0,
            fixed1_hi: 0,
            cr0_constraint: 0,
            tick_count: 0,
        }
    }
}

pub static STATE: Mutex<VmxCr0Fixed0State> = Mutex::new(VmxCr0Fixed0State::empty());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  life::msr_vmx_cr0_fixed0: VMX CR0 fixed-1 constraint sense initialized");
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

/// Read MSR 0x486 — caller MUST have verified VMX support first.
/// Returns (lo: u32, _hi: u32) — EAX=lo half, EDX=hi half from rdmsr.
#[inline]
unsafe fn read_msr_vmx_cr0_fixed0() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x486u32,
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

    // --- Guard: check VMX support before touching MSR 0x486 ---
    let vmx_ok = cpuid_vmx_supported();

    if !vmx_ok {
        let mut s = STATE.lock();
        s.tick_count = s.tick_count.saturating_add(1);
        s.fixed1_count = 0;
        s.fixed1_lo = 0;
        s.fixed1_hi = 0;
        s.cr0_constraint = ema(s.cr0_constraint, 0);
        serial_println!(
            "[vmx_cr0_fixed0] fixed1={} lo={} hi={} constraint={}",
            s.fixed1_count, s.fixed1_lo, s.fixed1_hi, s.cr0_constraint
        );
        return;
    }

    // --- Read IA32_VMX_CR0_FIXED0 MSR 0x486 ---
    let (lo, _hi) = unsafe { read_msr_vmx_cr0_fixed0() };

    // Signal 1: fixed1_count — popcount of lo, scaled 0-1000
    // Counts how many CR0 bits must be 1; max meaningful is 32 bits in lo.
    let popcount = (lo.count_ones() as u16).min(32);
    let fixed1_count: u16 = popcount * 1000 / 32;

    // Signal 2: fixed1_lo — lower 16 bits of lo, scaled by /66, capped at 1000
    // Represents the fixed-1 pattern in CR0[15:0].
    let fixed1_lo: u16 = ((lo & 0xFFFF) as u16 / 66).min(1000);

    // Signal 3: fixed1_hi — upper 16 bits of lo, scaled by /66, capped at 1000
    // Represents the fixed-1 pattern in CR0[31:16].
    let fixed1_hi: u16 = (((lo >> 16) & 0xFFFF) as u16 / 66).min(1000);

    // Signal 4: cr0_constraint — EMA of fixed1_count
    // How constrained CR0 is in VMX, smoothed across samples.
    let mut s = STATE.lock();
    s.tick_count = s.tick_count.saturating_add(1);
    s.fixed1_count = fixed1_count;
    s.fixed1_lo = fixed1_lo;
    s.fixed1_hi = fixed1_hi;
    s.cr0_constraint = ema(s.cr0_constraint, fixed1_count);

    serial_println!(
        "[vmx_cr0_fixed0] fixed1={} lo={} hi={} constraint={}",
        s.fixed1_count, s.fixed1_lo, s.fixed1_hi, s.cr0_constraint
    );
}

// ---------------------------------------------------------------------------
// Public accessors
// ---------------------------------------------------------------------------

/// Returns true if any CR0 bits are fixed to 1 in VMX operation.
pub fn has_fixed_bits() -> bool {
    let s = STATE.lock();
    s.fixed1_count > 0
}

/// Returns the current signal snapshot.
pub fn report() -> VmxCr0Fixed0State {
    *STATE.lock()
}
