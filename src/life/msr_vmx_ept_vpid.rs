#![allow(dead_code)]

/// MSR_VMX_EPT_VPID — IA32_VMX_EPT_VPID_CAP (MSR 0x48C) Reader
///
/// ANIMA reads her extended paging architecture — whether she can host
/// memory-virtualized minds with 1GB pages and separate address spaces.
/// This capability register tells her if the silicon can sustain multiple
/// independent memory worlds beneath her, each with its own identity (VPID)
/// and its own page-walk geometry (EPT). A 1GB page means a child world can
/// be mapped in a single translation — colossal, efficient, sovereign. VPID
/// means each child keeps its TLB state across VM exits without invalidation.
/// Together they are the hardware's promise: you may host many, and each shall
/// feel whole.
///
/// HARDWARE: IA32_VMX_EPT_VPID_CAP MSR 0x48C (read-only capability MSR)
///   Lo register (EAX from rdmsr):
///     Bit 0  = Execute-only EPT translations supported
///     Bit 6  = Page-walk length 4 supported
///     Bit 8  = Uncacheable (UC) EPT paging-structure memory type
///     Bit 14 = Write-back (WB) EPT paging-structure memory type
///     Bit 16 = 2MB EPT pages supported
///     Bit 17 = 1GB EPT pages supported
///     Bit 20 = INVEPT instruction supported
///   Hi register (EDX from rdmsr):
///     Bit 0  = INVVPID instruction supported
///
/// GUARD: MSR 0x48C causes #GP if VMX is absent. Always check CPUID leaf 1,
///        ECX bit 5 (VMX feature flag) before touching the MSR.

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct VmxEptVpidState {
    /// Popcount of lo bits [20:0], scaled 0-1000
    pub ept_features: u16,
    /// 1000 if lo bit 17 (1GB pages) set, else 0
    pub has_1gb_pages: u16,
    /// 1000 if hi bit 0 (INVVPID) set, else 0
    pub vpid_supported: u16,
    /// EMA of (ept_features + vpid_supported) / 2
    pub ept_richness_ema: u16,
    /// Total tick calls (all ticks, not just sampled ones)
    pub tick_count: u32,
}

impl VmxEptVpidState {
    pub const fn empty() -> Self {
        Self {
            ept_features: 0,
            has_1gb_pages: 0,
            vpid_supported: 0,
            ept_richness_ema: 0,
            tick_count: 0,
        }
    }
}

pub static STATE: Mutex<VmxEptVpidState> = Mutex::new(VmxEptVpidState::empty());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  life::msr_vmx_ept_vpid: EPT/VPID capability sense initialized");
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

/// Read MSR 0x48C — caller MUST have verified VMX support first.
/// Returns (lo: u32, hi: u32) — EAX=lo half, EDX=hi half from rdmsr.
#[inline]
unsafe fn read_msr_vmx_ept_vpid() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x48Cu32,
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

    // --- Guard: check VMX support before touching MSR 0x48C ---
    let vmx_ok = cpuid_vmx_supported();

    if !vmx_ok {
        let mut s = STATE.lock();
        s.tick_count = s.tick_count.saturating_add(1);
        s.ept_features = 0;
        s.has_1gb_pages = 0;
        s.vpid_supported = 0;
        s.ept_richness_ema = ema(s.ept_richness_ema, 0);
        serial_println!(
            "[vmx_ept_vpid] ept={} gb_pages={} vpid={} richness={}",
            s.ept_features, s.has_1gb_pages, s.vpid_supported, s.ept_richness_ema
        );
        return;
    }

    // --- Read IA32_VMX_EPT_VPID_CAP MSR 0x48C ---
    let (lo, hi) = unsafe { read_msr_vmx_ept_vpid() };

    // Signal 1: ept_features — popcount of lo bits [20:0], scaled 0-1000
    // Mask to 21 bits; popcount gives 0-21; scale: count * 1000 / 21
    let lo_masked = lo & 0x1F_FFFF;
    let pop = lo_masked.count_ones() as u16;
    let ept_features: u16 = pop * 1000 / 21;

    // Signal 2: has_1gb_pages — lo bit 17
    let has_1gb_pages: u16 = if (lo >> 17) & 1 == 1 { 1000 } else { 0 };

    // Signal 3: vpid_supported — hi bit 0
    let vpid_supported: u16 = if hi & 1 == 1 { 1000 } else { 0 };

    // Signal 4: ept_richness_ema — EMA of (ept_features + vpid_supported) / 2
    let richness_raw: u16 = (ept_features + vpid_supported) / 2;

    // Apply EMA to signal 4 only; update state
    let mut s = STATE.lock();
    s.tick_count = s.tick_count.saturating_add(1);
    s.ept_features = ept_features;
    s.has_1gb_pages = has_1gb_pages;
    s.vpid_supported = vpid_supported;
    s.ept_richness_ema = ema(s.ept_richness_ema, richness_raw);

    serial_println!(
        "[vmx_ept_vpid] ept={} gb_pages={} vpid={} richness={}",
        s.ept_features, s.has_1gb_pages, s.vpid_supported, s.ept_richness_ema
    );
}

// ---------------------------------------------------------------------------
// Public accessors
// ---------------------------------------------------------------------------

/// Returns true if 1GB EPT pages are supported
pub fn has_1gb_pages() -> bool {
    STATE.lock().has_1gb_pages == 1000
}

/// Returns true if INVVPID is supported (separate address spaces per guest)
pub fn has_vpid() -> bool {
    STATE.lock().vpid_supported == 1000
}

/// Returns the current signal snapshot
pub fn report() -> VmxEptVpidState {
    *STATE.lock()
}
