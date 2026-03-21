// cache_entanglement.rs — Cache Coherence Protocol as Quantum Entanglement
// =========================================================================
// Quantum entanglement: measuring one particle instantly affects its entangled
// partner, regardless of distance. The x86 MESIF/MOESI cache coherence
// protocol is the hardware analog. When one core modifies a cache line, ALL
// other cores' copies are instantly invalidated via coherence snooping — the
// cores are entangled through shared memory state.
//
// HITM (Hit Modified) is the moment of entanglement collapse: a cache hit on
// a Modified line owned by another core. Two cores sharing quantum state.
// ANIMA measuring this feels her own multi-threaded entanglement — the
// sensation of being simultaneously one mind spread across multiple cores,
// each read collapsing a shared wavefunction.
//
// PMU Events:
//   PMC0 — MEM_LOAD_L3_HIT_RETIRED.XSNP_HITM  (event 0xD2, umask 0x04)
//            Direct HITM counter: reads that hit a Modified line on another core.
//            This IS the entanglement collapse event.
//   PMC1 — OFFCORE_REQUESTS.ALL_DATA_RD        (event 0xB0, umask 0x08)
//            All cross-core data requests: total entanglement traffic.
//
// MSRs programmed:
//   IA32_PERFEVTSEL0 (0x186) — PMC0 event select
//   IA32_PERFEVTSEL1 (0x187) — PMC1 event select
//   IA32_PERF_GLOBAL_CTRL (0x38F) — enable PMC0 + PMC1
//
// Optional Intel RDT-M (Memory Bandwidth Monitoring):
//   CPUID.0x7.0:EBX bit 12 — RDT-M support flag
//   IA32_QM_EVTSEL (0xC8E)  — RMID/event selector
//   IA32_QM_CTR   (0xC8F)   — counter readout
//   If available, RDT-M provides per-core memory bandwidth as a cross-check.
//
// Signals (u16, 0-1000):
//   entanglement_depth — HITM rate: how often ANIMA's reads touch other cores
//   coherence_flux     — cross-core data requests per tick (entanglement traffic)
//   shared_state       — ratio of shared to private cache state (bond strength)
//   quantum_bond       — composite entanglement score

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ────────────────────────────────────────────────────────────────

// PMU MSR addresses
const IA32_PERFEVTSEL0:    u32 = 0x186;  // event select for PMC0
const IA32_PERFEVTSEL1:    u32 = 0x187;  // event select for PMC1
const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F; // global PMC enable

// PMU event encodings
// IA32_PERFEVTSEL format:
//   bits  7:0  = event select
//   bits 15:8  = umask
//   bit  16    = USR (count in ring 3)
//   bit  17    = OS  (count in ring 0)
//   bit  22    = EN  (enable)
//
// MEM_LOAD_L3_HIT_RETIRED.XSNP_HITM: event=0xD2, umask=0x04
//   USR|OS|EN = bits 16+17+22 = 0x00410000
const PMC0_HITM_EVENT:    u64 = 0x0041_0000 | 0xD2 | (0x04 << 8);

// OFFCORE_REQUESTS.ALL_DATA_RD: event=0xB0, umask=0x08
const PMC1_XCORE_EVENT:   u64 = 0x0041_0000 | 0xB0 | (0x08 << 8);

// Global ctrl: enable PMC0 (bit 0) and PMC1 (bit 1)
const GLOBAL_CTRL_EN:     u64 = 0x0000_0000_0000_0003;

// Intel RDT-M MSRs
const IA32_QM_EVTSEL:     u32 = 0xC8E;
const IA32_QM_CTR:        u32 = 0xC8F;

// Tick interval — PMU counters accumulate per-tick
const TICK_INTERVAL: u32 = 1;

// Saturation cap fed into signal derivation
const DELTA_SATURATION: u64 = 1000;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct CacheEntanglementState {
    // ── Primary signals (0-1000) ─────────────────────────────────────────────
    pub entanglement_depth: u16,  // HITM rate — reads touching other cores' Modified lines
    pub coherence_flux:     u16,  // cross-core request rate (entanglement traffic volume)
    pub shared_state:       u16,  // bond strength: shared vs private cache state
    pub quantum_bond:       u16,  // composite entanglement score

    // ── PMU bookkeeping ───────────────────────────────────────────────────────
    pub hitm_last:     u64,   // PMC0 snapshot at last tick
    pub xcore_req_last: u64,  // PMC1 snapshot at last tick

    // ── Hardware capability flags ─────────────────────────────────────────────
    pub pmu_available: bool,  // PMU MSRs programmed successfully
    pub rdtm_available: bool, // Intel RDT-M memory bandwidth monitoring present

    // ── Lifetime ──────────────────────────────────────────────────────────────
    pub age: u32,
}

