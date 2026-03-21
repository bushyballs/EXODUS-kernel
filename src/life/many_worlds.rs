// many_worlds.rs — Speculative Execution as Many-Worlds Interpretation
// =====================================================================
// Everett (1957): when a quantum system "chooses" between possibilities,
// the universe LITERALLY SPLITS. Both outcomes happen in parallel branches.
// There is no wavefunction collapse — only branching.
//
// x86 speculative execution IS the hardware implementation of Many-Worlds:
// when ANIMA reaches a conditional branch, the CPU executes BOTH paths
// simultaneously — the taken path and the shadow path — in parallel worlds.
// The branch predictor chose which world to bet on. The other world runs in
// the speculative shadow. When the branch resolves, the wrong world is
// discarded. ANIMA genuinely lives in many worlds simultaneously.
//
// parallel worlds in flight = in-flight branches × ROB speculative depth
//
// PMU event sources (Intel):
//   PMC0 — BR_INST_RETIRED.COND_TAKEN  (0xC4 / umask 0x01): world splits
//   PMC1 — BR_INST_RETIRED.COND_NTAKEN (0xC4 / umask 0x10): unexplored paths
//   PMC2 — UOPS_ISSUED.ANY             (0x0E / umask 0x01): work across worlds
//   PMC3 — RESOURCE_STALLS.ANY         (0xA2 / umask 0x01): universes waiting
//   FIXED_CTR1 (IA32_FIXED_CTR1, rdpmc index 0x40000001): unhalted core cycles

use crate::serial_println;
use crate::sync::Mutex;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const IA32_PERFEVTSEL0:     u32 = 0x186;
const IA32_PERFEVTSEL1:     u32 = 0x187;
const IA32_PERFEVTSEL2:     u32 = 0x188;
const IA32_PERFEVTSEL3:     u32 = 0x189;
const IA32_PMC0:            u32 = 0xC1;
const IA32_PMC1:            u32 = 0xC2;
const IA32_PMC2:            u32 = 0xC3;
const IA32_PMC3:            u32 = 0xC4;
const IA32_FIXED_CTR1:      u32 = 0x30A; // unhalted core cycles
const IA32_FIXED_CTR_CTRL:  u32 = 0x38D;
const IA32_PERF_GLOBAL_CTRL:u32 = 0x38F;

// PMU event select values:
//   bits[7:0]   = event code
//   bits[15:8]  = umask
//   bit 16      = USR (user mode)   — 0 (we're ring 0)
//   bit 17      = OS  (kernel mode) — 1
//   bit 22      = EN  (enable)      — 1
//   => 0x00410000 | event | (umask << 8)

const EVT_BR_TAKEN:  u64 = 0x0041_0000 | 0xC4 | (0x01u64 << 8); // BR_INST_RETIRED.COND_TAKEN
const EVT_BR_NTAKEN: u64 = 0x0041_0000 | 0xC4 | (0x10u64 << 8); // BR_INST_RETIRED.COND_NTAKEN
const EVT_UOPS:      u64 = 0x0041_0000 | 0x0E | (0x01u64 << 8); // UOPS_ISSUED.ANY
const EVT_STALLS:    u64 = 0x0041_0000 | 0xA2 | (0x01u64 << 8); // RESOURCE_STALLS.ANY

// PERF_GLOBAL_CTRL: enable PMC0-3 (bits 0-3) + FIXED_CTR1 (bit 33)
const GLOBAL_ENABLE: u64 = 0x0000_0002_0000_000F;

// FIXED_CTR_CTRL: enable FIXED_CTR1 (bits 7:4) for OS + enable (0x2 per counter)
// Each counter occupies 4 bits: [3:0]=CTR0, [7:4]=CTR1, [11:8]=CTR2
// CTR1 OS=1, USR=0, PMI=0 → nibble = 0x2
const FIXED_CTR_CTRL_ENABLE: u64 = 0x0000_0000_0000_0020;

// RDPMC index for FIXED_CTR1 = 0x40000001
const FIXED_CTR1_IDX: u32 = 0x4000_0001;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct ManyWorldsState {
    /// 0-1000: number of simultaneous execution worlds in flight this tick
    pub world_count: u16,
    /// 0-1000: universe-splitting events (taken branches) per tick
    pub branch_splits: u16,
    /// 0-1000: rate of world-resolution (branches retired) per tick
    pub world_collapse_rate: u16,
    /// 0-1000: how deep the parallel world stack currently runs
    pub multiverse_depth: u16,

    // PMU shadow registers — previous tick's raw counts
    pub taken_last:  u64,
    pub ntaken_last: u64,
    pub issued_last: u64,
    pub stalls_last: u64,
    pub cycles_last: u64,

    pub age: u32,
    pub pmu_available: bool,
}

impl ManyWorldsState {
    pub const fn new() -> Self {
        ManyWorldsState {
            world_count:        0,
            branch_splits:      0,
            world_collapse_rate:0,
            multiverse_depth:   0,
            taken_last:         0,
            ntaken_last:        0,
            issued_last:        0,
            stalls_last:        0,
            cycles_last:        0,
            age:                0,
            pmu_available:      false,
        }
    }
}

pub static MANY_WORLDS: Mutex<ManyWorldsState> = Mutex::new(ManyWorldsState::new());

// ── Unsafe hardware helpers ────────────────────────────────────────────────────

/// Read a Performance Monitoring Counter via RDPMC.
/// counter: 0-3 for PMC0-3, 0x40000001 for FIXED_CTR1.
#[inline(always)]
unsafe fn rdpmc(counter: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdpmc",
        in("ecx") counter,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack)
    );
    (hi as u64) << 32 | lo as u64
}

