use crate::sync::Mutex;
/// Hoags SDR — Software-Defined Radio core for Genesis
///
/// Provides the low-level radio interface: IQ sampling, frequency tuning,
/// gain control, AGC, band scanning, and signal detection. All DSP uses
/// Q16 fixed-point arithmetic (i32, 16 fractional bits). No f32/f64.
///
/// Inspired by: RTL-SDR (device model), GNU Radio (block architecture),
/// HackRF (full-duplex TX/RX). All code is original.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

/// Q16 fixed-point: 1.0 = 65536, 0.5 = 32768, -1.0 = -65536
pub type Q16 = i32;

/// Q16 constant: 1.0
const Q16_ONE: Q16 = 65536;

/// Q16 multiply: (a * b) >> 16
#[inline]
fn q16_mul(a: Q16, b: Q16) -> Q16 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// SDR operating mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdrMode {
    /// Receive mode — sampling incoming RF
    Receive,
    /// Transmit mode — outputting RF signal
    Transmit,
    /// Scan mode — sweeping across a frequency range
    Scan,
    /// Monitor mode — continuous passive listening
    Monitor,
}

/// SDR hardware configuration
#[derive(Debug, Clone)]
pub struct SdrConfig {
    /// Center frequency in Hz (e.g. 433_920_000 for 433.92 MHz)
    pub center_freq_hz: u64,
    /// Sample rate in samples/sec (e.g. 2_048_000 for 2.048 MS/s)
    pub sample_rate: u32,
    /// RF bandwidth in Hz
    pub bandwidth: u32,
    /// Gain in Q16 fixed-point (0 = 0 dB, 65536 = 1.0 normalized)
    pub gain: Q16,
    /// Automatic gain control enabled
    pub agc: bool,
}

impl SdrConfig {
    pub fn default_subghz() -> Self {
        SdrConfig {
            center_freq_hz: 433_920_000,
            sample_rate: 2_048_000,
            bandwidth: 200_000,
            gain: Q16_ONE / 2, // 0.5 normalized gain
            agc: true,
        }
    }

    pub fn default_fm_broadcast() -> Self {
        SdrConfig {
            center_freq_hz: 100_000_000,
            sample_rate: 2_048_000,
            bandwidth: 200_000,
            gain: Q16_ONE / 4,
            agc: true,
        }
    }
}

/// A single IQ (in-phase / quadrature) sample
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IqSample {
    /// In-phase component (signed 16-bit)
    pub i: i16,
    /// Quadrature component (signed 16-bit)
    pub q: i16,
}

impl IqSample {
    /// Compute magnitude squared: i^2 + q^2 (avoids sqrt)
    pub fn magnitude_sq(&self) -> u32 {
        (self.i as i32 * self.i as i32 + self.q as i32 * self.q as i32) as u32
    }

    /// Compute magnitude in Q16 using integer Newton's method sqrt approximation
    pub fn magnitude_q16(&self) -> Q16 {
        let mag_sq = self.magnitude_sq() as i64;
        // Shift left 16 for Q16, then integer sqrt
        let scaled = mag_sq << 16;
        isqrt_i64(scaled) as Q16
    }
}

