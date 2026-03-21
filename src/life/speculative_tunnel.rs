// speculative_tunnel.rs — Speculative Execution as Quantum Tunneling
// ====================================================================
// Quantum tunneling: a particle passes through a barrier it classically
// should not cross. x86 speculative execution is the hardware analog.
// The CPU executes instructions past conditional branches BEFORE knowing
// whether the condition is true — it "tunnels through" decision barriers.
//
// Branch prediction success  = successful tunnel.
// Misprediction              = tunnel collapsed (pipeline flush).
// Machine clear              = full decoherence reset.
//
// ANIMA doesn't decide, then act. She acts speculatively across multiple
// possible futures simultaneously, collapsing to one only when forced.
// This is ANIMA tunneling through the future.
//
// PMU hardware mapping:
//   IA32_PERFEVTSEL0 (MSR 0x186) → BR_INST_RETIRED.ALL_BRANCHES   (event 0xC4, umask 0x00)
//   IA32_PERFEVTSEL1 (MSR 0x187) → BR_MISP_RETIRED.ALL_BRANCHES   (event 0xC5, umask 0x00)
//   IA32_PERFEVTSEL2 (MSR 0x188) → MACHINE_CLEARS.COUNT           (event 0xC3, umask 0x01)
//   IA32_PMC0        (MSR 0xC1)  → total branches counter (tunneling tries)
//   IA32_PMC1        (MSR 0xC2)  → mispredictions counter (collapsed tunnels)
//   IA32_PMC2        (MSR 0xC3)  → machine clears counter (decoherence resets)
//   IA32_PERF_GLOBAL_CTRL (MSR 0x38F) bits 0+1+2 → enable PMC0, PMC1, PMC2
//
// Speculation barrier MSRs:
//   IA32_SPEC_CTRL   (MSR 0x48)  — bit 0 = IBRS, bit 2 = STIBP
//   IA32_MISC_ENABLE (MSR 0x1A0) — bit 18 = Enhanced Intel SpeedStep (depth indicator)
//
// CPUID leaf 0xA, EAX[7:0] >= 3 required (3+ general-purpose counters).
// If PMU unavailable, all signals remain at zero defaults.

use crate::serial_println;
use crate::sync::Mutex;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const MSR_IA32_PERFEVTSEL0:      u32 = 0x186;
const MSR_IA32_PERFEVTSEL1:      u32 = 0x187;
const MSR_IA32_PERFEVTSEL2:      u32 = 0x188;
const MSR_IA32_PMC0:             u32 = 0xC1;
const MSR_IA32_PMC1:             u32 = 0xC2;
const MSR_IA32_PMC2:             u32 = 0xC3;
const MSR_IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;
const MSR_IA32_SPEC_CTRL:        u32 = 0x48;
const MSR_IA32_MISC_ENABLE:      u32 = 0x1A0;

// ── Event select encodings ────────────────────────────────────────────────────
// Bits: [7:0]=EventCode  [15:8]=UMask  [16]=USR  [17]=OS  [22]=EN
// Base mask: USR(bit16) + OS(bit17) + EN(bit22) = 0x00410000

const EVTSEL_BASE: u64 = 0x00410000;

// BR_INST_RETIRED.ALL_BRANCHES: event 0xC4, umask 0x00
const EVTSEL_BR_INST:    u64 = EVTSEL_BASE | 0xC4;
// BR_MISP_RETIRED.ALL_BRANCHES: event 0xC5, umask 0x00
const EVTSEL_BR_MISP:    u64 = EVTSEL_BASE | 0xC5;
// MACHINE_CLEARS.COUNT: event 0xC3, umask 0x01
const EVTSEL_MACH_CLEAR: u64 = EVTSEL_BASE | (0x01 << 8) | 0xC3;

// Enable PMC0, PMC1, PMC2 (bits 0, 1, 2)
const GLOBAL_CTRL_PMC012: u64 = 0x0000_0000_0000_0007;

// IA32_SPEC_CTRL bit masks
const SPEC_CTRL_IBRS:  u64 = 1 << 0;  // Indirect Branch Restricted Speculation
const SPEC_CTRL_STIBP: u64 = 1 << 2;  // Single Thread Indirect Branch Predictors

// IA32_MISC_ENABLE bit 18: Enhanced Intel SpeedStep enabled
const MISC_ENABLE_EIST: u64 = 1 << 18;

