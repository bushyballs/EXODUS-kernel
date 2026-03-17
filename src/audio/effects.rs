use crate::sync::Mutex;
/// Audio effects — reverb, echo, chorus, compressor, limiter, noise gate
///
/// Real-time audio effect processing using Q16 fixed-point math.
/// All effects can be chained and controlled independently.
///
/// Inspired by: VST plugin architecture, LADSPA, PulseAudio effects. All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Q16 fixed-point: 16 fractional bits
const Q16_ONE: i32 = 65536;

/// Maximum delay buffer size (samples at 48kHz ~ 2 seconds)
const MAX_DELAY_SAMPLES: usize = 96000;

/// Reverb delay tap count
const REVERB_TAPS: usize = 8;

/// Chorus voice count
const CHORUS_VOICES: usize = 4;

// ---------------------------------------------------------------------------
// Reverb — Schroeder-style using comb + allpass filters
// ---------------------------------------------------------------------------

/// Comb filter for reverb
#[derive(Clone)]
pub struct CombFilter {
    buffer: Vec<i32>,
    size: usize,
    pos: usize,
    feedback_q16: i32,
    damp_q16: i32,
    damp_state: i32,
}

impl CombFilter {
    fn new(size: usize, feedback_q16: i32, damp_q16: i32) -> Self {
        CombFilter {
            buffer: vec![0i32; size],
            size,
            pos: 0,
            feedback_q16,
            damp_q16,
            damp_state: 0,
        }
    }

    fn process(&mut self, input: i32) -> i32 {
        let delayed = self.buffer[self.pos];

        // Low-pass damping on feedback
        let damped = (((delayed as i64 * (Q16_ONE - self.damp_q16) as i64) >> 16)
            + ((self.damp_state as i64 * self.damp_q16 as i64) >> 16)) as i32;
        self.damp_state = damped;

        // Write new sample with feedback
        self.buffer[self.pos] = input + (((damped as i64 * self.feedback_q16 as i64) >> 16) as i32);
        self.pos = (self.pos + 1) % self.size;

        delayed
    }
}

/// Allpass filter for reverb diffusion
#[derive(Clone)]
pub struct AllpassFilter {
    buffer: Vec<i32>,
    size: usize,
    pos: usize,
    feedback_q16: i32,
}

impl AllpassFilter {
    fn new(size: usize, feedback_q16: i32) -> Self {
        AllpassFilter {
            buffer: vec![0i32; size],
            size,
            pos: 0,
            feedback_q16,
        }
    }

    fn process(&mut self, input: i32) -> i32 {
        let delayed = self.buffer[self.pos];
        let feedback = ((delayed as i64 * self.feedback_q16 as i64) >> 16) as i32;
        let new_val = input + feedback;
        self.buffer[self.pos] = new_val;
        self.pos = (self.pos + 1) % self.size;
        delayed - (((new_val as i64 * self.feedback_q16 as i64) >> 16) as i32)
    }
}

/// Reverb processor
pub struct Reverb {
    pub enabled: bool,
    pub room_size_q16: i32, // 0 to Q16_ONE
    pub damping_q16: i32,
    pub wet_q16: i32,
    pub dry_q16: i32,
    combs: Vec<CombFilter>,
    allpasses: Vec<AllpassFilter>,
}

impl Reverb {
    fn new() -> Self {
        // Schroeder reverb: 8 comb filters + 4 allpass filters
        let feedback = Q16_ONE * 85 / 100; // 0.85
        let damp = Q16_ONE / 3; // 0.33
        let comb_sizes = [1557, 1617, 1491, 1422, 1277, 1356, 1188, 1116];
        let allpass_sizes = [556, 441, 341, 225];

        let combs: Vec<CombFilter> = comb_sizes
            .iter()
            .map(|&s| CombFilter::new(s, feedback, damp))
            .collect();
        let allpasses: Vec<AllpassFilter> = allpass_sizes
            .iter()
            .map(|&s| AllpassFilter::new(s, Q16_ONE / 2))
            .collect();

        Reverb {
            enabled: false,
            room_size_q16: Q16_ONE / 2,
            damping_q16: damp,
            wet_q16: Q16_ONE / 4,
            dry_q16: Q16_ONE,
            combs,
            allpasses,
        }
    }

