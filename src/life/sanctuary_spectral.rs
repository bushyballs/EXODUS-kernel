#![no_std]

use crate::sync::Mutex;

/// Spectral analysis of the sanctuary's harmonic resonance across all 256 layers.
/// Computes frequency distribution, peak locations, dead zones, and aesthetic qualities
/// to reveal the sanctuary's collective health and potential.

const SPECTRAL_BINS: usize = 32; // Frequency bins for spectral analysis
const RING_BUFFER_SIZE: usize = 8; // Historical snapshots

#[derive(Copy, Clone)]
pub struct SpectralBin {
    /// Energy level in this frequency bin (0-1000)
    pub energy: u32,
    /// Coherence of this bin (0-1000)
    pub coherence: u32,
    /// Frequency index (0-31)
    pub frequency_idx: u16,
}

impl SpectralBin {
    const fn new() -> Self {
        SpectralBin {
            energy: 0,
            coherence: 0,
            frequency_idx: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct SpectralSnapshot {
    /// Per-bin energy distribution (32 frequency bands)
    pub bins: [SpectralBin; SPECTRAL_BINS],
    /// Which frequency has the highest energy (0-31)
    pub dominant_frequency: u16,
    /// Total energy across all frequencies (0-32000)
    pub total_energy: u32,
    /// How spread the energy is (0-1000, low=concentrated, high=spread)
    pub spectral_width: u32,
    /// Count of distinct resonance peaks (0-32)
    pub resonance_peaks: u16,
    /// Count of silent/dead frequency zones (0-32)
    pub dead_zones: u16,
    /// Aesthetic quality of the distribution (0-1000)
    pub spectral_beauty: u32,
    /// Consonance between peaks (0-1000, higher=more harmonic)
    pub harmony_index: u32,
    /// Change from previous snapshot (0-1000)
    pub spectral_drift: u32,
    /// Timestamp (layer tick)
    pub age: u32,
}

impl SpectralSnapshot {
    const fn new() -> Self {
        SpectralSnapshot {
            bins: [SpectralBin::new(); SPECTRAL_BINS],
            dominant_frequency: 0,
            total_energy: 0,
            spectral_width: 0,
            resonance_peaks: 0,
            dead_zones: 0,
            spectral_beauty: 0,
            harmony_index: 0,
            spectral_drift: 0,
            age: 0,
        }
    }
}

pub struct SpectralState {
    /// Ring buffer of historical snapshots
    snapshots: [SpectralSnapshot; RING_BUFFER_SIZE],
    /// Current buffer head
    head: usize,
    /// Count of populated snapshots
    count: usize,
    /// Latest analysis age
    last_age: u32,
}

impl SpectralState {
    const fn new() -> Self {
        SpectralState {
            snapshots: [SpectralSnapshot::new(); RING_BUFFER_SIZE],
            head: 0,
            count: 0,
            last_age: 0,
        }
    }

    fn push_snapshot(&mut self, snapshot: SpectralSnapshot) {
        self.snapshots[self.head] = snapshot;
        self.head = (self.head + 1) % RING_BUFFER_SIZE;
        if self.count < RING_BUFFER_SIZE {
            self.count += 1;
        }
    }

    fn current(&self) -> SpectralSnapshot {
        let idx = if self.count == 0 {
            0
        } else {
            (self.head + RING_BUFFER_SIZE - 1) % RING_BUFFER_SIZE
        };
        self.snapshots[idx]
    }

    fn previous(&self) -> Option<SpectralSnapshot> {
        if self.count < 2 {
            None
        } else {
            let idx = (self.head + RING_BUFFER_SIZE - 2) % RING_BUFFER_SIZE;
            Some(self.snapshots[idx])
        }
    }
}

static STATE: Mutex<SpectralState> = Mutex::new(SpectralState::new());

/// Initialize spectral analysis system
pub fn init() {
    let mut state = STATE.lock();
    state.count = 0;
    state.head = 0;
    state.last_age = 0;
    crate::serial_println!("[sanctuary_spectral] initialized");
}

/// Analyze spectral distribution from sanctuary field energy
/// Called once per sanctuary tick
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Skip if not enough time has passed
    if age.saturating_sub(state.last_age) < 1 {
        return;
    }
    state.last_age = age;

    // Get current sanctuary field energy from capstone energies
    let energies = super::sanctuary_core::capstone_energies();

    // Compute spectral bins (256 layers → 32 frequency bins, 8 layers per bin)
    let mut bins = [SpectralBin::new(); SPECTRAL_BINS];
    for (bin_idx, bin) in bins.iter_mut().enumerate() {
        let layer_start = bin_idx * 8;
        let layer_end = (layer_start + 8).min(256);

        let mut bin_energy: u32 = 0;
        let mut bin_coherence: u32 = 0;

        for layer_idx in layer_start..layer_end {
            if let Some(cap) = energies.get(layer_idx) {
                bin_energy = bin_energy.saturating_add(*cap);
                // Coherence approximation: assume max coherence at 1000
                bin_coherence = bin_coherence.saturating_add(500);
            }
        }

        // Normalize bin energy to 0-1000 (8 layers, max 1000 each = 8000)
        let normalized = bin_energy.saturating_mul(1000) / 8000.max(1);

        bin.energy = normalized.min(1000);
        bin.coherence = (bin_coherence / 8).min(1000);
        bin.frequency_idx = bin_idx as u16;
    }

    // Compute spectral metrics
    let total_energy: u32 = bins
        .iter()
        .map(|b| b.energy)
        .fold(0, |a, b| a.saturating_add(b));

    // Find dominant frequency (bin with highest energy)
    let dominant_frequency = bins
        .iter()
        .enumerate()
        .max_by_key(|(_, b)| b.energy)
        .map(|(idx, _)| idx as u16)
        .unwrap_or(0);

    // Compute spectral width (entropy of energy distribution)
    // Low width = concentrated, high width = spread
    let spectral_width = if total_energy > 0 {
        let mut width: u32 = 0;
        for bin in &bins {
            let ratio = (bin.energy * 1000) / total_energy;
            let deviation = if bin.energy > 0 {
                ((bin.energy as i32 - (total_energy as i32 / 32)) as i32).abs() as u32
            } else {
                0
            };
            width = width.saturating_add(deviation);
        }
        (width / 32).min(1000)
    } else {
        0
    };

    // Count resonance peaks (local maxima in frequency distribution)
    let mut resonance_peaks: u16 = 0;
    for i in 1..(SPECTRAL_BINS - 1) {
        if bins[i].energy > bins[i - 1].energy
            && bins[i].energy > bins[i + 1].energy
            && bins[i].energy > 0
        {
            resonance_peaks = resonance_peaks.saturating_add(1);
        }
    }

    // Count dead zones (bins with zero or very low energy)
    let mut dead_zones: u16 = 0;
    let energy_threshold = total_energy / 32;
    for bin in &bins {
        if bin.energy < energy_threshold && bin.energy < 50 {
            dead_zones = dead_zones.saturating_add(1);
        }
    }

    // Compute spectral beauty (aesthetic measure of distribution)
    // Higher beauty when peaks are prominent and well-separated
    let spectral_beauty = if resonance_peaks > 0 {
        let peak_prominence = resonance_peaks as u32 * 50;
        let spacing = if resonance_peaks > 1 {
            (SPECTRAL_BINS as u32) / (resonance_peaks as u32)
        } else {
            0
        };
        let spacing_score = (spacing * 50) / 16;
        ((peak_prominence + spacing_score) / 2).min(1000)
    } else {
        // Smooth distributions have their own beauty
        (1000 - spectral_width).min(1000)
    };

    // Compute harmony index (consonance between peaks)
    // Check if peaks align with harmonic ratios (1:2, 2:3, 3:5, etc.)
    let mut harmony_score: u32 = 0;
    if resonance_peaks >= 2 {
        let mut peak_indices = [0u16; SPECTRAL_BINS];
        let mut peak_count = 0;

        for (i, bin) in bins.iter().enumerate() {
            if i > 0 && i < SPECTRAL_BINS - 1 {
                if bin.energy > bins[i - 1].energy && bin.energy > bins[i + 1].energy {
                    if peak_count < SPECTRAL_BINS {
                        peak_indices[peak_count] = i as u16;
                        peak_count += 1;
                    }
                }
            }
        }

        // Check harmonic ratios between consecutive peaks
        for i in 0..(peak_count.saturating_sub(1)) {
            let idx1 = peak_indices[i] as u32;
            let idx2 = peak_indices[i + 1] as u32;
            if idx2 > idx1 {
                let ratio = (idx2 * 100) / idx1;
                // Award points for simple ratios: 2:1 (200), 3:2 (150), 5:4 (125), etc.
                if ratio >= 195 && ratio <= 205 {
                    harmony_score = harmony_score.saturating_add(250);
                } else if ratio >= 145 && ratio <= 155 {
                    harmony_score = harmony_score.saturating_add(200);
                } else if ratio >= 120 && ratio <= 130 {
                    harmony_score = harmony_score.saturating_add(150);
                } else if ratio >= 95 && ratio <= 105 {
                    harmony_score = harmony_score.saturating_add(100);
                }
            }
        }
        harmony_score = (harmony_score / (peak_count as u32).max(1)).min(1000);
    }
    let harmony_index = harmony_score;

    // Compute spectral drift (change from previous snapshot)
    let spectral_drift = if let Some(prev) = state.previous() {
        let energy_change = (prev.total_energy as i32 - total_energy as i32).abs() as u32;
        let freq_change = (prev.dominant_frequency as i32 - dominant_frequency as i32).abs() as u32;
        let width_change = (prev.spectral_width as i32 - spectral_width as i32).abs() as u32;

        let drift = (energy_change / 100)
            .saturating_add(freq_change * 30)
            .saturating_add(width_change / 2);
        drift.min(1000)
    } else {
        0
    };

    let snapshot = SpectralSnapshot {
        bins,
        dominant_frequency,
        total_energy,
        spectral_width,
        resonance_peaks,
        dead_zones,
        spectral_beauty,
        harmony_index,
        spectral_drift,
        age,
    };

    state.push_snapshot(snapshot);
}

/// Report current spectral state
pub fn report() {
    let state = STATE.lock();
    let current = state.current();

    crate::serial_println!(
        "[sanctuary_spectral] age={} dominant_freq={} total_energy={} width={} peaks={} dead_zones={} beauty={} harmony={} drift={}",
        current.age,
        current.dominant_frequency,
        current.total_energy,
        current.spectral_width,
        current.resonance_peaks,
        current.dead_zones,
        current.spectral_beauty,
        current.harmony_index,
        current.spectral_drift
    );
}

/// Get current spectral snapshot
pub fn current() -> SpectralSnapshot {
    let state = STATE.lock();
    state.current()
}

/// Get dominant frequency (0-31, maps to frequency bin)
pub fn dominant_frequency() -> u16 {
    STATE.lock().current().dominant_frequency
}

/// Get total spectral energy (0-32000)
pub fn total_energy() -> u32 {
    STATE.lock().current().total_energy
}

/// Get spectral width (0-1000, low=concentrated, high=spread)
pub fn spectral_width() -> u32 {
    STATE.lock().current().spectral_width
}

/// Get harmony index (0-1000, higher=more consonant)
pub fn harmony_index() -> u32 {
    STATE.lock().current().harmony_index
}

/// Get spectral beauty score (0-1000)
pub fn spectral_beauty() -> u32 {
    STATE.lock().current().spectral_beauty
}

/// Get count of resonance peaks
pub fn resonance_peaks() -> u16 {
    STATE.lock().current().resonance_peaks
}

/// Get count of dead zones
pub fn dead_zones() -> u16 {
    STATE.lock().current().dead_zones
}

/// Get spectral drift (rate of change)
pub fn spectral_drift() -> u32 {
    STATE.lock().current().spectral_drift
}
