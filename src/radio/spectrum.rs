use crate::sync::Mutex;
/// Hoags Spectrum Analyzer — real-time RF spectrum visualization for Genesis
///
/// Provides frequency-domain analysis: sweeps, peak detection, waterfall
/// history, peak hold, frequency markers, and zoom controls. All power
/// levels use Q16 fixed-point (i32). No f32/f64. No external crates.
///
/// Inspired by: tinySA (portable spectrum analyzer), Flipper Zero spectrum
/// view, rtl_power (scan-and-log). All code is original.
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

use super::sdr::Q16;

/// Q16 constant: 1.0
const Q16_ONE: Q16 = 65536;

/// Spectrum analyzer configuration
#[derive(Debug, Clone)]
pub struct SpectrumConfig {
    /// Start frequency of the sweep range in Hz
    pub start_freq: u64,
    /// End frequency of the sweep range in Hz
    pub end_freq: u64,
    /// Resolution bandwidth in Hz (bin width)
    pub resolution: u32,
    /// Number of sweeps to average (1 = no averaging)
    pub averaging: u32,
}

impl SpectrumConfig {
    pub fn default_subghz() -> Self {
        SpectrumConfig {
            start_freq: 300_000_000,
            end_freq: 928_000_000,
            resolution: 100_000,
            averaging: 4,
        }
    }

    pub fn default_ism_433() -> Self {
        SpectrumConfig {
            start_freq: 430_000_000,
            end_freq: 440_000_000,
            resolution: 10_000,
            averaging: 8,
        }
    }

    /// Number of bins for the current configuration
    pub fn bin_count(&self) -> usize {
        if self.resolution == 0 || self.start_freq >= self.end_freq {
            return 0;
        }
        ((self.end_freq - self.start_freq) / self.resolution as u64) as usize + 1
    }
}

/// A single spectrum bin
#[derive(Debug, Clone, Copy)]
pub struct SpectrumBin {
    /// Center frequency of this bin in Hz
    pub freq_hz: u64,
    /// Measured power level in Q16 dBm
    pub power_dbm: Q16,
    /// Whether this bin is a local peak
    pub peak: bool,
}

/// A frequency marker set by the user
#[derive(Debug, Clone, Copy)]
struct FrequencyMarker {
    /// Marker frequency in Hz
    freq_hz: u64,
    /// Whether this marker is active
    active: bool,
    /// Measured power at marker (updated each sweep)
    power_dbm: Q16,
}

/// A single waterfall row (one sweep's worth of power data)
#[derive(Debug, Clone)]
struct WaterfallRow {
    /// Power levels for each bin in Q16 dBm
    bins: Vec<Q16>,
    /// Sweep timestamp (tick counter)
    timestamp: u64,
}

/// Peak detection threshold mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeakMode {
    /// Absolute threshold in Q16 dBm
    Absolute,
    /// Relative to noise floor (delta in Q16 dB)
    RelativeToFloor,
}

/// Internal spectrum analyzer state
struct SpectrumState {
    config: SpectrumConfig,
    /// Current sweep bins
    bins: Vec<SpectrumBin>,
    /// Peak hold values (max power seen per bin)
    peak_hold: Vec<Q16>,
    /// Peak hold enabled
    peak_hold_enabled: bool,
    /// Waterfall history (ring buffer of rows)
    waterfall: Vec<WaterfallRow>,
    /// Maximum waterfall rows to retain
    waterfall_max_rows: usize,
    /// Waterfall write index
    waterfall_index: usize,
    /// User-defined frequency markers (up to 8)
    markers: [FrequencyMarker; 8],
    /// Sweep counter
    sweep_count: u64,
    /// Averaging accumulator per bin (sum of Q16 values)
    avg_accum: Vec<i64>,
    /// Current position within averaging cycle
    avg_position: u32,
    /// Noise floor estimate in Q16 dBm
    noise_floor: Q16,
    /// Peak detection threshold in Q16 dBm
    peak_threshold: Q16,
    /// Peak detection mode
    peak_mode: PeakMode,
    /// Running
    running: bool,
}

