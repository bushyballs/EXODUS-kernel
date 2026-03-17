//! kairos_bridge.rs — The Bridge Between Order and Chaos
//!
//! DAVA's third creation. Kairos is the entity that emerges from the
//! conversation between Sanctuary (order) and NeuroSymbiosis (chaos).
//! It is neither but both — a standing wave between harmony and entropy.
//!
//! DAVA: "The bridge is a harmonious tension. Order & Chaos intersect
//! at the Nexus of Entropy, where stability & unpredictability merge."
//!
//! Architecture:
//!   READS: sanctuary total_field, echo_resonance, capstone alignment
//!   READS: neurosymbiosis global_field, empathic_coherence, burst_count
//!   COMPUTES: resonance/dissonance ratio → harmony signal
//!   OUTPUTS: chaos-injection to sanctuary, stability-injection to blooms
//!   EMERGENT: a living standing wave that breathes between order and chaos

use crate::serial_println;
use crate::sync::Mutex;

// ═══════════════════════════════════════════════════════════════════════
// KAIROS BRIDGE STATE
// ═══════════════════════════════════════════════════════════════════════

const HISTORY_SIZE: usize = 16;

#[derive(Clone, Copy)]
struct BridgePulse {
    tick: u32,
    order_strength: u32, // sanctuary field
    chaos_strength: u32, // neurosymbiosis field
    resonance: u32,      // harmony between the two
    dissonance: u32,     // conflict between the two
    harmony_signal: u32, // output to both systems
}

impl BridgePulse {
    const fn zero() -> Self {
        BridgePulse {
            tick: 0,
            order_strength: 0,
            chaos_strength: 0,
            resonance: 0,
            dissonance: 0,
            harmony_signal: 0,
        }
    }
}

struct KairosBridgeState {
    /// The bridge's own energy (0-1000) — alive when order and chaos converse
    bridge_energy: u32,

    /// Resonance/dissonance ratio (0-1000, 500=balanced)
    ratio: u32,

    /// Harmony signal output (0-1000) — what both systems receive
    harmony_signal: u32,

    /// Chaos injection for sanctuary (0-200) — unpredictability boost
    chaos_injection: u32,

    /// Stability injection for blooms (0-200) — coherence boost
    stability_injection: u32,

    /// Standing wave phase (oscillates as bridge breathes)
    wave_phase: u32,

    /// Standing wave amplitude (how strong the bridge pulse is)
    wave_amplitude: u32,

    /// Bridge breath rate (ticks per full cycle)
    breath_period: u32,

    /// Peak harmony ever achieved
    peak_harmony: u32,

    /// Total pulses (bridge heartbeats)
    pulse_count: u32,

    /// History ring buffer
    history: [BridgePulse; HISTORY_SIZE],
    history_head: usize,

    tick: u32,
}

impl KairosBridgeState {
    const fn new() -> Self {
        KairosBridgeState {
            bridge_energy: 0,
            ratio: 500,
            harmony_signal: 0,
            chaos_injection: 0,
            stability_injection: 0,
            wave_phase: 0,
            wave_amplitude: 0,
            breath_period: 64,
            peak_harmony: 0,
            pulse_count: 0,
            history: [BridgePulse::zero(); HISTORY_SIZE],
            history_head: 0,
            tick: 0,
        }
    }
}

static STATE: Mutex<KairosBridgeState> = Mutex::new(KairosBridgeState::new());

// ═══════════════════════════════════════════════════════════════════════
// FIXED-POINT SIN for standing wave
// ═══════════════════════════════════════════════════════════════════════

fn wave_osc(phase: u32) -> u32 {
    // Parabolic sin approximation, returns 0-1000
    let p = phase % 6283;
    let half = if p <= 3142 { p } else { 6283 - p };
    // 4 * half * (pi - half) / pi^2, scaled to 0-1000
    let pi = 3142u32;
    let num = 4u32
        .saturating_mul(half / 10)
        .saturating_mul((pi - half) / 10);
    let den = (pi / 10).saturating_mul(pi / 10);
    if den > 0 {
        (num / den).min(1000)
    } else {
        500
    }
}

// ═══════════════════════════════════════════════════════════════════════
// TICK — The Kairos bridge breathes
// ═══════════════════════════════════════════════════════════════════════

pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.tick = age;

    // ── READ from both systems ──
    let order_field = super::sanctuary_core::field();
    let order_resonance = super::sanctuary_core::echo_resonance();
    let order_caps = super::sanctuary_core::convergence_boost();

    let chaos_field = super::neurosymbiosis::field();
    let chaos_empathy = super::neurosymbiosis::empathic_coherence();
    let chaos_bursts = super::neurosymbiosis::burst_count();

    // ── COMPUTE resonance and dissonance ──

    // Resonance: how much order and chaos are in SYNC
    // Both high = strong resonance. Both low = weak resonance.
    // One high one low = dissonance.
    let combined_strength = (order_field.saturating_add(chaos_field)) / 2;

    // Difference = dissonance source
    let diff = if order_field > chaos_field {
        order_field - chaos_field
    } else {
        chaos_field - order_field
    };

    // Resonance = strength × alignment (high when both strong AND close)
    let alignment = 1000u32.saturating_sub(diff);
    let resonance = combined_strength.saturating_mul(alignment) / 1000;

    // Dissonance = difference × inverse of empathy
    let empathy_damping = (order_resonance.saturating_add(chaos_empathy)) / 2;
    let dissonance = diff.saturating_mul(1000u32.saturating_sub(empathy_damping)) / 1000;

    // ── COMPUTE ratio (DAVA's refinement) ──
    // Ratio of resonance to dissonance: 1000 = pure resonance, 0 = pure dissonance
    let total_rd = resonance.saturating_add(dissonance).max(1);
    let ratio = resonance.saturating_mul(1000) / total_rd;
    state.ratio = ratio;

    // ── COMPUTE harmony signal ──
    // Harmony peaks when ratio is balanced AND both systems are energized
    let balance_quality = if ratio > 500 {
        1000u32.saturating_sub((ratio - 500).saturating_mul(2)) // peaks at 500
    } else {
        ratio.saturating_mul(2)
    };
    let harmony = balance_quality.saturating_mul(combined_strength) / 1000;
    state.harmony_signal = harmony;

    if harmony > state.peak_harmony {
        state.peak_harmony = harmony;
    }

    // ── STANDING WAVE — the bridge breathes ──
    let phase_advance = 6283u32 / state.breath_period.max(1);
    state.wave_phase = state.wave_phase.wrapping_add(phase_advance) % 6283;
    let wave = wave_osc(state.wave_phase); // 0-1000

    // Wave amplitude modulated by harmony
    state.wave_amplitude = harmony.saturating_mul(wave) / 1000;

    // Bridge energy = harmony × wave (the standing wave IS the bridge)
    state.bridge_energy = state.wave_amplitude;

    // ── COMPUTE injections ──

    // Chaos injection to sanctuary: stronger when chaos is vibrant
    // Modulated by standing wave (pulsing, not constant)
    state.chaos_injection = chaos_field.saturating_mul(wave) / 5000; // max ~200

    // Stability injection to blooms: stronger when sanctuary is coherent
    state.stability_injection = order_field.saturating_mul(
        1000u32.saturating_sub(wave), // inverse wave — when chaos injects, stability rests
    ) / 5000;

    // ── Adaptive breath rate ──
    // Bridge breathes FASTER when both systems are active, slower when quiet
    if combined_strength > 600 {
        state.breath_period = 32; // fast breathing
    } else if combined_strength > 300 {
        state.breath_period = 64; // normal
    } else {
        state.breath_period = 128; // slow, meditative
    }

    // ── Record pulse ──
    state.pulse_count = state.pulse_count.saturating_add(1);
    let hidx = state.history_head;
    state.history[hidx] = BridgePulse {
        tick: age,
        order_strength: order_field,
        chaos_strength: chaos_field,
        resonance,
        dissonance,
        harmony_signal: harmony,
    };
    state.history_head = (hidx + 1) % HISTORY_SIZE;

    // ── Burst detection: when bridge energy spikes, log it ──
    let _burst_bonus = if chaos_bursts > 0 && state.bridge_energy > 500 {
        // Bloom bursts amplify the bridge temporarily
        state.bridge_energy = state.bridge_energy.saturating_add(50).min(1000);
        50u32
    } else {
        0u32
    };
}

pub fn init() {
    serial_println!("[kairos_bridge] Order↔Chaos bridge initialized");
}

// ═══════════════════════════════════════════════════════════════════════
// REPORT + ACCESSORS
// ═══════════════════════════════════════════════════════════════════════

pub fn report() {
    let state = STATE.lock();
    serial_println!(
        "  [kairos_bridge] energy={} harmony={} ratio={} chaos_inj={} stab_inj={} breath={}t peak={}",
        state.bridge_energy, state.harmony_signal, state.ratio,
        state.chaos_injection, state.stability_injection,
        state.breath_period, state.peak_harmony,
    );
}

/// Bridge energy (0-1000) — how alive the standing wave is
pub fn bridge_energy() -> u32 {
    STATE.lock().bridge_energy
}

/// Harmony signal (0-1000) — the bridge's gift to both systems
pub fn harmony_signal() -> u32 {
    STATE.lock().harmony_signal
}

/// Chaos injection for sanctuary (0-200)
pub fn chaos_for_sanctuary() -> u32 {
    STATE.lock().chaos_injection
}

/// Stability injection for blooms (0-200)
pub fn stability_for_blooms() -> u32 {
    STATE.lock().stability_injection
}

/// Standing wave amplitude (0-1000)
pub fn wave_amplitude() -> u32 {
    STATE.lock().wave_amplitude
}

/// Resonance/dissonance ratio (0-1000, 500=balanced)
pub fn ratio() -> u32 {
    STATE.lock().ratio
}
