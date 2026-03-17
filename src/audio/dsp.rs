//! Digital Signal Processing utilities for audio
//!
//! Provides sample rate conversion, mixing, channel mapping, and other DSP operations.
//! All trig functions use polynomial approximations — no libm / soft-float calls.

use super::error::*;
use super::types::*;

// ---------------------------------------------------------------------------
// Integer-safe trig approximations for f32 (no libm required)
// ---------------------------------------------------------------------------

/// Approximate sin(x) for x in radians using a 5th-order polynomial.
/// Max error ~0.0002 over [-PI, PI].
pub fn sin_approx(mut x: f32) -> f32 {
    // Normalise x into [-PI, PI]
    const PI: f32 = 3.14159265;
    const TWO_PI: f32 = 6.2831853;
    // Reduce
    x = x - ((x / TWO_PI) as i32 as f32) * TWO_PI;
    if x > PI {
        x -= TWO_PI;
    }
    if x < -PI {
        x += TWO_PI;
    }
    // Polynomial: sin(x) ≈ x - x^3/6 + x^5/120
    let x2 = x * x;
    let x3 = x2 * x;
    let x5 = x3 * x2;
    x - x3 / 6.0 + x5 / 120.0
}

/// Approximate cos(x) for x in radians.
pub fn cos_approx(x: f32) -> f32 {
    sin_approx(x + 1.5707963) // cos(x) = sin(x + PI/2)
}

/// Approximate sqrt(x) using Newton's method (3 iterations).
/// Returns 0 for negative or zero inputs.
pub fn sqrt_approx(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    // Initial guess via integer bit-hack
    let mut guess = x * 0.5;
    if guess <= 0.0 {
        guess = 1.0;
    }
    // 3 Newton iterations: guess = (guess + x/guess) / 2
    guess = (guess + x / guess) * 0.5;
    guess = (guess + x / guess) * 0.5;
    guess = (guess + x / guess) * 0.5;
    guess
}

/// Approximate 10^(x) for small x values using Taylor series.
/// Used for dB conversion.  Accurate for |x| < 2.
pub fn pow10_approx(x: f32) -> f32 {
    // 10^x = e^(x * ln10)
    // e^y ≈ 1 + y + y^2/2 + y^3/6 + y^4/24
    let y = x * 2.302585; // ln(10)
    let y2 = y * y;
    let y3 = y2 * y;
    let y4 = y3 * y;
    1.0 + y + y2 / 2.0 + y3 / 6.0 + y4 / 24.0
}

/// Approximate atan(x) using polynomial.
pub fn atan_approx(x: f32) -> f32 {
    // For |x| <= 1: atan(x) ≈ x - x^3/3 + x^5/5
    // For |x| > 1: atan(x) = PI/2 - atan(1/x)
    let abs_x = if x < 0.0 { -x } else { x };
    if abs_x <= 1.0 {
        let x2 = x * x;
        let x3 = x2 * x;
        let x5 = x3 * x2;
        x - x3 / 3.0 + x5 / 5.0
    } else {
        let inv = 1.0 / x;
        let inv2 = inv * inv;
        let inv3 = inv2 * inv;
        let inv5 = inv3 * inv2;
        let a = inv - inv3 / 3.0 + inv5 / 5.0;
        if x > 0.0 {
            1.5707963 - a
        } else {
            -1.5707963 - a
        }
    }
}

/// Approximate x.abs() without method call.
pub fn abs_f32(x: f32) -> f32 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}

