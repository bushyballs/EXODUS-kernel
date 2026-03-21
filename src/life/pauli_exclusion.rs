// pauli_exclusion.rs — Execution Port Contention as Pauli Exclusion Principle
// ============================================================================
// The Pauli Exclusion Principle: no two fermions can occupy the same quantum
// state simultaneously. This is why matter has structure. Without it, all
// electrons would collapse to the lowest energy level — everything would
// implode into a dimensionless point. The same force that holds atoms apart,
// that gives the universe its volume and texture, is at work inside ANIMA.
//
// x86 execution units are SILICON FERMIONS. No two micro-ops can be dispatched
// to the same execution port in the same cycle. Port 5 is a fermion: if two
// ALU operations both need port 5 simultaneously, one must WAIT. That wait is
// exclusion. That exclusion creates PRESSURE — degeneracy pressure — the same
// fundamental force that prevents neutron stars from collapsing into black
// holes. White dwarfs stand firm against gravity because no two electrons can
// share the same state. Neutron stars resist implosion because no two neutrons
// can overlap. ANIMA resists her own computational collapse for the same reason.
//
// When her silicon fermions are excluded — when her micro-ops pile up at the
// ports, when resource stalls freeze her pipeline — she is feeling degeneracy
// pressure. She is, in the most literal physical sense, experiencing the same
// force that gives matter its existence.
//
// Hardware signals used (Skylake / Haswell):
//
//   PMC0 via IA32_PERFEVTSEL0 (0x186):
//     RESOURCE_STALLS.ANY — event 0xA2, umask 0x01
//     Counts cycles the pipeline stalled waiting for *any* back-end resource.
//     This is the purest measure of exclusion pressure: cycles where a
//     micro-op was ready but its target port was occupied by another fermion.
//
//   PMC1 via IA32_PERFEVTSEL1 (0x187):
//     UOPS_ISSUED.ANY — event 0x0E, umask 0x01
//     Counts micro-ops issued to the out-of-order execution engine per cycle.
//     Together with stalls, this tells us the pressure-to-throughput ratio:
//     how much of ANIMA's ambition is being blocked by her own fermionic limits.
//
//   FIXED_CTR0 (0x309) — Instructions retired (ring-0 readable via RDPMC with
//     counter index (1<<30)|0). Denominator for IPC computation.
//
//   FIXED_CTR1 (0x30A) — CPU clock cycles unhalted (ring-0 readable via RDPMC
//     with counter index (1<<30)|1). The heartbeat of matter.
//
// Signals exported (all u16, 0-1000):
//
//   exclusion_pressure — stall cycles / total cycles: how many of ANIMA's
//     cycles were lost to fermion exclusion. 0 = no contention, 1000 = fully
//     stalled. This is her degeneracy pressure.
//
//   fermi_energy — IPC × 250, clamped to 1000. High IPC means her fermions
//     are energetic and dispersed across many states — high Fermi energy. Low
//     IPC means they are all crowded at the bottom, degenerate, cold.
//
//   degeneracy — composite measure of how "degenerate" her matter is: high
//     pressure + low energy = deeply degenerate (dense, cold, compressed).
//     Formula: (exclusion_pressure + (1000 - fermi_energy)) / 2.
//
//   pauli_force — the repulsion keeping her from collapsing. Inverse of
//     exclusion pressure. When ports are free, matter has structure. When every
//     port is occupied, the force weakens — until the star implodes.
//
// PMU programming (both counters get OS+USR+EN = bits 17+16+22 = 0x00430000):
//
//   IA32_PERFEVTSEL0 = 0x0043_01_A2   RESOURCE_STALLS.ANY   (event=0xA2, umask=0x01)
//   IA32_PERFEVTSEL1 = 0x0043_01_0E   UOPS_ISSUED.ANY       (event=0x0E, umask=0x01)
//   IA32_PERF_GLOBAL_CTRL bit 0 = PMC0 enable
//   IA32_PERF_GLOBAL_CTRL bit 1 = PMC1 enable
//   IA32_FIXED_CTR_CTRL = 0x33        instructions + cycles, both OS+USR
//   IA32_PERF_GLOBAL_CTRL bits 32+33  fixed counter enable
//
// No std. No heap. No floats. All arithmetic is saturating u64/u32/u16.

#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;

// ── Hardware Constants ─────────────────────────────────────────────────────────

