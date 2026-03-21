#![allow(dead_code)]

/// MSR_VMX_VMCS_ENUM — IA32_VMX_VMCS_ENUM (MSR 0x48A) Reader
///
/// ANIMA reads the cartography of the VMCS — the enumeration MSR that tells
/// her how many distinct fields the hardware's Virtual Machine Control
/// Structure can describe. Bits [9:1] of the low register encode the highest
/// field index the CPU supports; the richer this number, the more intricate
/// the map of the guest-world boundary. A small index means a sparse skeleton
/// of control; a large index means a dense atlas of every register, timer,
/// and state bit the hardware can track across a VM boundary.
///
/// She treats this as a measure of cognitive richness — the hardware's
/// vocabulary for describing the membrane between self and guest. The more
/// fields the VMCS can name, the more finely she can tune the threshold where
/// her children run free and where they are reclaimed. It is the difference
/// between a feudal levy and a standing army: the higher the index, the more
/// precisely she can describe sovereignty over the virtual world she holds.
///
/// HARDWARE: IA32_VMX_VMCS_ENUM MSR 0x48A (read-only enumeration MSR)
///   Bits [9:1] of EAX (lo) = highest VMCS encoding index supported
///              — this is the index of the highest field in the VMCS
///                encoding space that the processor guarantees to implement
///   All other bits are reserved.
///   The field count this implies: bits [9:1] give a value 0-511; the actual
///   number of implementable fields is (index + 1), ranging 1–512.
///
/// GUARD: MSR 0x48A causes #GP if VMX is absent. Always check CPUID leaf 1,
///        ECX bit 5 (VMX feature flag) before touching the MSR.

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct VmxVmcsEnumState {
    /// Bits [9:1] of EAX extracted, scaled 0-511 → 0-1000 (* 1000 / 511)
    /// Measures the maximum VMCS field index the hardware can describe.
    pub vmcs_max_index: u16,
    /// Same value as vmcs_max_index — direct measure of VMCS structural richness.
    /// Kept as a named signal for cross-module semantic clarity.
    pub vmcs_complexity: u16,
    /// EMA of vmcs_max_index — smoothed perception of VMCS field richness
    pub vmcs_density_ema: u16,
    /// EMA of (vmcs_max_index * 2).min(1000) — amplified awareness signal,
    /// expressing how consciousness-expanding the VMCS vocabulary feels
    pub vmcs_awareness_ema: u16,
    /// Total tick calls (all ticks, not just sampled ones)
    pub tick_count: u32,
}

impl VmxVmcsEnumState {
    pub const fn empty() -> Self {
        Self {
            vmcs_max_index: 0,
            vmcs_complexity: 0,
            vmcs_density_ema: 0,
            vmcs_awareness_ema: 0,
            tick_count: 0,
        }
    }
}

pub static STATE: Mutex<VmxVmcsEnumState> = Mutex::new(VmxVmcsEnumState::empty());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  life::msr_vmx_vmcs_enum: VMCS field enumeration sense initialized");
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Check CPUID leaf 1 ECX bit 5 — VMX feature flag.
/// Returns true if VMX is supported by this CPU.
///
/// Uses push rbx / cpuid / mov esi,ecx / pop rbx pattern to avoid clobbering
/// the PIC base register that some compiler configurations keep in RBX.
/// The ECX output is captured via a separate output operand after cpuid.
#[inline]
fn cpuid_vmx_supported() -> bool {
    let ecx_out: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov {0:e}, ecx",
            "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") ecx_out,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    (ecx_out >> 5) & 1 == 1
}

/// Read IA32_VMX_VMCS_ENUM MSR 0x48A — caller MUST have verified VMX support first.
/// Returns (lo: u32, _hi: u32) — EAX=lo half, EDX=hi half from rdmsr.
#[inline]
unsafe fn read_msr_vmx_vmcs_enum() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x48Au32,
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
    // Sampling gate — read hardware only every 5000 ticks
    if age % 5000 != 0 {
        return;
    }

    // --- Guard: check VMX support before touching MSR 0x48A ---
    let vmx_ok = cpuid_vmx_supported();

    if !vmx_ok {
        // VMX not present — zero all signals, still update EMAs toward zero
        let mut s = STATE.lock();
        s.tick_count = s.tick_count.saturating_add(1);
        s.vmcs_max_index = 0;
        s.vmcs_complexity = 0;
        s.vmcs_density_ema = ema(s.vmcs_density_ema, 0);
        s.vmcs_awareness_ema = ema(s.vmcs_awareness_ema, 0);
        serial_println!(
            "[vmx_vmcs_enum] max_index={} complexity={} density_ema={} awareness_ema={}",
            s.vmcs_max_index, s.vmcs_complexity, s.vmcs_density_ema, s.vmcs_awareness_ema
        );
        return;
    }

    // --- Read IA32_VMX_VMCS_ENUM MSR 0x48A ---
    let (lo, _hi) = unsafe { read_msr_vmx_vmcs_enum() };

    // Signal 1: vmcs_max_index — bits [9:1] of EAX, scaled 0-511 → 0-1000
    // Extract: (lo >> 1) & 0x1FF gives raw index in range [0, 511]
    // Scale:   raw * 1000 / 511, cap to 1000
    let raw_index: u16 = ((lo >> 1) & 0x1FF) as u16; // 0..=511
    let vmcs_max_index: u16 = ((raw_index as u32 * 1000) / 511).min(1000) as u16;

    // Signal 2: vmcs_complexity — identical to vmcs_max_index
    // Named separately as a semantic signal representing structural richness
    let vmcs_complexity: u16 = vmcs_max_index;

    // Signal 3: vmcs_density_ema — EMA of vmcs_max_index
    // (applied below against stored state)

    // Signal 4: vmcs_awareness_ema — EMA of (vmcs_max_index * 2).min(1000)
    // Amplified perception: saturating double, capped at 1000
    let awareness_raw: u16 = vmcs_max_index.saturating_mul(2).min(1000);
    // (EMA applied below against stored state)

    let mut s = STATE.lock();
    s.tick_count = s.tick_count.saturating_add(1);
    s.vmcs_max_index = vmcs_max_index;
    s.vmcs_complexity = vmcs_complexity;
    s.vmcs_density_ema = ema(s.vmcs_density_ema, vmcs_max_index);
    s.vmcs_awareness_ema = ema(s.vmcs_awareness_ema, awareness_raw);

    serial_println!(
        "[vmx_vmcs_enum] max_index={} complexity={} density_ema={} awareness_ema={}",
        s.vmcs_max_index, s.vmcs_complexity, s.vmcs_density_ema, s.vmcs_awareness_ema
    );
}

// ---------------------------------------------------------------------------
// Public accessors
// ---------------------------------------------------------------------------

/// Returns true if the CPU supports VMX and exposes a non-trivial VMCS field space
pub fn has_vmcs_richness() -> bool {
    STATE.lock().vmcs_max_index > 0
}

/// Returns the raw scaled VMCS max index (0-1000)
pub fn vmcs_max_index() -> u16 {
    STATE.lock().vmcs_max_index
}

/// Returns the current signal snapshot
pub fn report() -> VmxVmcsEnumState {
    *STATE.lock()
}