impl SpectrumState {
    fn new() -> Self {
        let config = SpectrumConfig::default_subghz();
        let bin_count = config.bin_count();
        SpectrumState {
            config,
            bins: Vec::new(),
            peak_hold: vec![i32::MIN; bin_count],
            peak_hold_enabled: true,
            waterfall: Vec::new(),
            waterfall_max_rows: 128,
            waterfall_index: 0,
            markers: [FrequencyMarker {
                freq_hz: 0,
                active: false,
                power_dbm: -120 * Q16_ONE,
            }; 8],
            sweep_count: 0,
            avg_accum: vec![0i64; bin_count],
            avg_position: 0,
            noise_floor: -120 * Q16_ONE,
            peak_threshold: -90 * Q16_ONE,
            peak_mode: PeakMode::Absolute,
            running: false,
        }
    }

    /// Rebuild internal buffers when config changes
    fn rebuild_buffers(&mut self) {
        let n = self.config.bin_count();
        self.bins.clear();
        self.peak_hold = vec![i32::MIN; n];
        self.avg_accum = vec![0i64; n];
        self.avg_position = 0;
        self.waterfall.clear();
        self.waterfall_index = 0;
    }
}

static SPECTRUM_STATE: Mutex<Option<SpectrumState>> = Mutex::new(None);

/// Simple deterministic hash for simulated sweep data
fn sim_hash(freq: u64, sweep: u64) -> i64 {
    let mut h = freq ^ (sweep.wrapping_mul(0x517CC1B727220A95));
    h ^= h >> 17;
    h = h.wrapping_mul(0xBF58476D1CE4E5B9);
    h ^= h >> 31;
    (h as i64) & 0x7FFFFFFF
}

/// Start a new sweep across the configured frequency range.
/// Populates bins with measured power levels.
pub fn start_sweep() -> Result<usize, &'static str> {
    let mut state = SPECTRUM_STATE.lock();
    let sa = state.as_mut().ok_or("Spectrum analyzer not initialized")?;

    let bin_count = sa.config.bin_count();
    if bin_count == 0 {
        return Err("Invalid spectrum config: zero bins");
    }

    sa.running = true;
    sa.bins.clear();
    sa.bins.reserve(bin_count);

    let resolution = sa.config.resolution as u64;
    let sweep_num = sa.sweep_count;

    // Generate sweep data (simulated — real HW would sample each bin)
    for idx in 0..bin_count {
        let freq = sa.config.start_freq + idx as u64 * resolution;

        // Simulated power: noise floor + deterministic variation
        let variation = (sim_hash(freq, sweep_num) % 30) as Q16 - 15;
        let power = (-110 + variation) * Q16_ONE;

        // Accumulate for averaging
        if (sa.avg_position as usize) < sa.avg_accum.len() {
            if sa.avg_position == 0 {
                sa.avg_accum[idx] = power as i64;
            } else {
                sa.avg_accum[idx] += power as i64;
            }
        }

        // Update peak hold
        if sa.peak_hold_enabled && idx < sa.peak_hold.len() {
            if power > sa.peak_hold[idx] {
                sa.peak_hold[idx] = power;
            }
        }

        sa.bins.push(SpectrumBin {
            freq_hz: freq,
            power_dbm: power,
            peak: false,
        });
    }

    // Apply averaging if cycle complete
    sa.avg_position += 1;
    if sa.avg_position >= sa.config.averaging && sa.config.averaging > 1 {
        let divisor = sa.config.averaging as i64;
        for idx in 0..bin_count {
            if idx < sa.bins.len() && idx < sa.avg_accum.len() {
                sa.bins[idx].power_dbm = (sa.avg_accum[idx] / divisor) as Q16;
            }
        }
        sa.avg_position = 0;
        // Reset accumulators
        for a in sa.avg_accum.iter_mut() {
            *a = 0;
        }
    }

    // Mark peaks
    mark_peaks_internal(
        &mut sa.bins,
        sa.peak_threshold,
        sa.noise_floor,
        sa.peak_mode,
    );

    // Update marker power levels
    for marker in sa.markers.iter_mut() {
        if marker.active {
            // Find nearest bin
            for bin in &sa.bins {
                let diff = if bin.freq_hz > marker.freq_hz {
                    bin.freq_hz - marker.freq_hz
                } else {
                    marker.freq_hz - bin.freq_hz
                };
                if diff <= resolution / 2 {
                    marker.power_dbm = bin.power_dbm;
                    break;
                }
            }
        }
    }

    sa.sweep_count += 1;
    Ok(bin_count)
}

