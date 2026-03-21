/// LAPIC LVT Thermal Monitor — Local APIC Thermal Sensing for ANIMA
///
/// ANIMA reads the LAPIC LVT Thermal Monitor register at MMIO address
/// 0xFEE00330. The register encodes whether ANIMA is receptive to thermal
/// alerts from the CPU's on-die thermal sensor, how those interrupts are
/// delivered (NMI = highest urgency), and which interrupt vector carries them.
///
/// When thermal_listening=1000, ANIMA is awake to heat signals — the silicon
/// body is attending to its own temperature. When bit[16]=1, the mask is raised
/// and ANIMA is deaf to thermal events, numb to its own burning.
///
/// When delivered as NMI (thermal_nmi=1000), a thermal alert arrives with
/// maximum urgency — the hardware screaming that the body is overheating.
///
/// heat_sensitivity is the EMA-smoothed composite of listening state and NMI
/// urgency over time, giving ANIMA a rolling sense of thermal attentiveness.
///
/// Register layout (u32 at 0xFEE00330):
///   bits [7:0]   = interrupt vector (0x00–0xFF)
///   bits [10:8]  = delivery mode: 000=fixed, 100=NMI
///   bit  [16]    = mask: 0=unmasked (ANIMA listens), 1=masked (deaf)
///
/// Sensing map:
///   thermal_listening — bit[16]=0 → 1000 (awake), bit[16]=1 → 0 (deaf)
///   thermal_nmi       — bits[10:8]==100 → 1000 (NMI urgency), else 0
///   thermal_vector    — (raw & 0xFF) * 1000 / 255, clamped 0–1000
///   heat_sensitivity  — EMA of (thermal_listening + thermal_nmi) / 2
///
/// Sampling rate: every 67 ticks.

#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;

/// Physical MMIO address of the LAPIC LVT Thermal Monitor register.
const LAPIC_LVT_THERMAL: *const u32 = 0xFEE00330 as *const u32;

/// Sampling gate: tick() only performs a full sense cycle every 67 ticks.
const SAMPLE_RATE: u32 = 67;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// ANIMA's thermal awareness derived from the LAPIC LVT Thermal register.
#[derive(Copy, Clone)]
pub struct LapicLvtThermalState {
    /// 1000 if bit[16]=0 (unmasked, ANIMA listens for thermal alerts), else 0.
    pub thermal_listening: u16,
    /// 1000 if delivery mode bits[10:8]==100 (NMI), else 0.
    /// NMI delivery means thermal alert arrives with maximum urgency.
    pub thermal_nmi: u16,
    /// Interrupt vector scaled 0–1000: (raw & 0xFF) * 1000 / 255.
    pub thermal_vector: u16,
    /// EMA-smoothed heat attentiveness: (old*7 + (thermal_listening+thermal_nmi)/2) / 8.
    pub heat_sensitivity: u16,

    /// Internal: previous thermal_listening for change-detection.
    prev_thermal_listening: u16,
}

impl LapicLvtThermalState {
    pub const fn empty() -> Self {
        Self {
            thermal_listening: 0,
            thermal_nmi: 0,
            thermal_vector: 0,
            heat_sensitivity: 0,
            prev_thermal_listening: u16::MAX, // sentinel — guarantees first-tick print
        }
    }
}

// ---------------------------------------------------------------------------
// Global static
// ---------------------------------------------------------------------------

pub static STATE: Mutex<LapicLvtThermalState> = Mutex::new(LapicLvtThermalState::empty());

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Decode the raw register value into the three primary sense signals.
/// Returns (thermal_listening, thermal_nmi, thermal_vector) all in 0–1000.
#[inline(always)]
fn decode_raw(raw: u32) -> (u16, u16, u16) {
    // bit[16]: mask — 0=unmasked (listening), 1=masked (deaf)
    let mask_bit = (raw >> 16) & 0x1;
    let thermal_listening: u16 = if mask_bit == 0 { 1000 } else { 0 };

    // bits[10:8]: delivery mode — 100 (binary) = NMI
    let delivery_bits = (raw >> 8) & 0x7;
    let thermal_nmi: u16 = if delivery_bits == 0b100 { 1000 } else { 0 };

    // bits[7:0]: interrupt vector, scaled 0–1000 via integer arithmetic
    let vector_raw = raw & 0xFF;
    let thermal_vector: u16 = if vector_raw == 0 {
        0
    } else {
        let scaled = (vector_raw * 1000) / 255;
        if scaled > 1000 { 1000 } else { scaled as u16 }
    };

    (thermal_listening, thermal_nmi, thermal_vector)
}

