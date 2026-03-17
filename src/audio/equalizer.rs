use crate::sync::Mutex;
/// Audio equalizer — 10-band parametric EQ, presets, per-app EQ, bass boost, virtualizer
///
/// Uses Q16 fixed-point math for all DSP calculations.
/// Supports per-application EQ profiles, preset management, bass enhancement,
/// and stereo widening (virtualizer).
///
/// Inspired by: PulseAudio equalizer, Android AudioFX. All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Q16 fixed-point: 16 fractional bits
const Q16_ONE: i32 = 65536;

/// Number of EQ bands
const NUM_BANDS: usize = 10;

/// Maximum number of per-app EQ profiles
const MAX_APP_PROFILES: usize = 16;

/// Center frequencies for each band (Hz)
const BAND_FREQUENCIES: [u32; NUM_BANDS] = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];

/// EQ band filter state (biquad)
#[derive(Debug, Clone, Copy)]
pub struct BiquadState {
    pub b0: i32, // Q16 coefficient
    pub b1: i32,
    pub b2: i32,
    pub a1: i32,
    pub a2: i32,
    pub x1: i32, // delay line
    pub x2: i32,
    pub y1: i32,
    pub y2: i32,
}

impl BiquadState {
    const fn zero() -> Self {
        BiquadState {
            b0: Q16_ONE,
            b1: 0,
            b2: 0,
            a1: 0,
            a2: 0,
            x1: 0,
            x2: 0,
            y1: 0,
            y2: 0,
        }
    }

    /// Process one sample through this biquad filter
    pub fn process(&mut self, input: i32) -> i32 {
        // y[n] = b0*x[n] + b1*x[n-1] + b2*x[n-2] - a1*y[n-1] - a2*y[n-2]
        let acc: i64 = (self.b0 as i64 * input as i64)
            + (self.b1 as i64 * self.x1 as i64)
            + (self.b2 as i64 * self.x2 as i64)
            - (self.a1 as i64 * self.y1 as i64)
            - (self.a2 as i64 * self.y2 as i64);
        let output = (acc >> 16) as i32;

        self.x2 = self.x1;
        self.x1 = input;
        self.y2 = self.y1;
        self.y1 = output;

        output
    }

    /// Reset delay line state
    pub fn reset(&mut self) {
        self.x1 = 0;
        self.x2 = 0;
        self.y1 = 0;
        self.y2 = 0;
    }
}

/// A single EQ band with gain and Q factor
#[derive(Debug, Clone, Copy)]
pub struct EqBand {
    pub frequency: u32,
    pub gain_q16: i32,     // gain in Q16 (-12dB to +12dB mapped)
    pub q_factor_q16: i32, // Q factor in Q16 (bandwidth)
    pub enabled: bool,
    pub filter: BiquadState,
}

impl EqBand {
    const fn new(freq: u32) -> Self {
        EqBand {
            frequency: freq,
            gain_q16: 0,
            q_factor_q16: Q16_ONE, // Q = 1.0
            enabled: true,
            filter: BiquadState::zero(),
        }
    }