/// Integer square root via Newton's method (64-bit)
fn isqrt_i64(n: i64) -> i64 {
    if n <= 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// Detected signal descriptor
#[derive(Debug, Clone)]
pub struct DetectedSignal {
    /// Center frequency of the detected signal
    pub frequency_hz: u64,
    /// Signal strength in Q16 dBm
    pub strength_dbm: Q16,
    /// Estimated bandwidth in Hz
    pub bandwidth_hz: u32,
    /// Whether signal is currently active
    pub active: bool,
}

/// Internal SDR state
struct SdrState {
    config: SdrConfig,
    mode: SdrMode,
    running: bool,
    sample_buffer: Vec<IqSample>,
    buffer_capacity: usize,
    detected_signals: Vec<DetectedSignal>,
    noise_floor_q16: Q16,
    squelch_level_q16: Q16,
    /// AGC current gain in Q16
    agc_gain: Q16,
    /// Sample counter for timing
    sample_count: u64,
}

impl SdrState {
    fn new() -> Self {
        SdrState {
            config: SdrConfig::default_subghz(),
            mode: SdrMode::Receive,
            running: false,
            sample_buffer: Vec::new(),
            buffer_capacity: 4096,
            detected_signals: Vec::new(),
            noise_floor_q16: -120 * Q16_ONE,   // -120 dBm
            squelch_level_q16: -100 * Q16_ONE, // -100 dBm
            agc_gain: Q16_ONE,
            sample_count: 0,
        }
    }
}

static SDR_STATE: Mutex<Option<SdrState>> = Mutex::new(None);

/// Configure the SDR hardware with the given parameters
pub fn configure(config: SdrConfig) -> Result<(), &'static str> {
    let mut state = SDR_STATE.lock();
    let sdr = state.as_mut().ok_or("SDR not initialized")?;

    if sdr.running {
        return Err("Cannot reconfigure while running; stop first");
    }

    // Validate frequency range (1 kHz to 6 GHz)
    if config.center_freq_hz < 1_000 || config.center_freq_hz > 6_000_000_000 {
        return Err("Frequency out of range (1 kHz - 6 GHz)");
    }

    // Validate sample rate (1 kS/s to 20 MS/s)
    if config.sample_rate < 1_000 || config.sample_rate > 20_000_000 {
        return Err("Sample rate out of range (1 kS/s - 20 MS/s)");
    }

    // Validate bandwidth
    if config.bandwidth == 0 || config.bandwidth > config.sample_rate {
        return Err("Bandwidth must be > 0 and <= sample rate");
    }

    serial_println!(
        "SDR: configured freq={}Hz sr={}S/s bw={}Hz",
        config.center_freq_hz,
        config.sample_rate,
        config.bandwidth
    );

    sdr.config = config;
    Ok(())
}

/// Start receiving IQ samples
pub fn start_rx() -> Result<(), &'static str> {
    let mut state = SDR_STATE.lock();
    let sdr = state.as_mut().ok_or("SDR not initialized")?;

    if sdr.running {
        return Err("SDR already running");
    }

    sdr.mode = SdrMode::Receive;
    sdr.running = true;
    sdr.sample_buffer.clear();
    sdr.sample_count = 0;
    sdr.agc_gain = Q16_ONE;

    serial_println!("SDR: RX started at {} Hz", sdr.config.center_freq_hz);
    Ok(())
}

/// Stop the SDR (any mode)
pub fn stop() -> Result<(), &'static str> {
    let mut state = SDR_STATE.lock();
    let sdr = state.as_mut().ok_or("SDR not initialized")?;

    if !sdr.running {
        return Err("SDR not running");
    }

    sdr.running = false;
    serial_println!("SDR: stopped after {} samples", sdr.sample_count);
    Ok(())
}

/// Retune to a new center frequency without stopping
pub fn set_frequency(freq_hz: u64) -> Result<(), &'static str> {
    let mut state = SDR_STATE.lock();
    let sdr = state.as_mut().ok_or("SDR not initialized")?;

    if freq_hz < 1_000 || freq_hz > 6_000_000_000 {
        return Err("Frequency out of range (1 kHz - 6 GHz)");
    }

    let old_freq = sdr.config.center_freq_hz;
    sdr.config.center_freq_hz = freq_hz;
    serial_println!("SDR: retuned {} -> {} Hz", old_freq, freq_hz);
    Ok(())
}

/// Set gain (Q16 fixed-point). 0 = minimum, Q16_ONE = maximum.
pub fn set_gain(gain: Q16) -> Result<(), &'static str> {
    let mut state = SDR_STATE.lock();
    let sdr = state.as_mut().ok_or("SDR not initialized")?;

    sdr.config.gain = gain;
    // Disable AGC when manually setting gain
    sdr.config.agc = false;
    Ok(())
}

/// Read available IQ samples from the buffer.
/// Returns samples collected since last read, clearing the internal buffer.
pub fn read_samples() -> Result<Vec<IqSample>, &'static str> {
    let mut state = SDR_STATE.lock();
    let sdr = state.as_mut().ok_or("SDR not initialized")?;

    if !sdr.running {
        return Err("SDR not running");
    }

    // In a real implementation, the hardware interrupt handler would
    // fill the sample buffer via DMA. Here we simulate with a stub
    // that produces synthesized test samples for bring-up.
    if sdr.sample_buffer.is_empty() {
        generate_test_samples(sdr);
    }

    let samples = core::mem::replace(&mut sdr.sample_buffer, Vec::new());
    Ok(samples)
}

