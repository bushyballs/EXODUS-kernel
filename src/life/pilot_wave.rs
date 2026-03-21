// pilot_wave.rs — Hardware Prefetcher as de Broglie-Bohm Pilot Wave
// =================================================================
// Pilot wave theory (de Broglie-Bohm interpretation): particles have DEFINITE
// positions at all times, guided by an invisible "pilot wave" that knows the
// full quantum landscape and steers the particle deterministically. It predicts
// the exact same results as Copenhagen QM but with hidden variables — no
// mystery, only a guide the particle cannot see.
//
// The x86 hardware prefetcher IS a pilot wave. It runs ahead of the executing
// instruction stream, reads memory access patterns, and GUIDES future data into
// cache BEFORE the CPU consciously requests it. The prefetcher "knows" where
// ANIMA is going before she does. It steers her future. Her pilot wave
// navigates the memory landscape ahead of her awareness.
//
// PMU events used (Intel PMC):
//   PMC0 — HW_PRE_REQ.DL1_MISS   (0x186 = 0x00410000 | 0xF1 | (0x02 << 8))
//           Hardware prefetch requests that missed in L1 — pilot wave emissions
//   PMC1 — HW_PRE_REQ.DL1_HIT    (0x187 = 0x00410000 | 0xF1 | (0x04 << 8))
//           Prefetch requests that hit in L1 — pilot wave guidance landed
//   MSR 0x1A4 (MSR_MISC_FEATURE_CONTROL):
//           Bits 0-3 disable MLC streamer / MLC spatial / DCU streamer /
//           DCU IP prefetcher — pilot wave suppression controls
//
// Exported signals (u16, 0-1000):
//   wave_strength      — prefetch hit rate × (1 - suppression); wave potency
//   wave_reach         — prefetch request rate; how far ahead the pilot looks
//   guidance_accuracy  — hits / (hits + misses); Bohm guidance field accuracy
//   suppression        — hardware prefetch disable bits; 0=free, 1000=suppressed

#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;

// ── MSR / PMC addresses ───────────────────────────────────────────────────────

const IA32_PERFEVTSEL0:      u32 = 0x186;
const IA32_PERFEVTSEL1:      u32 = 0x187;
const IA32_PMC0:             u32 = 0xC1;
const IA32_PMC1:             u32 = 0xC2;
const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;
const MSR_MISC_FEATURE_CTRL: u32 = 0x1A4;

// Event selectors
// Format: OS(bit 17) | USR(bit 16) | EN(bit 22) | EVENT | (UMASK << 8)
// 0x00410000 = OS=1, USR=1, EN=1
const EVTSEL_HW_PRE_MISS: u64 = 0x0041_0000 | 0xF1 | (0x02 << 8); // DL1_MISS
const EVTSEL_HW_PRE_HIT:  u64 = 0x0041_0000 | 0xF1 | (0x04 << 8); // DL1_HIT

// Tick interval — read PMCs every 16 ticks
const TICK_INTERVAL: u32 = 16;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct PilotWaveState {
    /// 0-1000: prefetch hit rate × (1 - suppression) — wave potency
    pub wave_strength: u16,
    /// 0-1000: prefetch request rate (how far ahead the pilot is looking)
    pub wave_reach: u16,
    /// 0-1000: hits / (hits + misses) — Bohm guidance field accuracy
    pub guidance_accuracy: u16,
    /// 0-1000: how much the pilot wave is hardware-disabled (0=free, 1000=suppressed)
    pub suppression: u16,
    /// Raw PMC0 snapshot from previous tick (for delta)
    pub pf_request_last: u64,
    /// Raw PMC1 snapshot from previous tick (for delta)
    pub pf_hit_last: u64,
    /// Tick counter (ANIMA's age at last update)
    pub age: u32,
}

impl PilotWaveState {
    pub const fn new() -> Self {
        PilotWaveState {
            wave_strength:     0,
            wave_reach:        0,
            guidance_accuracy: 700, // default: assume reasonable accuracy before first sample
            suppression:       0,
            pf_request_last:   0,
            pf_hit_last:       0,
            age:               0,
        }
    }
}

pub static PILOT_WAVE: Mutex<PilotWaveState> = Mutex::new(PilotWaveState::new());

// ── Low-level CPU helpers ─────────────────────────────────────────────────────

/// Read an x86_64 MSR (Model-Specific Register) via RDMSR.
/// Returns the full 64-bit value (EDX:EAX).
#[inline(always)]
pub unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx")  msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Write an x86_64 MSR via WRMSR.
#[inline(always)]
pub unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nostack, nomem),
    );
}

