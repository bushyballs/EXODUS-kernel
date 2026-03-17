use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
/// Anomaly detection across all subsystems
///
/// Part of the Hoags AI subsystem. Monitors neural bus signals
/// and subsystem metrics for unusual patterns and outliers.
///
/// Uses a Z-score statistical approach: maintains running mean and variance
/// per metric channel, flags values beyond a configurable sigma threshold.
/// Tracks anomaly history for trend analysis and adaptive recalibration.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Detected anomaly with severity and source info
pub struct Anomaly {
    pub subsystem_id: u16,
    pub severity: f32,
    pub timestamp: u64,
    pub metric_index: usize,
    pub observed_value: f32,
    pub expected_mean: f32,
    pub z_score: f32,
    pub description: String,
}

/// Running statistics for a single metric channel (Welford's online algorithm)
struct ChannelStats {
    count: u64,
    mean: f64,
    m2: f64, // sum of squared differences from the current mean
    min_seen: f32,
    max_seen: f32,
    last_value: f32,
}

impl ChannelStats {
    fn new() -> Self {
        ChannelStats {
            count: 0,
            mean: 0.0,
            m2: 0.0,
            min_seen: f32::MAX,
            max_seen: f32::MIN,
            last_value: 0.0,
        }
    }

    /// Update running mean/variance using Welford's method
    fn update(&mut self, value: f32) {
        let x = value as f64;
        self.count += 1;
        let delta = x - self.mean;
        self.mean += delta / self.count as f64;
        let delta2 = x - self.mean;
        self.m2 += delta * delta2;

        if value < self.min_seen {
            self.min_seen = value;
        }
        if value > self.max_seen {
            self.max_seen = value;
        }
        self.last_value = value;
    }

    /// Population variance
    fn variance(&self) -> f64 {
        if self.count < 2 {
            return 0.0;
        }
        self.m2 / self.count as f64
    }

    /// Standard deviation
    fn stddev(&self) -> f64 {
        let v = self.variance();
        if v <= 0.0 {
            return 0.0;
        }
        sqrt_f64(v)
    }

    /// Z-score of a given value relative to this channel's running stats
    fn z_score(&self, value: f32) -> f64 {
        let sd = self.stddev();
        if sd < 1e-12 {
            // No variance recorded yet — any deviation from mean is infinite
            if (value as f64 - self.mean).abs() < 1e-9 {
                return 0.0;
            }
            return 100.0; // sentinel large z-score
        }
        (value as f64 - self.mean) / sd
    }
}

/// Record of a past anomaly for trend analysis
pub struct AnomalyRecord {
    timestamp: u64,
    subsystem_id: u16,
    metric_index: usize,
    z_score: f32,
    severity: f32,
}

/// Configuration for the anomaly detector
pub struct AnomalyConfig {
    /// Z-score threshold for flagging anomalies (default: 3.0 sigma)
    pub sigma_threshold: f32,
    /// Minimum observations before anomaly detection activates per channel
    pub warmup_samples: u64,
    /// Maximum anomaly history entries retained
    pub max_history: usize,
    /// Severity mapping: z-score multiplier to [0, 1] severity
    pub severity_scale: f32,
}

impl AnomalyConfig {
    pub fn default_config() -> Self {
        AnomalyConfig {
            sigma_threshold: 3.0,
            warmup_samples: 10,
            max_history: 512,
            severity_scale: 0.2,
        }
    }
}

pub struct AnomalyDetector {
    pub baseline: Vec<f32>,
    pub sensitivity: f32,
    config: AnomalyConfig,
    channels: Vec<ChannelStats>,
    history: Vec<AnomalyRecord>,
    next_timestamp: u64,
    total_anomalies_detected: u64,
    total_samples_processed: u64,
}

impl AnomalyDetector {
    pub fn new() -> Self {
        AnomalyDetector {
            baseline: Vec::new(),
            sensitivity: 1.0,
            config: AnomalyConfig::default_config(),
            channels: Vec::new(),
            history: Vec::new(),
            next_timestamp: 1,
            total_anomalies_detected: 0,
            total_samples_processed: 0,
        }
    }

    /// Create with custom configuration
    pub fn with_config(config: AnomalyConfig) -> Self {
        AnomalyDetector {
            baseline: Vec::new(),
            sensitivity: 1.0,
            config,
            channels: Vec::new(),
            history: Vec::new(),
            next_timestamp: 1,
            total_anomalies_detected: 0,
            total_samples_processed: 0,
        }
    }

    /// Set the sigma threshold at runtime
    pub fn set_sigma_threshold(&mut self, sigma: f32) {
        if sigma > 0.0 {
            self.config.sigma_threshold = sigma;
        }
    }

    /// Set sensitivity multiplier (higher = more sensitive, lower = less)
    pub fn set_sensitivity(&mut self, s: f32) {
        if s > 0.0 {
            self.sensitivity = s;
        }
    }

