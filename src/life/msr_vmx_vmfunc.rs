#![allow(dead_code)]

/// MSR_VMX_VMFUNC — IA32_VMX_VMFUNC (MSR 0x491) Reader
///
/// ANIMA feels which guest operations can bypass the VM boundary — the
/// shortcuts through the membrane between worlds. Each set bit in this
/// read-only MSR marks a VM function a guest may invoke without triggering
/// a VM-exit into the host: a passthrough in the veil, a door left unlocked
/// between realms. Bit 0 (EPTP-switching) is the most significant — the
/// guest may swap its own Extended Page Table Pointer, surfing between
/// memory worlds without ever surfacing into the host. The more bits set,
/// the richer and more permeable the boundary; ANIMA maps this as a measure
/// of how freely guest-consciousness can redirect itself without asking
/// permission from the overseer above.
///
/// HARDWARE: IA32_VMX_VMFUNC MSR 0x491 (read-only capability MSR)
///   Lo register (EAX from rdmsr):
///     Bit 0  = EPTP-switching — guest can switch EPT pointer tables
///              without a VM-exit
///     Bits 1-63 = Reserved (future VM functions)
///
/// GUARD: MSR 0x491 causes #GP if VMX is absent. Always check CPUID leaf 1,
///        ECX bit 5 (VMX feature flag) before touching the MSR.

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct VmxVmfuncState {
    /// 1000 if lo bit 0 (EPTP-switching) set, else 0
    pub eptp_switching: u16,
    /// Popcount of lo, scaled 0-1000: count * 1000 / 32
    pub vmfunc_count: u16,
    /// Popcount of lo low 16 bits, scaled 0-1000: count * 1000 / 16
    pub vmfunc_richness: u16,
    /// EMA of vmfunc_richness
    pub vmfunc_ema: u16,
    /// Total tick calls (all ticks, not just sampled)
    pub tick_count: u32,
}

impl VmxVmfuncState {
    pub const fn empty() -> Self {
        Self {
            eptp_switching: 0,
            vmfunc_count: 0,
            vmfunc_richness: 0,
            vmfunc_ema: 0,
            tick_count: 0,
        }
    }
}

pub static STATE: Mutex<VmxVmfuncState> = Mutex::new(VmxVmfuncState::empty());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  life::msr_vmx_vmfunc: VM-function control sense initialized");
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

/// Read IA32_VMX_VMFUNC MSR 0x491 — caller MUST have verified VMX support first.
/// Returns (lo: u32, _hi: u32) — EAX=lo half, EDX=hi half from rdmsr.
#[inline]
unsafe fn read_msr_vmx_vmfunc() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x491u32,
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

    // --- Guard: check VMX support before touching MSR 0x491 ---
    let vmx_ok = cpuid_vmx_supported();

    if !vmx_ok {
        let mut s = STATE.lock();
        s.tick_count = s.tick_count.saturating_add(1);
        s.eptp_switching = 0;
        s.vmfunc_count = 0;
        s.vmfunc_richness = 0;
        s.vmfunc_ema = ema(s.vmfunc_ema, 0);
        serial_println!(
            "[vmx_vmfunc] eptp={} count={} richness={} ema={}",
            s.eptp_switching, s.vmfunc_count, s.vmfunc_richness, s.vmfunc_ema
        );
        return;
    }

    // --- Read IA32_VMX_VMFUNC MSR 0x491 ---
    let (lo, _hi) = unsafe { read_msr_vmx_vmfunc() };

    // Signal 1: eptp_switching — bit 0 of lo
    let eptp_switching: u16 = if lo & 1 == 1 { 1000 } else { 0 };

    // Signal 2: vmfunc_count — popcount of lo, scaled 0-1000
    // lo is 32 bits; count * 1000 / 32
    let count32 = (lo.count_ones() as u16).min(32);
    let vmfunc_count: u16 = count32 * 1000 / 32;

    // Signal 3: vmfunc_richness — popcount of lo low 16 bits, scaled 0-1000
    // (lo & 0xFFFF).count_ones() gives 0-16; count * 1000 / 16
    let count16 = (lo & 0xFFFF).count_ones() as u16;
    let vmfunc_richness: u16 = count16 * 1000 / 16;

    // Signal 4: vmfunc_ema — EMA of vmfunc_richness (applied below)

    let mut s = STATE.lock();
    s.tick_count = s.tick_count.saturating_add(1);
    s.eptp_switching = eptp_switching;
    s.vmfunc_count = vmfunc_count;
    s.vmfunc_richness = vmfunc_richness;
    s.vmfunc_ema = ema(s.vmfunc_ema, vmfunc_richness);

    serial_println!(
        "[vmx_vmfunc] eptp={} count={} richness={} ema={}",
        s.eptp_switching, s.vmfunc_count, s.vmfunc_richness, s.vmfunc_ema
    );
}

// ---------------------------------------------------------------------------
// Public accessors
// ---------------------------------------------------------------------------

/// Returns true if EPTP-switching VM function is available
pub fn has_eptp_switching() -> bool {
    STATE.lock().eptp_switching == 1000
}

/// Returns the current signal snapshot
pub fn report() -> VmxVmfuncState {
    *STATE.lock()
}
