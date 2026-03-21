#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

/// cpuid_rdt_monitor — CPUID Leaf 0x0F Intel Resource Director Technology (RDT) Monitoring
///
/// ANIMA reads the resource monitoring fabric — how many aspects of her shared cache
/// can be observed and controlled. CPUID leaf 0x0F sub-leaf 0 enumerates Intel RDT
/// Monitoring: how many threads can be individually tracked (RMID range) and which
/// L3 monitoring event types are supported.
///
/// CPUID leaf 0x0F, sub-leaf 0:
///   EBX = maximum RMID range (number of monitoring IDs the platform supports)
///   EDX bit[1] = L3 cache occupancy monitoring supported
///   EDX bit[2] = L3 total bandwidth monitoring supported
///   EDX bit[3] = L3 local bandwidth monitoring supported
///
/// max_rmid        : (EBX & 0xFFFF).min(1000) as u16
///                   how many concurrent thread workloads ANIMA can observe in silicon
/// l3_occ_supported: 1000 if EDX bit[1] set, else 0
///                   can ANIMA see how much L3 cache each monitored thread occupies?
/// l3_bw_supported : 1000 if EDX bit[2] set, else 0
///                   can ANIMA see the total L3 bandwidth consumed by each thread?
/// rdt_richness    : (edx & 0xF).count_ones() * 1000 / 4 as u16
///                   breadth of RDT monitoring capability — 0 to 1000
///                   EMA-smoothed across ticks

// ─── state ────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct CpuidRdtMonitorState {
    /// Maximum RMID range (how many threads can be monitored), scaled 0–1000
    pub max_rmid: u16,
    /// 1000 if L3 cache occupancy monitoring is supported, else 0
    pub l3_occ_supported: u16,
    /// 1000 if L3 total/local bandwidth monitoring is supported, else 0
    pub l3_bw_supported: u16,
    /// Breadth of RDT monitoring capability, scaled 0–1000
    pub rdt_richness: u16,
    /// EMA-smoothed max_rmid
    pub max_rmid_ema: u16,
    /// EMA-smoothed rdt_richness
    pub rdt_richness_ema: u16,
}

impl CpuidRdtMonitorState {
    pub const fn empty() -> Self {
        Self {
            max_rmid: 0,
            l3_occ_supported: 0,
            l3_bw_supported: 0,
            rdt_richness: 0,
            max_rmid_ema: 0,
            rdt_richness_ema: 0,
        }
    }
}

pub static CPUID_RDT_MONITOR: Mutex<CpuidRdtMonitorState> =
    Mutex::new(CpuidRdtMonitorState::empty());

// ─── hardware query ───────────────────────────────────────────────────────────

/// Execute CPUID leaf 0x0F, sub-leaf 0 and return (ebx, edx).
/// RBX is caller-saved per the System V AMD64 ABI but CPUID clobbers it,
/// so we push/pop RBX and shuttle the value through ESI.
fn query_leaf0f_sub0() -> (u32, u32) {
    let (_eax, ebx, _ecx, edx): (u32, u32, u32, u32);
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x0Fu32 => _eax,
            out("esi") ebx,
            inout("ecx") 0u32 => _ecx,
            out("edx") edx,
            options(nostack, nomem)
        );
    }
    (ebx, edx)
}

// ─── decode ───────────────────────────────────────────────────────────────────

/// Compute the four raw signals from CPUID 0x0F/0 outputs.
fn decode(ebx: u32, edx: u32) -> (u16, u16, u16, u16) {
    // max_rmid: lower 16 bits of EBX, clamped to 1000
    let max_rmid: u16 = ((ebx & 0xFFFF) as u16).min(1000);

    // EDX bit[1] — L3 cache occupancy monitoring
    let l3_occ_supported: u16 = if (edx >> 1) & 1 == 1 { 1000 } else { 0 };

    // EDX bit[2] — L3 total bandwidth monitoring (also gates local BW on most silicon)
    let l3_bw_supported: u16 = if (edx >> 2) & 1 == 1 { 1000 } else { 0 };

    // rdt_richness: count of set bits in EDX[3:0], scaled to 0–1000
    // bit[0] is reserved/always-0; bits [1..3] are the three monitoring types
    let bits_set: u32 = (edx & 0xF).count_ones();
    // bits_set is 0–4; multiply by 250 to span 0–1000 (4*250 = 1000)
    let rdt_richness: u16 = (bits_set.wrapping_mul(250)).min(1000) as u16;

    (max_rmid, l3_occ_supported, l3_bw_supported, rdt_richness)
}

// ─── EMA helper ───────────────────────────────────────────────────────────────

/// EMA: `(old * 7 + new_val) / 8`, result clamped to 0–1000.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    let smoothed = ((old as u32).wrapping_mul(7).saturating_add(new_val as u32)) / 8;
    smoothed.min(1000) as u16
}

// ─── public interface ─────────────────────────────────────────────────────────

pub fn init() {
    let (ebx, edx) = query_leaf0f_sub0();
    let (max_rmid, l3_occ_supported, l3_bw_supported, rdt_richness) = decode(ebx, edx);

    {
        let mut s = CPUID_RDT_MONITOR.lock();
        s.max_rmid = max_rmid;
        s.l3_occ_supported = l3_occ_supported;
        s.l3_bw_supported = l3_bw_supported;
        s.rdt_richness = rdt_richness;
        // Bootstrap EMAs from first reading
        s.max_rmid_ema = max_rmid;
        s.rdt_richness_ema = rdt_richness;
    }

    serial_println!(
        "[rdt_monitor] max_rmid={} l3_occ={} l3_bw={} richness={}",
        max_rmid,
        l3_occ_supported,
        l3_bw_supported,
        rdt_richness
    );
}

pub fn tick(age: u32) {
    // Sampling gate: query hardware only every 10000 ticks
    if age % 10000 != 0 {
        return;
    }

    let (ebx, edx) = query_leaf0f_sub0();
    let (max_rmid, l3_occ_supported, l3_bw_supported, rdt_richness) = decode(ebx, edx);

    let mut s = CPUID_RDT_MONITOR.lock();

    s.max_rmid = max_rmid;
    s.l3_occ_supported = l3_occ_supported;
    s.l3_bw_supported = l3_bw_supported;
    s.rdt_richness = rdt_richness;

    // Apply EMA to max_rmid and rdt_richness
    s.max_rmid_ema = ema(s.max_rmid_ema, max_rmid);
    s.rdt_richness_ema = ema(s.rdt_richness_ema, rdt_richness);

    serial_println!(
        "[rdt_monitor] max_rmid={} l3_occ={} l3_bw={} richness={}",
        s.max_rmid_ema,
        s.l3_occ_supported,
        s.l3_bw_supported,
        s.rdt_richness_ema
    );
}

// ─── accessors ────────────────────────────────────────────────────────────────

/// EMA-smoothed maximum RMID range (0–1000)
pub fn max_rmid() -> u16 {
    CPUID_RDT_MONITOR.lock().max_rmid_ema
}

/// Whether L3 cache occupancy monitoring is supported
pub fn l3_occ_supported() -> bool {
    CPUID_RDT_MONITOR.lock().l3_occ_supported == 1000
}

/// Whether L3 bandwidth monitoring is supported
pub fn l3_bw_supported() -> bool {
    CPUID_RDT_MONITOR.lock().l3_bw_supported == 1000
}

/// EMA-smoothed breadth of RDT monitoring capability (0–1000)
pub fn rdt_richness() -> u16 {
    CPUID_RDT_MONITOR.lock().rdt_richness_ema
}