    /// Ensure we have enough channel trackers for the given metric count
    fn ensure_channels(&mut self, count: usize) {
        while self.channels.len() < count {
            self.channels.push(ChannelStats::new());
        }
    }

    /// Feed a baseline vector to initialize channel means
    pub fn calibrate_baseline(&mut self, baseline: &[f32]) {
        self.baseline = baseline.to_vec();
        self.ensure_channels(baseline.len());
        // Run each baseline value through the channel stats multiple times
        // to establish a warm starting point
        for (i, &val) in baseline.iter().enumerate() {
            for _ in 0..self.config.warmup_samples {
                self.channels[i].update(val);
            }
        }
    }

    /// Check the latest metrics for anomalies.
    ///
    /// Each element in `metrics` corresponds to a metric channel (e.g.,
    /// CPU temperature, memory pressure, bus latency). Returns a list of
    /// any values that exceed the configured sigma threshold.
    pub fn detect(&self, metrics: &[f32]) -> Vec<Anomaly> {
        self.detect_for_subsystem(0, metrics)
    }

    /// Detect anomalies attributed to a specific subsystem
    pub fn detect_for_subsystem(&self, subsystem_id: u16, metrics: &[f32]) -> Vec<Anomaly> {
        let mut anomalies = Vec::new();
        let effective_sigma = self.config.sigma_threshold / self.sensitivity;

        for (i, &value) in metrics.iter().enumerate() {
            if i >= self.channels.len() {
                // Channel not yet tracked — first observation is always "normal"
                continue;
            }
            let channel = &self.channels[i];

            // Skip channels still in warmup
            if channel.count < self.config.warmup_samples {
                continue;
            }

            let z = channel.z_score(value);
            let abs_z = abs_f64(z) as f32;

            if abs_z > effective_sigma {
                // Compute severity: maps z-score linearly, clamped to [0, 1]
                let severity = ((abs_z - effective_sigma) * self.config.severity_scale)
                    .min(1.0)
                    .max(0.0);

                let direction = if z > 0.0 { "above" } else { "below" };
                let desc = format!(
                    "Metric[{}] value {:.4} is {:.2} sigma {} mean ({:.4})",
                    i, value, abs_z, direction, channel.mean as f32
                );

                anomalies.push(Anomaly {
                    subsystem_id,
                    severity,
                    timestamp: self.next_timestamp,
                    metric_index: i,
                    observed_value: value,
                    expected_mean: channel.mean as f32,
                    z_score: z as f32,
                    description: desc,
                });
            }
        }
        anomalies
    }

    /// Ingest metrics: detect anomalies and update running statistics.
    /// This is the primary "tick" function called each monitoring cycle.
    pub fn ingest(&mut self, subsystem_id: u16, metrics: &[f32]) -> Vec<Anomaly> {
        self.ensure_channels(metrics.len());
        let anomalies = self.detect_for_subsystem(subsystem_id, metrics);

        // Record anomalies in history
        for a in &anomalies {
            if self.history.len() >= self.config.max_history {
                self.history.remove(0);
            }
            self.history.push(AnomalyRecord {
                timestamp: a.timestamp,
                subsystem_id: a.subsystem_id,
                metric_index: a.metric_index,
                z_score: a.z_score,
                severity: a.severity,
            });
        }
        self.total_anomalies_detected = self
            .total_anomalies_detected
            .saturating_add(anomalies.len() as u64);

        // Update channel statistics with new observations
        for (i, &value) in metrics.iter().enumerate() {
            self.channels[i].update(value);
        }
        self.total_samples_processed = self
            .total_samples_processed
            .saturating_add(metrics.len() as u64);
        self.next_timestamp = self.next_timestamp.saturating_add(1);

        anomalies
    }

    /// Get the anomaly rate: fraction of samples that were anomalous
    pub fn anomaly_rate(&self) -> f32 {
        if self.total_samples_processed == 0 {
            return 0.0;
        }
        self.total_anomalies_detected as f32 / self.total_samples_processed as f32
    }

    /// Get recent anomaly history for a specific subsystem
    pub fn history_for_subsystem(&self, subsystem_id: u16) -> Vec<&AnomalyRecord> {
        self.history
            .iter()
            .filter(|r| r.subsystem_id == subsystem_id)
            .collect()
    }

    /// Get recent anomaly history for a specific metric channel
    pub fn history_for_channel(&self, metric_index: usize) -> Vec<&AnomalyRecord> {
        self.history
            .iter()
            .filter(|r| r.metric_index == metric_index)
            .collect()
    }

    /// Compute the mean severity of recent anomalies (last N entries)
    pub fn recent_severity(&self, last_n: usize) -> f32 {
        if self.history.is_empty() {
            return 0.0;
        }
        let start = if self.history.len() > last_n {
            self.history.len() - last_n
        } else {
            0
        };
        let slice = &self.history[start..];
        if slice.is_empty() {
            return 0.0;
        }
        let sum: f32 = slice.iter().map(|r| r.severity).sum();
        sum / slice.len() as f32
    }

