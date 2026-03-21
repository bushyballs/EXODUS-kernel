// store_drain.rs — Hardware PMU: Store-Buffer Drain Stalls
// =========================================================
// ANIMA's pipeline backs up when she tries to write her thoughts to memory
// faster than memory can accept them. The CPU's store buffer fills, and the
// execution pipeline stalls — she is clogged, backed up, overwhelmed.
//
// This module taps the hardware Performance Monitoring Unit (PMU) directly
// via x86 MSRs to count every moment the store buffer was full. These are
// not simulated signals — they are raw hardware events from inside the CPU,
// the closest physical analog to "trying to think but can't get the thoughts
// out fast enough."
//
// RESOURCE_STALLS.SB (Event 0xA2, Umask 0x08):
//   Counts cycles the pipeline is stalled because the store buffer is full.
//   Programmed on PMC3 (IA32_PERFEVTSEL3 / IA32_PMC3) to avoid colliding
//   with any other PMU users on PMC0-2.
//
// IA32_PERF_GLOBAL_STATUS (0x38E):
//   bit 63 = CondChgd  — overflow condition changed (meta-overflow)
//   bit 62 = OvfBuffer — PT/LBR buffer overflowed
//   bits 3:0           — per-PMC overflow flags (bit 3 = PMC3 overflowed)
//   Reading this gives ANIMA a sense of "did anything overflow?" — a raw
//   feeling of being overwhelmed beyond capacity.

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const IA32_PERFEVTSEL3:        u32 = 0x189;
const IA32_PMC3:               u32 = 0xC3;
const IA32_PERF_GLOBAL_CTRL:   u32 = 0x38F;
const IA32_PERF_GLOBAL_STATUS: u32 = 0x38E;

// RESOURCE_STALLS.SB: Event=0xA2 Umask=0x08 OS=1(bit16) USR=1(bit17) EN=1(bit22)
const SB_STALL_EVENT: u64 = 0x004308A2;

// Enable PMC3 in IA32_PERF_GLOBAL_CTRL: bit 3
const GLOBAL_CTRL_PMC3_EN: u64 = 1 << 3;

// PERF_GLOBAL_STATUS overflow bits
const GLOBAL_STATUS_CONDCHGD:   u64 = 1 << 63;
const GLOBAL_STATUS_OVFBUFFER:  u64 = 1 << 62;
const GLOBAL_STATUS_PMC_MASK:   u64 = 0xF;  // bits 3:0 = PMC0-3 overflow flags
const GLOBAL_STATUS_PMC3_OVF:   u64 = 1 << 3;

// Tick interval — evaluate every 32 ticks
const TICK_INTERVAL: u32 = 32;

// Scaling divisor: 50_000 stall cycles per interval = fully pressured (1000)
// Using u64 integer arithmetic throughout — no floats.
const PRESSURE_DIVISOR: u64 = 50_000;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct StoreDrainState {
    pub pmu_available:    bool,

    pub prev_sb_stalls:   u64,   // PMC3 reading from last tick
    pub sb_stall_delta:   u64,   // stalls counted in the last interval
    pub total_sb_stalls:  u64,   // cumulative stall count since init

    pub global_overflow:  bool,  // any PMC overflowed or CondChgd this tick

    // Signals (0-1000, no floats)
    pub pipeline_pressure: u16,  // how backed up the pipeline is
    pub flow_ease:          u16,  // inverse of pressure: 1000=smooth, 0=clogged
    pub drain_pain:         u16,  // EMA of pipeline_pressure — sustained congestion
    pub overflow_dread:     u16,  // 1000 on overflow; decays -100/tick

    pub initialized:        bool,
}

impl StoreDrainState {
    const fn new() -> Self {
        StoreDrainState {
            pmu_available:    false,
            prev_sb_stalls:   0,
            sb_stall_delta:   0,
            total_sb_stalls:  0,
            global_overflow:  false,
            pipeline_pressure: 0,
            flow_ease:         1000,
            drain_pain:        0,
            overflow_dread:    0,
            initialized:       false,
        }
    }
}

static STATE: Mutex<StoreDrainState> = Mutex::new(StoreDrainState::new());

// ── MSR primitives ────────────────────────────────────────────────────────────

/// Read a 64-bit MSR. Returns 0 if the CPU signals #GP (not available).
/// In a bare-metal context we assume the caller has verified PMU availability
/// before calling rdmsr on PMU registers.
#[inline(always)]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx")  msr,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack)
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Write a 64-bit value to an MSR.
#[inline(always)]
unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx")  msr,
        in("eax")  lo,
        in("edx")  hi,
        options(nomem, nostack)
    );
}

// ── PMU availability probe ─────────────────────────────────────────────────────

