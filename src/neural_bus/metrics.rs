use crate::sync::Mutex;
use crate::{serial_print, serial_println};
/// Neural bus performance metrics
///
/// Part of the AIOS neural bus layer. Tracks throughput, latency,
/// error rates, and health of the neural bus in real time. Maintains
/// rolling statistics using exponential moving averages (EMA) and
/// per-node breakdowns.
///
/// Metrics collected:
///   - Signals per second (throughput)
///   - Average and P99 latency in microseconds
///   - Drop count (signals lost to overflow)
///   - Per-node signal counts
///   - Health score (composite metric)
use alloc::vec::Vec;

/// Histogram bucket for latency distribution
const NUM_BUCKETS: usize = 16;
/// Bucket boundaries in microseconds: 1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1K, 2K, 4K, 8K, 16K, 32K+
const BUCKET_BOUNDARIES: [u64; NUM_BUCKETS] = [
    1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768,
];

/// Per-node metrics
struct NodeMetrics {
    node_id: u16,
    signal_count: u64,
    total_latency_us: u64,
    last_signal_time: u64,
}

/// Tracks throughput, latency, and health of the bus
pub struct BusMetrics {
    /// Approximate signals per second (EMA)
    pub signals_per_sec: u64,
    /// Average latency in microseconds (EMA)
    pub avg_latency_us: u64,
    /// Total signals dropped due to overflow
    pub dropped_count: u64,
    /// Total overflow events
    pub overflow_count: u64,
    /// Total signals recorded
    total_signals: u64,
    /// Sum of all latencies (for computing mean)
    total_latency_sum: u64,
    /// Latency histogram buckets
    latency_hist: [u64; NUM_BUCKETS],
    /// Window start timestamp (microseconds)
    window_start_us: u64,
    /// Signals in current window
    window_signals: u64,
    /// Window duration for rate calculation (microseconds)
    window_duration_us: u64,
    /// Per-node statistics
    node_stats: Vec<NodeMetrics>,
    /// Maximum tracked nodes
    max_nodes: usize,
    /// EMA smoothing factor (fixed-point, 0-256 where 256 = 1.0)
    ema_alpha: u64,
    /// Maximum latency ever seen
    max_latency_us: u64,
    /// Minimum latency ever seen (excluding 0)
    min_latency_us: u64,
    /// Error / anomaly counter
    error_count: u64,
}

impl BusMetrics {
    /// Create a new metrics tracker.
    pub fn new() -> Self {
        BusMetrics {
            signals_per_sec: 0,
            avg_latency_us: 0,
            dropped_count: 0,
            overflow_count: 0,
            total_signals: 0,
            total_latency_sum: 0,
            latency_hist: [0; NUM_BUCKETS],
            window_start_us: 0,
            window_signals: 0,
            window_duration_us: 1_000_000, // 1 second windows
            node_stats: Vec::new(),
            max_nodes: 128,
            ema_alpha: 51, // ~0.2 smoothing factor (51/256)
            max_latency_us: 0,
            min_latency_us: u64::MAX,
            error_count: 0,
        }
    }

    /// Record a signal event with its processing latency.
    pub fn record_signal(&mut self, latency_us: u64) {
        self.total_signals = self.total_signals.saturating_add(1);
        self.window_signals = self.window_signals.saturating_add(1);
        self.total_latency_sum += latency_us;

        // Update min/max
        if latency_us > self.max_latency_us {
            self.max_latency_us = latency_us;
        }
        if latency_us > 0 && latency_us < self.min_latency_us {
            self.min_latency_us = latency_us;
        }

        // Update histogram
        let bucket = self.latency_bucket(latency_us);
        self.latency_hist[bucket] = self.latency_hist[bucket].saturating_add(1);

        // Update EMA for average latency
        // new_avg = alpha * sample + (1 - alpha) * old_avg
        let alpha = self.ema_alpha;
        self.avg_latency_us = (alpha * latency_us + (256 - alpha) * self.avg_latency_us) / 256;

        // Check if we should update the rate
        // (In a real system, we'd use actual timestamps. Here we
        // approximate by counting signals.)
        if self.window_signals >= 100 {
            self.flush_window();
        }
    }

    /// Record a signal with its source node for per-node tracking.
    pub fn record_signal_from(&mut self, latency_us: u64, source_node: u16) {
        self.record_signal(latency_us);

        // Update per-node stats
        let found = self
            .node_stats
            .iter_mut()
            .find(|n| n.node_id == source_node);
        if let Some(node) = found {
            node.signal_count = node.signal_count.saturating_add(1);
            node.total_latency_us += latency_us;
            node.last_signal_time = self.total_signals;
        } else if self.node_stats.len() < self.max_nodes {
            self.node_stats.push(NodeMetrics {
                node_id: source_node,
                signal_count: 1,
                total_latency_us: latency_us,
                last_signal_time: self.total_signals,
            });
        }
    }

    /// Record a dropped signal.
    pub fn record_drop(&mut self) {
        self.dropped_count = self.dropped_count.saturating_add(1);
    }

    /// Record an overflow event.
    pub fn record_overflow(&mut self) {
        self.overflow_count = self.overflow_count.saturating_add(1);
    }

    /// Record an error/anomaly.
    pub fn record_error(&mut self) {
        self.error_count = self.error_count.saturating_add(1);
    }

