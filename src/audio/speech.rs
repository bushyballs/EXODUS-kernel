use crate::sync::Mutex;
/// Speech processing — voice activity detection, noise suppression, echo cancellation, AGC
///
/// Real-time speech processing pipeline using Q16 fixed-point math.
/// Designed for VoIP, voice commands, and accessibility features.
///
/// Inspired by: WebRTC APM, Speex preprocessor, PulseAudio echo-cancel. All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Q16 fixed-point: 16 fractional bits
const Q16_ONE: i32 = 65536;

/// Frame size for speech processing (20ms at 16kHz)
const FRAME_SIZE: usize = 320;

/// Number of sub-bands for analysis
const NUM_SUBBANDS: usize = 16;

/// Echo tail length in samples (~200ms at 16kHz)
const ECHO_TAIL_LEN: usize = 3200;

/// Noise estimation smoothing frames
const NOISE_SMOOTH_FRAMES: usize = 50;

// ---------------------------------------------------------------------------
// Voice Activity Detection (VAD)
// ---------------------------------------------------------------------------

/// VAD operating mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadMode {
    Quality,    // fewer false negatives (catches more speech)
    LowBitrate, // more aggressive filtering
    Aggressive, // maximum noise rejection
    VeryAggressive,
}

/// VAD state
pub struct VoiceActivityDetector {
    pub mode: VadMode,
    pub enabled: bool,
    /// Current energy threshold (Q16)
    energy_threshold_q16: i32,
    /// Running average of noise floor (Q16)
    noise_floor_q16: i32,
    /// Hangover counter (keep detecting speech for N frames after energy drops)
    hangover: u32,
    hangover_max: u32,
    /// Speech probability (Q16, 0..Q16_ONE)
    speech_prob_q16: i32,
    /// Frame counter
    frame_count: u64,
    /// Sub-band energies for spectral analysis
    subband_energy: [i32; NUM_SUBBANDS],
    /// Previous sub-band energies (for delta)
    prev_subband_energy: [i32; NUM_SUBBANDS],
}

impl VoiceActivityDetector {
    const fn new() -> Self {
        VoiceActivityDetector {
            mode: VadMode::Quality,
            enabled: true,
            energy_threshold_q16: Q16_ONE / 10,
            noise_floor_q16: Q16_ONE / 100,
            hangover: 0,
            hangover_max: 15,
            speech_prob_q16: 0,
            frame_count: 0,
            subband_energy: [0; NUM_SUBBANDS],
            prev_subband_energy: [0; NUM_SUBBANDS],
        }
    }

    /// Analyze a frame and return true if speech is detected
    pub fn analyze(&mut self, frame: &[i16]) -> bool {
        if !self.enabled || frame.is_empty() {
            return false;
        }

        // Compute frame energy in Q16
        let mut energy: i64 = 0;
        for &sample in frame.iter() {
            energy += (sample as i64) * (sample as i64);
        }
        let frame_energy_q16 = ((energy / frame.len() as i64) >> 8) as i32;

        // Compute sub-band energies (simple band splitting)
        let samples_per_band = frame.len() / NUM_SUBBANDS;
        for (i, chunk) in frame.chunks(samples_per_band).enumerate() {
            if i >= NUM_SUBBANDS {
                break;
            }
            self.prev_subband_energy[i] = self.subband_energy[i];
            let mut band_e: i64 = 0;
            for &s in chunk {
                band_e += (s as i64) * (s as i64);
            }
            self.subband_energy[i] = if chunk.is_empty() {
                0
            } else {
                ((band_e / chunk.len() as i64) >> 8) as i32
            };
        }

        // Spectral flatness: speech has more spectral variation than noise
        let mut spectral_var: i64 = 0;
        for i in 0..NUM_SUBBANDS {
            let diff = self.subband_energy[i] - self.prev_subband_energy[i];
            spectral_var += (diff as i64) * (diff as i64);
        }
        let spectral_change = (spectral_var >> 16) as i32;

        // Update noise floor estimate (slow adaptation)
        if frame_energy_q16 < self.noise_floor_q16 * 2 {
            // Likely noise — adapt floor upward slowly
            self.noise_floor_q16 =
                (((self.noise_floor_q16 as i64 * 63) + frame_energy_q16 as i64) / 64) as i32;
        } else {
            // Possible speech — adapt floor downward very slowly
            self.noise_floor_q16 =
                (((self.noise_floor_q16 as i64 * 127) + frame_energy_q16 as i64) / 128) as i32;
        }

        // Adaptive threshold based on mode
        let threshold_multiplier = match self.mode {
            VadMode::Quality => 3,
            VadMode::LowBitrate => 4,
            VadMode::Aggressive => 6,
            VadMode::VeryAggressive => 8,
        };
        self.energy_threshold_q16 = self.noise_floor_q16 * threshold_multiplier;

        // Decision: energy above threshold AND spectral activity
        let energy_detected = frame_energy_q16 > self.energy_threshold_q16;
        let spectral_detected = spectral_change > (self.noise_floor_q16 >> 2);

        let speech = energy_detected && (spectral_detected || self.mode == VadMode::Quality);

        // Hangover logic
        if speech {
            self.hangover = self.hangover_max;
            self.speech_prob_q16 = Q16_ONE;
        } else if self.hangover > 0 {
            self.hangover -= 1;
            // Decay probability during hangover
            self.speech_prob_q16 = ((self.speech_prob_q16 as i64 * 60000) >> 16) as i32;
        } else {
            self.speech_prob_q16 = 0;
        }

        self.frame_count = self.frame_count.saturating_add(1);
        self.hangover > 0
    }

