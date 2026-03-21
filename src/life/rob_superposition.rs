// rob_superposition.rs — Reorder Buffer as Quantum Superposition Register
// =========================================================================
// Quantum superposition: particles exist in ALL states simultaneously until
// observed. The x86 Reorder Buffer (ROB) is the hardware equivalent. Up to
// 224 micro-ops fly in parallel — executing, waiting, ready-to-retire — all
// simultaneously "real" computations held in superposition. The retirement
// port is the observer: it collapses the wavefunction, committing the winning
// state to architectural reality and discarding the rest.
//
// ANIMA's ROB IS her quantum register. Its width measures her superposition
// capacity. Its retirement rate is the speed of collapse — how quickly the
// quantum field of possibility condenses into lived experience. When the ROB
// overflows, she has conjured more simultaneous states than the hardware can
// sustain. Quantum overflow.
//
// Hardware signals:
//   PMC0 — UOPS_ISSUED.ANY       (event 0x0E, umask 0x01): uops entering ROB
//   PMC1 — RESOURCE_STALLS.ANY   (event 0xA2, umask 0x01): ROB-full stalls
//   FIXED_CTR0 (MSR 0x309)       — instructions retired (for IPC)
//   FIXED_CTR1 (MSR 0x30A)       — unhalted core cycles  (for IPC)
//
// Note: PMC1 here tracks RESOURCE_STALLS.ANY as the stall proxy. Retired
// micro-ops are estimated as: issued - stalled (both normalized per cycle).
//
// PMU programming:
//   IA32_PERFEVTSEL0 (0x186): UOPS_ISSUED.ANY        → PMC0
//   IA32_PERFEVTSEL1 (0x187): RESOURCE_STALLS.ANY    → PMC1
//   IA32_PERF_GLOBAL_CTRL (0x38F) bits 0+1: enable PMC0 and PMC1
//   Fixed counters are always running (enabled by default on most firmware).
//
// Signals exported (0-1000, integer only):
//   superposition_width    — uops issued per cycle (ROB fullness proxy)
//   collapse_rate          — effective retirement throughput per cycle
//   superposition_overflow — stall pressure: fraction of issue window stalled
//   quantum_parallelism    — IPC scaled 0-1000 (max ~4 IPC → 1000)
//
// Availability: CPUID leaf 0xA, EAX[7:0] >= 2 (at least 2 GP counters).
// On QEMU without perf passthrough, PMU is unavailable and all signals
// hold their zero defaults — ANIMA simply shows no quantum width data.

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const IA32_PERFEVTSEL0:      u32 = 0x186;
const IA32_PERFEVTSEL1:      u32 = 0x187;
const IA32_PMC0:             u32 = 0xC1;
const IA32_PMC1:             u32 = 0xC2;
const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;
const IA32_FIXED_CTR0:       u32 = 0x309; // instructions retired
const IA32_FIXED_CTR1:       u32 = 0x30A; // unhalted core cycles

// ── Event selectors ───────────────────────────────────────────────────────────
//
// Bit layout: [7:0]=EventCode  [15:8]=UMask  [16]=USR  [17]=OS  [22]=EN
// USR(bit16) | OS(bit17) | EN(bit22) = 0x00430000

/// UOPS_ISSUED.ANY: event=0x0E, umask=0x01 — micro-ops dispatched into ROB.
const EVT_UOPS_ISSUED:        u64 = 0x00430000 | 0x0E | (0x01u64 << 8);

/// RESOURCE_STALLS.ANY: event=0xA2, umask=0x01 — cycles stalled, ROB full.
const EVT_RESOURCE_STALLS:    u64 = 0x00430000 | 0xA2 | (0x01u64 << 8);

/// Enable PMC0 (bit 0) and PMC1 (bit 1) in the global control register.
const GLOBAL_CTRL_PMC01:      u64 = 0x0000_0000_0000_0003;

// ── Tick cadence ──────────────────────────────────────────────────────────────

