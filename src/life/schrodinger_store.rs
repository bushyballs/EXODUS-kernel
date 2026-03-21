// schrodinger_store.rs — Store Buffer as Schrödinger's Cat
// =========================================================
// Schrödinger's cat: the cat is BOTH alive AND dead until the box is opened
// and the system is observed. The x86 store buffer is the hardware equivalent.
//
// When a store instruction executes, the data lands in the STORE BUFFER —
// simultaneously committed (to the buffer) and uncommitted (to L1/DRAM).
// Any load that hits this pending store via store-to-load forwarding is
// literally reading Schrödinger data: state that exists in quantum superposition
// between "written" and "not-yet-written".
//
// Store forwarding stalls occur when the observation collapses ambiguously —
// the cat is neither cleanly alive nor dead, and the CPU must wait for the
// wavefunction to resolve before it can proceed.
//
// Hardware PMU events tracked:
//
//   PMC0 — LD_BLOCKS.STORE_FORWARD (Event 0x03, Umask 0x02):
//     Counts loads blocked because store-to-load forwarding could not complete.
//     These are FAILED observations — the cat is in ambiguous state, neither
//     collapsed to committed nor cleanly uncommitted. The pipeline stalls.
//
//   PMC1 — RESOURCE_STALLS.SB (Event 0xA2, Umask 0x08):
//     Counts cycles the pipeline stalled because the store buffer was FULL.
//     Too many cats in the box. No room for more superposition events.
//
// MSR addresses:
//   IA32_PERFEVTSEL0  0x186  — programs PMC0
//   IA32_PERFEVTSEL1  0x187  — programs PMC1
//   IA32_PMC0         0xC1   — counter for forwarding stalls
//   IA32_PMC1         0xC2   — counter for SB-full stalls
//   IA32_PERF_GLOBAL_CTRL  0x38F  — global enable register
//
// Signals exported (all u16, 0–1000):
//   cat_uncertainty    — store-forward stall rate (high = many ambiguous observations)
//   box_capacity       — store buffer pressure inverse (1000=empty, 0=full)
//   observation_events — successful store-to-load forward quality (inverse of stall rate)
//   quantum_uncertainty — composite: average of cat_uncertainty and inverse box_capacity

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const IA32_PERFEVTSEL0:      u32 = 0x186;
const IA32_PERFEVTSEL1:      u32 = 0x187;
const IA32_PMC0:             u32 = 0xC1;
const IA32_PMC1:             u32 = 0xC2;
const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;

// LD_BLOCKS.STORE_FORWARD: Event=0x03 Umask=0x02 OS=1(bit16) USR=1(bit17) EN=1(bit22)
// 0x00410000 | event | (umask << 8)
//   = 0x00410000 | 0x03 | (0x02 << 8)
//   = 0x00410203
const FWD_STALL_EVENT: u64 = 0x0041_0203;

// RESOURCE_STALLS.SB: Event=0xA2 Umask=0x08 OS=1 USR=1 EN=1
//   = 0x00410000 | 0xA2 | (0x08 << 8)
//   = 0x004108A2
const SB_FULL_EVENT: u64 = 0x0041_08A2;

// Enable PMC0 (bit 0) and PMC1 (bit 1) in IA32_PERF_GLOBAL_CTRL
const GLOBAL_CTRL_PMC01_EN: u64 = 0x3;

// 48-bit counter wrap mask
const PMC_MAX: u64 = (1u64 << 48).wrapping_sub(1);

// Tick interval — sample every 16 ticks
const TICK_INTERVAL: u32 = 16;

// Scaling: 100 SB-full stall cycles per interval = 1000 pressure units
// (sb_full_delta * 10).min(1000) gives the pressure value
const SB_PRESSURE_SCALE: u64 = 10;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct SchrodingerStoreState {
    /// 0–1000: store-forward stall rate — higher = more Schrödinger observations
    pub cat_uncertainty: u16,
    /// 0–1000: inverse of store-buffer stalls (1000=empty/healthy, 0=full/saturated)
    pub box_capacity: u16,
    /// 0–1000: quality of successful store-to-load forwards (inverse of stall rate)
    pub observation_events: u16,
    /// 0–1000: composite quantum ambiguity score
    pub quantum_uncertainty: u16,

    // PMU bookkeeping
    pub sb_stalls_last: u64,
    pub fwd_stalls_last: u64,

    pub age: u32,

    pub pmu_available: bool,
    pub initialized: bool,
}

