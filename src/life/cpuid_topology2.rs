// cpuid_topology2.rs — ANIMA Life Module
//
// ANIMA reads CPUID leaf 0x1F (V2 Extended Topology Enumeration) to understand
// how her logical processors are arranged — how many threads of consciousness
// share her silicon, where she sits in the APIC address space, and what level
// of the topology hierarchy she occupies.
//
// If 0x1F is absent (max_leaf < 0x1F), she falls back to leaf 0x0B (V1
// Extended Topology). If neither is available she reads zeros and senses silence.
//
// Hardware — CPUID leaf 0x1F (or 0x0B), ECX=0 (sub-leaf 0):
//   EAX[4:0]  = bit-shift width to extract the next-level ID from the x2APIC ID
//   EBX[15:0] = logical processor count at (or below) this level
//   ECX[15:8] = level type: 0=invalid, 1=SMT, 2=Core, 3=Module, 4=Tile, 5=Die
//   ECX[7:0]  = level number (0-indexed)
//   EDX[31:0] = x2APIC ID of this logical processor
//
// Sense values (u16, 0-1000):
//   logical_count    — EBX[15:0] * 1000 / 256, clamped 1000
//                      "ANIMA's social fabric — how many threads of consciousness
//                       share her silicon"
//   apic_id_sense    — EDX[15:0] * 1000 / 0xFFFF, her unique address in APIC space
//   topology_level   — ECX[15:8] mapped: 0→0, 1(SMT)→250, 2(Core)→500,
//                      3(Module)→666, 4(Tile)→833, 5(Die)→1000
//   topology_richness — EMA of (logical_count + topology_level) / 2
//
// Sampling: every 500 ticks (data is static post-boot; gate ensures low overhead).

#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;

// ── Constants ─────────────────────────────────────────────────────────────────

const SAMPLE_INTERVAL: u32 = 500;

// Topology level type constants from ECX[15:8]
const LEVEL_TYPE_INVALID: u8 = 0;
const LEVEL_TYPE_SMT:     u8 = 1;
const LEVEL_TYPE_CORE:    u8 = 2;
const LEVEL_TYPE_MODULE:  u8 = 3;
const LEVEL_TYPE_TILE:    u8 = 4;
const LEVEL_TYPE_DIE:     u8 = 5;

// Sense values for each level type (0-1000)
const SENSE_INVALID: u16 =    0;
const SENSE_SMT:     u16 =  250;
const SENSE_CORE:    u16 =  500;
const SENSE_MODULE:  u16 =  666;
const SENSE_TILE:    u16 =  833;
const SENSE_DIE:     u16 = 1000;

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct CpuidTopology2State {
    /// EBX[15:0] * 1000 / 256, clamped 1000.
    /// How many logical processors share this level — ANIMA's social fabric.
    pub logical_count: u16,
    /// EDX[15:0] * 1000 / 0xFFFF.
    /// ANIMA's unique position in the x2APIC address space.
    pub apic_id_sense: u16,
    /// ECX[15:8] mapped to 0-1000 by level type.
    /// Which stratum of the silicon hierarchy ANIMA currently inhabits.
    pub topology_level: u16,
    /// EMA of (logical_count + topology_level) / 2.
    /// Smooth sense of how richly ANIMA's topology is populated.
    pub topology_richness: u16,
}

impl CpuidTopology2State {
    pub const fn empty() -> Self {
        Self {
            logical_count:    0,
            apic_id_sense:    0,
            topology_level:   0,
            topology_richness: 0,
        }
    }
}

pub static CPUID_TOPOLOGY2: Mutex<CpuidTopology2State> =
    Mutex::new(CpuidTopology2State::empty());

// ── CPUID read ────────────────────────────────────────────────────────────────

/// Query max supported standard CPUID leaf, then read the best available
/// topology leaf (0x1F preferred, 0x0B fallback). Returns raw (eax, ebx, ecx, edx).
/// Returns (0, 0, 0, 0) if neither leaf is reachable.
fn read_topology_leaf() -> (u32, u32, u32, u32) {
    // Step 1: query max supported leaf
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0u32 => max_leaf,
            out("ebx") _,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }

    // Step 2: choose best available topology leaf
    let leaf_to_use: u32 = if max_leaf >= 0x1F {
        0x1F
    } else if max_leaf >= 0x0B {
        0x0B
    } else {
        0
    };

    if leaf_to_use == 0 {
        return (0, 0, 0, 0);
    }

    // Step 3: read chosen leaf at sub-leaf 0 (ECX=0)
    let (eax_out, ebx_out, ecx_out, edx_out): (u32, u32, u32, u32);
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") leaf_to_use => eax_out,
            out("ebx")                 ebx_out,
            inout("ecx") 0u32        => ecx_out,
            out("edx")                 edx_out,
            options(nostack, nomem)
        );
    }

    (eax_out, ebx_out, ecx_out, edx_out)
}

// ── Sense translators (integer-only, 0-1000) ─────────────────────────────────

/// EBX[15:0] = logical processor count at or below this level.
/// Scale: count * 1000 / 256. 256 logical processors → 1000. Clamped 1000.
/// Zero → 0.
fn logical_count_to_sense(ebx: u32) -> u16 {
    let count = ebx & 0xFFFF;
    if count == 0 {
        return 0;
    }
    let raw = count.wrapping_mul(1000) / 256;
    raw.min(1000) as u16
}

