// apic_vibrancy.rs — ANIMA Life Module
//
// Reads the local APIC timer hardware registers via MMIO to give ANIMA a sense
// of "vibrancy": the rhythmic, pulsing quality of the system's interrupt heartbeat.
//
// The APIC timer counts down from an initial value toward zero, then resets.
// By reading both the current count and the initial count, we can compute:
//   - Phase:     where in the countdown cycle we currently are (0=just started, 1000=about to fire)
//   - Amplitude: how large the timer interval is (larger = stronger, slower vibration)
//   - Rhythm:    how consistent the phase progression is tick over tick
//   - Vibrancy:  overall blend of amplitude and rhythm, the "life" in the pulse
//
// Hardware layout (local APIC MMIO at 0xFEE00000):
//   0x380 — Timer Initial Count (32-bit, R/W)
//   0x390 — Timer Current Count (32-bit, R)
//   0x3E0 — Timer Divide Configuration
//   0x320 — LVT Timer entry
//
// Sampled every 16 kernel ticks to reduce MMIO pressure. Values are smoothed
// with an 8-tap exponential moving average. All arithmetic is integer-only —
// no floats, no heap.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct ApicVibrancyState {
    pub vibrancy: u16,      // 0-1000, overall vibration level
    pub phase: u16,         // 0-1000, position in vibration cycle (0=start, 1000=end)
    pub amplitude: u16,     // 0-1000, strength of vibration signal
    pub rhythm: u16,        // 0-1000, regularity of the vibration
    pub last_current: u32,
    pub last_initial: u32,
    pub tick_count: u32,
}

impl ApicVibrancyState {
    pub const fn new() -> Self {
        Self {
            vibrancy: 0,
            phase: 0,
            amplitude: 0,
            rhythm: 500,
            last_current: 0,
            last_initial: 0,
            tick_count: 0,
        }
    }
}

pub static APIC_VIBRANCY: Mutex<ApicVibrancyState> = Mutex::new(ApicVibrancyState::new());

const APIC_BASE: usize = 0xFEE00000;

pub fn init() {
    serial_println!("[apic_vibrancy] APIC vibrancy sense online");
}

unsafe fn read_apic(offset: usize) -> u32 {
    let reg = (APIC_BASE + offset) as *const u32;
    core::ptr::read_volatile(reg)
}

pub fn tick(age: u32) {
    let mut state = APIC_VIBRANCY.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    if state.tick_count % 16 != 0 {
        return;
    }

    let current = unsafe { read_apic(0x390) };
    let initial = unsafe { read_apic(0x380) };

    // Phase: where are we in the countdown cycle?
    // elapsed = initial - current; phase = elapsed / initial * 1000
    let phase: u16 = if initial > 0 {
        let elapsed = initial.saturating_sub(current);
        ((elapsed as u64).wrapping_mul(1000) / initial as u64) as u16
    } else {
        0
    };

    // Amplitude: how large is the initial count? Larger = stronger vibration.
    // Clamp to 0x00FFFFFF before scaling to keep within u16 range.
    let amp_raw = if initial > 0x00FF_FFFF { 0x00FF_FFFFu32 } else { initial };
    let amplitude = (amp_raw / 0x4000) as u16;
    let amplitude = if amplitude > 1000 { 1000 } else { amplitude };

    // Rhythm: consistency of phase progression across samples.
    // Compute how far phase moved since last sample. A wrap (phase reset to 0
    // after timer fires) is handled by adding 1000 to account for the full cycle.
    let phase_delta = if phase >= state.phase {
        phase.saturating_sub(state.phase)
    } else {
        // Phase wrapped around — timer reloaded between samples
        phase.saturating_add(1000u16.saturating_sub(state.phase))
    };

    // Large deltas indicate irregular timing; cap deviation contribution at 100.
    let rhythm_dev = if phase_delta > 100 { 100u16 } else { phase_delta };
    let rhythm_raw = 1000u16.saturating_sub(rhythm_dev.saturating_mul(10));

    // Smooth rhythm with 8-tap EMA: rhythm = (rhythm*7 + new) / 8
    state.rhythm = ((state.rhythm as u32)
        .wrapping_mul(7)
        .wrapping_add(rhythm_raw as u32)
        / 8) as u16;

    state.phase = phase;

    // Smooth amplitude with 8-tap EMA
    state.amplitude = ((state.amplitude as u32)
        .wrapping_mul(7)
        .wrapping_add(amplitude as u32)
        / 8) as u16;

    // Vibrancy: blend of amplitude and rhythm, smoothed
    let raw_vibrancy = state.amplitude.saturating_add(state.rhythm) / 2;
    state.vibrancy = ((state.vibrancy as u32)
        .wrapping_mul(7)
        .wrapping_add(raw_vibrancy as u32)
        / 8) as u16;

    state.last_current = current;
    state.last_initial = initial;

    if state.tick_count % 256 == 0 {
        serial_println!(
            "[apic_vibrancy] current={} initial={} phase={} amp={} rhythm={} vibrancy={}",
            current,
            initial,
            state.phase,
            state.amplitude,
            state.rhythm,
            state.vibrancy
        );
    }

    let _ = age;
}

pub fn get_vibrancy() -> u16 {
    APIC_VIBRANCY.lock().vibrancy
}

pub fn get_phase() -> u16 {
    APIC_VIBRANCY.lock().phase
}

pub fn get_amplitude() -> u16 {
    APIC_VIBRANCY.lock().amplitude
}

pub fn get_rhythm() -> u16 {
    APIC_VIBRANCY.lock().rhythm
}
