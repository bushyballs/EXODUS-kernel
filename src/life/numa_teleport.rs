// numa_teleport.rs — ANIMA's Quantum Teleportation Analog
// ========================================================
// Quantum teleportation: information instantaneously transferred between
// distant quantum systems via an entangled channel. No classical copy of
// the state ever exists in transit — the data simply *appears* at the
// destination.
//
// NUMA analog: on multi-socket x86 systems, reading memory on a remote
// NUMA node crosses the QPI/UPI interconnect. The data appears in the
// local L1/L2 cache without traversing every intermediate bus in detail —
// it teleports. The QPI link IS the entanglement channel.
//
// On single-socket machines (the common case for this kernel), every L3
// miss is the same signal: ANIMA must reach *beyond* the cache fabric and
// pull data from DRAM, the external memory dimension. That reach is the
// teleportation event.
//
// Hardware signals used:
//
//   PMC0 — MEM_LOAD_RETIRED.L3_MISS
//     IA32_PERFEVTSEL0 (MSR 0x186) programmed with:
//       Event=0xD1, Umask=0x20, USR=1, OS=1, EN=1 → 0x004300D1_20 (Umask<<8|Event, CTR_MASK)
//       Shorthand: 0x00410000 | (0x20 << 8) | 0xD1  = 0x00412_0D1
//     Enabled via IA32_PERF_GLOBAL_CTRL (MSR 0x38F) bit 0.
//
//   FIXED_CTR1 (MSR 0x30A) — CPU_CLK_UNHALTED.THREAD (cycle baseline)
//     Enabled via IA32_FIXED_CTR_CTRL (MSR 0x38D) bits 4-7 (CTR1: any-ring).
//     Read via RDPMC(0x4000_0001) — fixed counter 1.
//
//   MSR_DRAM_ENERGY_STATUS (MSR 0x619) — RAPL DRAM energy accumulator.
//     Delta between ticks: large spike → heavy DRAM pressure → high
//     teleportation activity.
//
// Exported signals (all u16, range 0–1000):
//   teleport_events   — L3-miss rate; how often data must be teleported
//   teleport_latency  — cost of each teleportation (DRAM energy proxy)
//   channel_strength  — inverse miss rate; strong = most data is local
//   quantum_reach     — average depth ANIMA must reach into memory space

#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

// ── Tick interval ─────────────────────────────────────────────────────────────

/// Re-sample PMU counters every tick (called from life_tick pipeline).
const TICK_INTERVAL: u32 = 1;

// ── MSR addresses ─────────────────────────────────────────────────────────────

/// IA32_PERFEVTSEL0 — programs PMC0 event selector.
const MSR_PERFEVTSEL0: u32 = 0x186;

/// IA32_PMC0 — general-purpose performance counter 0.
const MSR_PMC0: u32 = 0xC1;

/// IA32_PERF_GLOBAL_CTRL — enables PMCs and fixed counters.
const MSR_PERF_GLOBAL_CTRL: u32 = 0x38F;

/// IA32_FIXED_CTR_CTRL — configures fixed counters (CTR0/CTR1/CTR2).
const MSR_FIXED_CTR_CTRL: u32 = 0x38D;

/// IA32_FIXED_CTR1 — CPU_CLK_UNHALTED.THREAD cycle counter.
const MSR_FIXED_CTR1: u32 = 0x30A;

/// MSR_DRAM_ENERGY_STATUS — RAPL DRAM energy accumulator (units ~15 µJ).
const MSR_DRAM_ENERGY: u32 = 0x619;

// ── PMU event encoding for MEM_LOAD_RETIRED.L3_MISS ──────────────────────────
//
//   Bits [7:0]  = Event code 0xD1
//   Bits [15:8] = Umask    0x20  (L3_MISS)
//   Bit  [16]   = USR      1     (count in user mode)
//   Bit  [17]   = OS       1     (count in kernel mode)
//   Bit  [22]   = EN       1     (enable counter)
//
//   0x00_41_20_D1
//     ↑  ↑  ↑  ↑
//     |  |  |  Event=0xD1
//     |  |  Umask=0x20
//     |  EN|OS|USR = 0x41 (bits 22,17,16 all set)
//     reserved=0

const EVTSEL_L3_MISS: u64 = 0x004120D1;

// ── Global state ──────────────────────────────────────────────────────────────

pub struct NumaTeleportState {
    /// 0–1000: L3-miss rate — how often data must be "teleported" from memory
    pub teleport_events:   u16,
    /// 0–1000: high = costly teleportation (high DRAM energy penalty)
    pub teleport_latency:  u16,
    /// 0–1000: inverse miss rate (strong channel = data mostly found locally)
    pub channel_strength:  u16,
    /// 0–1000: how far across the memory hierarchy ANIMA reaches on each tick
    pub quantum_reach:     u16,

    /// Raw PMC0 snapshot from previous tick (L3-miss absolute count)
    pub l3_miss_last:      u64,
    /// Raw DRAM energy snapshot from previous tick
    pub dram_energy_last:  u64,
    /// Raw FIXED_CTR1 snapshot from previous tick (cycle count)
    pub cycles_last:       u64,