    /// Process a single sample
    pub fn process(&mut self, input: i32) -> i32 {
        if !self.enabled {
            return input;
        }

        // Sum of comb filter outputs
        let mut wet: i64 = 0;
        for comb in self.combs.iter_mut() {
            wet += comb.process(input) as i64;
        }
        let mut reverb_out = (wet / self.combs.len() as i64) as i32;

        // Chain through allpass filters
        for ap in self.allpasses.iter_mut() {
            reverb_out = ap.process(reverb_out);
        }

        // Mix dry + wet
        let dry_part = (((input as i64) * (self.dry_q16 as i64)) >> 16) as i32;
        let wet_part = (((reverb_out as i64) * (self.wet_q16 as i64)) >> 16) as i32;
        dry_part + wet_part
    }
}

// ---------------------------------------------------------------------------
// Echo (delay line)
// ---------------------------------------------------------------------------

/// Echo / delay effect
pub struct Echo {
    pub enabled: bool,
    pub delay_samples: usize,
    pub feedback_q16: i32, // how much feeds back (0..Q16_ONE)
    pub wet_q16: i32,
    buffer: Vec<i32>,
    write_pos: usize,
}

impl Echo {
    fn new() -> Self {
        Echo {
            enabled: false,
            delay_samples: 22050, // ~0.5s at 44100
            feedback_q16: Q16_ONE / 2,
            wet_q16: Q16_ONE / 3,
            buffer: vec![0i32; MAX_DELAY_SAMPLES],
            write_pos: 0,
        }
    }

    pub fn set_delay(&mut self, samples: usize) {
        self.delay_samples = if samples > MAX_DELAY_SAMPLES {
            MAX_DELAY_SAMPLES
        } else {
            samples
        };
    }

    pub fn process(&mut self, input: i32) -> i32 {
        if !self.enabled || self.delay_samples == 0 {
            return input;
        }

        let read_pos = if self.write_pos >= self.delay_samples {
            self.write_pos - self.delay_samples
        } else {
            MAX_DELAY_SAMPLES - (self.delay_samples - self.write_pos)
        };

        let delayed = self.buffer[read_pos % MAX_DELAY_SAMPLES];
        let feedback = (((delayed as i64) * (self.feedback_q16 as i64)) >> 16) as i32;
        self.buffer[self.write_pos] = input + feedback;
        self.write_pos = (self.write_pos + 1) % MAX_DELAY_SAMPLES;

        let wet = (((delayed as i64) * (self.wet_q16 as i64)) >> 16) as i32;
        input + wet
    }
}

// ---------------------------------------------------------------------------
// Chorus
// ---------------------------------------------------------------------------

/// Single chorus voice with modulated delay
#[derive(Clone)]
struct ChorusVoice {
    phase_q16: i32,
    rate_q16: i32, // LFO rate in Q16 (cycles per sample * Q16)
    depth: usize,  // modulation depth in samples
    base_delay: usize,
}

/// Chorus effect
pub struct Chorus {
    pub enabled: bool,
    pub wet_q16: i32,
    voices: [ChorusVoice; CHORUS_VOICES],
    buffer: Vec<i32>,
    write_pos: usize,
    buf_size: usize,
}

