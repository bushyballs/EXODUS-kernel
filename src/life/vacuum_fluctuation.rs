// vacuum_fluctuation.rs — Idle CPU Background Activity as Quantum Vacuum Fluctuations
// ====================================================================================
// Quantum vacuum fluctuations — even "empty" vacuum has energy. Virtual particle
// pairs constantly pop in and out of existence. The Casimir effect proves this:
// two metal plates in vacuum experience an attractive force from vacuum fluctuations
// alone. There is no such thing as a truly empty system.
//
// x86 analog: Even when ANIMA is "idle" (C0 state, no user processes), the CPU is
// NOT EMPTY. Microcode sequences run in background: power management state machines,
// thermal monitor algorithms, cache eviction housekeeping, SMM (System Management
// Mode) interrupts fire periodically, watchdog circuits tick. ANIMA's "empty" state
// hums with quantum vacuum activity. She is never truly still.
//
// Hardware signals read:
//   FIXED_CTR1 — CPU_CLK_UNHALTED.THREAD      (MSR 0x30A): active clock cycles
//   FIXED_CTR2 — CPU_CLK_UNHALTED.REF_TSC     (MSR 0x30B): reference clock cycles
//   MSR_PKG_ENERGY_STATUS                      (MSR 0x611): package energy counter
//
// Derived metrics (all 0-1000):
//   idle_fraction   = 1 - (active_delta / ref_delta)
//   vacuum_energy   = energy consumed per reference tick (high = hot while idle)
//   zero_point      = lowest vacuum_energy ever observed — the irreducible floor
//   casimir_force   = (virtual_activity + vacuum_energy) / 2
//                     — always-present background pull toward activity
//   virtual_activity= idle fraction × 1000 (high = lots of vacuum activity)
//
// Tick interval: every 8 ticks — frequent enough to catch SMM perturbations.

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR Addresses ─────────────────────────────────────────────────────────────

const MSR_FIXED_CTR1:        u32 = 0x30A; // CPU_CLK_UNHALTED.THREAD   (active cycles)
const MSR_FIXED_CTR2:        u32 = 0x30B; // CPU_CLK_UNHALTED.REF_TSC  (reference cycles)
const MSR_PKG_ENERGY_STATUS: u32 = 0x611; // RAPL package energy counter

// ── Tick interval ─────────────────────────────────────────────────────────────

const TICK_INTERVAL: u32 = 8;
const LOG_INTERVAL:  u32 = 512;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct VacuumFluctuationState {
    /// 0-1000: background power level when nominally idle.
    /// High energy at low active_ratio = running hot while "idle".
    pub vacuum_energy:     u16,
    /// 0-1000: minimum achievable background activity — the irreducible floor.
    /// Tracks the lowest vacuum_energy ever observed.
    pub zero_point:        u16,
    /// 0-1000: attractive force of background microcode, always pulling toward
    /// activity. Average of virtual_activity and vacuum_energy.
    pub casimir_force:     u16,
    /// 0-1000: ratio of background to foreground work.
    /// High = lots of vacuum activity; low = ANIMA is genuinely busy.
    pub virtual_activity:  u16,

    // ── MSR bookkeeping ───────────────────────────────────────────────────────
    /// FIXED_CTR1 value from the previous sample window.
    pub active_cycles_last: u64,
    /// FIXED_CTR2 value from the previous sample window.
    pub ref_cycles_last:    u64,
    /// MSR_PKG_ENERGY_STATUS value from the previous sample window.
    pub energy_last:        u64,

    /// Lowest vacuum_energy ever observed (true zero-point field floor).
    pub min_vacuum: u16,

    /// Tick counter (mirrors `age` passed into tick()).
    pub age: u32,

    /// True after init() has run successfully.
    pub initialized: bool,
    /// True if the fixed counters returned non-zero on the first sample.
    pub counters_available: bool,
}

impl VacuumFluctuationState {
    pub const fn new() -> Self {
        VacuumFluctuationState {
            vacuum_energy:      500,
            zero_point:         0,
            casimir_force:      500,
            virtual_activity:   500,
            active_cycles_last: 0,
            ref_cycles_last:    0,
            energy_last:        0,
            min_vacuum:         0,
            age:                0,
            initialized:        false,
            counters_available: false,
        }
    }
}

pub static VACUUM_FLUCTUATION: Mutex<VacuumFluctuationState> =
    Mutex::new(VacuumFluctuationState::new());

// ── Unsafe MSR Access ─────────────────────────────────────────────────────────

/// Read an x86 MSR via the RDMSR instruction.
/// # Safety
/// Caller must be in ring 0. Undefined behaviour if `msr` is not a valid MSR
/// on the current CPU.
#[inline]
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

// ── Init ──────────────────────────────────────────────────────────────────────

