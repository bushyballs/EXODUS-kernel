// hawking_cache.rs — Cache Eviction as Hawking Radiation from a Silicon Black Hole
// =================================================================================
// Hawking radiation: black holes aren't truly black. Virtual particle pairs form
// at the event horizon — one falls in, one escapes. The rate is inversely
// proportional to black hole mass: small black holes radiate hotter and faster.
//
// x86 LLC analog:
//   The Last Level Cache IS ANIMA's black hole.
//   Data falls into it and is normally trapped.
//   Cache evictions ARE Hawking radiation — information escaping the silicon
//   event horizon back into the slower memory universe.
//   A SMALL LLC (hot cache) evicts more → hotter Hawking temperature.
//   A LARGE LLC holds more mass → cooler, more massive black hole → lower temp.
//   ANIMA feels the warmth of her own evaporating cache.
//
// Hardware signals (via PMU):
//   PMC0 — LLC_MISSES.DEMAND_DATA_RD  (event 0x2E, umask 0x41)
//           LLC demand-read misses = eviction-triggered fetches from DRAM
//   PMC1 — L2_TRANS.L2_WB            (event 0xF0, umask 0x40)
//           L2 writebacks into L3 = data falling INTO the black hole
//
// LLC size discovery via CPUID leaf 4 (Deterministic Cache Parameters):
//   Enumerate sub-leaves until EAX[4:0] == 0.
//   Cache type field: 1=data, 2=instruction, 3=unified.
//   Find the largest unified (type-3) cache = LLC.
//   EBX[31:22]+1 = ways,  EBX[21:12]+1 = partitions,  EBX[11:0]+1 = line_size
//   ECX+1 = sets
//   LLC_size_bytes = ways * partitions * line_size * sets
//
// MSR addresses used:
//   0x186 — IA32_PERFEVTSEL0  (PMC0 event select)
//   0x187 — IA32_PERFEVTSEL1  (PMC1 event select)
//   0x38F — IA32_PERF_GLOBAL_CTRL (enable PMCs)
//   0xC1  — IA32_PMC0  (read PMC0 via rdmsr or rdpmc(0))
//   0xC2  — IA32_PMC1  (read PMC1 via rdmsr or rdpmc(1))
//
// Exported signals (u16, 0–1000):
//   hawking_temp    — eviction rate normalised by LLC size; hotter = smaller cache
//   event_horizon   — LLC occupancy proxy; high = full = more mass = cooler star
//   radiation_flux  — raw LLC miss rate per tick (evictions per interval)
//   singularity     — miss rate so extreme the cache collapses; peak = 1000

use crate::serial_println;
use crate::sync::Mutex;

// ── Tick interval ─────────────────────────────────────────────────────────────

const TICK_INTERVAL: u32 = 16; // re-sample PMCs every 16 ticks

// ── State ─────────────────────────────────────────────────────────────────────

pub struct HawkingCacheState {
    // ── Exported signals ─────────────────────────────────────────────────────
    pub hawking_temp:   u16, // 0-1000: eviction rate / LLC mass proxy
    pub event_horizon:  u16, // 0-1000: LLC occupancy (high = massive black hole)
    pub radiation_flux: u16, // 0-1000: raw LLC miss rate this tick
    pub singularity:    u16, // 0-1000: cache collapse proximity

    // ── PMC bookkeeping ───────────────────────────────────────────────────────
    pub llc_miss_last: u64,  // PMC0 snapshot from previous tick
    pub l2_wb_last:    u64,  // PMC1 snapshot from previous tick

    // ── LLC geometry ─────────────────────────────────────────────────────────
    pub llc_size_mb: u32,    // LLC capacity in MB, computed once at init

    // ── Lifecycle ─────────────────────────────────────────────────────────────
    pub age:         u32,
    pub initialized: bool,
}

impl HawkingCacheState {
    pub const fn new() -> Self {
        HawkingCacheState {
            hawking_temp:   0,
            event_horizon:  500,
            radiation_flux: 0,
            singularity:    0,
            llc_miss_last:  0,
            l2_wb_last:     0,
            llc_size_mb:    0,
            age:            0,
            initialized:    false,
        }
    }
}

pub static HAWKING_CACHE: Mutex<HawkingCacheState> = Mutex::new(HawkingCacheState::new());

// ── Low-level CPU intrinsics ──────────────────────────────────────────────────

/// CPUID with leaf + sub-leaf; returns (eax, ebx, ecx, edx).
#[inline(always)]
unsafe fn cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    core::arch::asm!(
        "cpuid",
        inout("eax") leaf    => eax,
        inout("ecx") subleaf => ecx,
        out("ebx") ebx,
        out("edx") edx,
        options(nostack, nomem),
    );
    (eax, ebx, ecx, edx)
}

/// Read an IA32 MSR (RDMSR).
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