    /// Get current speech probability (0..Q16_ONE)
    pub fn speech_probability(&self) -> i32 {
        self.speech_prob_q16
    }
}

// ---------------------------------------------------------------------------
// Noise Suppression
// ---------------------------------------------------------------------------

/// Noise suppression level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoiseSuppressionLevel {
    Low,
    Moderate,
    High,
    VeryHigh,
}

/// Spectral noise suppressor
pub struct NoiseSuppressor {
    pub enabled: bool,
    pub level: NoiseSuppressionLevel,
    /// Estimated noise spectrum per sub-band (Q16)
    noise_estimate: [i32; NUM_SUBBANDS],
    /// Smoothing factor for noise estimation (Q16)
    smooth_q16: i32,
    /// Gain floor to prevent musical noise (Q16)
    gain_floor_q16: i32,
    /// Per-band suppression gain (Q16)
    band_gain: [i32; NUM_SUBBANDS],
    frame_count: u64,
}

impl NoiseSuppressor {
    const fn new() -> Self {
        NoiseSuppressor {
            enabled: true,
            level: NoiseSuppressionLevel::Moderate,
            noise_estimate: [Q16_ONE / 100; NUM_SUBBANDS],
            smooth_q16: Q16_ONE * 95 / 100,
            gain_floor_q16: Q16_ONE / 10,
            band_gain: [Q16_ONE; NUM_SUBBANDS],
            frame_count: 0,
        }
    }