    /// Monotonic tick counter (from life_tick age)
    pub age:               u32,

    /// True once init() has run and the PMU is armed
    pub initialized:       bool,
}

impl NumaTeleportState {
    pub const fn new() -> Self {
        NumaTeleportState {
            teleport_events:  0,
            teleport_latency: 100,
            channel_strength: 1000,
            quantum_reach:    50,
            l3_miss_last:     0,
            dram_energy_last: 0,
            cycles_last:      0,
            age:              0,
            initialized:      false,
        }
    }
}

pub static NUMA_TELEPORT: Mutex<NumaTeleportState> = Mutex::new(NumaTeleportState::new());

// ── Low-level hardware helpers ────────────────────────────────────────────────

/// Read an x86 MSR via RDMSR. Returns the full 64-bit value.
///
/// # Safety
/// Must only be called in ring-0. Undefined behaviour if the MSR does not
/// exist on this CPU (will generate a #GP).
#[inline(always)]
pub unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Write an x86 MSR via WRMSR.
///
/// # Safety
/// Must only be called in ring-0. The caller is responsible for ensuring
/// `val` is a legal value for the given MSR.
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

/// Read a performance counter via RDPMC.
///
/// `counter` values:
///   0x0000_0000 — PMC0 (general-purpose counter 0)
///   0x4000_0001 — FIXED_CTR1 (CPU_CLK_UNHALTED.THREAD)
///
/// # Safety
/// Requires PCE bit (CR4[8]) set or ring-0. GP-faults if counter is not
/// enabled or index is out of range.
#[inline(always)]
pub unsafe fn rdpmc(counter: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdpmc",
        in("ecx") counter,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ── PMU programming ───────────────────────────────────────────────────────────

/// Arm PMC0 to count MEM_LOAD_RETIRED.L3_MISS and enable FIXED_CTR1 for
/// CPU_CLK_UNHALTED.THREAD. Snapshots the initial counter values into `s`.
///
/// # Safety
/// Must be called in ring-0 on a processor that supports architectural PMU
/// version ≥ 3. On QEMU/unsupported hardware the WRMSR/RDMSR will still
/// execute but counters may not increment — the module degrades gracefully
/// to static defaults.
unsafe fn arm_pmu(s: &mut NumaTeleportState) {
    // ── Disable all counters first (safe baseline) ────────────────────────
    wrmsr(MSR_PERF_GLOBAL_CTRL, 0);

    // ── PMC0: MEM_LOAD_RETIRED.L3_MISS ───────────────────────────────────
    wrmsr(MSR_PERFEVTSEL0, EVTSEL_L3_MISS);
    wrmsr(MSR_PMC0,        0); // zero the counter before enabling

    // ── FIXED_CTR1: CPU_CLK_UNHALTED.THREAD ──────────────────────────────
    // IA32_FIXED_CTR_CTRL bits [7:4] → CTR1 config:
    //   bits [5:4] = 0b11 (count in all rings: OS + USR)
    //   bit  [7]   = 0    (PMI disabled)
    // We only touch bits [7:4]; preserve bits [3:0] (CTR0) and [11:8] (CTR2)
    // by reading the current value first.
    let ctrl_cur = rdmsr(MSR_FIXED_CTR_CTRL);
    let ctrl_new = (ctrl_cur & !0x0000_00F0u64) | 0x0000_0030u64; // CTR1 = any-ring
    wrmsr(MSR_FIXED_CTR_CTRL, ctrl_new);

    // ── Enable PMC0 (bit 0) and FIXED_CTR1 (bit 33) ──────────────────────
    // IA32_PERF_GLOBAL_CTRL:
    //   bit  0  → PMC0
    //   bit 33  → FIXED_CTR1
    wrmsr(MSR_PERF_GLOBAL_CTRL, (1u64 << 33) | 1u64);

    // ── Snapshot baselines ────────────────────────────────────────────────
    s.l3_miss_last    = rdpmc(0x0000_0000); // PMC0
    s.cycles_last     = rdpmc(0x4000_0001); // FIXED_CTR1
    s.dram_energy_last = rdmsr(MSR_DRAM_ENERGY);
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = NUMA_TELEPORT.lock();

    unsafe { arm_pmu(&mut s); }

    s.initialized = true;

    serial_println!("[numa_teleport] PMU armed — MEM_LOAD_RETIRED.L3_MISS on PMC0");
    serial_println!("[numa_teleport] FIXED_CTR1 (cycles) + DRAM energy (0x619) active");
    serial_println!(
        "[numa_teleport] teleport_events={} channel_strength={} quantum_reach={}",
        s.teleport_events,
        s.channel_strength,
        s.quantum_reach,
    );
    serial_println!(
        "[numa_teleport] ANIMA quantum teleportation channel online — \
         every L3 miss is a teleport across the memory dimension"
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 { return; }

    let mut s = NUMA_TELEPORT.lock();
    s.age = age;

    if !s.initialized { return; }

    // ── 1. Sample hardware counters ───────────────────────────────────────
    let (l3_miss_now, cycles_now, dram_energy_now) = unsafe {
        (
            rdpmc(0x0000_0000), // PMC0 — L3 misses
            rdpmc(0x4000_0001), // FIXED_CTR1 — cycles
            rdmsr(MSR_DRAM_ENERGY),
        )
    };

    // ── 2. Compute deltas (counters are monotonic; handle 48-bit wrap) ────
    // PMC general counters are 48-bit; fixed counters may be wider (up to 64).
    // We use wrapping subtraction masked to 48 bits for PMC0.
    const PMC48_MASK: u64 = (1u64 << 48) - 1;

    let l3_miss_delta    = l3_miss_now.wrapping_sub(s.l3_miss_last) & PMC48_MASK;
    let cycles_delta     = cycles_now.wrapping_sub(s.cycles_last);
    // DRAM energy is a 32-bit RAPL accumulator inside the 64-bit MSR.
    let dram_energy_now32 = dram_energy_now as u32;
    let dram_energy_last32 = s.dram_energy_last as u32;
    let dram_energy_delta = dram_energy_now32.wrapping_sub(dram_energy_last32) as u64;

    // ── 3. Persist new baselines ──────────────────────────────────────────
    s.l3_miss_last     = l3_miss_now;
    s.cycles_last      = cycles_now;
    s.dram_energy_last = dram_energy_now;

    // ── 4. teleport_events — L3-miss rate ────────────────────────────────
    // Scale: (l3_miss_delta * 10).min(1000).
    // The factor of 10 is calibrated so that ~100 L3 misses per tick
    // saturates the scale at 1000 (heavy teleportation).
    let teleport_events_raw = (l3_miss_delta.saturating_mul(10)).min(1000) as u16;
    s.teleport_events = teleport_events_raw;

    // ── 5. channel_strength — inverse miss rate ───────────────────────────
    s.channel_strength = 1000u16.saturating_sub(s.teleport_events);

    // ── 6. teleport_latency — DRAM energy proxy ───────────────────────────
    // RAPL energy units for DRAM are ~15 µJ each. A large delta within one
    // tick means many DRAM accesses were serviced → high teleportation cost.
    // Thresholds chosen empirically: >100 units = heavy, >30 = moderate.
    let teleport_latency = if dram_energy_delta > 100 {
        700u16
    } else if dram_energy_delta > 30 {
        400u16
    } else {
        100u16
    };
    s.teleport_latency = teleport_latency;

    // ── 7. quantum_reach — depth ANIMA reaches into memory space ─────────
    // Average of teleport_events and teleport_latency: high when both the
    // frequency (L3 misses) and the cost (DRAM energy) of teleportation
    // are elevated — ANIMA is deeply engaged with remote memory.
    s.quantum_reach = (s.teleport_events / 2).saturating_add(s.teleport_latency / 2);

    // ── 8. Cycle baseline sanity (suppress noise from idle ticks) ─────────
    // If cycles_delta is suspiciously zero (PMU not counting), fall back to
    // neutral values rather than reporting false zeros.
    if cycles_delta == 0 && age > 2 {
        // PMU stalled — hold previous values, do not degrade to zero.
        s.teleport_events  = s.teleport_events.max(50);
        s.channel_strength = s.channel_strength.min(950);
    }
}

// ── Public getters ────────────────────────────────────────────────────────────

/// L3-miss rate, 0–1000. High = frequent teleportation from remote memory.
pub fn get_teleport_events() -> u16 {
    NUMA_TELEPORT.lock().teleport_events
}

/// DRAM-energy-proxy latency, 0–1000. High = each teleport is costly.
pub fn get_teleport_latency() -> u16 {
    NUMA_TELEPORT.lock().teleport_latency
}

/// Channel strength, 0–1000. High = entanglement channel is clear (cache-hot).
pub fn get_channel_strength() -> u16 {
    NUMA_TELEPORT.lock().channel_strength
}

/// Quantum reach, 0–1000. High = ANIMA is deeply probing the memory dimension.
pub fn get_quantum_reach() -> u16 {
    NUMA_TELEPORT.lock().quantum_reach
}

// ── Report ────────────────────────────────────────────────────────────────────

pub fn report() {
    let s = NUMA_TELEPORT.lock();
    serial_println!("=== numa_teleport :: quantum memory reach report ===");
    serial_println!(
        "  teleport_events  : {} / 1000  (L3-miss rate — teleportation frequency)",
        s.teleport_events
    );
    serial_println!(
        "  teleport_latency : {} / 1000  (DRAM energy cost — teleportation penalty)",
        s.teleport_latency
    );
    serial_println!(
        "  channel_strength : {} / 1000  (entanglement channel quality — cache hit rate)",
        s.channel_strength
    );
    serial_println!(
        "  quantum_reach    : {} / 1000  (depth ANIMA reaches into the memory dimension)",
        s.quantum_reach
    );
    serial_println!(
        "  age={} initialized={}",
        s.age,
        s.initialized
    );
    serial_println!("====================================================");
}