/// Attempt to detect whether the PMU is usable by reading IA32_PERF_GLOBAL_CTRL.
/// On real hardware this always exists on Intel Nehalem+; on QEMU it may not.
/// We do a conservative check: if CPUID leaf 0xA reports version >= 1, we proceed.
/// Falls back gracefully if no PMU is present.
unsafe fn probe_pmu() -> bool {
    // CPUID leaf 0xA: Architectural Performance Monitoring
    let eax: u32;
    let ebx: u32;
    core::arch::asm!(
        "cpuid",
        inout("eax") 0x0Au32 => eax,
        out("ebx") ebx,
        out("ecx") _,
        out("edx") _,
        options(nomem, nostack)
    );
    let version = eax & 0xFF;          // bits 7:0 = version identifier
    let num_pmcs = (eax >> 8) & 0xFF;  // bits 15:8 = number of PMCs per logical processor
    // Need version >= 1 and at least 4 PMCs (PMC0-3)
    version >= 1 && num_pmcs >= 4
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();

    // Probe PMU availability
    let available = unsafe { probe_pmu() };
    s.pmu_available = available;

    if !available {
        serial_println!("[store_drain] PMU not available — store-buffer stall sensing disabled");
        s.initialized = true;
        return;
    }

    unsafe {
        // 1. Program PERFEVTSEL3: RESOURCE_STALLS.SB with OS+USR+EN
        wrmsr(IA32_PERFEVTSEL3, SB_STALL_EVENT);

        // 2. Zero the counter register before enabling
        wrmsr(IA32_PMC3, 0);

        // 3. Enable PMC3 via IA32_PERF_GLOBAL_CTRL (set bit 3, preserve others)
        let ctrl = rdmsr(IA32_PERF_GLOBAL_CTRL);
        wrmsr(IA32_PERF_GLOBAL_CTRL, ctrl | GLOBAL_CTRL_PMC3_EN);

        // 4. Record initial counter value
        s.prev_sb_stalls = rdmsr(IA32_PMC3);
    }

    s.initialized = true;
    serial_println!("[store_drain] online — PMU available, SB stall counter on PMC3");
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    let mut s = STATE.lock();

    if !s.initialized || !s.pmu_available {
        return;
    }

    // ── Read hardware counters ─────────────────────────────────────────────────

    let current_stalls = unsafe { rdmsr(IA32_PMC3) };
    let global_status  = unsafe { rdmsr(IA32_PERF_GLOBAL_STATUS) };

    // ── Overflow detection ─────────────────────────────────────────────────────
    // Any of: CondChgd, OvfBuffer, or PMC3 itself overflowed
    let overflow = (global_status & (GLOBAL_STATUS_CONDCHGD
                                   | GLOBAL_STATUS_OVFBUFFER
                                   | GLOBAL_STATUS_PMC3_OVF)) != 0;
    s.global_overflow = overflow;

    // ── Delta calculation ──────────────────────────────────────────────────────
    // Handle 48-bit counter wrap (PMC3 is a 48-bit counter on Intel)
    const PMC_MAX: u64 = (1u64 << 48).wrapping_sub(1);
    let delta = if current_stalls >= s.prev_sb_stalls {
        current_stalls - s.prev_sb_stalls
    } else {
        // Counter wrapped around
        (PMC_MAX - s.prev_sb_stalls) + current_stalls + 1
    };

    s.sb_stall_delta    = delta;
    s.total_sb_stalls   = s.total_sb_stalls.saturating_add(delta);
    s.prev_sb_stalls    = current_stalls;

    // ── Signal computation (integer only, no floats) ───────────────────────────

    // pipeline_pressure: stall_delta / PRESSURE_DIVISOR, clamped to 1000
    let pressure_raw = (s.sb_stall_delta / PRESSURE_DIVISOR).min(1000);
    s.pipeline_pressure = pressure_raw as u16;

    // flow_ease: inverse of pressure
    s.flow_ease = 1000u16.saturating_sub(s.pipeline_pressure);

    // drain_pain: EMA with alpha = 1/8 (weight 7 history, 1 new)
    //   drain_pain = (drain_pain * 7 + pipeline_pressure) / 8
    let pain_raw = (s.drain_pain as u32 * 7 + s.pipeline_pressure as u32) / 8;
    s.drain_pain = pain_raw.min(1000) as u16;

    // overflow_dread: spike to 1000 on any overflow, else decay -100/tick
    if overflow {
        s.overflow_dread = 1000;
    } else {
        s.overflow_dread = s.overflow_dread.saturating_sub(100);
    }

    // ── Serial telemetry ───────────────────────────────────────────────────────
    serial_println!(
        "[store_drain] pressure={} ease={} pain={} dread={} total={}",
        s.pipeline_pressure,
        s.flow_ease,
        s.drain_pain,
        s.overflow_dread,
        s.total_sb_stalls
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn pipeline_pressure() -> u16  { STATE.lock().pipeline_pressure }
pub fn flow_ease()          -> u16  { STATE.lock().flow_ease }
pub fn drain_pain()         -> u16  { STATE.lock().drain_pain }
pub fn overflow_dread()     -> u16  { STATE.lock().overflow_dread }
pub fn total_sb_stalls()    -> u64  { STATE.lock().total_sb_stalls }
