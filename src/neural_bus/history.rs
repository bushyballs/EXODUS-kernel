use super::{NeuralSignal, SignalKind, Q16};
use crate::sync::Mutex;
use crate::{serial_print, serial_println};
/// Signal history for pattern learning
///
/// Part of the Hoags Neural Bus. Records past signals so the
/// cortex can detect temporal patterns and predict future events.
///
/// The history maintains a bounded ring buffer of compressed records
/// and supports efficient queries:
///   - By signal kind within a time window
///   - By source node
///   - Frequency analysis (how often does a kind fire per time unit?)
///   - Co-occurrence detection (which kinds fire together?)
///   - Trend detection (is a signal kind becoming more/less frequent?)
///
/// This is the raw data that the cortex's Hebbian learning and
/// prediction engine consume.
use alloc::vec::Vec;

/// Compressed history record for a signal (smaller than full NeuralSignal)
pub struct HistoryRecord {
    /// Signal kind
    pub kind: SignalKind,
    /// Source node that emitted this signal
    pub source_node: u16,
    /// Timestamp (from the original signal)
    pub timestamp: u64,
    /// Signal strength in Q16
    pub strength_q16: i32,
}

impl HistoryRecord {
    /// Create from a NeuralSignal, discarding the payload to save memory.
    pub fn from_signal(signal: &NeuralSignal) -> Self {
        HistoryRecord {
            kind: signal.kind,
            source_node: signal.source_node,
            timestamp: signal.timestamp,
            strength_q16: signal.strength,
        }
    }
}

/// Co-occurrence pair: two signal kinds that fire within a time window
struct CoOccurrence {
    kind_a: SignalKind,
    kind_b: SignalKind,
    count: u64,
}

pub struct SignalHistory {
    /// Ring buffer of history records
    pub records: Vec<HistoryRecord>,
    /// Maximum records to keep
    pub max_records: usize,
    /// Write position in the ring buffer
    write_pos: usize,
    /// Total records ever written (may exceed max_records)
    total_written: u64,
    /// Co-occurrence tracking (pairs of kinds seen within a window)
    co_occurrences: Vec<CoOccurrence>,
    /// Time window for co-occurrence detection (in timestamp units)
    co_occurrence_window: u64,
    /// Maximum co-occurrence pairs to track
    max_co_pairs: usize,
    /// Per-kind frequency counters (for trend detection)
    kind_frequencies: Vec<(SignalKind, u64, u64)>, // (kind, recent_count, old_count)
    /// Frequency epoch boundary timestamp
    frequency_epoch: u64,
}

impl SignalHistory {
    /// Create a new signal history with the given capacity.
    pub fn new(max_records: usize) -> Self {
        SignalHistory {
            records: Vec::with_capacity(max_records),
            max_records,
            write_pos: 0,
            total_written: 0,
            co_occurrences: Vec::new(),
            co_occurrence_window: 1000, // default: 1 second (assuming ms timestamps)
            max_co_pairs: 256,
            kind_frequencies: Vec::new(),
            frequency_epoch: 0,
        }
    }

    /// Record a signal into history.
    pub fn record(&mut self, signal: &NeuralSignal) {
        let rec = HistoryRecord::from_signal(signal);
        let kind = rec.kind;
        let timestamp = rec.timestamp;

        // Insert into ring buffer
        if self.records.len() < self.max_records {
            self.records.push(rec);
            self.write_pos = self.records.len();
        } else {
            let idx = self.write_pos % self.max_records;
            self.records[idx] = rec;
            self.write_pos += 1;
        }
        self.total_written = self.total_written.saturating_add(1);

        // Update frequency counter
        self.update_frequency(kind);

        // Update co-occurrence tracking: check recent signals
        self.update_co_occurrences(kind, timestamp);
    }

    /// Query signals of a given kind within a time window.
    ///
    /// Returns records where `kind` matches and `timestamp >= since`.
    pub fn query(&self, kind: SignalKind, since: u64) -> Vec<&HistoryRecord> {
        self.records
            .iter()
            .filter(|r| r.kind == kind && r.timestamp >= since)
            .collect()
    }