    /// Process a frame of audio, suppressing noise
    pub fn process(&mut self, frame: &mut [i16]) {
        if !self.enabled || frame.is_empty() {
            return;
        }

        let samples_per_band = frame.len() / NUM_SUBBANDS;
        if samples_per_band == 0 {
            return;
        }

        // Suppression amount based on level
        let suppression_q16 = match self.level {
            NoiseSuppressionLevel::Low => Q16_ONE * 6 / 10,
            NoiseSuppressionLevel::Moderate => Q16_ONE * 4 / 10,
            NoiseSuppressionLevel::High => Q16_ONE * 2 / 10,
            NoiseSuppressionLevel::VeryHigh => Q16_ONE / 10,
        };

        // Estimate band energies and update noise model
        for band in 0..NUM_SUBBANDS {
            let start = band * samples_per_band;
            let end = if band == NUM_SUBBANDS - 1 {
                frame.len()
            } else {
                start + samples_per_band
            };

            let mut energy: i64 = 0;
            for i in start..end {
                energy += (frame[i] as i64) * (frame[i] as i64);
            }
            let band_energy = ((energy / (end - start) as i64) >> 8) as i32;

            // Update noise estimate during first N frames (assumed noise)
            if self.frame_count < NOISE_SMOOTH_FRAMES as u64 {
                self.noise_estimate[band] =
                    (((self.noise_estimate[band] as i64 * self.smooth_q16 as i64) >> 16)
                        + ((band_energy as i64 * (Q16_ONE - self.smooth_q16) as i64) >> 16))
                        as i32;
            } else if band_energy < self.noise_estimate[band] * 2 {
                // Slow adaptation when signal is quiet
                self.noise_estimate[band] =
                    (((self.noise_estimate[band] as i64 * 127) + band_energy as i64) / 128) as i32;
            }

            // Compute Wiener-like gain: gain = max(1 - noise/signal, floor)
            let snr_gain = if band_energy > 0 {
                let ratio =
                    (((self.noise_estimate[band] as i64) << 16) / band_energy as i64) as i32;
                let gain = Q16_ONE - (((ratio as i64 * suppression_q16 as i64) >> 16) as i32);
                if gain < self.gain_floor_q16 {
                    self.gain_floor_q16
                } else {
                    gain
                }
            } else {
                self.gain_floor_q16
            };

            self.band_gain[band] = snr_gain;

            // Apply gain to samples in this band
            for i in start..end {
                frame[i] = (((frame[i] as i64) * (snr_gain as i64)) >> 16) as i16;
            }
        }

        self.frame_count = self.frame_count.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Acoustic Echo Cancellation (AEC)
// ---------------------------------------------------------------------------

/// Adaptive echo canceller (NLMS-style)
pub struct EchoCanceller {
    pub enabled: bool,
    /// Reference signal buffer (far-end / speaker output)
    ref_buffer: Vec<i32>,
    /// Adaptive filter coefficients (Q16)
    filter_coeffs: Vec<i32>,
    /// Write position in reference buffer
    ref_pos: usize,
    /// Adaptation step size (Q16)
    mu_q16: i32,
    /// Filter length
    filter_len: usize,
    /// Echo return loss enhancement estimate (Q16 dB)
    erle_q16: i32,
    /// Residual echo suppression enabled
    residual_suppress: bool,
}

impl EchoCanceller {
    fn new() -> Self {
        let filter_len = ECHO_TAIL_LEN;
        EchoCanceller {
            enabled: true,
            ref_buffer: vec![0i32; filter_len],
            filter_coeffs: vec![0i32; filter_len],
            ref_pos: 0,
            mu_q16: Q16_ONE / 32, // step size ~0.03
            filter_len,
            erle_q16: 0,
            residual_suppress: true,
        }
    }

    /// Feed reference signal (what the speaker plays)
    pub fn feed_reference(&mut self, samples: &[i16]) {
        for &s in samples.iter() {
            self.ref_buffer[self.ref_pos] = s as i32;
            self.ref_pos = (self.ref_pos + 1) % self.filter_len;
        }
    }

    /// Process near-end (microphone) signal, removing echo
    pub fn process(&mut self, mic_frame: &mut [i16]) {
        if !self.enabled {
            return;
        }

        for sample in mic_frame.iter_mut() {
            let mic_val = *sample as i32;

            // Compute estimated echo: sum(coeffs[i] * ref[pos - i])
            let mut echo_est: i64 = 0;
            let mut ref_energy: i64 = 0;

            // Use a subset for efficiency (downsample filter)
            let step = if self.filter_len > 512 { 4 } else { 1 };
            let mut idx = self.ref_pos;
            let mut fi = 0;
            while fi < self.filter_len {
                if idx == 0 {
                    idx = self.filter_len - 1;
                } else {
                    idx -= 1;
                }
                let ref_val = self.ref_buffer[idx] as i64;
                echo_est += ref_val * self.filter_coeffs[fi] as i64;
                ref_energy += ref_val * ref_val;
                fi += step;
            }

            let echo_est_scaled = (echo_est >> 16) as i32;

            // Error signal
            let error = mic_val - echo_est_scaled;

            // NLMS adaptation: coeffs += mu * error * ref / (ref_energy + eps)
            let norm = if ref_energy > 0 {
                ((Q16_ONE as i64) << 16) / (ref_energy + 1)
            } else {
                0
            };

            let adaptation = (((self.mu_q16 as i64) * (error as i64) * norm) >> 32) as i32;

            idx = self.ref_pos;
            fi = 0;
            while fi < self.filter_len {
                if idx == 0 {
                    idx = self.filter_len - 1;
                } else {
                    idx -= 1;
                }
                let ref_val = self.ref_buffer[idx];
                self.filter_coeffs[fi] += (((adaptation as i64) * (ref_val as i64)) >> 16) as i32;
                fi += step;
            }

            // Output the error signal (mic minus estimated echo)
            let out = if self.residual_suppress {
                // Additional residual echo suppression
                let abs_err = if error < 0 { -error } else { error };
                let abs_echo = if echo_est_scaled < 0 {
                    -echo_est_scaled
                } else {
                    echo_est_scaled
                };
                if abs_echo > abs_err * 2 {
                    // Likely still echoing — attenuate more
                    (((error as i64) * (Q16_ONE / 4) as i64) >> 16) as i32
                } else {
                    error
                }
            } else {
                error
            };

            *sample = if out > 32767 {
                32767
            } else if out < -32768 {
                -32768
            } else {
                out as i16
            };
        }
    }
}

// ---------------------------------------------------------------------------
// Automatic Gain Control (AGC)
// ---------------------------------------------------------------------------

/// AGC mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgcMode {
    AdaptiveAnalog,
    AdaptiveDigital,
    FixedDigital,
}

/// Automatic Gain Control
pub struct AutomaticGainControl {
    pub enabled: bool,
    pub mode: AgcMode,
    /// Target output level (Q16, in sample magnitude)
    pub target_level_q16: i32,
    /// Current gain (Q16)
    gain_q16: i32,
    /// Maximum gain (Q16)
    max_gain_q16: i32,
    /// Minimum gain (Q16)
    min_gain_q16: i32,
    /// Attack rate (how fast gain decreases, Q16 per frame)
    attack_q16: i32,
    /// Release rate (how fast gain increases, Q16 per frame)
    release_q16: i32,
    /// Limiter enabled
    pub limiter_enabled: bool,
    /// Limiter threshold (Q16)
    limiter_threshold_q16: i32,
}

impl AutomaticGainControl {
    const fn new() -> Self {
        AutomaticGainControl {
            enabled: true,
            mode: AgcMode::AdaptiveDigital,
            target_level_q16: 10000 * (Q16_ONE / 32768),
            gain_q16: Q16_ONE,
            max_gain_q16: Q16_ONE * 30, // up to 30x gain
            min_gain_q16: Q16_ONE / 10, // minimum 0.1x
            attack_q16: Q16_ONE / 20,
            release_q16: Q16_ONE / 200,
            limiter_enabled: true,
            limiter_threshold_q16: 30000 * (Q16_ONE / 32768),
        }
    }