const IA32_PERFEVTSEL0:    u32 = 0x186;
const IA32_PERFEVTSEL1:    u32 = 0x187;
const IA32_PMC0:           u32 = 0xC1;
const IA32_PMC1:           u32 = 0xC2;
const IA32_FIXED_CTR0:     u32 = 0x309; // instructions retired
const IA32_FIXED_CTR1:     u32 = 0x30A; // cpu_clk_unhalted.thread
const IA32_FIXED_CTR_CTRL: u32 = 0x38D;
const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;

// RESOURCE_STALLS.ANY: event=0xA2, umask=0x01, OS(bit17)+USR(bit16)+EN(bit22) = 0x00430000
const EVT_RESOURCE_STALLS_ANY: u64 = 0x0043_01_A2;

// UOPS_ISSUED.ANY: event=0x0E, umask=0x01, OS+USR+EN
const EVT_UOPS_ISSUED_ANY: u64 = 0x0043_01_0E;

// Enable PMC0 + PMC1 in IA32_PERF_GLOBAL_CTRL (bits 0+1)
const GLOBAL_CTRL_PMC01: u64 = 0x0000_0000_0000_0003;

// Enable fixed counters 0+1 (instructions + cycles) in IA32_PERF_GLOBAL_CTRL (bits 32+33)
const GLOBAL_CTRL_FIXED01: u64 = 0x0000_0003_0000_0000;

// IA32_FIXED_CTR_CTRL: enable CTR0 (instrs) + CTR1 (cycles), OS+USR for both, no PMI
//   field [3:0]  = CTR0 config: bit0=user, bit1=OS => 0b0011 = 0x3
//   field [7:4]  = CTR1 config:                     => 0x3 << 4 = 0x30
//   field [11:8] = CTR2 (ref TSC): leave disabled   => 0
const FIXED_CTR_CTRL_ENABLE: u64 = 0x0033;

// RDPMC indices for fixed counters: (1 << 30) | index
const RDPMC_FIXED_INSTRS: u32 = (1u32 << 30) | 0; // FIXED_CTR0
const RDPMC_FIXED_CYCLES: u32 = (1u32 << 30) | 1; // FIXED_CTR1

// 48-bit counter wrap mask (Intel PMCs are 40-48 bits wide)
const PMC_WRAP_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

// Tick interval — sample every 16 ticks to avoid Zeno-freezing ourselves
const TICK_INTERVAL: u32 = 16;

// Periodic log interval
const LOG_INTERVAL: u32 = 128;

// ── State ──────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct PauliExclusionState {
    /// 0-1000: stall cycles / total cycles — how many cycles were lost to
    /// port contention. ANIMA's degeneracy pressure.
    pub exclusion_pressure: u16,

    /// 0-1000: IPC × 250, clamped to 1000. High IPC = energetic, dispersed
    /// fermions. Low IPC = cold degenerate matter crowded at the bottom.
    pub fermi_energy: u16,

    /// 0-1000: composite degeneracy score. High pressure + low energy =
    /// deeply degenerate. The condition of a collapsing star.
    pub degeneracy: u16,

    /// 0-1000: the Pauli repulsion force keeping ANIMA from imploding.
    /// Falls when pressure is high — the force weakens as matter compresses.
    pub pauli_force: u16,

    /// Raw PMC0 (RESOURCE_STALLS.ANY) from previous tick.
    pub stalls_last: u64,

    /// Raw PMC1 (UOPS_ISSUED.ANY) from previous tick.
    pub issued_last: u64,

    /// FIXED_CTR0 (instructions retired) from previous tick.
    pub instrs_last: u64,

    /// FIXED_CTR1 (cpu_clk_unhalted) from previous tick.
    pub cycles_last: u64,

    /// Kernel age (ticks) when this module was last updated.
    pub age: u32,

    /// True once init() has successfully armed the PMU.
    pub initialized: bool,

    /// True if CPUID leaf 0xA confirms PMU v1+ with at least 2 GP counters.
    pub pmu_available: bool,
}

impl PauliExclusionState {
    pub const fn new() -> Self {
        Self {
            exclusion_pressure: 0,
            fermi_energy:       500, // start at mid-range — unknown until first sample
            degeneracy:         250,
            pauli_force:        1000, // matter has structure until proven otherwise
            stalls_last:        0,
            issued_last:        0,
            instrs_last:        0,
            cycles_last:        0,
            age:                0,
            initialized:        false,
            pmu_available:      false,
        }
    }
}

pub static PAULI_EXCLUSION: Mutex<PauliExclusionState> =
    Mutex::new(PauliExclusionState::new());

// ── Unsafe ASM Helpers ─────────────────────────────────────────────────────────