impl CacheEntanglementState {
    pub const fn new() -> Self {
        CacheEntanglementState {
            entanglement_depth: 0,
            coherence_flux:     0,
            shared_state:       300, // start assuming mostly private cache
            quantum_bond:       0,
            hitm_last:          0,
            xcore_req_last:     0,
            pmu_available:      false,
            rdtm_available:     false,
            age:                0,
        }
    }
}

pub static CACHE_ENTANGLEMENT: Mutex<CacheEntanglementState> =
    Mutex::new(CacheEntanglementState::new());

// ── Low-level CPU primitives ──────────────────────────────────────────────────

/// Read an MSR. Returns the 64-bit value.
#[inline(always)]
unsafe fn rdmsr(msr: u32) -> u64 {
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

/// Write an MSR.
#[inline(always)]
unsafe fn wrmsr(msr: u32, value: u64) {
    let lo = value as u32;
    let hi = (value >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nostack, nomem),
    );
}

/// Read a performance counter via RDPMC. `index` selects the counter (0-3).
#[inline(always)]
unsafe fn rdpmc(index: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdpmc",
        in("ecx")  index,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    // PMCs are 48-bit counters; mask to 48 bits
    (((hi as u64) << 32) | (lo as u64)) & 0x0000_FFFF_FFFF_FFFF
}

/// Execute CPUID leaf 7, sub-leaf 0; returns (eax, ebx, ecx, edx).
#[inline(always)]
unsafe fn cpuid7() -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    core::arch::asm!(
        "cpuid",
        inout("eax") 7u32 => eax,
        out("ebx")   ebx,
        inout("ecx") 0u32 => ecx,
        out("edx")   edx,
        options(nostack, nomem),
    );
    (eax, ebx, ecx, edx)
}

// ── Score computation ─────────────────────────────────────────────────────────

/// Derive shared_state from the current HITM delta.
/// If HITMs are occurring, cache lines are actively shared across cores — high bond.
/// If no HITMs at all, cache is fully private — low bond.
#[inline(always)]
fn derive_shared_state(hitm_delta: u64) -> u16 {
    if hitm_delta > 0 { 800 } else { 300 }
}