    /// Recompute biquad coefficients from gain and Q factor
    pub fn update_coefficients(&mut self, sample_rate: u32) {
        if self.gain_q16 == 0 || !self.enabled {
            // Passthrough
            self.filter.b0 = Q16_ONE;
            self.filter.b1 = 0;
            self.filter.b2 = 0;
            self.filter.a1 = 0;
            self.filter.a2 = 0;
            return;
        }

        // Approximate peaking EQ biquad using integer math
        // omega = 2*pi*freq/sample_rate approximated via lookup
        let omega_q16 = (((self.frequency as i64) << 17) / sample_rate as i64) as i32; // ~2*freq/sr in Q16

        // alpha ~ sin(omega) / (2*Q) approximated as omega / (2*Q)
        let alpha_q16 = (((omega_q16 as i64) << 16) / (2 * self.q_factor_q16) as i64) as i32;

        // A = 10^(gain_dB/40) approximated: for small gains, A ~ 1 + gain*ln(10)/40
        // We use gain_q16 directly as a linear scale factor
        let a_q16 = Q16_ONE + (self.gain_q16 >> 2);

        // Peaking EQ coefficients (simplified integer approximation)
        let alpha_a = (((alpha_q16 as i64) * (a_q16 as i64)) >> 16) as i32;
        let alpha_inv_a = if a_q16 != 0 {
            (((alpha_q16 as i64) << 16) / a_q16 as i64) as i32
        } else {
            alpha_q16
        };

        let norm = Q16_ONE + alpha_inv_a;
        if norm == 0 {
            return;
        }

        self.filter.b0 = (((Q16_ONE + alpha_a) as i64 * Q16_ONE as i64) / norm as i64) as i32;
        self.filter.b1 = (((-2 * omega_q16) as i64 * Q16_ONE as i64) / norm as i64) as i32;
        self.filter.b2 = (((Q16_ONE - alpha_a) as i64 * Q16_ONE as i64) / norm as i64) as i32;
        self.filter.a1 = self.filter.b1; // symmetric for peaking
        self.filter.a2 = (((Q16_ONE - alpha_inv_a) as i64 * Q16_ONE as i64) / norm as i64) as i32;
    }
}

/// Named EQ preset
#[derive(Debug, Clone)]
pub struct EqPreset {
    pub name: String,
    pub gains: [i32; NUM_BANDS], // Q16 gain per band
}

/// Per-application EQ profile
#[derive(Debug, Clone)]
pub struct AppEqProfile {
    pub app_id: u32,
    pub name: String,
    pub gains: [i32; NUM_BANDS],
    pub bass_boost_level: i32,  // Q16
    pub virtualizer_level: i32, // Q16
    pub enabled: bool,
}

/// Bass boost processor state
#[derive(Debug, Clone, Copy)]
pub struct BassBoost {
    pub enabled: bool,
    pub strength_q16: i32, // 0 to Q16_ONE
    pub cutoff_hz: u32,
    filter: BiquadState,
    prev_sample: i32,
}

impl BassBoost {
    const fn new() -> Self {
        BassBoost {
            enabled: false,
            strength_q16: Q16_ONE / 2,
            cutoff_hz: 150,
            filter: BiquadState::zero(),
            prev_sample: 0,
        }
    }

    /// Process a sample with bass enhancement
    pub fn process(&mut self, input: i32) -> i32 {
        if !self.enabled || self.strength_q16 == 0 {
            return input;
        }
        // Simple low-pass accumulator for bass extraction
        let bass = self.prev_sample + (((input - self.prev_sample) as i64 * 8192) >> 16) as i32;
        self.prev_sample = bass;

        // Add boosted bass back
        let boost = (((bass as i64) * (self.strength_q16 as i64)) >> 16) as i32;
        let output = input + boost;

        // Soft clamp to prevent overflow
        clamp_q16(output)
    }
}

/// Stereo virtualizer (stereo widening)
#[derive(Debug, Clone, Copy)]
pub struct Virtualizer {
    pub enabled: bool,
    pub width_q16: i32, // 0 = mono, Q16_ONE = normal, 2*Q16_ONE = wide
    delay_buffer: [i32; 128],
    delay_pos: usize,
}

impl Virtualizer {
    const fn new() -> Self {
        Virtualizer {
            enabled: false,
            width_q16: Q16_ONE + (Q16_ONE / 2), // 1.5x width
            delay_buffer: [0; 128],
            delay_pos: 0,
        }
    }

    /// Process stereo pair (left, right) -> (new_left, new_right)
    pub fn process_stereo(&mut self, left: i32, right: i32) -> (i32, i32) {
        if !self.enabled {
            return (left, right);
        }

        // Mid-side processing
        let mid = (left + right) >> 1;
        let side = (left - right) >> 1;

        // Enhance side signal with width factor
        let enhanced_side = (((side as i64) * (self.width_q16 as i64)) >> 16) as i32;

        // Add subtle delay-based widening
        let delayed = self.delay_buffer[self.delay_pos];
        self.delay_buffer[self.delay_pos] = side;
        self.delay_pos = (self.delay_pos + 1) % 128;

        let cross = (((delayed as i64) * (self.width_q16 as i64 / 4)) >> 16) as i32;

        let new_left = clamp_q16(mid + enhanced_side + cross);
        let new_right = clamp_q16(mid - enhanced_side - cross);

        (new_left, new_right)
    }
}

