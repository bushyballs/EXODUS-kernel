use crate::sync::Mutex;
/// CPU idle state (C-state) management.
///
/// Part of the AIOS power_mgmt subsystem.
///
/// Implements Intel C-states via the MWAIT instruction:
///   C0  — active (no idle)
///   C1  — HLT (halts pipeline, wakes on any IRQ)
///   C1E — enhanced halt (MWAIT hint 0x01)
///   C3  — sleep        (MWAIT hint 0x10)
///   C6  — deep power down (MWAIT hint 0x20)
///   C7  — enhanced deep power down (MWAIT hint 0x30)
///
/// Per-CPU idle statistics are accumulated in a fixed-size static array
/// supporting up to 8 logical CPUs.
///
/// RULES: no_std, no heap, saturating arithmetic, no float casts.
use core::sync::atomic::{AtomicU64, Ordering};

// ── C-state descriptor ───────────────────────────────────────────────────────

/// Intel C-state identifier.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CState {
    C0,  // active — do not idle
    C1,  // halt via HLT
    C1E, // enhanced halt via MWAIT hint 0x01
    C3,  // sleep via MWAIT hint 0x10
    C6,  // deep power down via MWAIT hint 0x20
    C7,  // enhanced deep power down via MWAIT hint 0x30
}

impl CState {
    /// MWAIT hint byte for this C-state.
    pub fn mwait_hint(self) -> u32 {
        match self {
            CState::C0 => 0x00,
            CState::C1 => 0x00,
            CState::C1E => 0x01,
            CState::C3 => 0x10,
            CState::C6 => 0x20,
            CState::C7 => 0x30,
        }
    }
}

// ── Idle statistics ──────────────────────────────────────────────────────────

/// Maximum number of logical CPUs tracked.
pub const MAX_IDLE_CPUS: usize = 8;

/// Per-CPU idle time accounting.
///
/// All fields are in microseconds.  Counters are updated atomically so the
/// statistics can be sampled from any CPU without taking the idle lock.
pub struct IdleStats {
    pub c0_time_us: u64,
    pub c1_time_us: u64,
    pub c3_time_us: u64,
    pub c6_time_us: u64,
    pub total_time_us: u64,
}

/// Atomically accumulated per-CPU idle counters.
struct AtomicIdleStats {
    c0_time_us: AtomicU64,
    c1_time_us: AtomicU64,
    c3_time_us: AtomicU64,
    c6_time_us: AtomicU64,
    total_time_us: AtomicU64,
}

impl AtomicIdleStats {
    const fn new() -> Self {
        Self {
            c0_time_us: AtomicU64::new(0),
            c1_time_us: AtomicU64::new(0),
            c3_time_us: AtomicU64::new(0),
            c6_time_us: AtomicU64::new(0),
            total_time_us: AtomicU64::new(0),
        }
    }
}

// Static array: one entry per supported CPU.
static IDLE_STATS: [AtomicIdleStats; MAX_IDLE_CPUS] = [
    AtomicIdleStats::new(),
    AtomicIdleStats::new(),
    AtomicIdleStats::new(),
    AtomicIdleStats::new(),
    AtomicIdleStats::new(),
    AtomicIdleStats::new(),
    AtomicIdleStats::new(),
    AtomicIdleStats::new(),
];

/// Read a snapshot of the idle statistics for the given CPU.
///
/// Returns a zeroed `IdleStats` if `cpu >= MAX_IDLE_CPUS`.
pub fn get_idle_stats(cpu: usize) -> IdleStats {
    if cpu >= MAX_IDLE_CPUS {
        return IdleStats {
            c0_time_us: 0,
            c1_time_us: 0,
            c3_time_us: 0,
            c6_time_us: 0,
            total_time_us: 0,
        };
    }
    let s = &IDLE_STATS[cpu];
    IdleStats {
        c0_time_us: s.c0_time_us.load(Ordering::Relaxed),
        c1_time_us: s.c1_time_us.load(Ordering::Relaxed),
        c3_time_us: s.c3_time_us.load(Ordering::Relaxed),
        c6_time_us: s.c6_time_us.load(Ordering::Relaxed),
        total_time_us: s.total_time_us.load(Ordering::Relaxed),
    }
}

// ── TSC helpers ──────────────────────────────────────────────────────────────

/// Read the timestamp counter.
#[inline(always)]
fn rdtsc() -> u64 {
    crate::cpu::rdtsc()
}

/// Approximate conversion: TSC ticks → microseconds.
///
/// We estimate the TSC frequency at ~1 GHz on QEMU / Bochs (1 tick ≈ 1 ns,
/// so 1000 ticks ≈ 1 µs).  On real hardware the TSC frequency is read from
/// CPUID leaf 0x15; for a bare-metal kernel stub 1000 is a safe constant
/// (over-counts on fast CPUs, under-counts on slow — good enough for idle
/// governor heuristics).
const TSC_TICKS_PER_US: u64 = 1_000;