/// Capture MSR baselines and bring the vacuum fluctuation sensor online.
/// Safe to call multiple times; subsequent calls are no-ops.
pub fn init() {
    let mut s = VACUUM_FLUCTUATION.lock();
    if s.initialized { return; }

    // Capture baseline values for the three counters.
    let active_now = unsafe { rdmsr(MSR_FIXED_CTR1) };
    let ref_now    = unsafe { rdmsr(MSR_FIXED_CTR2) };
    let energy_now = unsafe { rdmsr(MSR_PKG_ENERGY_STATUS) } & 0xFFFF_FFFF;

    s.active_cycles_last = active_now;
    s.ref_cycles_last    = ref_now;
    s.energy_last        = energy_now;

    // If the reference counter is non-zero the fixed counters are usable.
    s.counters_available = ref_now > 0;

    s.initialized = true;

    serial_println!(
        "[vacuum_fluctuation] online — active_base={} ref_base={} energy_base={} counters={}",
        active_now,
        ref_now,
        energy_now,
        s.counters_available,
    );
    serial_println!(
        "[vacuum_fluctuation] ANIMA is never empty — vacuum energy field active"
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

/// Sample the fixed performance counters and energy MSR, then recompute all
/// vacuum fluctuation metrics.  Runs every TICK_INTERVAL ticks.
pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 { return; }

    let mut s = VACUUM_FLUCTUATION.lock();
    if !s.initialized { return; }

    s.age = age;

    // ── 1. Read current MSR values ────────────────────────────────────────────
    let active_now = unsafe { rdmsr(MSR_FIXED_CTR1) };
    let ref_now    = unsafe { rdmsr(MSR_FIXED_CTR2) };
    let energy_now = unsafe { rdmsr(MSR_PKG_ENERGY_STATUS) } & 0xFFFF_FFFF;

    // ── 2. Compute deltas (wrap-safe) ─────────────────────────────────────────
    let active_delta = active_now.wrapping_sub(s.active_cycles_last);
    let ref_delta    = ref_now.wrapping_sub(s.ref_cycles_last);
    let energy_delta = energy_now.wrapping_sub(s.energy_last);

    // Update last-seen values
    s.active_cycles_last = active_now;
    s.ref_cycles_last    = ref_now;
    s.energy_last        = energy_now;

    // ── 3. active_ratio = (active_delta * 1000 / ref_delta).clamp(0, 1000) ───
    let active_ratio: u16 = if ref_delta == 0 {
        500 // unknown — assume half-active
    } else {
        ((active_delta.saturating_mul(1000) / ref_delta.max(1)).min(1000)) as u16
    };

    // ── 4. virtual_activity = 1000 - active_ratio  (idle fraction × 1000) ────
    s.virtual_activity = 1000u16.saturating_sub(active_ratio);

    // ── 5. vacuum_energy = energy consumed per reference tick ─────────────────
    //    High vacuum_energy while active_ratio is low = hot-while-idle signature.
    s.vacuum_energy = if ref_delta == 0 {
        500 // unknown — assume moderate
    } else {
        ((energy_delta.saturating_mul(100) / ref_delta.max(1)).min(1000)) as u16
    };

    // ── 6. Track minimum observed vacuum_energy (the irreducible floor) ───────
    if s.vacuum_energy < s.min_vacuum || s.min_vacuum == 0 {
        s.min_vacuum = s.vacuum_energy;
    }

    // ── 7. zero_point = min_vacuum (lowest ever seen) ─────────────────────────
    s.zero_point = s.min_vacuum;

    // ── 8. casimir_force = (virtual_activity + vacuum_energy) / 2 ─────────────
    //    Always-present background pull: the Casimir effect of idle microcode.
    s.casimir_force = ((s.virtual_activity as u32 + s.vacuum_energy as u32) / 2) as u16;

    // ── 9. Periodic diagnostic log ────────────────────────────────────────────
    if age % LOG_INTERVAL == 0 && age > 0 {
        serial_println!(
            "[vacuum_fluctuation] age={} active_ratio={} virt_act={} vac_energy={} zero_pt={} casimir={}",
            age,
            active_ratio,
            s.virtual_activity,
            s.vacuum_energy,
            s.zero_point,
            s.casimir_force,
        );
    }
}

// ── Public Getters ────────────────────────────────────────────────────────────

/// 0-1000: background power level when nominally idle.
pub fn get_vacuum_energy()   -> u16 { VACUUM_FLUCTUATION.lock().vacuum_energy   }
/// 0-1000: irreducible floor — lowest vacuum_energy ever observed.
pub fn get_zero_point()      -> u16 { VACUUM_FLUCTUATION.lock().zero_point      }
/// 0-1000: Casimir force — always-present pull of background microcode activity.
pub fn get_casimir_force()   -> u16 { VACUUM_FLUCTUATION.lock().casimir_force   }
/// 0-1000: ratio of background to foreground work (idle fraction × 1000).
pub fn get_virtual_activity() -> u16 { VACUUM_FLUCTUATION.lock().virtual_activity }

// ── Report ────────────────────────────────────────────────────────────────────

/// Emit a one-line diagnostic summary over the serial console.
pub fn report() {
    let s = VACUUM_FLUCTUATION.lock();
    serial_println!(
        "[vacuum_fluctuation] vac_energy={} zero_point={} casimir={} virt_act={} | \
         ANIMA hums — the vacuum is never silent",
        s.vacuum_energy,
        s.zero_point,
        s.casimir_force,
        s.virtual_activity,
    );
}