/// Main equalizer engine
pub struct Equalizer {
    pub bands: [EqBand; NUM_BANDS],
    pub enabled: bool,
    pub sample_rate: u32,
    pub presets: Vec<EqPreset>,
    pub active_preset: Option<usize>,
    pub app_profiles: Vec<AppEqProfile>,
    pub bass_boost: BassBoost,
    pub virtualizer: Virtualizer,
    pub output_gain_q16: i32,
    pub samples_processed: u64,
}

impl Equalizer {
    const fn new() -> Self {
        Equalizer {
            bands: [
                EqBand::new(31),
                EqBand::new(62),
                EqBand::new(125),
                EqBand::new(250),
                EqBand::new(500),
                EqBand::new(1000),
                EqBand::new(2000),
                EqBand::new(4000),
                EqBand::new(8000),
                EqBand::new(16000),
            ],
            enabled: true,
            sample_rate: 44100,
            presets: Vec::new(),
            active_preset: None,
            app_profiles: Vec::new(),
            bass_boost: BassBoost::new(),
            virtualizer: Virtualizer::new(),
            output_gain_q16: Q16_ONE,
            samples_processed: 0,
        }
    }

    /// Set gain for a specific band (in Q16 fixed-point)
    pub fn set_band_gain(&mut self, band: usize, gain_q16: i32) {
        if band < NUM_BANDS {
            // Clamp to +/-12dB in Q16 (~786432 = 12 * 65536)
            let max_gain = 12 * Q16_ONE;
            let clamped = if gain_q16 > max_gain {
                max_gain
            } else if gain_q16 < -max_gain {
                -max_gain
            } else {
                gain_q16
            };
            self.bands[band].gain_q16 = clamped;
            self.bands[band].update_coefficients(self.sample_rate);
        }
    }

    /// Set all band gains at once
    pub fn set_all_gains(&mut self, gains: &[i32; NUM_BANDS]) {
        for i in 0..NUM_BANDS {
            self.set_band_gain(i, gains[i]);
        }
    }

    /// Apply a preset by index
    pub fn apply_preset(&mut self, index: usize) -> bool {
        if index >= self.presets.len() {
            return false;
        }
        let gains = self.presets[index].gains;
        self.set_all_gains(&gains);
        self.active_preset = Some(index);
        true
    }

    /// Add a new preset
    pub fn add_preset(&mut self, name: String, gains: [i32; NUM_BANDS]) {
        self.presets.push(EqPreset { name, gains });
    }

    /// Set per-app EQ profile
    pub fn set_app_profile(&mut self, app_id: u32, name: String, gains: [i32; NUM_BANDS]) {
        // Update existing or add new
        for profile in self.app_profiles.iter_mut() {
            if profile.app_id == app_id {
                profile.gains = gains;
                profile.name = name;
                return;
            }
        }
        if self.app_profiles.len() < MAX_APP_PROFILES {
            self.app_profiles.push(AppEqProfile {
                app_id,
                name,
                gains,
                bass_boost_level: 0,
                virtualizer_level: 0,
                enabled: true,
            });
        }
    }

    /// Switch active EQ to an app's profile
    pub fn activate_app_profile(&mut self, app_id: u32) -> bool {
        // Copy gains out to avoid borrow conflict with self.set_all_gains()
        let mut found_gains: Option<([i32; NUM_BANDS], i32, i32)> = None;
        for profile in self.app_profiles.iter() {
            if profile.app_id == app_id && profile.enabled {
                found_gains = Some((
                    profile.gains,
                    profile.bass_boost_level,
                    profile.virtualizer_level,
                ));
                break;
            }
        }
        if let Some((gains, bass, virt)) = found_gains {
            self.set_all_gains(&gains);
            self.bass_boost.strength_q16 = bass;
            self.virtualizer.width_q16 = virt;
            true
        } else {
            false
        }
    }