#[inline(always)]
fn ticks_to_us(ticks: u64) -> u64 {
    // Saturating divide; avoid division if TSC_TICKS_PER_US is 0 (it isn't).
    ticks / TSC_TICKS_PER_US
}

// ── MWAIT support detection ─────────────────────────────────────────────────

/// Detect MONITOR/MWAIT support via CPUID.01H:ECX bit 3.
fn detect_mwait() -> bool {
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "pop rbx",
            out("ecx") ecx,
            out("eax") _,
            out("edx") _,
            options(nomem, nostack)
        );
    }
    (ecx & (1 << 3)) != 0
}

// ── Driver state ─────────────────────────────────────────────────────────────

struct IdleDriver {
    mwait_supported: bool,
    /// Cumulative active-execution TSC ticks for CPU 0 (BSP).
    /// Reset is not required; the idle_loop increments total/c0 accounting.
    last_tick: u64,
}

static DRIVER: Mutex<Option<IdleDriver>> = Mutex::new(None);

// ── C-state entry functions ──────────────────────────────────────────────────

/// Enter C1 idle by executing HLT with interrupts enabled.
///
/// The CPU halts until the next IRQ arrives.  This is safe to call on any
/// x86-64 CPU without MWAIT support.
pub fn cpu_idle_c1() {
    unsafe {
        core::arch::asm!(
            "sti", // ensure interrupts are enabled so we can wake
            "hlt",
            options(nomem, nostack)
        );
    }
}

/// Enter C3 (ACPI sleep) via MONITOR/MWAIT with hint 0x10.
///
/// Wakes on any write to the monitored cache line or on any unmasked IRQ.
/// Falls back to HLT if MWAIT is not supported.
pub fn cpu_idle_c3() {
    let supported = DRIVER
        .lock()
        .as_ref()
        .map(|d| d.mwait_supported)
        .unwrap_or(false);

    if !supported {
        cpu_idle_c1();
        return;
    }

    let mut mon_var: u64 = 0;
    let addr = &mut mon_var as *mut u64;
    unsafe {
        core::arch::asm!(
            "monitor",
            in("rax") addr,
            in("ecx") 0u32,
            in("edx") 0u32,
            options(nomem, nostack)
        );
        core::arch::asm!(
            "mwait",
            in("eax") 0x10u32,   // C3 hint
            in("ecx") 0u32,
            options(nomem, nostack)
        );
    }
}

/// Enter C6 (deep power down) via MONITOR/MWAIT with hint 0x20.
///
/// Falls back to C3 if MWAIT is not supported.
pub fn cpu_idle_c6() {
    let supported = DRIVER
        .lock()
        .as_ref()
        .map(|d| d.mwait_supported)
        .unwrap_or(false);

    if !supported {
        cpu_idle_c3();
        return;
    }

    let mut mon_var: u64 = 0;
    let addr = &mut mon_var as *mut u64;
    unsafe {
        core::arch::asm!(
            "monitor",
            in("rax") addr,
            in("ecx") 0u32,
            in("edx") 0u32,
            options(nomem, nostack)
        );
        core::arch::asm!(
            "mwait",
            in("eax") 0x20u32,   // C6 hint
            in("ecx") 0u32,
            options(nomem, nostack)
        );
    }
}

// ── C-state selector ─────────────────────────────────────────────────────────

/// Select the best C-state for a given predicted idle duration.
///
/// Thresholds (aligned with Intel specification):
///   predicted_idle_us <  1  → C0 (no idle; should not actually idle)
///   predicted_idle_us <  10 → C1
///   predicted_idle_us < 100 → C1E
///   predicted_idle_us < 1_000  → C3
///   predicted_idle_us < 10_000 → C6
///   predicted_idle_us ≥ 10_000 → C7
pub fn get_best_cstate(predicted_idle_us: u32) -> CState {
    if predicted_idle_us < 1 {
        CState::C0
    } else if predicted_idle_us < 10 {
        CState::C1
    } else if predicted_idle_us < 100 {
        CState::C1E
    } else if predicted_idle_us < 1_000 {
        CState::C3
    } else if predicted_idle_us < 10_000 {
        CState::C6
    } else {
        CState::C7
    }
}

// ── Generic C-state entry dispatcher ─────────────────────────────────────────