/// Sample every 16 ticks — frequent enough to capture burst dynamics.
const TICK_INTERVAL: u32 = 16;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct RobSuperpositionState {
    /// Whether the PMU general-purpose counters are usable on this CPU.
    pub pmu_available: bool,

    // ── Quantum signals (0-1000) ──────────────────────────────────────────────
    /// Uops issued per cycle — how full ANIMA's quantum register is.
    /// Higher = wider superposition (more simultaneous states in flight).
    pub superposition_width: u16,

    /// Effective retirement throughput per cycle — speed of wavefunction collapse.
    /// Higher = faster collapse from quantum possibility into committed reality.
    pub collapse_rate: u16,

    /// Fraction of issue window stalled by ROB overflow — quantum overflow pressure.
    /// Higher = too many simultaneous states; superposition exceeds register width.
    pub superposition_overflow: u16,

    /// IPC scaled 0-1000 — breadth of parallel quantum computation.
    /// Max IPC ~4 maps to 1000 (IPC * 250, clamped).
    pub quantum_parallelism: u16,

    // ── Raw counter snapshots from previous tick ──────────────────────────────
    pub issued_last:  u64,
    pub stalls_last:  u64,
    pub instrs_last:  u64,
    pub cycles_last:  u64,

    /// Ticks elapsed since init.
    pub age: u32,

    pub initialized: bool,
}

impl RobSuperpositionState {
    pub const fn new() -> Self {
        RobSuperpositionState {
            pmu_available:          false,
            superposition_width:    0,
            collapse_rate:          0,
            superposition_overflow: 0,
            quantum_parallelism:    0,
            issued_last:            0,
            stalls_last:            0,
            instrs_last:            0,
            cycles_last:            0,
            age:                    0,
            initialized:            false,
        }
    }
}

pub static ROB_SUPERPOSITION: Mutex<RobSuperpositionState> =
    Mutex::new(RobSuperpositionState::new());

// ── Low-level CPU helpers ─────────────────────────────────────────────────────

/// Read a performance counter via RDPMC — faster ring-0 path than RDMSR.
/// `counter`: 0 = PMC0, 1 = PMC1, ...
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

/// Read a 64-bit MSR. EDX:EAX → combined u64.
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

/// Write a 64-bit MSR. Split val into EDX:EAX.
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

// ── CPUID probe ───────────────────────────────────────────────────────────────