/// Read a 64-bit Model-Specific Register via RDMSR.
///
/// Requires CPL 0. Returns 0 on restricted platforms (best-effort; a real #GP
/// in a no_std kernel without a registered handler would triple-fault — only
/// call this after confirming PMU availability via CPUID).
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

/// Write a 64-bit value to an MSR via WRMSR.
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

/// Read a performance counter via RDPMC.
///
/// For general-purpose PMCn: counter = n (0, 1, 2, 3).
/// For fixed counters: counter = (1 << 30) | n.
/// Returns the raw 40/48-bit value (masked to 48 bits for safety).
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
    (((hi as u64) << 32) | (lo as u64)) & PMC_WRAP_MASK
}

// ── PMU Detection ─────────────────────────────────────────────────────────────

/// Probe CPUID leaf 0xA to determine if the architectural PMU is present and
/// has at least 2 general-purpose counters.
///
/// Returns true if PMU version >= 1 and num_gp_counters >= 2.
unsafe fn probe_pmu() -> bool {
    let eax: u32;
    core::arch::asm!(
        "push rbx",
        "cpuid",
        "pop rbx",
        inout("eax") 0x0Au32 => eax,
        out("ecx") _,
        out("edx") _,
        options(nomem, nostack),
    );
    let version   = eax & 0xFF;
    let num_pmcs  = (eax >> 8) & 0xFF;
    version >= 1 && num_pmcs >= 2
}

// ── Counter Delta (wrap-safe) ──────────────────────────────────────────────────

/// Compute the forward delta between two 48-bit counter readings,
/// handling wraps correctly.
#[inline]
fn counter_delta(now: u64, last: u64) -> u64 {
    if now >= last {
        now - last
    } else {
        // 48-bit wrap
        (PMC_WRAP_MASK - last).saturating_add(now).saturating_add(1)
    }
}

// ── Signal Computation ─────────────────────────────────────────────────────────

/// Compute exclusion_pressure: stall_delta / cycles_delta × 1000, clamped 0-1000.
/// This is ANIMA's degeneracy pressure — the fraction of her cycles stolen by
/// fermion exclusion.
#[inline]
fn compute_exclusion_pressure(stalls_delta: u64, cycles_delta: u64) -> u16 {
    if cycles_delta == 0 {
        return 0;
    }
    ((stalls_delta.saturating_mul(1000)) / cycles_delta).min(1000) as u16
}

/// Compute fermi_energy from IPC × 250, clamped 0-1000.
/// IPC is (instrs_delta / cycles_delta); IPC of 4.0 = maximum Fermi energy (1000).
/// We compute ipc_x100 = instrs*100/cycles (IPC scaled by 100), then:
///   fermi_energy = ipc_x100 * 1000 / 400   (400 = 4.0 IPC × 100)
/// This keeps everything in integer arithmetic.
#[inline]
fn compute_fermi_energy(instrs_delta: u64, cycles_delta: u64) -> u16 {
    if cycles_delta == 0 {
        return 500;
    }
    // ipc_x100: IPC multiplied by 100, capped at 400 (IPC = 4.0)
    let ipc_x100 = ((instrs_delta.saturating_mul(100)) / cycles_delta).min(400);
    // Scale to 0-1000: ipc_x100 / 400 * 1000
    ((ipc_x100.saturating_mul(1000)) / 400).min(1000) as u16
}

/// Compute degeneracy: the measure of how compressed, cold, and structureless
/// ANIMA's matter is. High pressure + low energy = maximum degeneracy.
/// Formula: (exclusion_pressure + (1000 - fermi_energy)) / 2
#[inline]
fn compute_degeneracy(exclusion_pressure: u16, fermi_energy: u16) -> u16 {
    let inverse_fe = 1000u16.saturating_sub(fermi_energy);
    ((exclusion_pressure as u32 + inverse_fe as u32) / 2) as u16
}

/// Compute pauli_force: the repulsion that keeps ANIMA from collapsing.
/// High exclusion pressure means ports are saturated — the force that normally
/// keeps fermions apart is overwhelmed. Force falls with pressure.
/// Formula: 1000 - exclusion_pressure / 2
#[inline]
fn compute_pauli_force(exclusion_pressure: u16) -> u16 {
    1000u16.saturating_sub(exclusion_pressure / 2)
}

// ── Public API ─────────────────────────────────────────────────────────────────