    /// Process a mono buffer through the EQ chain
    pub fn process_mono(&mut self, samples: &mut [i16]) {
        if !self.enabled {
            return;
        }

        for sample in samples.iter_mut() {
            let mut val = (*sample as i32) << 8; // scale up for headroom

            // Process through each enabled EQ band
            for band in self.bands.iter_mut() {
                if band.enabled && band.gain_q16 != 0 {
                    val = band.filter.process(val);
                }
            }

            // Bass boost
            val = self.bass_boost.process(val);

            // Output gain
            val = (((val as i64) * (self.output_gain_q16 as i64)) >> 16) as i32;

            // Scale back and clamp
            val >>= 8;
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

    /// Process a stereo interleaved buffer (L, R, L, R, ...)
    pub fn process_stereo(&mut self, samples: &mut [i16]) {
        if !self.enabled || samples.len() < 2 {
            return;
        }

        let mut i = 0;
        while i + 1 < samples.len() {
            let mut left = (samples[i] as i32) << 8;
            let mut right = (samples[i + 1] as i32) << 8;

            // EQ both channels
            for band in self.bands.iter_mut() {
                if band.enabled && band.gain_q16 != 0 {
                    left = band.filter.process(left);
                    right = band.filter.process(right);
                }
            }

            // Bass boost
            left = self.bass_boost.process(left);
            right = self.bass_boost.process(right);

            // Virtualizer
            let (vl, vr) = self.virtualizer.process_stereo(left, right);

            // Output gain
            let fl = (((vl as i64) * (self.output_gain_q16 as i64)) >> 16) as i32;
            let fr = (((vr as i64) * (self.output_gain_q16 as i64)) >> 16) as i32;

            samples[i] = clamp_i16(fl >> 8);
            samples[i + 1] = clamp_i16(fr >> 8);

            i += 2;
        }

        self.samples_processed += (samples.len() / 2) as u64;
    }

    /// Reset all filter states (call on stream change)
    pub fn reset_filters(&mut self) {
        for band in self.bands.iter_mut() {
            band.filter.reset();
        }
        self.bass_boost.prev_sample = 0;
    }
}

/// Clamp a Q16 value to prevent overflow
fn clamp_q16(val: i32) -> i32 {
    const MAX: i32 = i32::MAX >> 1;
    const MIN: i32 = i32::MIN >> 1;
    if val > MAX {
        MAX
    } else if val < MIN {
        MIN
    } else {
        val
    }
}

/// Clamp to i16 range
fn clamp_i16(val: i32) -> i16 {
    if val > 32767 {
        32767
    } else if val < -32768 {
        -32768
    } else {
        val as i16
    }
}

static EQUALIZER: Mutex<Option<Equalizer>> = Mutex::new(None);

/// Seed built-in presets
fn seed_presets(eq: &mut Equalizer) {
    // Flat
    eq.add_preset(String::from("Flat"), [0; NUM_BANDS]);

    // Bass Boost
    eq.add_preset(
        String::from("Bass Boost"),
        [
            4 * Q16_ONE,
            3 * Q16_ONE,
            2 * Q16_ONE,
            Q16_ONE,
            0,
            0,
            0,
            0,
            0,
            0,
        ],
    );

    // Treble Boost
    eq.add_preset(
        String::from("Treble Boost"),
        [
            0,
            0,
            0,
            0,
            0,
            Q16_ONE,
            2 * Q16_ONE,
            3 * Q16_ONE,
            4 * Q16_ONE,
            4 * Q16_ONE,
        ],
    );

    // Vocal / Podcast
    eq.add_preset(
        String::from("Vocal"),
        [
            -2 * Q16_ONE,
            -Q16_ONE,
            0,
            2 * Q16_ONE,
            4 * Q16_ONE,
            4 * Q16_ONE,
            2 * Q16_ONE,
            0,
            -Q16_ONE,
            -2 * Q16_ONE,
        ],
    );

    // Rock
    eq.add_preset(
        String::from("Rock"),
        [
            3 * Q16_ONE,
            2 * Q16_ONE,
            -Q16_ONE,
            -2 * Q16_ONE,
            0,
            2 * Q16_ONE,
            3 * Q16_ONE,
            3 * Q16_ONE,
            2 * Q16_ONE,
            Q16_ONE,
        ],
    );

    // Classical
    eq.add_preset(
        String::from("Classical"),
        [
            0,
            0,
            0,
            0,
            0,
            0,
            -Q16_ONE,
            -Q16_ONE,
            -2 * Q16_ONE,
            -3 * Q16_ONE,
        ],
    );

    // Electronic
    eq.add_preset(
        String::from("Electronic"),
        [
            4 * Q16_ONE,
            3 * Q16_ONE,
            Q16_ONE,
            0,
            -Q16_ONE,
            Q16_ONE,
            2 * Q16_ONE,
            3 * Q16_ONE,
            4 * Q16_ONE,
            3 * Q16_ONE,
        ],
    );

    // Night Mode (reduced bass & treble)
    eq.add_preset(
        String::from("Night Mode"),
        [
            -3 * Q16_ONE,
            -2 * Q16_ONE,
            -Q16_ONE,
            Q16_ONE,
            2 * Q16_ONE,
            2 * Q16_ONE,
            Q16_ONE,
            -Q16_ONE,
            -2 * Q16_ONE,
            -3 * Q16_ONE,
        ],
    );
}

pub fn init() {
    let mut eq = Equalizer::new();
    seed_presets(&mut eq);
    *EQUALIZER.lock() = Some(eq);
    serial_println!(
        "    [equalizer] 10-band parametric EQ, presets, per-app EQ, bass boost, virtualizer"
    );
}

/// Set band gain (band 0-9, gain in Q16)
pub fn set_band(band: usize, gain_q16: i32) {
    if let Some(ref mut eq) = *EQUALIZER.lock() {
        eq.set_band_gain(band, gain_q16);
    }
}

/// Apply preset by index
pub fn apply_preset(index: usize) -> bool {
    if let Some(ref mut eq) = *EQUALIZER.lock() {
        eq.apply_preset(index)
    } else {
        false
    }
}

/// Process mono buffer
pub fn process_mono(samples: &mut [i16]) {
    if let Some(ref mut eq) = *EQUALIZER.lock() {
        eq.process_mono(samples);
    }
}

/// Process stereo interleaved buffer
pub fn process_stereo(samples: &mut [i16]) {
    if let Some(ref mut eq) = *EQUALIZER.lock() {
        eq.process_stereo(samples);
    }
}

/// Enable/disable bass boost
pub fn set_bass_boost(enabled: bool, strength_q16: i32) {
    if let Some(ref mut eq) = *EQUALIZER.lock() {
        eq.bass_boost.enabled = enabled;
        eq.bass_boost.strength_q16 = strength_q16;
    }
}

/// Enable/disable virtualizer
pub fn set_virtualizer(enabled: bool, width_q16: i32) {
    if let Some(ref mut eq) = *EQUALIZER.lock() {
        eq.virtualizer.enabled = enabled;
        eq.virtualizer.width_q16 = width_q16;
    }
}

/// Set per-app EQ profile
pub fn set_app_eq(app_id: u32, name: String, gains: [i32; NUM_BANDS]) {
    if let Some(ref mut eq) = *EQUALIZER.lock() {
        eq.set_app_profile(app_id, name, gains);
    }
}

/// Activate an app's EQ profile
pub fn activate_app(app_id: u32) -> bool {
    if let Some(ref mut eq) = *EQUALIZER.lock() {
        eq.activate_app_profile(app_id)
    } else {
        false
    }
}