    /// Query signals from a specific source node within a time window.
    pub fn query_source(&self, source: u16, since: u64) -> Vec<&HistoryRecord> {
        self.records
            .iter()
            .filter(|r| r.source_node == source && r.timestamp >= since)
            .collect()
    }

    /// Query all signals within a time window, regardless of kind.
    pub fn query_window(&self, since: u64, until: u64) -> Vec<&HistoryRecord> {
        self.records
            .iter()
            .filter(|r| r.timestamp >= since && r.timestamp <= until)
            .collect()
    }

    /// Count signals of a given kind in the history.
    pub fn count_kind(&self, kind: SignalKind) -> usize {
        self.records.iter().filter(|r| r.kind == kind).count()
    }

    /// Count signals from a given source.
    pub fn count_source(&self, source: u16) -> usize {
        self.records
            .iter()
            .filter(|r| r.source_node == source)
            .count()
    }

    /// Get the most frequent signal kinds, sorted by count descending.
    pub fn top_kinds(&self, n: usize) -> Vec<(SignalKind, usize)> {
        let mut counts: Vec<(SignalKind, usize)> = Vec::new();
        for rec in &self.records {
            let found = counts.iter_mut().find(|(k, _)| *k == rec.kind);
            if let Some((_, count)) = found {
                *count += 1;
            } else {
                counts.push((rec.kind, 1));
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1));
        counts.truncate(n);
        counts
    }

    /// Get the most active source nodes, sorted by count descending.
    pub fn top_sources(&self, n: usize) -> Vec<(u16, usize)> {
        let mut counts: Vec<(u16, usize)> = Vec::new();
        for rec in &self.records {
            let found = counts.iter_mut().find(|(s, _)| *s == rec.source_node);
            if let Some((_, count)) = found {
                *count += 1;
            } else {
                counts.push((rec.source_node, 1));
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1));
        counts.truncate(n);
        counts
    }

    /// Compute the average signal strength for a kind.
    pub fn avg_strength(&self, kind: SignalKind) -> Q16 {
        let mut sum = 0i64;
        let mut count = 0u64;
        for rec in &self.records {
            if rec.kind == kind {
                sum += rec.strength_q16 as i64;
                count += 1;
            }
        }
        if count == 0 {
            return 0;
        }
        (sum / count as i64) as Q16
    }

    /// Detect if a signal kind is trending up (firing more frequently).
    ///
    /// Returns a ratio: recent_count / old_count. Values > 1 mean trending up.
    /// Returns 0 if no data.
    pub fn trend(&self, kind: SignalKind) -> f32 {
        for &(k, recent, old) in &self.kind_frequencies {
            if k == kind {
                if old == 0 {
                    return if recent > 0 { 2.0 } else { 0.0 };
                }
                return recent as f32 / old as f32;
            }
        }
        0.0
    }

    /// Get co-occurring signal kinds (kinds that frequently fire together).
    ///
    /// Returns (kind_a, kind_b, co_occurrence_count) sorted by count.
    pub fn co_occurring_kinds(&self, n: usize) -> Vec<(SignalKind, SignalKind, u64)> {
        let mut result: Vec<(SignalKind, SignalKind, u64)> = self
            .co_occurrences
            .iter()
            .map(|co| (co.kind_a, co.kind_b, co.count))
            .collect();
        result.sort_by(|a, b| b.2.cmp(&a.2));
        result.truncate(n);
        result
    }

    /// Advance the frequency epoch. Call periodically (e.g. every N seconds).
    /// Shifts recent counts to old counts for trend detection.
    pub fn advance_epoch(&mut self) {
        for freq in self.kind_frequencies.iter_mut() {
            freq.2 = freq.1; // old = recent
            freq.1 = 0; // reset recent
        }
        self.frequency_epoch = self.frequency_epoch.saturating_add(1);
    }

    /// Get the total number of records ever written.
    pub fn total_written(&self) -> u64 {
        self.total_written
    }

    /// Get the number of records currently in the buffer.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the history is empty.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Clear all history.
    pub fn clear(&mut self) {
        self.records.clear();
        self.write_pos = 0;
        self.co_occurrences.clear();
        self.kind_frequencies.clear();
    }

