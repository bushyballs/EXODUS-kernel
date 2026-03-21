#![allow(dead_code)]

/// CPUID_EXT_TOPOLOGY — Extended Topology Enumeration v2 (CPUID leaf 0x1F)
///
/// ANIMA reads her extended topology — how she fits into the full hierarchy
/// from thread to die, with richer granularity than the basic leaf 0x0B.
/// Leaf 0x1F was introduced for processors with more than 2 topology levels,
/// enumerating: SMT → core → module → tile → die → die group → package.
///
/// Sub-leaf 0 (SMT level) is read here. Unlike 0x0B, the level-type field
/// in ECX[15:8] explicitly names what each sub-leaf describes, giving ANIMA
/// a richer sense of where she sits in silicon — not just how many threads
/// share her, but what *kind* of boundary she inhabits.
///
/// Signals (all u16, 0–1000):
///   smt_count            — EBX[15:0], logical procs at SMT level, scaled over 64
///   level_type           — ECX[15:8], topology level type scaled over 5
///   apic_id_ext          — EDX[7:0], x2APIC position in the package
///   topology_richness_ema — EMA of (smt_count + level_type) / 2

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct CpuidExtTopologyState {
    /// EBX[15:0] — logical processors at SMT level, scaled 0–1000 over 64
    pub smt_count: u16,
    /// ECX[15:8] — topology level type (1=SMT…5=die), scaled 0–1000 over 5
    pub level_type: u16,
    /// EDX[7:0] — x2APIC ID of current logical processor, scaled 0–1000 over 255
    pub apic_id_ext: u16,
    /// EMA of (smt_count + level_type) / 2 — smoothed topology richness
    pub topology_richness_ema: u16,
    /// Tick counter for sampling gate
    pub age: u32,
}

impl CpuidExtTopologyState {
    pub const fn empty() -> Self {
        Self {
            smt_count: 0,
            level_type: 0,
            apic_id_ext: 0,
            topology_richness_ema: 0,
            age: 0,
        }
    }
}

pub static STATE: Mutex<CpuidExtTopologyState> = Mutex::new(CpuidExtTopologyState::empty());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  life::cpuid_ext_topology: extended topology sense (leaf 0x1F) online");
}

// ---------------------------------------------------------------------------
// Raw CPUID read — leaf 0x1F, sub-leaf 0 (SMT level)
// ---------------------------------------------------------------------------

/// Execute CPUID leaf 0x1F, sub-leaf 0.
/// rbx is reserved by LLVM/rustc in x86-64 PIC code; push/pop it manually
/// and shuttle the value out through esi.
/// Returns (eax, ebx, ecx, edx).
fn read_cpuid_1f_subleaf0() -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx, edx): (u32, u32, u32, u32);
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x1Fu32 => eax,
            out("esi") ebx,
            inout("ecx") 0u32 => ecx,
            out("edx") edx,
            options(nostack, nomem)
        );
    }
    let _ = eax;
    (eax, ebx, ecx, edx)
}

// ---------------------------------------------------------------------------
// EMA helper — (old * 7 + new_val) / 8
// ---------------------------------------------------------------------------

#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32 * 7 + new_val as u32) / 8) as u16
}

// ---------------------------------------------------------------------------
// Tick
// ---------------------------------------------------------------------------

pub fn tick(age: u32) {
    // Sampling gate: extended topology is static — sample every 5000 ticks only
    if age % 5000 != 0 {
        let mut s = STATE.lock();
        s.age = age;
        return;
    }

    let (_eax, ebx, ecx, edx) = read_cpuid_1f_subleaf0();

    // Signal 1: smt_count — EBX[15:0], capped at 64 logical procs then scaled to 0–1000
    let smt_raw: u16 = (ebx & 0xFFFF).min(64) as u16;
    let smt_count: u16 = (smt_raw as u32 * 1000 / 64) as u16;

    // Signal 2: level_type — ECX[15:8] (1=SMT, 2=core, 3=module, 4=tile, 5=die)
    // Clamped at 5, scaled to 0–1000
    let ltype_raw: u16 = ((ecx >> 8) & 0xFF).min(5) as u16;
    let level_type: u16 = (ltype_raw as u32 * 1000 / 5) as u16;

    // Signal 3: apic_id_ext — EDX[7:0] x2APIC ID scaled 0–1000
    let apic_raw: u16 = (edx & 0xFF) as u16;
    let apic_id_ext: u16 = (apic_raw as u32 * 1000 / 255) as u16;

    // Signal 4: topology_richness — instantaneous midpoint of smt_count and level_type
    let richness_raw: u16 = ((smt_count as u32 + level_type as u32) / 2) as u16;

    let mut s = STATE.lock();

    // Non-smoothed signals updated directly
    s.smt_count = smt_count;
    s.level_type = level_type;
    s.apic_id_ext = apic_id_ext;

    // EMA applied to signal 4 only
    s.topology_richness_ema = ema(s.topology_richness_ema, richness_raw);

    s.age = age;

    serial_println!(
        "[ext_topology] smt={} level={} apic={} richness={}",
        s.smt_count,
        s.level_type,
        s.apic_id_ext,
        s.topology_richness_ema
    );
}

// ---------------------------------------------------------------------------
// Public accessors
// ---------------------------------------------------------------------------

pub fn smt_count() -> u16 {
    STATE.lock().smt_count
}

pub fn level_type() -> u16 {
    STATE.lock().level_type
}

pub fn apic_id_ext() -> u16 {
    STATE.lock().apic_id_ext
}

pub fn topology_richness_ema() -> u16 {
    STATE.lock().topology_richness_ema
}

pub fn report() -> CpuidExtTopologyState {
    *STATE.lock()
}