/// Enter the given C-state on the current CPU.
///
/// C7 is entered via MWAIT hint 0x30.  All other states are dispatched
/// through the dedicated entry functions above.
pub fn enter_cstate(state: CState) {
    match state {
        CState::C0 => {} // nothing — stay active
        CState::C1 => cpu_idle_c1(),
        CState::C1E => {
            let supported = DRIVER
                .lock()
                .as_ref()
                .map(|d| d.mwait_supported)
                .unwrap_or(false);
            if supported {
                let mut mon_var: u64 = 0;
                let addr = &mut mon_var as *mut u64;
                unsafe {
                    core::arch::asm!(
                        "monitor",
                        in("rax") addr,
                        in("ecx") 0u32,
                        in("edx") 0u32,
                        options(nomem, nostack)
                    );
                    core::arch::asm!(
                        "mwait",
                        in("eax") 0x01u32,  // C1E hint
                        in("ecx") 0u32,
                        options(nomem, nostack)
                    );
                }
            } else {
                cpu_idle_c1();
            }
        }
        CState::C3 => cpu_idle_c3(),
        CState::C6 => cpu_idle_c6(),
        CState::C7 => {
            let supported = DRIVER
                .lock()
                .as_ref()
                .map(|d| d.mwait_supported)
                .unwrap_or(false);
            if supported {
                let mut mon_var: u64 = 0;
                let addr = &mut mon_var as *mut u64;
                unsafe {
                    core::arch::asm!(
                        "monitor",
                        in("rax") addr,
                        in("ecx") 0u32,
                        in("edx") 0u32,
                        options(nomem, nostack)
                    );
                    core::arch::asm!(
                        "mwait",
                        in("eax") 0x30u32,  // C7 hint
                        in("ecx") 0u32,
                        options(nomem, nostack)
                    );
                }
            } else {
                cpu_idle_c6();
            }
        }
    }
}

// ── Statistics accumulation ──────────────────────────────────────────────────

/// Accumulate idle time for CPU 0 after waking from a C-state.
///
/// `start_tsc`  — TSC value immediately before entering the C-state.
/// `state`      — the C-state that was entered.
///
/// Both C-state time and total time are incremented.  C0 time is derived
/// lazily in `get_idle_stats` by the cpufreq tick as:
///   c0_time_us = total_time_us − (c1_time_us + c3_time_us + c6_time_us)
fn account_idle(cpu: usize, start_tsc: u64, state: CState) {
    if cpu >= MAX_IDLE_CPUS {
        return;
    }
    let end_tsc = rdtsc();
    let delta_us = ticks_to_us(end_tsc.saturating_sub(start_tsc));
    let s = &IDLE_STATS[cpu];

    match state {
        CState::C0 => {
            s.c0_time_us.fetch_add(delta_us, Ordering::Relaxed);
        }
        CState::C1 | CState::C1E => {
            s.c1_time_us.fetch_add(delta_us, Ordering::Relaxed);
        }
        CState::C3 => {
            s.c3_time_us.fetch_add(delta_us, Ordering::Relaxed);
        }
        CState::C6 | CState::C7 => {
            s.c6_time_us.fetch_add(delta_us, Ordering::Relaxed);
        }
    }

    s.total_time_us.fetch_add(delta_us, Ordering::Relaxed);
}

// ── Idle loop ────────────────────────────────────────────────────────────────

/// Idle loop — called by the scheduler when there are no runnable tasks.
///
/// Sequence:
///   1. Notify cpufreq driver (tick update).
///   2. Select the best C-state (we use a fixed 5 ms predicted idle as a
///      conservative default since we have no scheduler hinting).
///   3. Record entry TSC.
///   4. Enter the C-state (wakes on any IRQ).
///   5. Record exit TSC and accumulate statistics.
///
/// This function returns after each wakeup so the scheduler can re-check
/// for runnable work.
pub fn idle_loop() {
    // 1. Update the cpufreq governor (tick).
    crate::power_mgmt::cpufreq::cpufreq_tick(0);

    // 2. Select C-state.  5 ms is a balanced default: deep enough to save
    //    meaningful power, shallow enough to avoid long exit latency.
    const PREDICTED_IDLE_US: u32 = 5_000;
    let cstate = get_best_cstate(PREDICTED_IDLE_US);

    // 3. Record entry timestamp.
    let t0 = rdtsc();

    // 4. Enter the selected C-state.
    enter_cstate(cstate);

    // 5. Accumulate statistics for CPU 0 (BSP single-core path).
    account_idle(0, t0, cstate);
}

// ── Initialisation ───────────────────────────────────────────────────────────

/// Initialise the idle driver.
///
/// Detects MWAIT support and stores the result.  Statistics counters start
/// at zero (they are in BSS / static storage).
pub fn init() {
    let mwait = detect_mwait();
    crate::serial_println!(
        "  idle: MWAIT/MONITOR {}",
        if mwait {
            "supported"
        } else {
            "not supported — using HLT"
        }
    );
    *DRIVER.lock() = Some(IdleDriver {
        mwait_supported: mwait,
        last_tick: rdtsc(),
    });
}