    /// Get the timestamp range of the current history.
    pub fn time_range(&self) -> Option<(u64, u64)> {
        if self.records.is_empty() {
            return None;
        }
        let min = self.records.iter().map(|r| r.timestamp).min().unwrap_or(0);
        let max = self.records.iter().map(|r| r.timestamp).max().unwrap_or(0);
        Some((min, max))
    }

    // ── Internal helpers ────────────────────────────────────────────

    /// Update the per-kind frequency counter.
    fn update_frequency(&mut self, kind: SignalKind) {
        for freq in self.kind_frequencies.iter_mut() {
            if freq.0 == kind {
                freq.1 = freq.1.saturating_add(1); // Increment recent count
                return;
            }
        }
        // Not found: add a new entry
        self.kind_frequencies.push((kind, 1, 0));
    }

    /// Update co-occurrence tracking.
    fn update_co_occurrences(&mut self, new_kind: SignalKind, timestamp: u64) {
        // Look back in recent history for other kinds within the window
        let window_start = timestamp.saturating_sub(self.co_occurrence_window);
        let mut recent_kinds: Vec<SignalKind> = Vec::new();

        // Scan the last N records (bounded to avoid scanning the whole buffer)
        let scan_limit = 50.min(self.records.len());
        let start = if self.records.len() > scan_limit {
            self.records.len() - scan_limit
        } else {
            0
        };

        for i in start..self.records.len() {
            let rec = &self.records[i];
            if rec.timestamp >= window_start && rec.kind != new_kind {
                if !recent_kinds.contains(&rec.kind) {
                    recent_kinds.push(rec.kind);
                }
            }
        }

        // Update co-occurrence counts
        for kind_b in recent_kinds {
            // Canonical ordering: lower kind first (by discriminant)
            let (a, b) = if (new_kind as u8) <= (kind_b as u8) {
                (new_kind, kind_b)
            } else {
                (kind_b, new_kind)
            };

            let found = self
                .co_occurrences
                .iter_mut()
                .find(|co| co.kind_a == a && co.kind_b == b);
            if let Some(co) = found {
                co.count = co.count.saturating_add(1);
            } else if self.co_occurrences.len() < self.max_co_pairs {
                self.co_occurrences.push(CoOccurrence {
                    kind_a: a,
                    kind_b: b,
                    count: 1,
                });
            }
        }
    }

    /// Predict which signal kinds are likely to fire next based on
    /// co-occurrence patterns with the given kind.
    pub fn predict_next(&self, kind: SignalKind, n: usize) -> Vec<(SignalKind, u64)> {
        let mut predictions: Vec<(SignalKind, u64)> = Vec::new();

        for co in &self.co_occurrences {
            if co.kind_a == kind {
                predictions.push((co.kind_b, co.count));
            } else if co.kind_b == kind {
                predictions.push((co.kind_a, co.count));
            }
        }

        predictions.sort_by(|a, b| b.1.cmp(&a.1));
        predictions.truncate(n);
        predictions
    }
}

// ── Global Singleton ────────────────────────────────────────────────

struct HistoryState {
    history: SignalHistory,
}

static HISTORY: Mutex<Option<HistoryState>> = Mutex::new(None);

const DEFAULT_MAX_RECORDS: usize = 8192;

pub fn init() {
    let history = SignalHistory::new(DEFAULT_MAX_RECORDS);
    let mut guard = HISTORY.lock();
    *guard = Some(HistoryState { history });
    serial_println!(
        "    [history] Signal history subsystem initialised (max_records={})",
        DEFAULT_MAX_RECORDS
    );
}

/// Record a signal into the global history.
pub fn record_global(signal: &NeuralSignal) {
    let mut guard = HISTORY.lock();
    if let Some(state) = guard.as_mut() {
        state.history.record(signal);
    }
}

/// Query the global history by kind and time window.
pub fn query_global(kind: SignalKind, since: u64) -> usize {
    let guard = HISTORY.lock();
    if let Some(state) = guard.as_ref() {
        state.history.query(kind, since).len()
    } else {
        0
    }
}

/// Predict next signals from the global history.
pub fn predict_global(kind: SignalKind, n: usize) -> Vec<(SignalKind, u64)> {
    let guard = HISTORY.lock();
    if let Some(state) = guard.as_ref() {
        state.history.predict_next(kind, n)
    } else {
        Vec::new()
    }
}