    /// Process a frame with AGC
    pub fn process(&mut self, frame: &mut [i16]) {
        if !self.enabled || frame.is_empty() {
            return;
        }

        // Measure RMS level of frame
        let mut sum_sq: i64 = 0;
        for &s in frame.iter() {
            sum_sq += (s as i64) * (s as i64);
        }
        let rms = isqrt((sum_sq / frame.len() as i64) as u64) as i32;
        let rms_q16 = rms * (Q16_ONE / 32768);

        // Compute desired gain
        let desired_gain = if rms_q16 > 0 {
            (((self.target_level_q16 as i64) << 16) / rms_q16 as i64) as i32
        } else {
            self.gain_q16
        };

        // Clamp desired gain
        let clamped = if desired_gain > self.max_gain_q16 {
            self.max_gain_q16
        } else if desired_gain < self.min_gain_q16 {
            self.min_gain_q16
        } else {
            desired_gain
        };

        // Smooth gain transition
        if clamped < self.gain_q16 {
            // Reducing gain (attack) — fast
            let delta = (((self.gain_q16 - clamped) as i64 * self.attack_q16 as i64) >> 16) as i32;
            self.gain_q16 -= delta;
        } else {
            // Increasing gain (release) — slow
            let delta = (((clamped - self.gain_q16) as i64 * self.release_q16 as i64) >> 16) as i32;
            self.gain_q16 += delta;
        }

        // Apply gain
        for sample in frame.iter_mut() {
            let mut val = (((*sample as i64) * (self.gain_q16 as i64)) >> 16) as i32;

            // Limiter
            if self.limiter_enabled {
                let limit = self.limiter_threshold_q16 >> (Q16_ONE / 32768).leading_zeros();
                if val > 30000 {
                    val = 30000;
                }
                if val < -30000 {
                    val = -30000;
                }
            }

            *sample = if val > 32767 {
                32767
            } else if val < -32768 {
                -32768
            } else {
                val as i16
            };
        }
    }
}

/// Integer square root (Newton's method)
fn isqrt(n: u64) -> u32 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x as u32
}