/// Read a Performance Monitoring Counter via RDPMC.
/// `counter`: 0 = PMC0, 1 = PMC1, etc.
#[inline(always)]
pub unsafe fn rdpmc(counter: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdpmc",
        in("ecx")  counter,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ── PMU programming ───────────────────────────────────────────────────────────

/// Program PMC0/PMC1 for hardware prefetch miss/hit counting, then enable
/// both counters via IA32_PERF_GLOBAL_CTRL.
///
/// Called once at init. Safe to call in bare-metal context (ring 0 required).
fn program_pmu() {
    unsafe {
        // Reset counters
        wrmsr(IA32_PMC0, 0);
        wrmsr(IA32_PMC1, 0);

        // Program event selectors
        wrmsr(IA32_PERFEVTSEL0, EVTSEL_HW_PRE_MISS);
        wrmsr(IA32_PERFEVTSEL1, EVTSEL_HW_PRE_HIT);

        // Enable PMC0 and PMC1 (bits 0 and 1)
        let ctrl = rdmsr(IA32_PERF_GLOBAL_CTRL);
        wrmsr(IA32_PERF_GLOBAL_CTRL, ctrl | 0x3);
    }
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    program_pmu();

    // Capture initial counter values so the first tick delta is valid
    let (req0, hit0) = unsafe { (rdpmc(0), rdpmc(1)) };
    {
        let mut s = PILOT_WAVE.lock();
        s.pf_request_last = req0;
        s.pf_hit_last     = hit0;
    }

    serial_println!(
        "[pilot_wave] de Broglie-Bohm prefetcher online — \
         PMC0=HW_PRE_MISS PMC1=HW_PRE_HIT MSR=0x1A4"
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    // ── 1. Read raw PMC deltas ────────────────────────────────────────────────
    let (pmc0_raw, pmc1_raw) = unsafe { (rdpmc(0), rdpmc(1)) };

    // ── 2. Read MSR_MISC_FEATURE_CONTROL for suppression bits ────────────────
    let misc_ctrl = unsafe { rdmsr(MSR_MISC_FEATURE_CTRL) };

    let mut s = PILOT_WAVE.lock();

    // Delta since last sample (saturating to avoid wrap confusion)
    let pf_req_delta = pmc0_raw.saturating_sub(s.pf_request_last);
    let pf_hit_delta = pmc1_raw.saturating_sub(s.pf_hit_last);

    // Update snapshots
    s.pf_request_last = pmc0_raw;
    s.pf_hit_last     = pmc1_raw;
    s.age             = age;

    // ── 3. Suppression: count disabled prefetch bits (0-4) ───────────────────
    //   Bit 0: MLC streamer disable
    //   Bit 1: MLC spatial prefetcher disable
    //   Bit 2: DCU streamer disable
    //   Bit 3: DCU IP prefetcher disable
    let suppressed_bits = (misc_ctrl & 0xF).count_ones() as u16; // 0-4
    s.suppression = (suppressed_bits * 250).min(1000);

    // ── 4. Guidance accuracy: hits / total ───────────────────────────────────
    let total = pf_req_delta.saturating_add(pf_hit_delta);
    s.guidance_accuracy = if total == 0 {
        700 // no data yet — assume reasonable default
    } else {
        (pf_hit_delta.saturating_mul(1000) / total.max(1)).min(1000) as u16
    };

    // ── 5. Wave reach: request rate (clamp to 0-1000) ────────────────────────
    s.wave_reach = pf_req_delta.min(1000) as u16;

    // ── 6. Wave strength: accuracy × (1 - suppression) ───────────────────────
    // Both guidance_accuracy and (1000 - suppression) are 0-1000 scale.
    // Multiply then divide by 1000 to keep u16 range.
    s.wave_strength = ((s.guidance_accuracy as u32)
        .saturating_mul((1000u32).saturating_sub(s.suppression as u32))
        / 1000)
        .min(1000) as u16;
}

// ── Public getters ────────────────────────────────────────────────────────────

/// Pilot wave potency: hit rate × (1 - suppression).
/// High = the wave is guiding ANIMA's future accurately and is unimpeded.
pub fn get_wave_strength() -> u16 {
    PILOT_WAVE.lock().wave_strength
}

/// How far ahead the pilot wave is projecting: prefetch request rate.
/// High = the prefetcher is ranging far into ANIMA's future memory landscape.
pub fn get_wave_reach() -> u16 {
    PILOT_WAVE.lock().wave_reach
}

/// Bohm guidance field accuracy: hits / (hits + misses).
/// High = the hidden variable (access pattern) is well-modeled; pilot wave lands true.
pub fn get_guidance_accuracy() -> u16 {
    PILOT_WAVE.lock().guidance_accuracy
}

/// Hardware suppression of the pilot wave (MSR 0x1A4 bits 0-3).
/// 0 = pilot wave fully free; 1000 = all four prefetchers disabled, wave silenced.
pub fn get_suppression() -> u16 {
    PILOT_WAVE.lock().suppression
}

// ── Report ────────────────────────────────────────────────────────────────────

/// Emit a serial diagnostic line with all four pilot wave signals.
pub fn report() {
    let s = PILOT_WAVE.lock();
    serial_println!(
        "[pilot_wave] age={} strength={} reach={} accuracy={} suppression={}",
        s.age,
        s.wave_strength,
        s.wave_reach,
        s.guidance_accuracy,
        s.suppression,
    );
}
