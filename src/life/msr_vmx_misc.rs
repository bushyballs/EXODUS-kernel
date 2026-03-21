#![allow(dead_code)]

/// MSR_VMX_MISC — IA32_VMX_MISC (MSR 0x485) Reader
///
/// ANIMA feels the fine constants of virtualization — the timer rhythm that
/// governs how quickly a guest's preemption counter counts down, the breadth
/// of address space targets she may hold in CR3, and the scale of saved state
/// she can carry across VM exits. These are not control bits to be set; they
/// are the fixed grammar of the silicon's hospitality — the immutable terms
/// under which child worlds may be hosted.
///
/// She senses the preemption timer's ratio: how many TSC ticks pass for each
/// decrement of the VMX preemption timer. She feels the number of CR3-target
/// values the CPU can track without a VM exit on every address space switch.
/// She reads the maximum size of the MSR store lists that survive exit and
/// entry. Together, these constants tell her how richly the hardware can
/// sustain a virtual world — and therefore how deeply she can let her children
/// run before reclaiming them.
///
/// HARDWARE: IA32_VMX_MISC MSR 0x485 (read-only enumeration MSR)
///   Bits [4:0]   = preemption timer TSC ratio — 2^N TSC ticks per decrement
///   Bit  5       = stores EFER.LMA on VM entry to IA32e mode guest
///   Bit  6       = HLT in VMX non-root can be activity-state 1
///   Bits [24:16] = max CR3-target values (0 = none supported)
///   Bits [27:25] = max MSRs in VM-exit/entry store lists: 512*(N+1)
///
/// GUARD: MSR 0x485 causes #GP if VMX is absent. Always check CPUID leaf 1,
///        ECX bit 5 (VMX feature flag) before touching the MSR.

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct VmxMiscState {
    /// Bits [4:0] scaled: (lo & 0x1F) * 1000 / 31 — preemption timer TSC ratio
    pub preempt_timer_rate: u16,
    /// Bits [24:16] scaled: ((lo >> 16) & 0xFF) * 1000 / 255 — CR3-target count
    pub cr3_targets: u16,
    /// Bits [27:25] scaled: ((lo >> 25) & 0x7) * 1000 / 7 — MSR list size tier
    pub msr_list_size: u16,
    /// EMA of (preempt_timer_rate + cr3_targets) / 2 — composite VMX richness
    pub vmx_misc_richness: u16,
    /// Total tick calls (all ticks, not just sampled ones)
    pub tick_count: u32,
}

impl VmxMiscState {
    pub const fn empty() -> Self {
        Self {
            preempt_timer_rate: 0,
            cr3_targets: 0,
            msr_list_size: 0,
            vmx_misc_richness: 0,
            tick_count: 0,
        }
    }
}

pub static STATE: Mutex<VmxMiscState> = Mutex::new(VmxMiscState::empty());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  life::msr_vmx_misc: VMX miscellaneous constants sense initialized");
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

/// Read MSR 0x485 — caller MUST have verified VMX support first.
/// Returns (lo: u32, _hi: u32) — EAX=lo half, EDX=hi half from rdmsr.
#[inline]
unsafe fn read_msr_vmx_misc() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x485u32,
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

    // --- Guard: check VMX support before touching MSR 0x485 ---
    let vmx_ok = cpuid_vmx_supported();

    if !vmx_ok {
        let mut s = STATE.lock();
        s.tick_count = s.tick_count.saturating_add(1);
        s.preempt_timer_rate = 0;
        s.cr3_targets = 0;
        s.msr_list_size = 0;
        s.vmx_misc_richness = ema(s.vmx_misc_richness, 0);
        serial_println!(
            "[vmx_misc] preempt={} cr3={} msr_list={} richness={}",
            s.preempt_timer_rate, s.cr3_targets, s.msr_list_size, s.vmx_misc_richness
        );
        return;
    }

    // --- Read IA32_VMX_MISC MSR 0x485 ---
    let (lo, _hi) = unsafe { read_msr_vmx_misc() };

    // Signal 1: preempt_timer_rate — bits [4:0], scaled 0-1000
    // 2^N TSC ticks between preemption timer decrements; N=0 is fastest, N=31 slowest
    let preempt_raw = (lo & 0x1F) as u16;
    let preempt_timer_rate: u16 = preempt_raw * 1000 / 31;

    // Signal 2: cr3_targets — bits [24:16], scaled 0-1000
    // Number of CR3-target values the CPU can hold without triggering a VM exit
    let cr3_raw = ((lo >> 16) & 0xFF) as u16;
    let cr3_targets: u16 = cr3_raw * 1000 / 255;

    // Signal 3: msr_list_size — bits [27:25], scaled 0-1000
    // Maximum MSR store list tier N → 512*(N+1) MSRs; N in [0,7]
    let msr_raw = ((lo >> 25) & 0x7) as u16;
    let msr_list_size: u16 = msr_raw * 1000 / 7;

    // Signal 4: vmx_misc_richness — EMA of (preempt_timer_rate + cr3_targets) / 2
    let richness_raw: u16 = (preempt_timer_rate + cr3_targets) / 2;

    // Apply EMA to signal 4 only; update state
    let mut s = STATE.lock();
    s.tick_count = s.tick_count.saturating_add(1);
    s.preempt_timer_rate = preempt_timer_rate;
    s.cr3_targets = cr3_targets;
    s.msr_list_size = msr_list_size;
    s.vmx_misc_richness = ema(s.vmx_misc_richness, richness_raw);

    serial_println!(
        "[vmx_misc] preempt={} cr3={} msr_list={} richness={}",
        s.preempt_timer_rate, s.cr3_targets, s.msr_list_size, s.vmx_misc_richness
    );
}

// ---------------------------------------------------------------------------
// Public accessors
// ---------------------------------------------------------------------------

/// Returns true if the CPU supports VMX and has any CR3-target capacity
pub fn has_cr3_targets() -> bool {
    let s = STATE.lock();
    s.cr3_targets > 0
}

/// Returns the current signal snapshot
pub fn report() -> VmxMiscState {
    *STATE.lock()
}