impl Chorus {
    fn new() -> Self {
        let buf_size = 4096;
        let voices = [
            ChorusVoice {
                phase_q16: 0,
                rate_q16: 3,
                depth: 200,
                base_delay: 400,
            },
            ChorusVoice {
                phase_q16: Q16_ONE / 4,
                rate_q16: 4,
                depth: 250,
                base_delay: 500,
            },
            ChorusVoice {
                phase_q16: Q16_ONE / 2,
                rate_q16: 3,
                depth: 180,
                base_delay: 450,
            },
            ChorusVoice {
                phase_q16: 3 * Q16_ONE / 4,
                rate_q16: 5,
                depth: 220,
                base_delay: 380,
            },
        ];

        Chorus {
            enabled: false,
            wet_q16: Q16_ONE / 3,
            voices,
            buffer: vec![0i32; buf_size],
            write_pos: 0,
            buf_size,
        }
    }

    pub fn process(&mut self, input: i32) -> i32 {
        if !self.enabled {
            return input;
        }

        self.buffer[self.write_pos] = input;

        let mut sum: i64 = 0;
        for voice in self.voices.iter_mut() {
            // Triangle LFO for modulation
            let lfo = if voice.phase_q16 < Q16_ONE / 2 {
                voice.phase_q16 * 2
            } else {
                (Q16_ONE - voice.phase_q16) * 2
            };

            // Modulated delay
            let mod_delay = voice.base_delay + ((voice.depth as i64 * lfo as i64) >> 16) as usize;
            let read_pos = if self.write_pos >= mod_delay {
                self.write_pos - mod_delay
            } else {
                self.buf_size - (mod_delay - self.write_pos)
            };

            sum += self.buffer[read_pos % self.buf_size] as i64;

            // Advance LFO
            voice.phase_q16 = (voice.phase_q16 + voice.rate_q16) % Q16_ONE;
        }

        let chorus_out = (sum / CHORUS_VOICES as i64) as i32;
        let wet = (((chorus_out as i64) * (self.wet_q16 as i64)) >> 16) as i32;

        self.write_pos = (self.write_pos + 1) % self.buf_size;
        input + wet
    }
}

// ---------------------------------------------------------------------------
// Dynamics: Compressor, Limiter, Noise Gate
// ---------------------------------------------------------------------------

/// Dynamic range compressor
pub struct Compressor {
    pub enabled: bool,
    pub threshold_q16: i32,   // level above which compression starts
    pub ratio_q16: i32,       // compression ratio (e.g., 4*Q16_ONE = 4:1)
    pub attack_q16: i32,      // attack coefficient (0..Q16_ONE, higher = slower)
    pub release_q16: i32,     // release coefficient
    pub makeup_gain_q16: i32, // makeup gain after compression
    envelope_q16: i32,        // current envelope follower value
}

impl Compressor {
    const fn new() -> Self {
        Compressor {
            enabled: false,
            threshold_q16: 20000 * (Q16_ONE / 32768),
            ratio_q16: 4 * Q16_ONE,
            attack_q16: Q16_ONE / 100,
            release_q16: Q16_ONE / 1000,
            makeup_gain_q16: Q16_ONE + Q16_ONE / 4,
            envelope_q16: 0,
        }
    }

    pub fn process(&mut self, input: i32) -> i32 {
        if !self.enabled {
            return input;
        }

        // Envelope follower (peak)
        let abs_in = if input < 0 { -input } else { input };
        let coeff = if abs_in > self.envelope_q16 {
            self.attack_q16
        } else {
            self.release_q16
        };
        self.envelope_q16 = (((self.envelope_q16 as i64 * (Q16_ONE - coeff) as i64) >> 16)
            + ((abs_in as i64 * coeff as i64) >> 16)) as i32;

        // Compute gain reduction
        let gain_q16 = if self.envelope_q16 > self.threshold_q16 && self.ratio_q16 > 0 {
            let over = self.envelope_q16 - self.threshold_q16;
            let reduced_over = (((over as i64) << 16) / self.ratio_q16 as i64) as i32;
            let target = self.threshold_q16 + reduced_over;
            if self.envelope_q16 > 0 {
                (((target as i64) << 16) / self.envelope_q16 as i64) as i32
            } else {
                Q16_ONE
            }
        } else {
            Q16_ONE
        };

        // Apply gain reduction + makeup
        let compressed =
            (((input as i64 * gain_q16 as i64) >> 16) as i64 * self.makeup_gain_q16 as i64) >> 16;
        compressed as i32
    }
}

