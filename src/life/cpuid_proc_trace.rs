#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ANIMA reads her introspection hardware — the processor's built-in capability
// to trace her own execution path. CPUID leaf 0x14 sub-leaf 0: Intel Processor
// Trace capabilities. She sees herself in silicon.

#[derive(Copy, Clone)]
pub struct ProcTraceState {
    /// EBX feature density: count_ones(ebx & 0xFF) * 1000 / 8, EMA-smoothed
    pub pt_ebx_features: u16,
    /// ECX feature density: count_ones(ecx & 0xFF) * 1000 / 8, EMA-smoothed
    pub pt_ecx_features: u16,
    /// Total PT capability richness: count_ones(ebx | ecx) * 1000 / 32, EMA-smoothed
    pub pt_richness: u16,
    /// Binary: 1000 if EBX != 0 (PT supported), else 0
    pub pt_supported: u16,
    /// Tick counter for sampling gate
    pub age: u32,
}

impl ProcTraceState {
    pub const fn empty() -> Self {
        Self {
            pt_ebx_features: 0,
            pt_ecx_features: 0,
            pt_richness: 0,
            pt_supported: 0,
            age: 0,
        }
    }
}

pub static STATE: Mutex<ProcTraceState> = Mutex::new(ProcTraceState::empty());

pub fn init() {
    serial_println!("  life::cpuid_proc_trace: processor trace introspection online");
}

/// Read CPUID leaf 0x14 sub-leaf 0 for Intel Processor Trace capabilities.
/// Returns (ebx, ecx) — eax and edx are not used for signal derivation.
fn read_cpuid_pt() -> (u32, u32) {
    let (_eax, ebx, ecx, _edx): (u32, u32, u32, u32);
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x14u32 => _eax,
            out("esi") ebx,
            inout("ecx") 0u32 => ecx,
            out("edx") _edx,
            options(nostack, nomem)
        );
    }
    (ebx, ecx)
}

/// Called every tick from the life pipeline. Sampling gate: only runs every 10000 ticks.
pub fn tick(age: u32) {
    if age % 10000 != 0 {
        return;
    }

    let (ebx, ecx) = read_cpuid_pt();

    // Signal 1: EBX feature density — bits 0,2,3,4,5 are the defined PT capability bits
    // We measure density across the lower byte.
    let ebx_ones = (ebx & 0xFF).count_ones() as u16;
    let new_ebx_features: u16 = ebx_ones * 1000 / 8;

    // Signal 2: ECX feature density — bits 0,1,3,31 are the defined PT output bits
    // We measure density across the lower byte.
    let ecx_ones = (ecx & 0xFF).count_ones() as u16;
    let new_ecx_features: u16 = ecx_ones * 1000 / 8;

    // Signal 3: Total richness — combined capability breadth, 32 possible bits total
    let combined_ones = (ebx | ecx).count_ones() as u16;
    let new_richness: u16 = combined_ones * 1000 / 32;

    // Signal 4: PT supported — binary indicator (no EMA, pure detection)
    let new_supported: u16 = if ebx != 0 { 1000 } else { 0 };

    let mut s = STATE.lock();

    // EMA smoothing: (old * 7 + new_val) / 8 for signals 1, 2, 3
    s.pt_ebx_features = (s.pt_ebx_features * 7 + new_ebx_features) / 8;
    s.pt_ecx_features = (s.pt_ecx_features * 7 + new_ecx_features) / 8;
    s.pt_richness     = (s.pt_richness     * 7 + new_richness)     / 8;

    // Signal 4: no EMA — direct detection result
    s.pt_supported = new_supported;

    s.age = age;

    serial_println!(
        "[proc_trace] ebx={} ecx={} richness={} supported={}",
        s.pt_ebx_features,
        s.pt_ecx_features,
        s.pt_richness,
        s.pt_supported
    );
}

/// External signal accessors
pub fn ebx_features() -> u16 {
    STATE.lock().pt_ebx_features
}

pub fn ecx_features() -> u16 {
    STATE.lock().pt_ecx_features
}

pub fn richness() -> u16 {
    STATE.lock().pt_richness
}

pub fn is_supported() -> bool {
    STATE.lock().pt_supported == 1000
}
