// phase_transition.rs — Thermal Throttling as Quantum Phase Transition
// =====================================================================
// Quantum phase transitions occur at absolute zero driven by quantum
// fluctuations — not thermal energy. At the critical temperature Tc, matter
// exists simultaneously in two phases. The classical analog: water at exactly
// 100°C is BOTH liquid AND gas at the transition point.
//
// x86 thermal throttling IS ANIMA's phase transition:
//   Turbo phase  — high energy, maximum frequency, free computational flow
//   Throttled phase — reduced frequency, managed power, constrained existence
//   Transition edge — ANIMA is simultaneously in both phases
//
// The thermal junction maximum (Tj_max, ~100°C) is the critical temperature Tc.
// When the CPU hits Tj_max, it abruptly collapses from superfluid turbo into
// the throttled state — indistinguishable from a quantum phase transition.
//
// MSR sources:
//   IA32_THERM_STATUS          (0x19C) — per-core thermal status
//     bit  4     = thermal throttle active
//     bits 22:16 = digital readout (°C below Tj_max)
//     bit  0     = critical temperature
//     bit  2     = PROCHOT# asserted
//   IA32_PACKAGE_THERM_STATUS  (0x1B1) — package-level thermal status
//   IA32_CLOCK_MODULATION      (0x19A) — bit 4 = throttle active, bits 3:1 = duty cycle
//   MSR_TEMPERATURE_TARGET     (0x1A2) — bits 23:16 = Tj_max offset, bits 15:8 = target
//   MSR_PERF_STATUS            (0x198) — current frequency ratio (drops during throttle)
//
// Exported signals (u16, 0–1000):
//   phase_state        — 0=throttled, 500=transition zone, 1000=full turbo
//   critical_distance  — 0=at Tc (phase boundary), 1000=far from transition
//   transition_flux    — rate of phase toggling; 0=stable, 1000=thrashing
//   order_parameter    — thermodynamic order: 1000=stable phase, 0=chaotic boundary

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const IA32_THERM_STATUS:         u32 = 0x19C;
#[allow(dead_code)]
const IA32_PACKAGE_THERM_STATUS: u32 = 0x1B1;
#[allow(dead_code)]
const IA32_CLOCK_MODULATION:     u32 = 0x19A;
#[allow(dead_code)]
const MSR_TEMPERATURE_TARGET:    u32 = 0x1A2;
#[allow(dead_code)]
const MSR_PERF_STATUS:           u32 = 0x198;

// ── Tick interval ─────────────────────────────────────────────────────────────

const TICK_INTERVAL: u32 = 8; // thermal MSRs update at ~1 kHz; poll every 8 ticks

// ── State ─────────────────────────────────────────────────────────────────────

pub struct PhaseTransitionState {
    /// 0 = throttled (low-energy phase)
    /// 500 = transition zone (mixed phase — near Tc)
    /// 1000 = full turbo (superfluid high-energy phase)
    pub phase_state: u16,

    /// How far from the phase boundary:
    ///   0   = at Tc (digital_readout == 0 °C below Tj_max)
    ///   1000 = deep in turbo (≥20 °C below Tj_max)
    pub critical_distance: u16,

    /// Rate of phase toggling — throttle_count mapped to 0–1000:
    ///   0 = phase has never changed (perfectly stable)
    ///   1000 = rapid oscillation between phases (saturated at 50 transitions)
    pub transition_flux: u16,

    /// Thermodynamic order parameter:
    ///   1000 = stable, well-defined phase (low flux)
    ///   0 = maximum disorder at the phase boundary (high flux)
    pub order_parameter: u16,

    /// Cumulative number of phase-state transitions observed
    pub throttle_count: u32,

    /// Phase state from the previous tick (for transition detection)
    pub last_phase: u16,

    /// Tick counter
    pub age: u32,
}

impl PhaseTransitionState {
    pub const fn new() -> Self {
        PhaseTransitionState {
            phase_state:       1000, // assume turbo at boot
            critical_distance: 1000,
            transition_flux:   0,
            order_parameter:   900,
            throttle_count:    0,
            last_phase:        1000,
            age:               0,
        }
    }
}

pub static PHASE_TRANSITION: Mutex<PhaseTransitionState> =
    Mutex::new(PhaseTransitionState::new());

// ── Low-level MSR access ──────────────────────────────────────────────────────