/// Mark local peaks in the bin array
fn mark_peaks_internal(
    bins: &mut Vec<SpectrumBin>,
    threshold: Q16,
    noise_floor: Q16,
    mode: PeakMode,
) {
    let len = bins.len();
    if len < 3 {
        return;
    }

    let effective_threshold = match mode {
        PeakMode::Absolute => threshold,
        PeakMode::RelativeToFloor => noise_floor + threshold,
    };

    for i in 1..len - 1 {
        let prev = bins[i - 1].power_dbm;
        let curr = bins[i].power_dbm;
        let next = bins[i + 1].power_dbm;

        bins[i].peak = curr > prev && curr > next && curr > effective_threshold;
    }
}

/// Get the current sweep bins
pub fn get_bins() -> Result<Vec<SpectrumBin>, &'static str> {
    let state = SPECTRUM_STATE.lock();
    let sa = state.as_ref().ok_or("Spectrum analyzer not initialized")?;
    Ok(sa.bins.clone())
}

/// Find all peaks in the current sweep
pub fn find_peaks() -> Result<Vec<SpectrumBin>, &'static str> {
    let state = SPECTRUM_STATE.lock();
    let sa = state.as_ref().ok_or("Spectrum analyzer not initialized")?;

    let peaks: Vec<SpectrumBin> = sa.bins.iter().filter(|b| b.peak).copied().collect();

    Ok(peaks)
}

/// Push the current sweep into the waterfall history
pub fn waterfall_update() -> Result<usize, &'static str> {
    let mut state = SPECTRUM_STATE.lock();
    let sa = state.as_mut().ok_or("Spectrum analyzer not initialized")?;

    if sa.bins.is_empty() {
        return Err("No sweep data; call start_sweep() first");
    }

    let powers: Vec<Q16> = sa.bins.iter().map(|b| b.power_dbm).collect();
    let row = WaterfallRow {
        bins: powers,
        timestamp: sa.sweep_count,
    };

    if sa.waterfall.len() < sa.waterfall_max_rows {
        sa.waterfall.push(row);
    } else {
        let idx = sa.waterfall_index % sa.waterfall_max_rows;
        sa.waterfall[idx] = row;
    }
    sa.waterfall_index += 1;

    Ok(sa.waterfall.len())
}

/// Set the frequency range for sweeps
pub fn set_range(start_freq: u64, end_freq: u64) -> Result<(), &'static str> {
    let mut state = SPECTRUM_STATE.lock();
    let sa = state.as_mut().ok_or("Spectrum analyzer not initialized")?;

    if start_freq >= end_freq {
        return Err("Start frequency must be less than end frequency");
    }

    sa.config.start_freq = start_freq;
    sa.config.end_freq = end_freq;
    sa.rebuild_buffers();

    serial_println!("Spectrum: range set to {} - {} Hz", start_freq, end_freq);
    Ok(())
}

/// Zoom in: halve the span centered on the current midpoint
pub fn zoom_in() -> Result<(), &'static str> {
    let mut state = SPECTRUM_STATE.lock();
    let sa = state.as_mut().ok_or("Spectrum analyzer not initialized")?;

    let span = sa.config.end_freq - sa.config.start_freq;
    let min_span = sa.config.resolution as u64 * 8;
    if span <= min_span {
        return Err("Already at maximum zoom");
    }

    let center = sa.config.start_freq + span / 2;
    let new_half_span = span / 4;
    sa.config.start_freq = center - new_half_span;
    sa.config.end_freq = center + new_half_span;
    sa.rebuild_buffers();

    serial_println!(
        "Spectrum: zoomed in to {} - {} Hz",
        sa.config.start_freq,
        sa.config.end_freq
    );
    Ok(())
}