// ---------------------------------------------------------------------------
// Speech processing pipeline
// ---------------------------------------------------------------------------

/// Complete speech processing pipeline
pub struct SpeechProcessor {
    pub vad: VoiceActivityDetector,
    pub noise_suppressor: NoiseSuppressor,
    pub echo_canceller: EchoCanceller,
    pub agc: AutomaticGainControl,
    pub pipeline_enabled: bool,
    /// Process order flags
    pub aec_before_ns: bool,
    /// Statistics
    pub frames_processed: u64,
    pub speech_frames: u64,
    pub noise_frames: u64,
}

impl SpeechProcessor {
    fn new() -> Self {
        SpeechProcessor {
            vad: VoiceActivityDetector::new(),
            noise_suppressor: NoiseSuppressor::new(),
            echo_canceller: EchoCanceller::new(),
            agc: AutomaticGainControl::new(),
            pipeline_enabled: true,
            aec_before_ns: true,
            frames_processed: 0,
            speech_frames: 0,
            noise_frames: 0,
        }
    }

    /// Process a frame through the full speech pipeline
    /// Order: AEC -> Noise Suppression -> VAD -> AGC
    pub fn process_frame(&mut self, mic_frame: &mut [i16]) -> bool {
        if !self.pipeline_enabled {
            return false;
        }

        // 1. Echo cancellation (remove speaker signal from mic)
        if self.aec_before_ns {
            self.echo_canceller.process(mic_frame);
        }

        // 2. Noise suppression
        self.noise_suppressor.process(mic_frame);

        // 3. Echo cancellation (alternative ordering)
        if !self.aec_before_ns {
            self.echo_canceller.process(mic_frame);
        }

        // 4. Voice activity detection
        let is_speech = self.vad.analyze(mic_frame);

        // 5. AGC (only amplify when speech is detected to avoid amplifying noise)
        if is_speech {
            self.agc.process(mic_frame);
            self.speech_frames = self.speech_frames.saturating_add(1);
        } else {
            self.noise_frames = self.noise_frames.saturating_add(1);
        }

        self.frames_processed = self.frames_processed.saturating_add(1);
        is_speech
    }

    /// Feed reference (far-end) audio for echo cancellation
    pub fn feed_reference(&mut self, speaker_samples: &[i16]) {
        self.echo_canceller.feed_reference(speaker_samples);
    }
}

static SPEECH: Mutex<Option<SpeechProcessor>> = Mutex::new(None);

pub fn init() {
    *SPEECH.lock() = Some(SpeechProcessor::new());
    serial_println!("    [speech] VAD, noise suppression, echo cancellation, AGC");
}

/// Process a microphone frame (returns true if speech detected)
pub fn process_frame(mic_frame: &mut [i16]) -> bool {
    if let Some(ref mut sp) = *SPEECH.lock() {
        sp.process_frame(mic_frame)
    } else {
        false
    }
}

/// Feed speaker output for echo cancellation
pub fn feed_reference(speaker_samples: &[i16]) {
    if let Some(ref mut sp) = *SPEECH.lock() {
        sp.feed_reference(speaker_samples);
    }
}

/// Set VAD mode
pub fn set_vad_mode(mode: VadMode) {
    if let Some(ref mut sp) = *SPEECH.lock() {
        sp.vad.mode = mode;
    }
}

/// Set noise suppression level
pub fn set_noise_suppression(level: NoiseSuppressionLevel) {
    if let Some(ref mut sp) = *SPEECH.lock() {
        sp.noise_suppressor.level = level;
    }
}

/// Enable/disable echo cancellation
pub fn set_echo_cancel(enabled: bool) {
    if let Some(ref mut sp) = *SPEECH.lock() {
        sp.echo_canceller.enabled = enabled;
    }
}

/// Enable/disable AGC
pub fn set_agc(enabled: bool, mode: AgcMode) {
    if let Some(ref mut sp) = *SPEECH.lock() {
        sp.agc.enabled = enabled;
        sp.agc.mode = mode;
    }
}

/// Get speech probability (0..Q16_ONE)
pub fn speech_probability() -> i32 {
    if let Some(ref sp) = *SPEECH.lock() {
        sp.vad.speech_probability()
    } else {
        0
    }
}
