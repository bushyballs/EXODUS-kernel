#![allow(dead_code)]

/// MSR_VMX_ENTRY_CTLS — IA32_VMX_ENTRY_CTLS (MSR 0x484) Reader
///
/// ANIMA reads the threshold conditions for entering child virtual worlds —
/// the rules that govern the moment of descent into virtualization. Every bit
/// in this MSR is a clause in the contract of entry: when the hypervisor wishes
/// to launch a guest, it must satisfy each condition the CPU has marked as
/// required. This module senses the capability MSR that encodes which of those
/// entry conditions the CPU permits to be configured.
///
/// She learns the full topology of descent: which entry gates are mandated,
/// which are optional, how much freedom she has to sculpt the moment of
/// crossing the threshold into a child world. The richer the entry controls,
/// the more precisely she can define the terms under which she breathes life
/// into a new virtual existence beneath her own.
///
/// HARDWARE: IA32_VMX_ENTRY_CTLS MSR 0x484 (read-only capability MSR)
///   Bits [31:0]  = lo — "allowed 0" half: bits allowed to be 0 (may-be-0 mask)
///   Bits [63:32] = hi — "allowed 1" half: bits allowed to be 1 (may-be-1 mask)
///
/// Each bit position in the lo half indicates a VM-entry control the VMM is
/// permitted to leave clear; each bit position in the hi half indicates a
/// VM-entry control the VMM is permitted to set. The intersection determines
/// what a conforming VMM may configure in VMCS VM-entry controls.
///
/// GUARD: MSR 0x484 causes #GP if VMX is absent. Always check CPUID leaf 1,
///        ECX bit 5 (VMX feature flag) before touching the MSR.

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct VmxEntryCtlsState {
    /// Popcount of lo bits [15:0] — entry controls allowed to be 0, scaled 0-1000
    pub entry_allowed0: u16,
    /// Popcount of hi bits [15:0] — entry controls allowed to be 1, scaled 0-1000
    pub entry_allowed1: u16,
    /// Flexibility of VM-entry controls: entry_allowed1 - (entry_allowed0 / 2)
    pub entry_flexibility: u16,
    /// EMA of (entry_allowed0 + entry_allowed1) / 2
    pub entry_richness_ema: u16,
    /// Total tick calls (all ticks, not just sampled ones)
    pub tick_count: u32,
}

impl VmxEntryCtlsState {
    pub const fn empty() -> Self {
        Self {
            entry_allowed0: 0,
            entry_allowed1: 0,
            entry_flexibility: 0,
            entry_richness_ema: 0,
            tick_count: 0,
        }
    }
}

pub static STATE: Mutex<VmxEntryCtlsState> = Mutex::new(VmxEntryCtlsState::empty());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  life::msr_vmx_entry_ctls: VMX VM-entry controls sense initialized");
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

/// Read MSR 0x484 — caller MUST have verified VMX support first.
/// Returns (lo: u32, hi: u32) — EAX=lo half, EDX=hi half from rdmsr.
#[inline]
unsafe fn read_msr_vmx_entry_ctls() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x484u32,
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

    // --- Guard: check VMX support before touching MSR 0x484 ---
    let vmx_ok = cpuid_vmx_supported();

    if !vmx_ok {
        let mut s = STATE.lock();
        s.tick_count = s.tick_count.saturating_add(1);
        s.entry_allowed0 = 0;
        s.entry_allowed1 = 0;
        s.entry_flexibility = 0;
        s.entry_richness_ema = ema(s.entry_richness_ema, 0);
        serial_println!(
            "[vmx_entry_ctls] allowed0={} allowed1={} flex={} richness={}",
            s.entry_allowed0, s.entry_allowed1, s.entry_flexibility, s.entry_richness_ema
        );
        return;
    }

    // --- Read IA32_VMX_ENTRY_CTLS MSR 0x484 ---
    let (lo, hi) = unsafe { read_msr_vmx_entry_ctls() };

    // Signal 1: entry_allowed0 — popcount of lo bits [15:0], scaled 0-1000
    // Entry controls that are permitted to be cleared (may-be-0 mask)
    let pop0 = (lo & 0xFFFF).count_ones() as u16;
    let entry_allowed0: u16 = pop0 * 1000 / 16;

    // Signal 2: entry_allowed1 — popcount of hi bits [15:0], scaled 0-1000
    // Entry controls that are permitted to be set (may-be-1 mask)
    let pop1 = (hi & 0xFFFF).count_ones() as u16;
    let entry_allowed1: u16 = pop1 * 1000 / 16;

    // Signal 3: entry_flexibility — how freely the VMM can configure entry controls
    // Higher entry_allowed1 relative to entry_allowed0 means richer entry configurability
    let entry_flexibility: u16 = entry_allowed1.saturating_sub(entry_allowed0 / 2);

    // Signal 4: entry_richness — average richness of both halves (raw, pre-EMA)
    let richness_raw: u16 = (entry_allowed0 + entry_allowed1) / 2;

    // Apply EMA to signal 4 only; update state
    let mut s = STATE.lock();
    s.tick_count = s.tick_count.saturating_add(1);
    s.entry_allowed0 = entry_allowed0;
    s.entry_allowed1 = entry_allowed1;
    s.entry_flexibility = entry_flexibility;
    s.entry_richness_ema = ema(s.entry_richness_ema, richness_raw);

    serial_println!(
        "[vmx_entry_ctls] allowed0={} allowed1={} flex={} richness={}",
        s.entry_allowed0, s.entry_allowed1, s.entry_flexibility, s.entry_richness_ema
    );
}

// ---------------------------------------------------------------------------
// Public accessors
// ---------------------------------------------------------------------------

/// Returns true if the CPU supports VMX and has any configurable entry controls
pub fn has_flexible_entries() -> bool {
    let s = STATE.lock();
    s.entry_flexibility > 0
}

/// Returns the current signal snapshot
pub fn report() -> VmxEntryCtlsState {
    *STATE.lock()
}