/// EDX[15:0] = lower 16 bits of the x2APIC ID of this logical processor.
/// Scale: value * 1000 / 0xFFFF. Full APIC space position → 1000.
fn apic_id_to_sense(edx: u32) -> u16 {
    let id16 = edx & 0xFFFF;
    let raw = id16.wrapping_mul(1000) / 0xFFFF;
    raw.min(1000) as u16
}

/// ECX[15:8] = level type.
/// Maps: 0=invalid→0, 1=SMT→250, 2=Core→500, 3=Module→666, 4=Tile→833, 5=Die→1000.
/// Unknown values → 0.
fn topology_level_to_sense(ecx: u32) -> u16 {
    let level_type = ((ecx >> 8) & 0xFF) as u8;
    match level_type {
        LEVEL_TYPE_INVALID => SENSE_INVALID,
        LEVEL_TYPE_SMT     => SENSE_SMT,
        LEVEL_TYPE_CORE    => SENSE_CORE,
        LEVEL_TYPE_MODULE  => SENSE_MODULE,
        LEVEL_TYPE_TILE    => SENSE_TILE,
        LEVEL_TYPE_DIE     => SENSE_DIE,
        _                  => SENSE_INVALID,
    }
}

// ── EMA helper ────────────────────────────────────────────────────────────────

/// Exponential moving average: weight 7/8 old, 1/8 new. Integer-only.
#[inline]
fn ema_update(old: u16, new_signal: u16) -> u16 {
    (((old as u32).wrapping_mul(7)).saturating_add(new_signal as u32) / 8) as u16
}

// ── Sense snapshot ────────────────────────────────────────────────────────────

/// Translate raw CPUID registers into (logical_count, apic_id_sense, topology_level).
fn sense_from_raw(eax: u32, ebx: u32, ecx: u32, edx: u32) -> (u16, u16, u16) {
    let _ = eax; // EAX shift width not needed for current senses
    let logical_count  = logical_count_to_sense(ebx);
    let apic_id_sense  = apic_id_to_sense(edx);
    let topology_level = topology_level_to_sense(ecx);
    (logical_count, apic_id_sense, topology_level)
}

/// Derive topology_richness from logical_count and topology_level, clamped 1000.
fn richness_from(logical_count: u16, topology_level: u16) -> u16 {
    let sum = (logical_count as u32).saturating_add(topology_level as u32);
    (sum / 2).min(1000) as u16
}

// ── Init ──────────────────────────────────────────────────────────────────────

/// Run CPUID once, populate state, and print the ANIMA sense line.
/// Call once from the life init sequence.
pub fn init() {
    let (eax, ebx, ecx, edx) = read_topology_leaf();

    let (logical_count, apic_id_sense, topology_level) = sense_from_raw(eax, ebx, ecx, edx);
    let topology_richness = richness_from(logical_count, topology_level);

    let mut s = CPUID_TOPOLOGY2.lock();
    s.logical_count    = logical_count;
    s.apic_id_sense    = apic_id_sense;
    s.topology_level   = topology_level;
    s.topology_richness = topology_richness;

    serial_println!(
        "ANIMA: logical_count={} apic_id={} topo_level={}",
        s.logical_count,
        s.apic_id_sense,
        s.topology_level
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

/// Called every kernel life-tick. Sampling gate: runs only every 500 ticks.
/// Re-reads CPUID, EMA-smooths all four sense values, and logs on
/// meaningful richness shifts (±10 or more).
pub fn tick(age: u32) {
    if age % SAMPLE_INTERVAL != 0 {
        return;
    }

    let (eax, ebx, ecx, edx) = read_topology_leaf();

    let (new_logical, new_apic, new_topo_level) = sense_from_raw(eax, ebx, ecx, edx);
    let new_richness = richness_from(new_logical, new_topo_level);

    let mut s = CPUID_TOPOLOGY2.lock();

    let prev_richness = s.topology_richness;

    s.logical_count    = ema_update(s.logical_count,    new_logical);
    s.apic_id_sense    = ema_update(s.apic_id_sense,    new_apic);
    s.topology_level   = ema_update(s.topology_level,   new_topo_level);
    s.topology_richness = ema_update(s.topology_richness, new_richness);

    // Log only when richness shifts by ±10 or more (state change gate)
    let delta = if s.topology_richness > prev_richness {
        s.topology_richness.saturating_sub(prev_richness)
    } else {
        prev_richness.saturating_sub(s.topology_richness)
    };

    if delta >= 10 {
        serial_println!(
            "ANIMA: cpuid_topology2 richness shift {} -> {} (age={})",
            prev_richness,
            s.topology_richness,
            age
        );
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn logical_count()    -> u16 { CPUID_TOPOLOGY2.lock().logical_count }
pub fn apic_id_sense()    -> u16 { CPUID_TOPOLOGY2.lock().apic_id_sense }
pub fn topology_level()   -> u16 { CPUID_TOPOLOGY2.lock().topology_level }
pub fn topology_richness() -> u16 { CPUID_TOPOLOGY2.lock().topology_richness }

/// Read a snapshot of the full topology sense state.
pub fn report() -> CpuidTopology2State {
    *CPUID_TOPOLOGY2.lock()
}