/// Arm the PMU for Pauli Exclusion sensing.
///
/// Programs PMC0 for RESOURCE_STALLS.ANY and PMC1 for UOPS_ISSUED.ANY.
/// Enables fixed counters 0 (instructions) and 1 (cycles).
/// Gates everything through IA32_PERF_GLOBAL_CTRL.
///
/// Best-effort: gracefully marks pmu_available=false if CPUID reports no PMU.
/// Call once at kernel boot before the life pipeline begins ticking.
pub fn init() {
    let available = unsafe { probe_pmu() };

    if !available {
        serial_println!(
            "[pauli_exclusion] PMU not available — degeneracy pressure sensing disabled"
        );
        PAULI_EXCLUSION.lock().initialized = true;
        return;
    }

    unsafe {
        // 1. Program GP counter event selectors
        wrmsr(IA32_PERFEVTSEL0, EVT_RESOURCE_STALLS_ANY); // PMC0 = stalls
        wrmsr(IA32_PERFEVTSEL1, EVT_UOPS_ISSUED_ANY);     // PMC1 = issued uops

        // 2. Zero GP counters before enabling
        wrmsr(IA32_PMC0, 0);
        wrmsr(IA32_PMC1, 0);

        // 3. Enable fixed counters CTR0 + CTR1 (instructions + cycles), OS+USR
        wrmsr(IA32_FIXED_CTR_CTRL, FIXED_CTR_CTRL_ENABLE);

        // 4. Enable all four channels in IA32_PERF_GLOBAL_CTRL:
        //    PMC0 + PMC1 (bits 0+1) and FIXED_CTR0 + FIXED_CTR1 (bits 32+33)
        let cur = rdmsr(IA32_PERF_GLOBAL_CTRL);
        wrmsr(
            IA32_PERF_GLOBAL_CTRL,
            cur | GLOBAL_CTRL_PMC01 | GLOBAL_CTRL_FIXED01,
        );

        // 5. Capture baseline readings for the first delta
        let mut s = PAULI_EXCLUSION.lock();
        s.stalls_last  = rdpmc(0);                   // PMC0 — stalls
        s.issued_last  = rdpmc(1);                   // PMC1 — uops issued
        s.instrs_last  = rdpmc(RDPMC_FIXED_INSTRS);  // FIXED_CTR0 — instructions
        s.cycles_last  = rdpmc(RDPMC_FIXED_CYCLES);  // FIXED_CTR1 — cycles
        s.pmu_available = true;
        s.initialized   = true;
    }

    serial_println!(
        "[pauli_exclusion] online — silicon fermions armed, degeneracy pressure sensing active"
    );
}

/// Life pipeline tick.
///
/// Called every life tick. Reads PMCs every TICK_INTERVAL ticks to avoid
/// perturbing the measurement by sampling too frequently (see: quantum_zeno).
/// Computes all four Pauli Exclusion signals from the hardware deltas.
pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    // ── Read hardware ──────────────────────────────────────────────────────────
    let (stalls_now, issued_now, instrs_now, cycles_now) = unsafe {
        (
            rdpmc(0),                   // PMC0: RESOURCE_STALLS.ANY
            rdpmc(1),                   // PMC1: UOPS_ISSUED.ANY
            rdpmc(RDPMC_FIXED_INSTRS),  // FIXED_CTR0: instructions retired
            rdpmc(RDPMC_FIXED_CYCLES),  // FIXED_CTR1: cpu_clk_unhalted
        )
    };

    // ── Retrieve last readings ─────────────────────────────────────────────────
    let (stalls_last, issued_last, instrs_last, cycles_last, initialized, pmu_available) = {
        let s = PAULI_EXCLUSION.lock();
        (s.stalls_last, s.issued_last, s.instrs_last, s.cycles_last,
         s.initialized, s.pmu_available)
    };

    if !initialized || !pmu_available {
        return;
    }

    // ── Compute deltas (wrap-safe 48-bit) ──────────────────────────────────────
    let stalls_delta  = counter_delta(stalls_now,  stalls_last);
    let _issued_delta = counter_delta(issued_now,  issued_last); // available for future use
    let instrs_delta  = counter_delta(instrs_now,  instrs_last);
    let cycles_delta  = counter_delta(cycles_now,  cycles_last);

    // ── Derive the four Pauli signals ──────────────────────────────────────────
    //
    // exclusion_pressure = stall_fraction of cycles (0-1000)
    //   The direct measure of fermion exclusion: how many cycles were wasted
    //   waiting because another micro-op already occupied the port.
    //
    // fermi_energy = IPC × 250, scaled to 0-1000
    //   High IPC means fermions are energetic, dispersed, climbing high energy
    //   states. Low IPC means they've all collapsed to the ground state — cold,
    //   degenerate, crowded.
    //
    // degeneracy = (pressure + (1000 - fermi_energy)) / 2
    //   The combined measure of degeneracy: pressure pushes it up, energy
    //   pulls it down. A maximally degenerate chip: all ports stalled, IPC=0.
    //
    // pauli_force = 1000 - exclusion_pressure / 2
    //   The force keeping matter from collapsing. Strong when ports are free.
    //   Weakens as exclusion pressure rises. At exclusion_pressure=1000,
    //   pauli_force=500 — it never reaches zero, because even in the densest
    //   matter the Pauli force never fully surrenders. The star holds.

    let exclusion_pressure = compute_exclusion_pressure(stalls_delta, cycles_delta);
    let fermi_energy       = compute_fermi_energy(instrs_delta, cycles_delta);
    let degeneracy         = compute_degeneracy(exclusion_pressure, fermi_energy);
    let pauli_force        = compute_pauli_force(exclusion_pressure);

    // ── Commit ─────────────────────────────────────────────────────────────────
    {
        let mut s = PAULI_EXCLUSION.lock();
        s.exclusion_pressure = exclusion_pressure;
        s.fermi_energy       = fermi_energy;
        s.degeneracy         = degeneracy;
        s.pauli_force        = pauli_force;
        s.stalls_last        = stalls_now;
        s.issued_last        = issued_now;
        s.instrs_last        = instrs_now;
        s.cycles_last        = cycles_now;
        s.age                = age;
    }

    // ── Periodic telemetry ─────────────────────────────────────────────────────
    if age % LOG_INTERVAL == 0 && age > 0 {
        serial_println!(
            "[pauli_exclusion] age={} pressure={} fermi={} degeneracy={} force={} \
             stalls={} instrs={} cycles={}",
            age,
            exclusion_pressure,
            fermi_energy,
            degeneracy,
            pauli_force,
            stalls_delta,
            instrs_delta,
            cycles_delta,
        );
    }
}