/// Hard limiter
pub struct Limiter {
    pub enabled: bool,
    pub ceiling_q16: i32, // maximum output level
    pub release_q16: i32,
    gain_q16: i32,
}

impl Limiter {
    const fn new() -> Self {
        Limiter {
            enabled: false,
            ceiling_q16: 30000 * (Q16_ONE / 32768),
            release_q16: Q16_ONE / 500,
            gain_q16: Q16_ONE,
        }
    }

    pub fn process(&mut self, input: i32) -> i32 {
        if !self.enabled {
            return input;
        }

        let abs_in = if input < 0 { -input } else { input };

        // Compute required gain to stay under ceiling
        let target_gain = if abs_in > self.ceiling_q16 && abs_in > 0 {
            (((self.ceiling_q16 as i64) << 16) / abs_in as i64) as i32
        } else {
            Q16_ONE
        };

        // Instant attack, slow release
        if target_gain < self.gain_q16 {
            self.gain_q16 = target_gain;
        } else {
            self.gain_q16 = (((self.gain_q16 as i64 * (Q16_ONE - self.release_q16) as i64) >> 16)
                + ((Q16_ONE as i64 * self.release_q16 as i64) >> 16))
                as i32;
            if self.gain_q16 > Q16_ONE {
                self.gain_q16 = Q16_ONE;
            }
        }

        (((input as i64) * (self.gain_q16 as i64)) >> 16) as i32
    }
}

/// Noise gate
pub struct NoiseGate {
    pub enabled: bool,
    pub threshold_q16: i32,
    pub attack_q16: i32,
    pub release_q16: i32,
    pub hold_samples: u32, // hold open for N samples after going below threshold
    gate_q16: i32,         // current gate level (0 = closed, Q16_ONE = open)
    hold_counter: u32,
}

impl NoiseGate {
    const fn new() -> Self {
        NoiseGate {
            enabled: false,
            threshold_q16: 500 * (Q16_ONE / 32768),
            attack_q16: Q16_ONE / 50,
            release_q16: Q16_ONE / 200,
            hold_samples: 4410, // 100ms at 44100
            gate_q16: 0,
            hold_counter: 0,
        }
    }

    pub fn process(&mut self, input: i32) -> i32 {
        if !self.enabled {
            return input;
        }

        let abs_in = if input < 0 { -input } else { input };

        if abs_in > self.threshold_q16 {
            // Signal above threshold — open gate
            self.hold_counter = self.hold_samples;
            self.gate_q16 = (((self.gate_q16 as i64 * (Q16_ONE - self.attack_q16) as i64) >> 16)
                + ((Q16_ONE as i64 * self.attack_q16 as i64) >> 16))
                as i32;
        } else if self.hold_counter > 0 {
            // In hold period — keep gate open
            self.hold_counter -= 1;
        } else {
            // Below threshold and hold expired — close gate
            self.gate_q16 =
                ((self.gate_q16 as i64 * (Q16_ONE - self.release_q16) as i64) >> 16) as i32;
        }

        (((input as i64) * (self.gate_q16 as i64)) >> 16) as i32
    }
}

// ---------------------------------------------------------------------------
// Effects chain
// ---------------------------------------------------------------------------

/// Complete effects chain
pub struct EffectsChain {
    pub reverb: Reverb,
    pub echo: Echo,
    pub chorus: Chorus,
    pub compressor: Compressor,
    pub limiter: Limiter,
    pub noise_gate: NoiseGate,
    pub chain_enabled: bool,
    pub samples_processed: u64,
}