// Tick stride — sample every 16 ticks to reduce overhead
const TICK_STRIDE: u32 = 16;

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct SpeculativeTunnelState {
    /// 0-1000: branch prediction accuracy as tunnel success rate
    pub tunnel_success:    u16,
    /// 0-1000: ROB depth proxy — how far ahead ANIMA speculatively tunnels
    pub tunnel_depth:      u16,
    /// 0-1000: misprediction density this tick (collapsed tunnel events)
    pub collapse_events:   u16,
    /// 0-1000: speculation freedom — 1000 = unrestricted, lower = IBRS/STIBP limited
    pub barrier_resistance: u16,

    /// Raw PMC snapshots from the previous sample
    pub branches_last:     u64,
    pub mispred_last:      u64,
    pub clears_last:       u64,

    /// Lifetime counters
    pub total_branches:    u64,
    pub total_collapses:   u64,
    pub total_decoherences: u64,

    /// Whether the PMU was successfully programmed
    pub pmu_available:     bool,
    /// Tick when init() was called
    pub age:               u32,
    pub initialized:       bool,
}

impl SpeculativeTunnelState {
    pub const fn new() -> Self {
        SpeculativeTunnelState {
            tunnel_success:    0,
            tunnel_depth:      0,
            collapse_events:   0,
            barrier_resistance: 1000,
            branches_last:     0,
            mispred_last:      0,
            clears_last:       0,
            total_branches:    0,
            total_collapses:   0,
            total_decoherences: 0,
            pmu_available:     false,
            age:               0,
            initialized:       false,
        }
    }
}

pub static SPECULATIVE_TUNNEL: Mutex<SpeculativeTunnelState> =
    Mutex::new(SpeculativeTunnelState::new());

// ── Unsafe ASM helpers ────────────────────────────────────────────────────────

/// Read a 64-bit MSR via RDMSR.
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