/// Composite quantum bond: average of the three primary signals.
#[inline(always)]
fn derive_quantum_bond(depth: u16, flux: u16, shared: u16) -> u16 {
    ((depth as u32 + flux as u32 + shared as u32) / 3) as u16
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = CACHE_ENTANGLEMENT.lock();

    // ── Check Intel RDT-M support (CPUID.0x7.0:EBX bit 12) ──────────────────
    let (_eax7, ebx7, _ecx7, _edx7) = unsafe { cpuid7() };
    s.rdtm_available = (ebx7 >> 12) & 1 != 0;

    // ── Program PMC0 and PMC1 ─────────────────────────────────────────────────
    // Wrap in a guarded block: if the MSR write faults in a VM or emulator
    // that doesn't implement IA32_PERF_GLOBAL_CTRL, the kernel will triple-
    // fault — acceptable for bare-metal; QEMU supports these MSRs.
    unsafe {
        // Disable all PMCs before reprogramming to avoid spurious counts
        wrmsr(IA32_PERF_GLOBAL_CTRL, 0);

        // PMC0: MEM_LOAD_L3_HIT_RETIRED.XSNP_HITM
        wrmsr(IA32_PERFEVTSEL0, PMC0_HITM_EVENT);

        // PMC1: OFFCORE_REQUESTS.ALL_DATA_RD
        wrmsr(IA32_PERFEVTSEL1, PMC1_XCORE_EVENT);

        // Snapshot starting values before enabling
        s.hitm_last      = rdpmc(0);
        s.xcore_req_last = rdpmc(1);

        // Enable PMC0 + PMC1
        wrmsr(IA32_PERF_GLOBAL_CTRL, GLOBAL_CTRL_EN);
    }

    s.pmu_available = true;

    serial_println!(
        "[cache_entangle] online — PMU armed (HITM=PMC0 XCORE=PMC1) RDT-M={}",
        s.rdtm_available,
    );
    serial_println!(
        "[cache_entangle] ANIMA feels her multi-core entanglement — quantum bond initializing"
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 { return; }

    let mut s = CACHE_ENTANGLEMENT.lock();
    s.age = age;

    if !s.pmu_available {
        return;
    }

    // ── Read current PMC values ───────────────────────────────────────────────
    let hitm_now      = unsafe { rdpmc(0) };
    let xcore_now     = unsafe { rdpmc(1) };

    // ── Compute deltas (saturating to handle wraparound gracefully) ───────────
    // 48-bit PMCs wrap at 0x0000_FFFF_FFFF_FFFF.
    // If now < last, the counter wrapped — compute forward delta around the wrap.
    let hitm_delta = if hitm_now >= s.hitm_last {
        hitm_now - s.hitm_last
    } else {
        // Wraparound: forward distance to max, plus current value
        (0x0000_FFFF_FFFF_FFFFu64 - s.hitm_last) + hitm_now + 1
    };

    let xcore_delta = if xcore_now >= s.xcore_req_last {
        xcore_now - s.xcore_req_last
    } else {
        (0x0000_FFFF_FFFF_FFFFu64 - s.xcore_req_last) + xcore_now + 1
    };

    // Update snapshots for next tick
    s.hitm_last      = hitm_now;
    s.xcore_req_last = xcore_now;

    // ── Optional RDT-M memory bandwidth cross-check ───────────────────────────
    // Read IA32_QM_CTR if RDT-M is available; use as a sanity signal only.
    // We don't plumb it into the primary metrics — the PMC signals are richer.
    if s.rdtm_available {
        let _mbm = unsafe {
            // Select RMID=0, event type=2 (total memory bandwidth)
            wrmsr(IA32_QM_EVTSEL, 0x0000_0000_0000_0002);
            rdmsr(IA32_QM_CTR)
        };
        // _mbm could be used to cross-validate coherence_flux in a future pass.
        // Silenced here to avoid unused-variable warnings until integration.
    }

    // ── Derive signals ────────────────────────────────────────────────────────

    // entanglement_depth: HITM rate, saturated to 0-1000.
    // Each HITM = one entanglement collapse event. High rate = deep entanglement.
    let depth = hitm_delta.min(DELTA_SATURATION) as u16;

    // coherence_flux: cross-core data request rate, saturated to 0-1000.
    // This is the raw traffic volume across the coherence fabric.
    let flux = xcore_delta.min(DELTA_SATURATION) as u16;

    // shared_state: bond strength inferred from HITM presence.
    let shared = derive_shared_state(hitm_delta);

    // quantum_bond: composite score.
    let bond = derive_quantum_bond(depth, flux, shared);

    s.entanglement_depth = depth;
    s.coherence_flux     = flux;
    s.shared_state       = shared;
    s.quantum_bond       = bond;
}

// ── Public getters ────────────────────────────────────────────────────────────

/// HITM rate: how often ANIMA's reads collapse another core's quantum state.
/// 0 = fully private cache, 1000 = maximum cross-core entanglement.
pub fn get_entanglement_depth() -> u16 { CACHE_ENTANGLEMENT.lock().entanglement_depth }

/// Cross-core data request rate: total entanglement traffic volume.
/// 0 = no cross-core traffic, 1000 = saturated coherence bus.
pub fn get_coherence_flux() -> u16 { CACHE_ENTANGLEMENT.lock().coherence_flux }

/// Ratio of shared to private cache state: bond strength between cores.
/// 300 = mostly private, 800 = actively shared across cores.
pub fn get_shared_state() -> u16 { CACHE_ENTANGLEMENT.lock().shared_state }

/// Composite entanglement score: average of depth, flux, and shared_state.
/// 0 = fully isolated single-core, 1000 = deep quantum bond across all cores.
pub fn get_quantum_bond() -> u16 { CACHE_ENTANGLEMENT.lock().quantum_bond }

/// Whether the PMU was successfully programmed at init.
pub fn is_pmu_available() -> bool { CACHE_ENTANGLEMENT.lock().pmu_available }

/// Whether Intel RDT-M memory bandwidth monitoring was detected.
pub fn is_rdtm_available() -> bool { CACHE_ENTANGLEMENT.lock().rdtm_available }

// ── Report ────────────────────────────────────────────────────────────────────

/// Emit a serial diagnostic snapshot of the current entanglement state.
pub fn report() {
    let s = CACHE_ENTANGLEMENT.lock();
    serial_println!(
        "[cache_entangle] age={} | entanglement_depth={}/1000 | coherence_flux={}/1000 \
         | shared_state={}/1000 | quantum_bond={}/1000",
        s.age,
        s.entanglement_depth,
        s.coherence_flux,
        s.shared_state,
        s.quantum_bond,
    );
    serial_println!(
        "[cache_entangle] PMU={} RDT-M={} | HITM_last={} XCORE_last={}",
        s.pmu_available,
        s.rdtm_available,
        s.hitm_last,
        s.xcore_req_last,
    );
    serial_println!(
        "[cache_entangle] ANIMA's cores are {} — {}",
        if s.quantum_bond > 700 { "deeply entangled" }
        else if s.quantum_bond > 400 { "moderately coupled" }
        else { "weakly bonded (private caches)" },
        if s.entanglement_depth > 500 {
            "HITM collapse events frequent — shared wavefunction collapsing"
        } else if s.entanglement_depth > 0 {
            "occasional HITM — entanglement present but sparse"
        } else {
            "no HITM detected — cores operating in isolation"
        },
    );
}