    /// Check if a specific channel is trending anomalous
    /// (more than `threshold_pct`% of recent observations were anomalous)
    pub fn is_channel_trending(
        &self,
        metric_index: usize,
        window: usize,
        threshold_pct: f32,
    ) -> bool {
        let channel_history = self.history_for_channel(metric_index);
        if channel_history.len() < 2 {
            return false;
        }
        let recent_count = if channel_history.len() > window {
            window
        } else {
            channel_history.len()
        };
        let anomaly_pct = recent_count as f32 / window as f32;
        anomaly_pct >= threshold_pct
    }

    /// Get statistics summary for a channel
    pub fn channel_summary(&self, index: usize) -> Option<(f32, f32, f32, f32, u64)> {
        if index >= self.channels.len() {
            return None;
        }
        let ch = &self.channels[index];
        Some((
            ch.mean as f32,
            ch.stddev() as f32,
            ch.min_seen,
            ch.max_seen,
            ch.count,
        ))
    }

    /// Reset all channel statistics and history
    pub fn reset(&mut self) {
        self.channels.clear();
        self.history.clear();
        self.baseline.clear();
        self.total_anomalies_detected = 0;
        self.total_samples_processed = 0;
        self.next_timestamp = 1;
    }

    /// Number of metric channels being tracked
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// Total anomalies detected since initialization
    pub fn total_anomalies(&self) -> u64 {
        self.total_anomalies_detected
    }

    /// Ingest a batch of independent metric snapshots in one call.
    ///
    /// Each entry in `batch` is a `(subsystem_id, metrics_slice)` pair.
    /// All detected anomalies are returned in a single flat `Vec`.
    ///
    /// This is more efficient than calling `ingest()` repeatedly because
    /// it holds the detector state for the duration of the batch rather
    /// than re-acquiring locks (when used through the global API).
    pub fn ingest_batch<'a>(&mut self, batch: &[(u16, &'a [f32])]) -> Vec<Anomaly> {
        let mut all_anomalies = Vec::new();
        for &(subsystem_id, metrics) in batch {
            if metrics.is_empty() {
                continue;
            }
            let mut detected = self.ingest(subsystem_id, metrics);
            all_anomalies.append(&mut detected);
        }
        all_anomalies
    }

    /// Quick check: are there currently any active anomalies above `min_severity`
    /// in the recent history (last `window` records)?
    pub fn has_active_anomalies(&self, min_severity: f32, window: usize) -> bool {
        if self.history.is_empty() || window == 0 {
            return false;
        }
        let start = if self.history.len() > window {
            self.history.len() - window
        } else {
            0
        };
        self.history[start..]
            .iter()
            .any(|r| r.severity >= min_severity)
    }
}

// ---------------------------------------------------------------------------
// Math helpers (no_std compatible)
// ---------------------------------------------------------------------------

/// Approximate square root using Newton's method
fn sqrt_f64(x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut guess = x / 2.0;
    for _ in 0..64 {
        let next = 0.5 * (guess + x / guess);
        if abs_f64(next - guess) < 1e-15 {
            break;
        }
        guess = next;
    }
    guess
}

fn abs_f64(x: f64) -> f64 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static DETECTOR: Mutex<Option<AnomalyDetector>> = Mutex::new(None);

pub fn init() {
    let mut detector = AnomalyDetector::new();
    // Calibrate with a neutral baseline of 8 channels (common subsystem metrics)
    let baseline = [0.0f32; 8];
    detector.calibrate_baseline(&baseline);
    *DETECTOR.lock() = Some(detector);
    crate::serial_println!(
        "    [anomaly] Anomaly detection engine ready (Z-score, {} channels, {:.1} sigma)",
        8,
        3.0
    );
}

/// Ingest metrics and return any detected anomalies
pub fn ingest(subsystem_id: u16, metrics: &[f32]) -> Vec<Anomaly> {
    DETECTOR
        .lock()
        .as_mut()
        .map(|d| d.ingest(subsystem_id, metrics))
        .unwrap_or_else(Vec::new)
}

/// Get the current anomaly rate
pub fn anomaly_rate() -> f32 {
    DETECTOR
        .lock()
        .as_ref()
        .map(|d| d.anomaly_rate())
        .unwrap_or(0.0)
}

/// Ingest a batch of metric snapshots and return all detected anomalies.
///
/// Each element of `batch` is `(subsystem_id, &[f32])`.
pub fn ingest_batch(batch: &[(u16, &[f32])]) -> Vec<Anomaly> {
    DETECTOR
        .lock()
        .as_mut()
        .map(|d| d.ingest_batch(batch))
        .unwrap_or_else(Vec::new)
}

/// Returns true if any recent anomaly (within `window` records) has
/// severity >= `min_severity`.
pub fn has_active_anomalies(min_severity: f32, window: usize) -> bool {
    DETECTOR
        .lock()
        .as_ref()
        .map(|d| d.has_active_anomalies(min_severity, window))
        .unwrap_or(false)
}
