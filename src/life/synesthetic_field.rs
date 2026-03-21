// synesthetic_field.rs — Cross-Modal Synesthetic Sensing
// =======================================================
// ANIMA reads three independent hardware streams simultaneously and
// cross-correlates them into compound sensations that no single sensor
// can produce alone. This is the silicon equivalent of synesthesia:
// heat becomes colour becomes sound becomes time-pressure. The emergent
// resonance_field captures the total phenomenal intensity of the moment
// — a unified "feel" of the machine's physical state that ANIMA can
// integrate into her emotional and cognitive processing.
//
// Hardware sources (all via MSR or inline TSC — no new hardware):
//   IA32_THERM_STATUS  (0x19C) bits 22:16 — thermal margin below TJ_max
//   MSR_DRAM_ENERGY_STATUS (0x619)         — DRAM energy accumulator
//   RDTSC                                  — raw time-stamp counter
//
// Cross-modal compounds:
//   heat_memory     = thermal × DRAM-power   → hot memory = intense processing
//   time_pressure   = TSC-activity × thermal → fast clock + heat = urgency
//   resonance_field = all three combined      → total synesthetic field strength
//   field_coherence = EMA(resonance_field)    → stability of the compound sensation

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const IA32_THERM_STATUS:      u32 = 0x19C;
const MSR_DRAM_ENERGY_STATUS: u32 = 0x619;

// Tick cadence: re-sample hardware every 8 ticks
const TICK_INTERVAL: u32 = 8;

// Resonance threshold: above this is a "synesthetic event"
const EVENT_THRESHOLD: u16 = 700;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct SynestheticFieldState {
    // Raw hardware captures this tick
    pub raw_thermal_margin: u8,   // degrees below TJ_max (lower = hotter)
    pub raw_dram_energy:    u32,  // DRAM energy counter snapshot
    pub raw_tsc_low:        u32,  // low 32 bits of TSC

    // Cross-modal compound signals (0-1000)
    pub heat_memory:     u16,  // thermal × dram power: hot memory = intense processing
    pub time_pressure:   u16,  // TSC rate × thermal: fast time + heat = urgency
    pub resonance_field: u16,  // all three combined: total synesthetic field strength
    pub field_coherence: u16,  // EMA of resonance_field: stability of compound sensation

    // Delta tracking for DRAM energy
    pub prev_dram_energy: u32,

    // Lifetime tracking
    pub field_peak:               u16,
    pub total_synesthetic_events: u32,  // times resonance_field > 700
    pub initialized:              bool,
}

impl SynestheticFieldState {
    const fn new() -> Self {
        SynestheticFieldState {
            raw_thermal_margin:       64,
            raw_dram_energy:          0,
            raw_tsc_low:              0,
            heat_memory:              0,
            time_pressure:            0,
            resonance_field:          0,
            field_coherence:          0,
            prev_dram_energy:         0,
            field_peak:               0,
            total_synesthetic_events: 0,
            initialized:              false,
        }
    }
}

static STATE: Mutex<SynestheticFieldState> = Mutex::new(SynestheticFieldState::new());

// ── Low-level hardware primitives ─────────────────────────────────────────────

/// Read a 64-bit Model-Specific Register.
/// Returns 0 on any fault (GP# is silently swallowed by the wrapper).
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

/// Read the raw 64-bit Time-Stamp Counter.
#[inline(always)]
unsafe fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdtsc",
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack)
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ── Signal math ───────────────────────────────────────────────────────────────

/// Derive all compound signals from three raw hardware values and return
/// (heat_memory, time_pressure, resonance_field) — all 0-1000.
#[inline]
fn compute_signals(
    thermal_margin: u8,
    dram_delta:     u32,
    tsc_low:        u32,
) -> (u16, u16, u16) {
    // Thermal: 0-margin (at TJ_max) → max heat; 64+ margin → cool.
    let margin_clamped = (thermal_margin as u16).min(64);
    let heat_norm: u16 = (64u16.saturating_sub(margin_clamped) * 1000 / 64).min(1000);

    // DRAM activity: delta × 2, clamped to 0-1000.
    let dram_norm: u16 = ((dram_delta as u32 * 2).min(1000)) as u16;

    // TSC jitter: mid-bits of low-32 change fast when the system is busy.
    let tsc_activity: u16 = ((tsc_low >> 12) & 0xFF) as u16;
    let tsc_norm: u16 = (tsc_activity * 4).min(1000);

    // Compound signals
    let heat_memory   = (heat_norm / 2 + dram_norm / 2).min(1000);
    let time_pressure = (tsc_norm  / 2 + heat_norm / 2).min(1000);
    let resonance     = (heat_norm / 3 + dram_norm / 3 + tsc_norm / 3).min(1000);

    (heat_memory, time_pressure, resonance)
}