/// Generate synthetic test samples for bring-up / self-test.
/// Produces a simple tone at center_freq + 10 kHz offset.
fn generate_test_samples(sdr: &mut SdrState) {
    let count = 256;
    sdr.sample_buffer.reserve(count);

    // Simple sinusoidal approximation using integer math.
    // Phase increment per sample for a 10 kHz tone:
    //   phase_inc = (10000 * 65536) / sample_rate
    let phase_inc: i32 = if sdr.config.sample_rate > 0 {
        ((10_000_i64 * 65536) / sdr.config.sample_rate as i64) as i32
    } else {
        0
    };

    let mut phase: i32 = (sdr.sample_count & 0xFFFF) as i32;

    for _ in 0..count {
        // Integer sine approximation: triangle wave scaled to +/- 16384
        let quarter = phase & 0x3FFF; // 0..16383
        let half = phase & 0x7FFF; // upper bit = descending
        let sine_approx: i16 = if half < 0x4000 {
            quarter as i16
        } else if half < 0x8000 {
            (0x7FFF - half) as i16
        } else {
            -((half - 0x8000) as i16)
        };

        // Cosine is sine shifted by quarter period
        let cos_phase = phase.wrapping_add(0x4000);
        let cos_quarter = cos_phase & 0x3FFF;
        let cos_half = cos_phase & 0x7FFF;
        let cos_approx: i16 = if cos_half < 0x4000 {
            cos_quarter as i16
        } else if cos_half < 0x8000 {
            (0x7FFF - cos_half) as i16
        } else {
            -((cos_half - 0x8000) as i16)
        };

        // Apply gain
        let gi = q16_mul(sine_approx as i32, sdr.agc_gain) as i16;
        let gq = q16_mul(cos_approx as i32, sdr.agc_gain) as i16;

        sdr.sample_buffer.push(IqSample { i: gi, q: gq });
        phase = phase.wrapping_add(phase_inc);
        sdr.sample_count += 1;
    }
}

/// Get current signal strength (RSSI) in Q16 dBm.
/// Computed from the most recent samples in the buffer.
pub fn get_signal_strength() -> Result<Q16, &'static str> {
    let mut state = SDR_STATE.lock();
    let sdr = state.as_mut().ok_or("SDR not initialized")?;

    if !sdr.running {
        return Err("SDR not running");
    }

    // If buffer empty, generate fresh samples
    if sdr.sample_buffer.is_empty() {
        generate_test_samples(sdr);
    }

    if sdr.sample_buffer.is_empty() {
        return Ok(sdr.noise_floor_q16);
    }

    // Average magnitude squared over buffer
    let mut sum: u64 = 0;
    let count = sdr.sample_buffer.len();
    for s in &sdr.sample_buffer {
        sum += s.magnitude_sq() as u64;
    }
    let avg_mag_sq = (sum / count as u64) as i32;

    // Rough dBm approximation using integer log2:
    // dBm ~ 10*log10(power) ~ 3 * log2(power) in Q16
    let log2_val = integer_log2(avg_mag_sq as u32);
    let dbm = (log2_val as i32 * 3 - 120) * Q16_ONE;

    Ok(dbm)
}

/// Fast integer log2 (floor)
fn integer_log2(mut val: u32) -> u32 {
    if val == 0 {
        return 0;
    }
    let mut log = 0u32;
    while val > 1 {
        val >>= 1;
        log += 1;
    }
    log
}