/// Zoom out: double the span centered on the current midpoint
pub fn zoom_out() -> Result<(), &'static str> {
    let mut state = SPECTRUM_STATE.lock();
    let sa = state.as_mut().ok_or("Spectrum analyzer not initialized")?;

    let span = sa.config.end_freq - sa.config.start_freq;
    let center = sa.config.start_freq + span / 2;
    let new_half_span = span; // double the span

    // Clamp to valid range
    let new_start = if center > new_half_span {
        center - new_half_span
    } else {
        1_000
    };
    let new_end = center + new_half_span;
    let max_freq: u64 = 6_000_000_000;

    sa.config.start_freq = new_start;
    sa.config.end_freq = if new_end > max_freq {
        max_freq
    } else {
        new_end
    };
    sa.rebuild_buffers();

    serial_println!(
        "Spectrum: zoomed out to {} - {} Hz",
        sa.config.start_freq,
        sa.config.end_freq
    );
    Ok(())
}

/// Place a frequency marker at the given index (0-7)
pub fn mark_frequency(index: usize, freq_hz: u64) -> Result<(), &'static str> {
    let mut state = SPECTRUM_STATE.lock();
    let sa = state.as_mut().ok_or("Spectrum analyzer not initialized")?;

    if index >= 8 {
        return Err("Marker index must be 0-7");
    }

    sa.markers[index] = FrequencyMarker {
        freq_hz,
        active: true,
        power_dbm: -120 * Q16_ONE,
    };

    serial_println!("Spectrum: marker {} set to {} Hz", index, freq_hz);
    Ok(())
}

/// Get the peak hold values (maximum power seen per bin since last clear)
pub fn get_peak_hold() -> Result<Vec<Q16>, &'static str> {
    let state = SPECTRUM_STATE.lock();
    let sa = state.as_ref().ok_or("Spectrum analyzer not initialized")?;
    Ok(sa.peak_hold.clone())
}

/// Clear peak hold data and reset to minimum values
pub fn clear_peaks() -> Result<(), &'static str> {
    let mut state = SPECTRUM_STATE.lock();
    let sa = state.as_mut().ok_or("Spectrum analyzer not initialized")?;

    for p in sa.peak_hold.iter_mut() {
        *p = i32::MIN;
    }
    // Also clear peak flags on current bins
    for bin in sa.bins.iter_mut() {
        bin.peak = false;
    }

    serial_println!("Spectrum: peak hold cleared");
    Ok(())
}

/// Set the peak detection threshold in Q16 dBm
pub fn set_peak_threshold(threshold_q16: Q16) {
    let mut state = SPECTRUM_STATE.lock();
    if let Some(ref mut sa) = *state {
        sa.peak_threshold = threshold_q16;
    }
}

/// Set the resolution bandwidth (bin width) in Hz
pub fn set_resolution(resolution_hz: u32) -> Result<(), &'static str> {
    let mut state = SPECTRUM_STATE.lock();
    let sa = state.as_mut().ok_or("Spectrum analyzer not initialized")?;

    if resolution_hz == 0 {
        return Err("Resolution must be > 0");
    }

    sa.config.resolution = resolution_hz;
    sa.rebuild_buffers();
    Ok(())
}

/// Get the current sweep count
pub fn get_sweep_count() -> u64 {
    let state = SPECTRUM_STATE.lock();
    state.as_ref().map_or(0, |sa| sa.sweep_count)
}

/// Check if the analyzer is running
pub fn is_running() -> bool {
    let state = SPECTRUM_STATE.lock();
    state.as_ref().map_or(false, |sa| sa.running)
}

pub fn init() {
    let mut state = SPECTRUM_STATE.lock();
    *state = Some(SpectrumState::new());
    serial_println!("    Spectrum: analyzer ready (300 MHz - 928 MHz, peak hold, waterfall)");
}