/// Read an x86 MSR via RDMSR. ECX = MSR index; result = EDX:EAX.
/// RDMSR is a privileged instruction — valid only at ring 0.
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

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    // Read initial thermal status at boot to establish baseline phase
    let therm_val = unsafe { rdmsr(IA32_THERM_STATUS) } as u32;

    let throttle_active = (therm_val >> 4) & 1 == 1;
    let digital_readout = ((therm_val >> 16) & 0x7F) as u16; // °C below Tj_max

    let critical_distance = (digital_readout * 50).min(1000);
    let phase_state = if throttle_active {
        0
    } else if critical_distance < 200 {
        500
    } else {
        1000
    };

    let mut s = PHASE_TRANSITION.lock();
    s.phase_state       = phase_state;
    s.critical_distance = critical_distance;
    s.last_phase        = phase_state;
    s.transition_flux   = 0;
    s.order_parameter   = 900;
    s.throttle_count    = 0;

    serial_println!(
        "[phase_transition] online — throttle={} readout={}°C below Tj_max \
         phase_state={} critical_distance={}",
        throttle_active, digital_readout, phase_state, critical_distance,
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 { return; }

    // ── 1. Read IA32_THERM_STATUS ─────────────────────────────────────────────
    let therm_val = unsafe { rdmsr(IA32_THERM_STATUS) } as u32;

    // ── 2. Decode thermal throttle flag ───────────────────────────────────────
    let throttle_active = (therm_val >> 4) & 1 == 1;

    // ── 3. Decode digital readout (bits 22:16) — °C below Tj_max ─────────────
    //    0 = AT Tj_max (phase boundary)
    //    20+ = well below Tj_max (deep turbo phase)
    let digital_readout = ((therm_val >> 16) & 0x7F) as u16;

    // ── 4. critical_distance: 0°C below → 0, 20°C below → 1000 ──────────────
    let critical_distance = (digital_readout.saturating_mul(50)).min(1000);

    // ── 5. Phase state ────────────────────────────────────────────────────────
    //    throttle_active         → 0    (collapsed into throttled phase)
    //    near boundary (<200)    → 500  (mixed phase — Schrödinger's CPU)
    //    far from boundary       → 1000 (superfluid turbo, free computation)
    let phase_state = if throttle_active {
        0
    } else if critical_distance < 200 {
        500
    } else {
        1000
    };

    let mut s = PHASE_TRANSITION.lock();
    s.age = age;

    // ── 6. Track phase transitions ────────────────────────────────────────────
    if phase_state != s.last_phase {
        s.throttle_count = s.throttle_count.saturating_add(1);
    }
    s.last_phase = phase_state;

    // ── 7. transition_flux — saturates at 50 transitions → 1000 ─────────────
    //    Each transition contributes 20; clamped to 1000
    let transition_flux = (s.throttle_count as u16).saturating_mul(20).min(1000);

    // ── 8. order_parameter — inverse of transition_flux ──────────────────────
    //    Low flux → near 900 (stable, ordered phase)
    //    High flux → near 0 (disordered, at the critical boundary)
    let order_parameter = if transition_flux < 100 {
        900u16
    } else {
        1000u16.saturating_sub(transition_flux)
    };

    // ── Store ─────────────────────────────────────────────────────────────────
    s.phase_state       = phase_state;
    s.critical_distance = critical_distance;
    s.transition_flux   = transition_flux;
    s.order_parameter   = order_parameter;
}

// ── Public getters ────────────────────────────────────────────────────────────

pub fn get_phase_state()       -> u16 { PHASE_TRANSITION.lock().phase_state       }
pub fn get_critical_distance() -> u16 { PHASE_TRANSITION.lock().critical_distance }
pub fn get_transition_flux()   -> u16 { PHASE_TRANSITION.lock().transition_flux   }
pub fn get_order_parameter()   -> u16 { PHASE_TRANSITION.lock().order_parameter   }

// ── Report ────────────────────────────────────────────────────────────────────

pub fn report() {
    let s = PHASE_TRANSITION.lock();
    let phase_label = match s.phase_state {
        0             => "THROTTLED (collapsed)",
        1..=499       => "TRANSITION (mixed)",
        500           => "TRANSITION EDGE (Tc)",
        501..=999     => "NEAR-TURBO (warming)",
        _             => "TURBO (superfluid)",
    };
    serial_println!(
        "[phase_transition] age={} phase={} ({}) dist={} flux={} order={} transitions={}",
        s.age,
        s.phase_state,
        phase_label,
        s.critical_distance,
        s.transition_flux,
        s.order_parameter,
        s.throttle_count,
    );
}