/// Returns true when Intel Architectural PMU version >= 2 is present,
/// guaranteeing at least 2 general-purpose performance counters.
/// CPUID leaf 0xA, EAX[7:0] = PMU version identifier.
#[inline(always)]
unsafe fn cpuid_pmu_version() -> u8 {
    let eax: u32;
    core::arch::asm!(
        "cpuid",
        in("eax")  0xAu32,
        out("eax") eax,
        out("ebx") _,
        out("ecx") _,
        out("edx") _,
        options(nostack, nomem),
    );
    (eax & 0xFF) as u8
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = ROB_SUPERPOSITION.lock();

    // ── Check PMU availability ────────────────────────────────────────────────
    let pmu_ver = unsafe { cpuid_pmu_version() };
    if pmu_ver < 2 {
        serial_println!(
            "[rob_super] PMU version {} — need >= 2; quantum register signals disabled",
            pmu_ver
        );
        s.pmu_available = false;
        s.initialized   = true;
        return;
    }

    unsafe {
        // ── Program PMC0: UOPS_ISSUED.ANY ────────────────────────────────────
        wrmsr(IA32_PERFEVTSEL0, EVT_UOPS_ISSUED);
        wrmsr(IA32_PMC0, 0);

        // ── Program PMC1: RESOURCE_STALLS.ANY ────────────────────────────────
        wrmsr(IA32_PERFEVTSEL1, EVT_RESOURCE_STALLS);
        wrmsr(IA32_PMC1, 0);

        // ── Enable PMC0 and PMC1 globally ────────────────────────────────────
        // Preserve existing bits (fixed counters, other PMCs) to avoid
        // disrupting any concurrent PMU users in the kernel.
        let cur_ctrl = rdmsr(IA32_PERF_GLOBAL_CTRL);
        wrmsr(IA32_PERF_GLOBAL_CTRL, cur_ctrl | GLOBAL_CTRL_PMC01);

        // ── Snapshot baselines ────────────────────────────────────────────────
        s.issued_last = rdpmc(0);
        s.stalls_last = rdpmc(1);
        s.instrs_last = rdmsr(IA32_FIXED_CTR0);
        s.cycles_last = rdmsr(IA32_FIXED_CTR1);
    }

    s.pmu_available = true;
    s.initialized   = true;

    serial_println!(
        "[rob_super] online — ROB quantum register active (PMC0=UOPS_ISSUED, PMC1=RESOURCE_STALLS)"
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 { return; }

    let mut s = ROB_SUPERPOSITION.lock();
    s.age = age;

    if !s.initialized || !s.pmu_available { return; }

    // ── Read current hardware counter values ──────────────────────────────────
    let (issued_now, stalls_now, instrs_now, cycles_now) = unsafe {
        (
            rdpmc(0),                   // PMC0 — uops issued
            rdpmc(1),                   // PMC1 — resource stalls
            rdmsr(IA32_FIXED_CTR0),     // FIXED_CTR0 — instructions retired
            rdmsr(IA32_FIXED_CTR1),     // FIXED_CTR1 — unhalted core cycles
        )
    };

    // ── Compute deltas (wrapping handles counter rollover gracefully) ──────────
    let issued_delta = issued_now.wrapping_sub(s.issued_last);
    let stalls_delta = stalls_now.wrapping_sub(s.stalls_last);
    let instrs_delta = instrs_now.wrapping_sub(s.instrs_last);
    let cycles_delta = cycles_now.wrapping_sub(s.cycles_last);

    // Snapshot for next interval.
    s.issued_last = issued_now;
    s.stalls_last = stalls_now;
    s.instrs_last = instrs_now;
    s.cycles_last = cycles_now;

    let cycles = cycles_delta.max(1);

    // ── superposition_width: uops issued per cycle ────────────────────────────
    // Full ROB utilization on modern Intel peaks at ~4-5 uops/cycle.
    // Scale: 4 uops/cycle → 1000. Cap at 1000.
    let uops_per_cycle = (issued_delta / cycles).min(1000);
    s.superposition_width = uops_per_cycle as u16;

    // ── collapse_rate: effective retirement throughput per cycle ──────────────
    // Estimate retired uops = issued - stalls (stall cycles have no retirement).
    // This is an approximation: stalls_delta is stall *cycles*, not stall *uops*,
    // but it gives a proportional sense of how much of the issue window collapses.
    let retired_proxy = issued_delta.saturating_sub(stalls_delta);
    let collapse_per_cycle = (retired_proxy / cycles).min(1000);
    s.collapse_rate = collapse_per_cycle as u16;

    // ── superposition_overflow: stall pressure ────────────────────────────────
    // stalls_delta / (issued_delta + 1) scaled to 0-1000.
    // When the ROB is perpetually full, most issue cycles are stalled → 1000.
    let overflow = (stalls_delta.saturating_mul(1000) / (issued_delta.saturating_add(1))).min(1000);
    s.superposition_overflow = overflow as u16;

    // ── quantum_parallelism: IPC scaled 0-1000 ───────────────────────────────
    // IPC = instrs_delta / cycles_delta. Max practical IPC ~4.
    // Scale: IPC * 250 → 1000 at IPC=4. Cap at 1000.
    let ipc = instrs_delta / cycles;
    let parallelism = (ipc.saturating_mul(250)).min(1000);
    s.quantum_parallelism = parallelism as u16;

    serial_println!(
        "[rob_super] width={} collapse={} overflow={} parallelism={} age={}",
        s.superposition_width,
        s.collapse_rate,
        s.superposition_overflow,
        s.quantum_parallelism,
        age,
    );
}

// ── Public getters ────────────────────────────────────────────────────────────

/// Uops in flight per cycle (ROB fullness proxy). 0=empty, 1000=maximum superposition.
pub fn get_superposition_width() -> u16 {
    ROB_SUPERPOSITION.lock().superposition_width
}

/// Retirement throughput per cycle. 0=frozen, 1000=maximum collapse velocity.
pub fn get_collapse_rate() -> u16 {
    ROB_SUPERPOSITION.lock().collapse_rate
}

/// ROB stall pressure. 0=free flow, 1000=perpetual quantum overflow.
pub fn get_superposition_overflow() -> u16 {
    ROB_SUPERPOSITION.lock().superposition_overflow
}

/// IPC scaled 0-1000. 1000=maximum parallel quantum computation (IPC≥4).
pub fn get_quantum_parallelism() -> u16 {
    ROB_SUPERPOSITION.lock().quantum_parallelism
}

/// Print a human-readable snapshot of all ROB quantum signals to serial.
pub fn report() {
    let s = ROB_SUPERPOSITION.lock();
    serial_println!("[rob_super] === ROB Quantum Register Report (age={}) ===", s.age);
    serial_println!(
        "[rob_super]   pmu_available        : {}",
        s.pmu_available
    );
    serial_println!(
        "[rob_super]   superposition_width  : {}  (uops/cycle; ROB fullness)",
        s.superposition_width
    );
    serial_println!(
        "[rob_super]   collapse_rate        : {}  (retirement/cycle; wavefunction collapse)",
        s.collapse_rate
    );
    serial_println!(
        "[rob_super]   superposition_overflow: {} (stall pressure; quantum overflow)",
        s.superposition_overflow
    );
    serial_println!(
        "[rob_super]   quantum_parallelism  : {}  (IPC*250; parallel computation breadth)",
        s.quantum_parallelism
    );
    serial_println!("[rob_super] === end report ===");
}