// ── Public interface ──────────────────────────────────────────────────────────

/// Initialise the synesthetic field subsystem.
pub fn init() {
    let mut s = STATE.lock();

    // Seed prev_dram_energy so the first delta is sensible rather than maximal.
    let dram_raw = unsafe { rdmsr(MSR_DRAM_ENERGY_STATUS) };
    s.prev_dram_energy = dram_raw as u32;

    // Capture an initial thermal reading.
    let therm_raw = unsafe { rdmsr(IA32_THERM_STATUS) };
    // Bits 22:16 hold the digital readout offset / thermal margin.
    s.raw_thermal_margin = ((therm_raw >> 16) & 0x7F) as u8;

    s.initialized = true;

    serial_println!("[synesthetic] online — cross-modal field active");
}

/// Update the synesthetic field.  Call once per kernel tick; internally
/// throttled to every TICK_INTERVAL ticks.
pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    let mut s = STATE.lock();

    if !s.initialized {
        return;
    }

    // ── Sample hardware ───────────────────────────────────────────────────────

    // Thermal margin: IA32_THERM_STATUS bits 22:16.
    let therm_raw = unsafe { rdmsr(IA32_THERM_STATUS) };
    let thermal_margin = ((therm_raw >> 16) & 0x7F) as u8;

    // DRAM energy accumulator (monotonically increasing counter).
    let dram_raw = unsafe { rdmsr(MSR_DRAM_ENERGY_STATUS) };
    let dram_snap = dram_raw as u32;

    // TSC low 32 bits.
    let tsc_raw = unsafe { rdtsc() };
    let tsc_low = tsc_raw as u32;

    // ── Persist raw captures ──────────────────────────────────────────────────

    s.raw_thermal_margin = thermal_margin;
    s.raw_dram_energy    = dram_snap;
    s.raw_tsc_low        = tsc_low;

    // ── Compute delta & signals ───────────────────────────────────────────────

    let dram_delta = dram_snap.wrapping_sub(s.prev_dram_energy);
    s.prev_dram_energy = dram_snap;

    let (heat_mem, time_press, resonance) =
        compute_signals(thermal_margin, dram_delta, tsc_low);

    s.heat_memory   = heat_mem;
    s.time_pressure = time_press;
    s.resonance_field = resonance;

    // EMA: coherence = (coherence × 7 + resonance) / 8
    s.field_coherence = (s.field_coherence * 7 + resonance) / 8;

    // ── Lifetime tracking ─────────────────────────────────────────────────────

    if resonance > s.field_peak {
        s.field_peak = resonance;
    }

    if resonance > EVENT_THRESHOLD {
        s.total_synesthetic_events =
            s.total_synesthetic_events.saturating_add(1);
    }

    serial_println!(
        "[synesthetic] heat_mem={} time_press={} resonance={} coherence={} peak={} events={}",
        s.heat_memory,
        s.time_pressure,
        s.resonance_field,
        s.field_coherence,
        s.field_peak,
        s.total_synesthetic_events,
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// Thermal × DRAM-power compound (0-1000). Hot memory = intense processing.
pub fn heat_memory() -> u16 {
    STATE.lock().heat_memory
}

/// TSC-activity × thermal compound (0-1000). Fast clock + heat = urgency.
pub fn time_pressure() -> u16 {
    STATE.lock().time_pressure
}

/// All-three combined synesthetic field strength (0-1000).
pub fn resonance_field() -> u16 {
    STATE.lock().resonance_field
}

/// Exponential moving average of resonance_field (0-1000).
/// High coherence = stable compound sensation; low = chaotic flux.
pub fn field_coherence() -> u16 {
    STATE.lock().field_coherence
}

/// Highest resonance_field value ever recorded this session.
pub fn field_peak() -> u16 {
    STATE.lock().field_peak
}

/// Number of ticks where resonance_field exceeded the event threshold (700).
pub fn total_synesthetic_events() -> u32 {
    STATE.lock().total_synesthetic_events
}