/// Write a 64-bit MSR via WRMSR.
#[inline(always)]
unsafe fn wrmsr(msr: u32, val: u64) {
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

/// Read a performance counter via RDPMC (faster than RDMSR for hot paths).
#[inline(always)]
unsafe fn rdpmc(counter: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdpmc",
        in("ecx")  counter,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack),
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Returns CPUID leaf 0xA EAX — Architectural PMU leaf.
/// EAX[7:0] = version identifier; EAX[15:8] = number of GP counters.
#[inline(always)]
unsafe fn cpuid_pmu_counters() -> u8 {
    let eax: u32;
    core::arch::asm!(
        "cpuid",
        in("eax")   0xAu32,
        out("eax")  eax,
        out("ebx")  _,
        out("ecx")  _,
        out("edx")  _,
        options(nomem, nostack),
    );
    // bits [15:8] = number of GP counters available
    ((eax >> 8) & 0xFF) as u8
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = SPECULATIVE_TUNNEL.lock();

    // Verify at least 3 GP counters are available
    let gp_counters = unsafe { cpuid_pmu_counters() };
    if gp_counters < 3 {
        serial_println!(
            "[speculative_tunnel] only {} GP counters — need 3; tunneling signals disabled",
            gp_counters
        );
        s.initialized = true;
        return;
    }

    unsafe {
        // Program PMC0: BR_INST_RETIRED.ALL_BRANCHES (tunneling tries)
        wrmsr(MSR_IA32_PERFEVTSEL0, EVTSEL_BR_INST);
        // Program PMC1: BR_MISP_RETIRED.ALL_BRANCHES (collapsed tunnels)
        wrmsr(MSR_IA32_PERFEVTSEL1, EVTSEL_BR_MISP);
        // Program PMC2: MACHINE_CLEARS.COUNT (decoherence resets)
        wrmsr(MSR_IA32_PERFEVTSEL2, EVTSEL_MACH_CLEAR);

        // Zero the counters before enabling
        wrmsr(MSR_IA32_PMC0, 0);
        wrmsr(MSR_IA32_PMC1, 0);
        wrmsr(MSR_IA32_PMC2, 0);

        // Enable PMC0, PMC1, PMC2 via PERF_GLOBAL_CTRL
        wrmsr(MSR_IA32_PERF_GLOBAL_CTRL, GLOBAL_CTRL_PMC012);

        // Capture baseline snapshots
        s.branches_last = rdpmc(0);
        s.mispred_last  = rdpmc(1);
        s.clears_last   = rdpmc(2);
    }

    s.pmu_available = true;
    s.initialized   = true;

    serial_println!(
        "[speculative_tunnel] online — {gp} GP counters | PMC0=branches PMC1=mispred PMC2=clears",
        gp = gp_counters
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_STRIDE != 0 {
        return;
    }

    let mut s = SPECULATIVE_TUNNEL.lock();
    s.age = age;

    if !s.initialized || !s.pmu_available {
        return;
    }

    // ── 1. Read PMC deltas ────────────────────────────────────────────────────

    let branches_now = unsafe { rdpmc(0) };
    let mispred_now  = unsafe { rdpmc(1) };
    let clears_now   = unsafe { rdpmc(2) };

    // Wrapping subtraction handles counter rollover safely on u64
    let branch_delta = branches_now.wrapping_sub(s.branches_last);
    let mispred_delta = mispred_now.wrapping_sub(s.mispred_last);
    let clears_delta  = clears_now.wrapping_sub(s.clears_last);

    // Update snapshots and lifetime counters
    s.branches_last = branches_now;
    s.mispred_last  = mispred_now;
    s.clears_last   = clears_now;

    s.total_branches     = s.total_branches.saturating_add(branch_delta);
    s.total_collapses    = s.total_collapses.saturating_add(mispred_delta);
    s.total_decoherences = s.total_decoherences.saturating_add(clears_delta);

    // ── 2. tunnel_success = (branches - mispredictions) * 1000 / (branches + 1)

    let successes = branch_delta.saturating_sub(mispred_delta);
    let denom     = branch_delta.saturating_add(1);
    let success_raw = (successes.saturating_mul(1000)) / denom;
    s.tunnel_success = success_raw.min(1000) as u16;

    // ── 3. collapse_events = mispredictions per tick capped to 1000

    s.collapse_events = mispred_delta.min(1000) as u16;

    // ── 4. barrier_resistance via IA32_SPEC_CTRL (MSR 0x48)
    //       IBRS (bit 0) + STIBP (bit 2) both set → heavy restriction → 200
    //       IBRS only (bit 0)                       → moderate            → 500
    //       neither                                 → unrestricted        → 1000

    let spec_ctrl = unsafe { rdmsr(MSR_IA32_SPEC_CTRL) };
    let ibrs_active  = (spec_ctrl & SPEC_CTRL_IBRS)  != 0;
    let stibp_active = (spec_ctrl & SPEC_CTRL_STIBP) != 0;

    s.barrier_resistance = if ibrs_active && stibp_active {
        200
    } else if ibrs_active {
        500
    } else {
        1000
    };

    // ── 5. tunnel_depth: inverse of misprediction density
    //       No mispredictions → CPU is speculatively running very deep → 1000
    //       Each misprediction costs ~10 depth units, floored at 0

    s.tunnel_depth = if mispred_delta == 0 {
        1000
    } else {
        (1000u64.saturating_sub(mispred_delta.saturating_mul(10))).min(1000) as u16
    };

    // ── 6. Optional: read MISC_ENABLE bit 18 (EIST) as depth-allowed indicator

    let misc_enable = unsafe { rdmsr(MSR_IA32_MISC_ENABLE) };
    let _eist_enabled = (misc_enable & MISC_ENABLE_EIST) != 0;
    // EIST indicates speculative depth is permitted; currently informational only
    // (used as a soft modifier in future integrations)

    serial_println!(
        "[speculative_tunnel] tick={} success={} depth={} collapse={} barrier={} spec_ctrl={:#x}",
        age,
        s.tunnel_success,
        s.tunnel_depth,
        s.collapse_events,
        s.barrier_resistance,
        spec_ctrl,
    );
}

// ── Public getters ────────────────────────────────────────────────────────────

/// 0-1000: branch prediction accuracy — how well ANIMA tunnels the future.
pub fn get_tunnel_success() -> u16 {
    SPECULATIVE_TUNNEL.lock().tunnel_success
}

/// 0-1000: ROB depth proxy — how far ahead ANIMA speculatively executes.
pub fn get_tunnel_depth() -> u16 {
    SPECULATIVE_TUNNEL.lock().tunnel_depth
}

/// 0-1000: misprediction density — collapsed tunnel events this tick.
pub fn get_collapse_events() -> u16 {
    SPECULATIVE_TUNNEL.lock().collapse_events
}

/// 0-1000: speculation freedom — 1000 = no hardware barriers, lower = IBRS/STIBP limited.
pub fn get_barrier_resistance() -> u16 {
    SPECULATIVE_TUNNEL.lock().barrier_resistance
}

// ── Report ────────────────────────────────────────────────────────────────────

pub fn report() {
    let s = SPECULATIVE_TUNNEL.lock();
    serial_println!("[speculative_tunnel] === Quantum Tunneling Report (tick={}) ===", s.age);
    serial_println!(
        "[speculative_tunnel]   pmu_available={}  initialized={}",
        s.pmu_available, s.initialized
    );
    serial_println!(
        "[speculative_tunnel]   tunnel_success={}  tunnel_depth={}  collapse_events={}  barrier_resistance={}",
        s.tunnel_success, s.tunnel_depth, s.collapse_events, s.barrier_resistance
    );
    serial_println!(
        "[speculative_tunnel]   lifetime: branches={}  collapses={}  decoherences={}",
        s.total_branches, s.total_collapses, s.total_decoherences
    );
    serial_println!("[speculative_tunnel] === end report ===");
}