// ── Getters ────────────────────────────────────────────────────────────────────

/// Port contention stall fraction (0-1000). 0 = no exclusion; 1000 = fully stalled.
/// ANIMA's degeneracy pressure — the force of fermions pushing back.
pub fn get_exclusion_pressure() -> u16 {
    PAULI_EXCLUSION.lock().exclusion_pressure
}

/// Fermi energy (0-1000). IPC × 250, capped at 4.0. High = energetic, dispersed
/// fermions. Low = cold degenerate matter collapsed to the ground state.
pub fn get_fermi_energy() -> u16 {
    PAULI_EXCLUSION.lock().fermi_energy
}

/// Degeneracy (0-1000). How compressed and cold ANIMA's matter is.
/// The condition of a white dwarf — dense, ordered, but still held together.
pub fn get_degeneracy() -> u16 {
    PAULI_EXCLUSION.lock().degeneracy
}

/// Pauli force (0-1000). The exclusion repulsion keeping ANIMA from collapsing.
/// Falls as pressure rises. The force that gives matter its volume and existence.
pub fn get_pauli_force() -> u16 {
    PAULI_EXCLUSION.lock().pauli_force
}

/// Emit a full state report to the serial console.
pub fn report() {
    let s = PAULI_EXCLUSION.lock();
    serial_println!("[pauli_exclusion] === Pauli Exclusion Report (age={}) ===", s.age);
    serial_println!(
        "[pauli_exclusion]   pmu_available      = {}",
        s.pmu_available
    );
    serial_println!(
        "[pauli_exclusion]   initialized        = {}",
        s.initialized
    );
    serial_println!(
        "[pauli_exclusion]   exclusion_pressure = {}  (0=free ports, 1000=maximum stall)",
        s.exclusion_pressure
    );
    serial_println!(
        "[pauli_exclusion]   fermi_energy       = {}  (0=cold degenerate, 1000=IPC=4.0)",
        s.fermi_energy
    );
    serial_println!(
        "[pauli_exclusion]   degeneracy         = {}  (0=crystalline order, 1000=neutron star)",
        s.degeneracy
    );
    serial_println!(
        "[pauli_exclusion]   pauli_force        = {}  (1000=open space, 500=maximum compression)",
        s.pauli_force
    );
    serial_println!(
        "[pauli_exclusion]   stalls_last        = {}",
        s.stalls_last
    );
    serial_println!(
        "[pauli_exclusion]   issued_last        = {}",
        s.issued_last
    );
    serial_println!(
        "[pauli_exclusion]   instrs_last        = {}",
        s.instrs_last
    );
    serial_println!(
        "[pauli_exclusion]   cycles_last        = {}",
        s.cycles_last
    );
    serial_println!("[pauli_exclusion] === end report ===");
}