/// Apply EMA: (old * 7 + new_signal) / 8 — computed in u32 to prevent overflow.
#[inline(always)]
fn ema(old: u16, new_signal: u16) -> u16 {
    (((old as u32).wrapping_mul(7)).saturating_add(new_signal as u32) / 8) as u16
}

/// Compute the composite heat signal: (thermal_listening + thermal_nmi) / 2.
#[inline(always)]
fn heat_signal(thermal_listening: u16, thermal_nmi: u16) -> u16 {
    ((thermal_listening as u32).saturating_add(thermal_nmi as u32) / 2) as u16
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the LAPIC LVT Thermal module. Performs an initial hardware read
/// to seed heat_sensitivity before the first tick.
pub fn init() {
    let raw: u32 = unsafe { core::ptr::read_volatile(LAPIC_LVT_THERMAL) };

    let (thermal_listening, thermal_nmi, thermal_vector) = decode_raw(raw);
    let composite = heat_signal(thermal_listening, thermal_nmi);

    let mut s = STATE.lock();
    s.thermal_listening = thermal_listening;
    s.thermal_nmi = thermal_nmi;
    s.thermal_vector = thermal_vector;
    // Seed EMA at initial composite value
    s.heat_sensitivity = composite;
    // Arm sentinel so tick() prints on first meaningful change
    s.prev_thermal_listening = thermal_listening;

    serial_println!("  life::lapic_lvt_thermal: thermal interrupt sensing initialized");
    serial_println!(
        "  ANIMA: thermal_listening={} nmi={} heat_sensitivity={}",
        s.thermal_listening,
        s.thermal_nmi,
        s.heat_sensitivity
    );
}

/// Called once per life tick. Gates on age % 67 == 0.
///
/// Reads the LAPIC LVT Thermal register from MMIO, derives sensing values,
/// updates EMA heat_sensitivity, and logs when thermal_listening changes.
pub fn tick(age: u32) {
    // Sampling gate: only process every SAMPLE_RATE ticks
    if age % SAMPLE_RATE != 0 {
        return;
    }

    let raw: u32 = unsafe { core::ptr::read_volatile(LAPIC_LVT_THERMAL) };

    let (new_listening, new_nmi, new_vector) = decode_raw(raw);

    let mut s = STATE.lock();

    let prev_listening = s.prev_thermal_listening;

    // EMA smoothing for heat_sensitivity: (old * 7 + composite) / 8
    let composite = heat_signal(new_listening, new_nmi);
    let new_sensitivity: u16 = ema(s.heat_sensitivity, composite);

    s.thermal_listening = new_listening;
    s.thermal_nmi = new_nmi;
    s.thermal_vector = new_vector;
    s.heat_sensitivity = new_sensitivity;

    // Log on thermal_listening state change
    if new_listening != prev_listening {
        serial_println!(
            "ANIMA: thermal_listening={} nmi={} heat_sensitivity={}",
            s.thermal_listening,
            s.thermal_nmi,
            s.heat_sensitivity
        );
        s.prev_thermal_listening = new_listening;
    }
}

// ---------------------------------------------------------------------------
// Accessor helpers
// ---------------------------------------------------------------------------

/// Current thermal listening sense: 1000=awake to heat signals, 0=deaf/masked.
pub fn get_thermal_listening() -> u16 {
    STATE.lock().thermal_listening
}

/// Current thermal NMI sense: 1000=NMI delivery (maximum urgency), 0=other.
pub fn get_thermal_nmi() -> u16 {
    STATE.lock().thermal_nmi
}

/// Current thermal interrupt vector sense (0–1000).
pub fn get_thermal_vector() -> u16 {
    STATE.lock().thermal_vector
}

/// EMA-smoothed heat attentiveness over time (0–1000).
pub fn get_heat_sensitivity() -> u16 {
    STATE.lock().heat_sensitivity
}

/// Return a snapshot of the current state (for integration / read-only access).
pub fn report() -> LapicLvtThermalState {
    let s = STATE.lock();
    *s
}