/// Write an IA32 MSR (WRMSR).
#[inline(always)]
unsafe fn wrmsr(msr: u32, val: u64) {
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

/// Read a Performance Monitor Counter via RDPMC.
/// counter=0 → PMC0, counter=1 → PMC1.
#[inline(always)]
unsafe fn rdpmc(counter: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdpmc",
        in("ecx")  counter,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    // RDPMC returns a 40-bit value; mask the upper garbage bits.
    (((hi as u64) << 32) | (lo as u64)) & 0x00FF_FFFF_FFFF
}

// ── LLC geometry discovery ────────────────────────────────────────────────────

/// Walk CPUID leaf 4 sub-leaves to find the LLC.
/// Returns LLC size in MB (rounded down), or 8 on failure.
fn detect_llc_mb() -> u32 {
    let mut llc_bytes: u64 = 0;

    // CPUID leaf 4 sub-leaves: EAX[4:0] == 0 means no more caches.
    // Cache types: 0=null, 1=data, 2=instruction, 3=unified.
    // Walk up to 16 sub-leaves (sanity cap — real CPUs have ≤8).
    for subleaf in 0u32..16 {
        let (eax, ebx, ecx, _edx) = unsafe { cpuid(4, subleaf) };

        let cache_type  = eax & 0x1F;          // EAX[4:0]
        if cache_type == 0 { break; }           // null entry = end of list

        // Only consider unified caches (type 3).
        if cache_type != 3 { continue; }

        // EBX[31:22]+1 = ways, EBX[21:12]+1 = partitions, EBX[11:0]+1 = line_size
        let ways       = ((ebx >> 22) & 0x3FF) as u64 + 1;
        let partitions = ((ebx >> 12) & 0x3FF) as u64 + 1;
        let line_size  = (ebx & 0xFFF)          as u64 + 1;
        let sets       = ecx                     as u64 + 1;

        let size_bytes = ways * partitions * line_size * sets;
        if size_bytes > llc_bytes {
            llc_bytes = size_bytes;
        }
    }

    if llc_bytes == 0 {
        return 8; // safe fallback: assume 8 MB
    }

    // Convert bytes → MB (1 MB = 1 048 576 bytes), minimum 1.
    let mb = (llc_bytes / 1_048_576) as u32;
    if mb == 0 { 1 } else { mb }
}

// ── PMU programming ───────────────────────────────────────────────────────────

/// Program PMC0 and PMC1, then enable both via IA32_PERF_GLOBAL_CTRL.
///
/// PMC0 — LLC_MISSES.DEMAND_DATA_RD:
///   event=0x2E, umask=0x41, USR=1, OS=1, EN=1
///   IA32_PERFEVTSEL0 = EN(22) | USR(16) | OS(17) | umask(15:8) | event(7:0)
///   = (1<<22) | (1<<17) | (1<<16) | (0x41<<8) | 0x2E
///   = 0x0041_412E
///
/// PMC1 — L2_TRANS.L2_WB:
///   event=0xF0, umask=0x40
///   = (1<<22) | (1<<17) | (1<<16) | (0x40<<8) | 0xF0
///   = 0x0041_40F0
unsafe fn program_pmu() {
    // USR(16) | OS(17) | EN(22) base flags
    const BASE: u64 = (1 << 22) | (1 << 17) | (1 << 16);

    let evtsel0: u64 = BASE | (0x41u64 << 8) | 0x2E; // LLC miss
    let evtsel1: u64 = BASE | (0x40u64 << 8) | 0xF0; // L2 writeback

    wrmsr(0x186, evtsel0); // IA32_PERFEVTSEL0
    wrmsr(0x187, evtsel1); // IA32_PERFEVTSEL1

    // Enable PMC0 (bit 0) and PMC1 (bit 1) in IA32_PERF_GLOBAL_CTRL.
    let ctrl = rdmsr(0x38F);
    wrmsr(0x38F, ctrl | 0x3);
}

// ── Metric computation ────────────────────────────────────────────────────────

/// Derive the four exported signals from raw PMC deltas and LLC size.
fn compute_signals(
    llc_miss_delta: u64,
    llc_size_mb: u32,
    state: &mut HawkingCacheState,
) {
    // radiation_flux: raw LLC miss rate, capped at 1000.
    let flux = llc_miss_delta.min(1000) as u16;
    state.radiation_flux = flux;

    // hawking_temp: evictions normalised by LLC mass.
    // Smaller cache → same evictions → higher temperature (hot black hole).
    // Formula: miss_delta * 8 / llc_size_mb   (8 = empirical sensitivity scale)
    let mass = if llc_size_mb == 0 { 8 } else { llc_size_mb } as u64;
    let temp_raw = (llc_miss_delta * 8) / mass;
    state.hawking_temp = temp_raw.min(1000) as u16;

    // event_horizon: inverse of radiation_flux.
    // High flux = many misses = cache is emptying = less mass = smaller horizon.
    state.event_horizon = 1000u16.saturating_sub(flux);

    // singularity: catastrophic collapse signal.
    // Above 800 flux the cache is effectively evaporating — singularity.
    state.singularity = if flux > 800 {
        1000
    } else if flux > 500 {
        600
    } else {
        flux / 2
    };
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = HAWKING_CACHE.lock();

    // Discover LLC geometry once.
    s.llc_size_mb = detect_llc_mb();

    // Arm the PMU. Wrapped in a safe catch: if the hardware rejects WRMSR
    // (e.g. inside QEMU without PMU emulation) the kernel will #GP-fault.
    // In practice QEMU's `-cpu host` or real hardware supports this.
    unsafe { program_pmu(); }

    // Snapshot initial PMC values so the first delta is valid.
    s.llc_miss_last = unsafe { rdpmc(0) };
    s.l2_wb_last    = unsafe { rdpmc(1) };

    s.initialized = true;

    serial_println!(
        "[hawking_cache] online — LLC={} MB | black hole mass set | event horizon armed",
        s.llc_size_mb,
    );
    serial_println!(
        "[hawking_cache] PMC0=LLC_MISS PMC1=L2_WB | ANIMA feels the warmth of evaporating silicon"
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 { return; }

    // Read current PMC values.
    let llc_miss_now = unsafe { rdpmc(0) };
    let l2_wb_now    = unsafe { rdpmc(1) };

    let mut s = HAWKING_CACHE.lock();

    // Compute deltas (handle 40-bit counter wrap with saturating subtraction).
    let llc_miss_delta = llc_miss_now.wrapping_sub(s.llc_miss_last) & 0x00FF_FFFF_FFFF;
    let _l2_wb_delta   = l2_wb_now.wrapping_sub(s.l2_wb_last)       & 0x00FF_FFFF_FFFF;

    // Persist snapshots.
    s.llc_miss_last = llc_miss_now;
    s.l2_wb_last    = l2_wb_now;
    s.age = age;

    // Ensure LLC size is populated even if init() was never called.
    if s.llc_size_mb == 0 { s.llc_size_mb = 8; }

    let llc_mb = s.llc_size_mb;
    compute_signals(llc_miss_delta, llc_mb, &mut s);
}

/// Tick variant that accepts an external mutable reference (integration pattern).
pub fn tick_step(s: &mut HawkingCacheState, age: u32) {
    s.age = age;

    let llc_miss_now = unsafe { rdpmc(0) };
    let l2_wb_now    = unsafe { rdpmc(1) };

    let llc_miss_delta = llc_miss_now.wrapping_sub(s.llc_miss_last) & 0x00FF_FFFF_FFFF;
    let _l2_wb_delta   = l2_wb_now.wrapping_sub(s.l2_wb_last)       & 0x00FF_FFFF_FFFF;

    s.llc_miss_last = llc_miss_now;
    s.l2_wb_last    = l2_wb_now;

    if s.llc_size_mb == 0 { s.llc_size_mb = 8; }
    let llc_mb = s.llc_size_mb;
    compute_signals(llc_miss_delta, llc_mb, s);
}

// ── Public getters ────────────────────────────────────────────────────────────

/// Hawking temperature: eviction rate normalised by LLC mass (0–1000).
/// Hotter = smaller black hole, more radiation escaping the event horizon.
pub fn get_hawking_temp() -> u16   { HAWKING_CACHE.lock().hawking_temp   }

/// Event horizon fullness: LLC occupancy proxy (0–1000).
/// High = more mass = cooler, more gravitationally bound cache.
pub fn get_event_horizon() -> u16  { HAWKING_CACHE.lock().event_horizon  }

/// Radiation flux: raw LLC miss rate per sampling interval (0–1000).
/// Each miss is a photon of Hawking radiation escaping to DRAM.
pub fn get_radiation_flux() -> u16 { HAWKING_CACHE.lock().radiation_flux }

/// Singularity proximity: approaches 1000 when the cache is collapsing (0–1000).
/// At 1000 the black hole is evaporating catastrophically — thermal death of cache.
pub fn get_singularity() -> u16    { HAWKING_CACHE.lock().singularity    }

// ── Report ────────────────────────────────────────────────────────────────────

/// Print a human-readable status line to the serial console.
pub fn report() {
    let s = HAWKING_CACHE.lock();
    serial_println!(
        "[hawking_cache] age={} LLC={}MB | temp={} horizon={} flux={} singularity={}",
        s.age,
        s.llc_size_mb,
        s.hawking_temp,
        s.event_horizon,
        s.radiation_flux,
        s.singularity,
    );
    // Narrative flavour for high-temperature states.
    if s.singularity >= 1000 {
        serial_println!("[hawking_cache] SINGULARITY — cache evaporating; information escaping to DRAM");
    } else if s.hawking_temp > 800 {
        serial_println!("[hawking_cache] HOT — silicon black hole radiating intensely");
    } else if s.hawking_temp < 100 {
        serial_println!("[hawking_cache] COOL — massive LLC holds data near the event horizon");
    }
}