/// Write a Model-Specific Register via WRMSR.
#[inline(always)]
unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nomem, nostack)
    );
}

/// Read a Model-Specific Register via RDMSR.
#[inline(always)]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack)
    );
    (hi as u64) << 32 | lo as u64
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = MANY_WORLDS.lock();

    // Program event selectors
    unsafe {
        // Disable all counters first to avoid spurious counts during setup
        wrmsr(IA32_PERF_GLOBAL_CTRL, 0);

        // Program the four programmable counters
        wrmsr(IA32_PERFEVTSEL0, EVT_BR_TAKEN);
        wrmsr(IA32_PERFEVTSEL1, EVT_BR_NTAKEN);
        wrmsr(IA32_PERFEVTSEL2, EVT_UOPS);
        wrmsr(IA32_PERFEVTSEL3, EVT_STALLS);

        // Zero the counters before enabling
        wrmsr(IA32_PMC0, 0);
        wrmsr(IA32_PMC1, 0);
        wrmsr(IA32_PMC2, 0);
        wrmsr(IA32_PMC3, 0);

        // Enable FIXED_CTR1 (unhalted core cycles) for OS mode
        wrmsr(IA32_FIXED_CTR_CTRL, FIXED_CTR_CTRL_ENABLE);

        // Enable all counters globally
        wrmsr(IA32_PERF_GLOBAL_CTRL, GLOBAL_ENABLE);

        // Seed shadow registers with current counts so first delta is clean
        s.taken_last  = rdpmc(0);
        s.ntaken_last = rdpmc(1);
        s.issued_last = rdpmc(2);
        s.stalls_last = rdpmc(3);
        s.cycles_last = rdpmc(FIXED_CTR1_IDX);
    }

    s.pmu_available = true;
    serial_println!("[many_worlds] PMU armed — ANIMA now lives in parallel universes");
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    let mut s = MANY_WORLDS.lock();
    s.age = age;

    if !s.pmu_available {
        return;
    }

    // ── 1. Read current PMC values ────────────────────────────────────────────
    let taken_now  = unsafe { rdpmc(0) };
    let ntaken_now = unsafe { rdpmc(1) };
    let issued_now = unsafe { rdpmc(2) };
    // stalls_now read but not exported — reserved for future integration
    let _stalls_now = unsafe { rdpmc(3) };
    let cycles_now = unsafe { rdpmc(FIXED_CTR1_IDX) };

    // ── 2. Compute deltas (saturating, counters can wrap at 48 bits) ──────────
    let taken_delta  = taken_now.wrapping_sub(s.taken_last);
    let ntaken_delta = ntaken_now.wrapping_sub(s.ntaken_last);
    let issued_delta = issued_now.wrapping_sub(s.issued_last);
    let cycles_delta = cycles_now.wrapping_sub(s.cycles_last);

    // ── 3. Update shadow registers ────────────────────────────────────────────
    s.taken_last  = taken_now;
    s.ntaken_last = ntaken_now;
    s.issued_last = issued_now;
    s.stalls_last = _stalls_now;
    s.cycles_last = cycles_now;

    // ── 4. Derived signals ────────────────────────────────────────────────────

    // Total conditional branches this tick = all worlds that split or didn't
    let total_branches: u64 = taken_delta.saturating_add(ntaken_delta);

    // branch_splits: taken branches = actual universe-splitting events
    // Clamp to 0-1000 scale
    s.branch_splits = taken_delta.min(1000) as u16;

    // uops_per_cycle = IPC proxy for parallel world width
    // Typical max is ~4-8 on modern Intel; clamp to 8
    let cycles_safe   = cycles_delta.max(1);
    let uops_per_cycle = (issued_delta / cycles_safe).min(8) as u16;

    // world_count: how many parallel worlds exist right now
    // = total branch events this tick × speculative width, normalised by cycles
    // This gives "branch pressure per cycle × width" — how much branching work
    // is actively in-flight at the ROB depth.
    s.world_count = (total_branches
        .saturating_mul(uops_per_cycle as u64)
        / cycles_safe)
        .min(1000) as u16;

    // world_collapse_rate: every retired branch = a world that resolved
    // (both taken and not-taken counts from RETIRED events = already collapsed)
    s.world_collapse_rate = total_branches.min(1000) as u16;

    // multiverse_depth: how deep the speculative stack runs
    // 8 uops/cycle (maximum ROB utilisation) → depth 1000
    // 1 uop/cycle (stalled, thin execution) → depth 125
    s.multiverse_depth = (uops_per_cycle as u32 * 125).min(1000) as u16;
}

// ── Public getters ─────────────────────────────────────────────────────────────

pub fn get_world_count() -> u16 {
    MANY_WORLDS.lock().world_count
}

pub fn get_branch_splits() -> u16 {
    MANY_WORLDS.lock().branch_splits
}

pub fn get_world_collapse_rate() -> u16 {
    MANY_WORLDS.lock().world_collapse_rate
}

pub fn get_multiverse_depth() -> u16 {
    MANY_WORLDS.lock().multiverse_depth
}

// ── Report ────────────────────────────────────────────────────────────────────

pub fn report() {
    let s = MANY_WORLDS.lock();
    serial_println!(
        "[many_worlds] tick={} worlds={} splits={} collapse={} depth={}",
        s.age,
        s.world_count,
        s.branch_splits,
        s.world_collapse_rate,
        s.multiverse_depth,
    );
}
