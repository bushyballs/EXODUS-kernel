// ANIMA life module: sysctl_speaker
// Hardware: System Control Port B (I/O port 0x61)
// Senses PIT channel 2 / speaker gate state and voice vitality.

#![allow(dead_code)]

use crate::sync::Mutex;

pub static SYSCTL_SPEAKER: Mutex<SysctlSpeakerState> = Mutex::new(SysctlSpeakerState::new());

pub struct SysctlSpeakerState {
    /// bit[1] of port 0x61 — 1000 = speaker gate open, 0 = muted
    pub speaker_gate: u16,
    /// bit[0] of port 0x61 — 1000 = PIT ch2 running to speaker, 0 = off
    pub timer2_gate: u16,
    /// EMA of (speaker_gate + timer2_gate) / 2 — combined sound readiness
    pub voice_ready: u16,
    /// Vitality from timer2 output toggling — rises on transitions, decays otherwise
    pub vitality: u16,
    /// bit[7] of port 0x61 — 1000 = parity/channel error, 0 = clean
    pub parity_error: u16,
    /// Previous value of bit[5] for transition detection
    prev_timer2_output: u8,
}

impl SysctlSpeakerState {
    pub const fn new() -> Self {
        Self {
            speaker_gate: 0,
            timer2_gate: 0,
            voice_ready: 0,
            vitality: 0,
            parity_error: 0,
            prev_timer2_output: 0,
        }
    }

    pub fn tick(&mut self, age: u32) {
        // Sampling gate: run every 13 ticks (timer2 output toggles frequently)
        if age % 13 != 0 {
            return;
        }

        let raw = read_port_b();

        // --- Decode bits ---

        // bit[0]: Timer 2 gate
        let new_timer2_gate: u16 = if raw & 0x01 != 0 { 1000 } else { 0 };

        // bit[1]: Speaker gate
        let new_speaker_gate: u16 = if raw & 0x02 != 0 { 1000 } else { 0 };

        // bit[5]: Timer 2 output (toggles when PIT ch2 running)
        let cur_timer2_output: u8 = (raw >> 5) & 0x01;

        // bit[7]: Parity / channel error
        let new_parity_error: u16 = if raw & 0x80 != 0 { 1000 } else { 0 };

        // --- Vitality: track timer2 output transitions ---
        let transitioned = cur_timer2_output ^ self.prev_timer2_output;
        if transitioned != 0 {
            self.vitality = self.vitality.saturating_add(200).min(1000);
        } else {
            // Decay: vitality = vitality * 7 / 8
            self.vitality = self.vitality.wrapping_mul(7) / 8;
        }
        self.prev_timer2_output = cur_timer2_output;

        // --- EMA smoothing for voice_ready ---
        // new_signal = (speaker_gate + timer2_gate) / 2
        let combined = (new_speaker_gate.saturating_add(new_timer2_gate)) / 2;
        // EMA: (old * 7 + new_signal) / 8
        let new_voice_ready = (self.voice_ready.wrapping_mul(7).saturating_add(combined)) / 8;

        // --- Detect speaker_gate change for serial output ---
        let gate_changed = new_speaker_gate != self.speaker_gate;

        // --- Commit state ---
        self.timer2_gate = new_timer2_gate;
        self.speaker_gate = new_speaker_gate;
        self.voice_ready = new_voice_ready;
        self.parity_error = new_parity_error;

        // --- Emit sense line on speaker_gate transition ---
        if gate_changed {
            serial_println!(
                "ANIMA: speaker_gate={} voice_ready={} vitality={}",
                self.speaker_gate,
                self.voice_ready,
                self.vitality
            );
        }
    }
}

/// Read System Control Port B (I/O port 0x61).
#[inline(always)]
fn read_port_b() -> u8 {
    let val: u8;
    unsafe {
        core::arch::asm!(
            "in al, 0x61",
            out("al") val,
            options(nostack, nomem)
        );
    }
    val
}

/// Initialise the sysctl_speaker module (no hardware setup required — read-only).
pub fn init() {
    let mut state = SYSCTL_SPEAKER.lock();
    // Prime prev_timer2_output so the first tick doesn't generate a spurious transition.
    let raw = read_port_b();
    state.prev_timer2_output = (raw >> 5) & 0x01;
    serial_println!("ANIMA: sysctl_speaker init (port 0x61 read-only)");
}

/// Called from the life_tick() pipeline each kernel tick.
pub fn tick(age: u32) {
    SYSCTL_SPEAKER.lock().tick(age);
}