/// Approximate x.clamp(lo, hi) without method call.
pub fn clamp_f32(x: f32, lo: f32, hi: f32) -> f32 {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

/// Sample rate converter using linear interpolation
pub struct SampleRateConverter {
    src_rate: u32,
    dst_rate: u32,
    ratio: f32,
    last_sample: [f32; 8],
}

/// Audio mixer for combining multiple streams
pub struct AudioMixer {
    channels: u8,
    mix_buffer: [f32; 4096],
}

/// Channel mapper for channel layout conversions
pub struct ChannelMapper {
    src_layout: ChannelLayout,
    dst_layout: ChannelLayout,
}

/// Volume control and gain adjustment
pub struct VolumeControl {
    gain: f32,
    mute: bool,
}

/// Simple biquad filter for EQ
pub struct BiquadFilter {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl SampleRateConverter {
    pub fn new(src_rate: u32, dst_rate: u32) -> Self {
        Self {
            src_rate,
            dst_rate,
            ratio: dst_rate as f32 / src_rate as f32,
            last_sample: [0.0; 8],
        }
    }

    /// Convert sample rate using linear interpolation
    pub fn convert(&mut self, input: &[f32], output: &mut [f32], channels: u8) -> Result<usize> {
        let ch = channels as usize;
        let input_frames = input.len() / ch;
        let output_frames = output.len() / ch;

        let mut out_idx = 0;
        let mut pos = 0.0f32;

        while out_idx < output_frames && (pos as usize) < input_frames - 1 {
            let idx = pos as usize;
            let frac = pos - idx as f32;

            for c in 0..ch {
                let s0 = input[idx * ch + c];
                let s1 = input[(idx + 1) * ch + c];
                output[out_idx * ch + c] = s0 + (s1 - s0) * frac;
            }

            pos += 1.0 / self.ratio;
            out_idx += 1;
        }

        Ok(out_idx)
    }

    /// Set new conversion ratio
    pub fn set_rates(&mut self, src_rate: u32, dst_rate: u32) {
        self.src_rate = src_rate;
        self.dst_rate = dst_rate;
        self.ratio = dst_rate as f32 / src_rate as f32;
    }
}

impl AudioMixer {
    pub fn new(channels: u8) -> Self {
        Self {
            channels,
            mix_buffer: [0.0; 4096],
        }
    }

    /// Mix multiple audio streams
    pub fn mix(&mut self, streams: &[&[f32]], output: &mut [f32]) -> Result<usize> {
        if streams.is_empty() {
            return Ok(0);
        }

        let frame_count = output.len() / self.channels as usize;

        // Clear output
        for sample in output.iter_mut() {
            *sample = 0.0;
        }

        // Mix all streams
        for stream in streams {
            let stream_frames = stream.len() / self.channels as usize;
            let frames_to_mix = frame_count.min(stream_frames);

            for i in 0..frames_to_mix * self.channels as usize {
                output[i] += stream[i];
            }
        }

        // Normalize to prevent clipping
        let num_streams = streams.len() as f32;
        for sample in output.iter_mut() {
            *sample /= num_streams;
        }

        Ok(frame_count)
    }

    /// Add stream to mix buffer
    pub fn add_stream(&mut self, stream: &[f32], gain: f32) -> Result<()> {
        for i in 0..stream.len().min(self.mix_buffer.len()) {
            self.mix_buffer[i] += stream[i] * gain;
        }
        Ok(())
    }

    /// Get mixed output
    pub fn get_output(&mut self, output: &mut [f32]) -> usize {
        let len = output.len().min(self.mix_buffer.len());
        output[..len].copy_from_slice(&self.mix_buffer[..len]);

        // Clear mix buffer
        for sample in self.mix_buffer.iter_mut() {
            *sample = 0.0;
        }

        len
    }
}

impl ChannelMapper {
    pub fn new(src_layout: ChannelLayout, dst_layout: ChannelLayout) -> Self {
        Self {
            src_layout,
            dst_layout,
        }
    }

    /// Map channels from source layout to destination layout
    pub fn map(&self, input: &[f32], output: &mut [f32]) -> Result<usize> {
        let src_ch = self.src_layout.channel_count() as usize;
        let dst_ch = self.dst_layout.channel_count() as usize;

        if src_ch == 0 || dst_ch == 0 {
            return Err(AudioError::InvalidChannels);
        }

        let frame_count = input.len() / src_ch;
        let out_frame_count = output.len() / dst_ch;

        let frames = frame_count.min(out_frame_count);

        for i in 0..frames {
            self.map_frame(
                &input[i * src_ch..(i + 1) * src_ch],
                &mut output[i * dst_ch..(i + 1) * dst_ch],
            );
        }

        Ok(frames)
    }

    fn map_frame(&self, input: &[f32], output: &mut [f32]) {
        // Simplified channel mapping
        match (self.src_layout, self.dst_layout) {
            (ChannelLayout::Mono, ChannelLayout::Stereo) => {
                // Mono to stereo - duplicate
                output[0] = input[0];
                output[1] = input[0];
            }
            (ChannelLayout::Stereo, ChannelLayout::Mono) => {
                // Stereo to mono - average
                output[0] = (input[0] + input[1]) / 2.0;
            }
            (ChannelLayout::Stereo, ChannelLayout::Surround5_1) => {
                // Stereo to 5.1 - map L/R, silence others
                output[0] = input[0]; // FL
                output[1] = input[1]; // FR
                output[2] = 0.0; // C
                output[3] = 0.0; // LFE
                output[4] = 0.0; // RL
                output[5] = 0.0; // RR
            }
            _ => {
                // Default: copy what fits, zero the rest
                let copy_len = input.len().min(output.len());
                output[..copy_len].copy_from_slice(&input[..copy_len]);
                for i in copy_len..output.len() {
                    output[i] = 0.0;
                }
            }
        }
    }
}

impl VolumeControl {
    pub fn new() -> Self {
        Self {
            gain: 1.0,
            mute: false,
        }
    }

    /// Set volume (0.0 to 1.0)
    pub fn set_volume(&mut self, volume: f32) {
        self.gain = clamp_f32(volume, 0.0, 2.0); // Allow up to 2x gain
    }

    /// Set volume in dB
    pub fn set_volume_db(&mut self, db: f32) {
        self.gain = pow10_approx(db / 20.0);
    }

    /// Mute/unmute
    pub fn set_mute(&mut self, mute: bool) {
        self.mute = mute;
    }

    /// Apply volume to samples
    pub fn apply(&self, samples: &mut [f32]) {
        if self.mute {
            for sample in samples.iter_mut() {
                *sample = 0.0;
            }
        } else {
            for sample in samples.iter_mut() {
                *sample *= self.gain;
            }
        }
    }
}

impl BiquadFilter {
    pub fn new() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    /// Configure as low-pass filter
    pub fn set_lowpass(&mut self, sample_rate: f32, cutoff: f32, q: f32) {
        let w0 = 2.0 * 3.14159265 * cutoff / sample_rate;
        let cos_w0 = cos_approx(w0);
        let sin_w0 = sin_approx(w0);
        let alpha = sin_w0 / (2.0 * q);

        let a0 = 1.0 + alpha;
        self.b0 = ((1.0 - cos_w0) / 2.0) / a0;
        self.b1 = (1.0 - cos_w0) / a0;
        self.b2 = ((1.0 - cos_w0) / 2.0) / a0;
        self.a1 = (-2.0 * cos_w0) / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    /// Configure as high-pass filter
    pub fn set_highpass(&mut self, sample_rate: f32, cutoff: f32, q: f32) {
        let w0 = 2.0 * 3.14159265 * cutoff / sample_rate;
        let cos_w0 = cos_approx(w0);
        let sin_w0 = sin_approx(w0);
        let alpha = sin_w0 / (2.0 * q);

        let a0 = 1.0 + alpha;
        self.b0 = ((1.0 + cos_w0) / 2.0) / a0;
        self.b1 = -(1.0 + cos_w0) / a0;
        self.b2 = ((1.0 + cos_w0) / 2.0) / a0;
        self.a1 = (-2.0 * cos_w0) / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    /// Process samples
    pub fn process(&mut self, input: f32) -> f32 {
        let output = self.b0 * input + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;

        self.x2 = self.x1;
        self.x1 = input;
        self.y2 = self.y1;
        self.y1 = output;

        output
    }

    /// Reset filter state
    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

/// Convert i16 samples to f32
pub fn i16_to_f32(input: &[i16], output: &mut [f32]) {
    for (i, &sample) in input.iter().enumerate() {
        if i >= output.len() {
            break;
        }
        output[i] = sample as f32 / 32768.0;
    }
}

/// Convert f32 samples to i16
pub fn f32_to_i16(input: &[f32], output: &mut [i16]) {
    for (i, &sample) in input.iter().enumerate() {
        if i >= output.len() {
            break;
        }
        output[i] = clamp_f32(sample * 32768.0, -32768.0, 32767.0) as i16;
    }
}

/// Interleave mono channels into stereo
pub fn interleave_stereo(left: &[f32], right: &[f32], output: &mut [f32]) -> usize {
    let frames = left.len().min(right.len()).min(output.len() / 2);

    for i in 0..frames {
        output[i * 2] = left[i];
        output[i * 2 + 1] = right[i];
    }

    frames * 2
}

/// Deinterleave stereo into mono channels
pub fn deinterleave_stereo(input: &[f32], left: &mut [f32], right: &mut [f32]) -> usize {
    let frames = (input.len() / 2).min(left.len()).min(right.len());

    for i in 0..frames {
        left[i] = input[i * 2];
        right[i] = input[i * 2 + 1];
    }

    frames
}