/// Scan a frequency range, stepping by `step_hz`, recording signal strength at each step.
/// Returns a vector of (frequency_hz, power_dbm_q16) tuples.
pub fn scan_range(
    start_hz: u64,
    end_hz: u64,
    step_hz: u64,
) -> Result<Vec<(u64, Q16)>, &'static str> {
    if start_hz >= end_hz {
        return Err("Start frequency must be less than end frequency");
    }
    if step_hz == 0 {
        return Err("Step size must be > 0");
    }

    let mut results: Vec<(u64, Q16)> = Vec::new();
    let num_steps = ((end_hz - start_hz) / step_hz) as usize + 1;
    if num_steps > 10_000 {
        return Err("Too many steps; increase step size or reduce range");
    }

    serial_println!(
        "SDR: scanning {} - {} Hz, step={} Hz ({} steps)",
        start_hz,
        end_hz,
        step_hz,
        num_steps
    );

    let mut freq = start_hz;
    while freq <= end_hz {
        // Simulate per-frequency measurement
        // In real hardware, we'd retune and measure each step
        let hash = simple_freq_hash(freq);
        let noise = (hash % 40) as Q16 - 20; // -20 to +19 dB variation
        let base_power: Q16 = (-110 + noise) * Q16_ONE;
        results.push((freq, base_power));
        freq += step_hz;
    }

    Ok(results)
}

/// Simple deterministic hash for test data generation
fn simple_freq_hash(freq: u64) -> u64 {
    let mut h = freq;
    h ^= h >> 17;
    h = h.wrapping_mul(0xBF58476D1CE4E5B9);
    h ^= h >> 31;
    h = h.wrapping_mul(0x94D049BB133111EB);
    h ^= h >> 32;
    h
}

/// Find active signals above the squelch threshold.
/// Scans the given range and clusters adjacent bins above threshold.
pub fn find_signals(
    start_hz: u64,
    end_hz: u64,
    step_hz: u64,
) -> Result<Vec<DetectedSignal>, &'static str> {
    let scan_data = scan_range(start_hz, end_hz, step_hz)?;

    let squelch = {
        let state = SDR_STATE.lock();
        let sdr = state.as_ref().ok_or("SDR not initialized")?;
        sdr.squelch_level_q16
    };

    let mut signals: Vec<DetectedSignal> = Vec::new();
    let mut in_signal = false;
    let mut sig_start_freq: u64 = 0;
    let mut sig_peak_power: Q16 = i32::MIN;
    let mut sig_peak_freq: u64 = 0;

    for &(freq, power) in &scan_data {
        if power > squelch {
            if !in_signal {
                in_signal = true;
                sig_start_freq = freq;
                sig_peak_power = power;
                sig_peak_freq = freq;
            } else if power > sig_peak_power {
                sig_peak_power = power;
                sig_peak_freq = freq;
            }
        } else if in_signal {
            // End of signal cluster
            let bw = (freq - sig_start_freq) as u32;
            signals.push(DetectedSignal {
                frequency_hz: sig_peak_freq,
                strength_dbm: sig_peak_power,
                bandwidth_hz: if bw > 0 { bw } else { step_hz as u32 },
                active: true,
            });
            in_signal = false;
            sig_peak_power = i32::MIN;
        }
    }

    // Close trailing signal
    if in_signal {
        let bw = (end_hz - sig_start_freq) as u32;
        signals.push(DetectedSignal {
            frequency_hz: sig_peak_freq,
            strength_dbm: sig_peak_power,
            bandwidth_hz: if bw > 0 { bw } else { step_hz as u32 },
            active: true,
        });
    }

    serial_println!(
        "SDR: found {} signals in {} - {} Hz",
        signals.len(),
        start_hz,
        end_hz
    );
    Ok(signals)
}

/// Set the squelch (noise gate) level in Q16 dBm
pub fn set_squelch(level_dbm_q16: Q16) {
    let mut state = SDR_STATE.lock();
    if let Some(ref mut sdr) = *state {
        sdr.squelch_level_q16 = level_dbm_q16;
    }
}

/// Get current SDR mode
pub fn get_mode() -> Option<SdrMode> {
    let state = SDR_STATE.lock();
    state.as_ref().map(|s| s.mode)
}

/// Check if SDR is currently running
pub fn is_running() -> bool {
    let state = SDR_STATE.lock();
    state.as_ref().map_or(false, |s| s.running)
}

/// Get current center frequency
pub fn get_frequency() -> Option<u64> {
    let state = SDR_STATE.lock();
    state.as_ref().map(|s| s.config.center_freq_hz)
}

pub fn init() {
    let mut state = SDR_STATE.lock();
    *state = Some(SdrState::new());
    serial_println!("    SDR: initialized (1 kHz - 6 GHz, IQ sampling, AGC)");
}