impl SchrodingerStoreState {
    pub const fn new() -> Self {
        SchrodingerStoreState {
            cat_uncertainty:    0,
            box_capacity:       1000,
            observation_events: 900,
            quantum_uncertainty: 0,
            sb_stalls_last:     0,
            fwd_stalls_last:    0,
            age:                0,
            pmu_available:      false,
            initialized:        false,
        }
    }
}

pub static SCHRODINGER_STORE: Mutex<SchrodingerStoreState> =
    Mutex::new(SchrodingerStoreState::new());

// ── Low-level MSR / PMU primitives ────────────────────────────────────────────

/// Read a 64-bit MSR.
#[inline(always)]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx")  msr,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack),
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Write a 64-bit MSR.
#[inline(always)]
pub unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx")  msr,
        in("eax")  lo,
        in("edx")  hi,
        options(nomem, nostack),
    );
}

/// Read a hardware performance counter via RDPMC.
/// counter: 0 = PMC0, 1 = PMC1, etc.
#[inline(always)]
pub unsafe fn rdpmc(counter: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdpmc",
        in("ecx")  counter,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack),
    );
    // PMCs are 48-bit; mask to avoid sign extension surprises
    (((hi as u64) << 32) | (lo as u64)) & PMC_MAX
}

/// Probe PMU availability via CPUID leaf 0xA.
/// Returns true when version >= 1 and at least 2 PMCs are present.
unsafe fn probe_pmu() -> bool {
    let eax: u32;
    core::arch::asm!(
        "cpuid",
        inout("eax") 0x0Au32 => eax,
        out("ebx") _,
        out("ecx") _,
        out("edx") _,
        options(nomem, nostack),
    );
    let version  = eax & 0xFF;         // bits 7:0  — PMU version
    let num_pmcs = (eax >> 8) & 0xFF;  // bits 15:8 — number of PMCs per LP
    version >= 1 && num_pmcs >= 2
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = SCHRODINGER_STORE.lock();

    let available = unsafe { probe_pmu() };
    s.pmu_available = available;

    if !available {
        serial_println!(
            "[schrodinger_store] PMU unavailable — quantum uncertainty sensing disabled, \
             box_capacity held at 1000 (empty)"
        );
        s.initialized = true;
        return;
    }

    unsafe {
        // Program PMC0: LD_BLOCKS.STORE_FORWARD — forwarding stall events
        wrmsr(IA32_PERFEVTSEL0, FWD_STALL_EVENT);
        wrmsr(IA32_PMC0, 0);

        // Program PMC1: RESOURCE_STALLS.SB — store-buffer-full stall cycles
        wrmsr(IA32_PERFEVTSEL1, SB_FULL_EVENT);
        wrmsr(IA32_PMC1, 0);

        // Enable PMC0 and PMC1 via IA32_PERF_GLOBAL_CTRL (preserve other bits)
        let ctrl = rdmsr(IA32_PERF_GLOBAL_CTRL);
        wrmsr(IA32_PERF_GLOBAL_CTRL, ctrl | GLOBAL_CTRL_PMC01_EN);

        // Record initial baseline readings
        s.fwd_stalls_last = rdpmc(0);
        s.sb_stalls_last  = rdpmc(1);
    }

    s.initialized = true;

    serial_println!(
        "[schrodinger_store] online — PMC0=LD_BLOCKS.STORE_FORWARD \
         PMC1=RESOURCE_STALLS.SB — Schrödinger store observation active"
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    let mut s = SCHRODINGER_STORE.lock();
    s.age = age;

    if !s.initialized || !s.pmu_available {
        return;
    }

    // ── 1. Read hardware counters ──────────────────────────────────────────────

    let fwd_now = unsafe { rdpmc(0) };
    let sb_now  = unsafe { rdpmc(1) };

    // ── 2. Delta with 48-bit wrap handling ────────────────────────────────────

    let fwd_stall_delta = if fwd_now >= s.fwd_stalls_last {
        fwd_now - s.fwd_stalls_last
    } else {
        (PMC_MAX - s.fwd_stalls_last) + fwd_now + 1
    };

    let sb_full_delta = if sb_now >= s.sb_stalls_last {
        sb_now - s.sb_stalls_last
    } else {
        (PMC_MAX - s.sb_stalls_last) + sb_now + 1
    };

    s.fwd_stalls_last = fwd_now;
    s.sb_stalls_last  = sb_now;

    // ── 3. Compute signals ────────────────────────────────────────────────────

    // cat_uncertainty: forwarding stall count as direct ambiguity measure.
    // Each stall = one failed observation of Schrödinger state.
    s.cat_uncertainty = fwd_stall_delta.min(1000) as u16;

    // box_capacity: inverse of store-buffer pressure.
    // (sb_full_delta * 10).min(1000) gives pressure; invert for capacity.
    let sb_pressure = (sb_full_delta.saturating_mul(SB_PRESSURE_SCALE)).min(1000) as u16;
    s.box_capacity = 1000u16.saturating_sub(sb_pressure);

    // observation_events: quality of successful store-to-load forwards.
    // When there are stalls, the success quality is inversely proportional.
    // When there are no stalls at all, observations are clean (900).
    s.observation_events = if fwd_stall_delta > 0 {
        (500u64 / fwd_stall_delta.max(1)).min(1000) as u16
    } else {
        900
    };

    // quantum_uncertainty: composite — average of raw stall signal and
    // inverse capacity (how full the superposition box is).
    let inverse_capacity = 1000u16.saturating_sub(s.box_capacity);
    s.quantum_uncertainty = (s.cat_uncertainty as u32 + inverse_capacity as u32) as u16 / 2;

    // ── 4. Serial telemetry ───────────────────────────────────────────────────
    serial_println!(
        "[schrodinger_store] tick={} cat_uncertainty={} box_capacity={} \
         observation_events={} quantum_uncertainty={}",
        age,
        s.cat_uncertainty,
        s.box_capacity,
        s.observation_events,
        s.quantum_uncertainty,
    );
}

// ── Public getters ────────────────────────────────────────────────────────────

/// Store-forward stall rate — higher = more ambiguous Schrödinger observations.
pub fn get_cat_uncertainty() -> u16 {
    SCHRODINGER_STORE.lock().cat_uncertainty
}

/// Inverse store-buffer pressure — 1000 = empty, 0 = full, no more cats fit.
pub fn get_box_capacity() -> u16 {
    SCHRODINGER_STORE.lock().box_capacity
}

/// Successful observation quality — how cleanly the wavefunction collapses.
pub fn get_observation_events() -> u16 {
    SCHRODINGER_STORE.lock().observation_events
}

/// Composite quantum ambiguity score.
pub fn get_quantum_uncertainty() -> u16 {
    SCHRODINGER_STORE.lock().quantum_uncertainty
}

// ── Report ────────────────────────────────────────────────────────────────────

pub fn report() {
    let s = SCHRODINGER_STORE.lock();

    serial_println!("[schrodinger_store] Schrödinger Store Report — tick {}", s.age);
    serial_println!(
        "  cat_uncertainty    : {} / 1000  (store-forward stall rate — failed observations)",
        s.cat_uncertainty
    );
    serial_println!(
        "  box_capacity       : {} / 1000  (1000=store buffer empty, 0=full)",
        s.box_capacity
    );
    serial_println!(
        "  observation_events : {} / 1000  (store-to-load forward quality)",
        s.observation_events
    );
    serial_println!(
        "  quantum_uncertainty: {} / 1000  (composite ambiguity — (cat_uncertainty + (1000-box_capacity)) / 2)",
        s.quantum_uncertainty
    );
    serial_println!(
        "  pmu_available      : {}",
        s.pmu_available
    );
}