    /// Take a snapshot of the current metrics.
    pub fn snapshot(&self) -> BusMetrics {
        BusMetrics {
            signals_per_sec: self.signals_per_sec,
            avg_latency_us: self.avg_latency_us,
            dropped_count: self.dropped_count,
            overflow_count: self.overflow_count,
            total_signals: self.total_signals,
            total_latency_sum: self.total_latency_sum,
            latency_hist: self.latency_hist,
            window_start_us: self.window_start_us,
            window_signals: 0,
            window_duration_us: self.window_duration_us,
            node_stats: Vec::new(), // Don't clone per-node stats for snapshots
            max_nodes: self.max_nodes,
            ema_alpha: self.ema_alpha,
            max_latency_us: self.max_latency_us,
            min_latency_us: self.min_latency_us,
            error_count: self.error_count,
        }
    }

    /// Compute the P99 latency from the histogram.
    pub fn p99_latency_us(&self) -> u64 {
        if self.total_signals == 0 {
            return 0;
        }
        let target = (self.total_signals * 99) / 100;
        let mut cumulative = 0u64;
        for (i, &count) in self.latency_hist.iter().enumerate() {
            cumulative += count;
            if cumulative >= target {
                return BUCKET_BOUNDARIES[i];
            }
        }
        // Everything is in the last bucket
        BUCKET_BOUNDARIES[NUM_BUCKETS - 1]
    }

    /// Compute the P50 (median) latency from the histogram.
    pub fn p50_latency_us(&self) -> u64 {
        if self.total_signals == 0 {
            return 0;
        }
        let target = self.total_signals / 2;
        let mut cumulative = 0u64;
        for (i, &count) in self.latency_hist.iter().enumerate() {
            cumulative += count;
            if cumulative >= target {
                return BUCKET_BOUNDARIES[i];
            }
        }
        BUCKET_BOUNDARIES[NUM_BUCKETS - 1]
    }

    /// Compute a composite health score (0-100).
    /// 100 = perfect health, 0 = critical.
    pub fn health_score(&self) -> u32 {
        let mut score = 100u32;

        // Penalty for high drop rate
        if self.total_signals > 0 {
            let drop_pct = (self.dropped_count * 100) / self.total_signals.max(1);
            if drop_pct > 10 {
                score = score.saturating_sub(30);
            } else if drop_pct > 5 {
                score = score.saturating_sub(15);
            } else if drop_pct > 1 {
                score = score.saturating_sub(5);
            }
        }

        // Penalty for high latency
        if self.avg_latency_us > 10000 {
            score = score.saturating_sub(25);
        } else if self.avg_latency_us > 1000 {
            score = score.saturating_sub(10);
        } else if self.avg_latency_us > 100 {
            score = score.saturating_sub(3);
        }

        // Penalty for errors
        if self.error_count > 100 {
            score = score.saturating_sub(20);
        } else if self.error_count > 10 {
            score = score.saturating_sub(10);
        } else if self.error_count > 0 {
            score = score.saturating_sub(5);
        }

        // Penalty for overflow
        if self.overflow_count > 50 {
            score = score.saturating_sub(15);
        } else if self.overflow_count > 5 {
            score = score.saturating_sub(5);
        }

        score
    }

    /// Get the mean latency from raw totals.
    pub fn mean_latency_us(&self) -> u64 {
        if self.total_signals == 0 {
            return 0;
        }
        self.total_latency_sum / self.total_signals
    }

    /// Get per-node signal counts, sorted by count descending.
    pub fn top_nodes(&self, n: usize) -> Vec<(u16, u64)> {
        let mut sorted: Vec<(u16, u64)> = self
            .node_stats
            .iter()
            .map(|n| (n.node_id, n.signal_count))
            .collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        sorted.truncate(n);
        sorted
    }

    /// Get the total signal count.
    pub fn total(&self) -> u64 {
        self.total_signals
    }

    /// Reset all metrics.
    pub fn reset(&mut self) {
        *self = BusMetrics::new();
    }

    // ── Internal helpers ────────────────────────────────────────────

    /// Map a latency value to a histogram bucket index.
    fn latency_bucket(&self, latency_us: u64) -> usize {
        for (i, &boundary) in BUCKET_BOUNDARIES.iter().enumerate() {
            if latency_us <= boundary {
                return i;
            }
        }
        NUM_BUCKETS - 1
    }

    /// Flush the current window and update the signals-per-second rate.
    fn flush_window(&mut self) {
        // Approximate: treat each batch of 100 signals as one window tick
        let alpha = self.ema_alpha;
        let instant_rate = self.window_signals;
        self.signals_per_sec = (alpha * instant_rate + (256 - alpha) * self.signals_per_sec) / 256;
        self.window_signals = 0;
    }
}

// ── Global Singleton ────────────────────────────────────────────────

struct MetricsState {
    metrics: BusMetrics,
}

static METRICS: Mutex<Option<MetricsState>> = Mutex::new(None);

pub fn init() {
    let metrics = BusMetrics::new();
    let mut guard = METRICS.lock();
    *guard = Some(MetricsState { metrics });
    serial_println!("    [metrics] Bus metrics subsystem initialised");
}

/// Record a signal in the global metrics.
pub fn record_global(latency_us: u64, source_node: u16) {
    let mut guard = METRICS.lock();
    if let Some(state) = guard.as_mut() {
        state.metrics.record_signal_from(latency_us, source_node);
    }
}

/// Get the current health score.
pub fn health_score_global() -> u32 {
    let guard = METRICS.lock();
    if let Some(state) = guard.as_ref() {
        state.metrics.health_score()
    } else {
        0
    }
}

/// Take a snapshot of global metrics.
pub fn snapshot_global() -> Option<BusMetrics> {
    let guard = METRICS.lock();
    guard.as_ref().map(|state| state.metrics.snapshot())
}