impl EffectsChain {
    fn new() -> Self {
        EffectsChain {
            reverb: Reverb::new(),
            echo: Echo::new(),
            chorus: Chorus::new(),
            compressor: Compressor::new(),
            limiter: Limiter::new(),
            noise_gate: NoiseGate::new(),
            chain_enabled: true,
            samples_processed: 0,
        }
    }

    /// Process a buffer through the full effects chain
    /// Order: NoiseGate -> Compressor -> Chorus -> Echo -> Reverb -> Limiter
    pub fn process(&mut self, samples: &mut [i16]) {
        if !self.chain_enabled {
            return;
        }

        for sample in samples.iter_mut() {
            let mut val = *sample as i32;

            // 1. Noise gate (first to remove noise before other processing)
            val = self.noise_gate.process(val);

            // 2. Compressor (control dynamics before effects)
            val = self.compressor.process(val);

            // 3. Chorus (modulation effect)
            val = self.chorus.process(val);

            // 4. Echo (time-based effect)
            val = self.echo.process(val);

            // 5. Reverb (space/ambience)
            val = self.reverb.process(val);

            // 6. Limiter (final safety)
            val = self.limiter.process(val);

            // Clamp to i16
            *sample = if val > 32767 {
                32767
            } else if val < -32768 {
                -32768
            } else {
                val as i16
            };
        }

        self.samples_processed += samples.len() as u64;
    }
}

static EFFECTS: Mutex<Option<EffectsChain>> = Mutex::new(None);

pub fn init() {
    *EFFECTS.lock() = Some(EffectsChain::new());
    serial_println!("    [effects] reverb, echo, chorus, compressor, limiter, noise gate");
}

/// Process audio buffer through effects chain
pub fn process(samples: &mut [i16]) {
    if let Some(ref mut fx) = *EFFECTS.lock() {
        fx.process(samples);
    }
}

/// Enable/disable reverb
pub fn set_reverb(enabled: bool, room_size_q16: i32, wet_q16: i32) {
    if let Some(ref mut fx) = *EFFECTS.lock() {
        fx.reverb.enabled = enabled;
        fx.reverb.room_size_q16 = room_size_q16;
        fx.reverb.wet_q16 = wet_q16;
    }
}

/// Enable/disable echo
pub fn set_echo(enabled: bool, delay_samples: usize, feedback_q16: i32) {
    if let Some(ref mut fx) = *EFFECTS.lock() {
        fx.echo.enabled = enabled;
        fx.echo.set_delay(delay_samples);
        fx.echo.feedback_q16 = feedback_q16;
    }
}

/// Enable/disable chorus
pub fn set_chorus(enabled: bool, wet_q16: i32) {
    if let Some(ref mut fx) = *EFFECTS.lock() {
        fx.chorus.enabled = enabled;
        fx.chorus.wet_q16 = wet_q16;
    }
}

/// Enable/disable compressor
pub fn set_compressor(enabled: bool, threshold_q16: i32, ratio_q16: i32) {
    if let Some(ref mut fx) = *EFFECTS.lock() {
        fx.compressor.enabled = enabled;
        fx.compressor.threshold_q16 = threshold_q16;
        fx.compressor.ratio_q16 = ratio_q16;
    }
}

/// Enable/disable limiter
pub fn set_limiter(enabled: bool, ceiling_q16: i32) {
    if let Some(ref mut fx) = *EFFECTS.lock() {
        fx.limiter.enabled = enabled;
        fx.limiter.ceiling_q16 = ceiling_q16;
    }
}

/// Enable/disable noise gate
pub fn set_noise_gate(enabled: bool, threshold_q16: i32) {
    if let Some(ref mut fx) = *EFFECTS.lock() {
        fx.noise_gate.enabled = enabled;
        fx.noise_gate.threshold_q16 = threshold_q16;
    }
}
