use super::{NeuralSignal, SignalKind};
use crate::sync::Mutex;
use crate::{serial_print, serial_println};
/// Signal replay for debugging and learning
///
/// Part of the AIOS neural bus layer. Records neural signal traces
/// that can be replayed later for:
///   - Debugging: step through signal sequences to diagnose issues
///   - Learning: train the cortex on historical signal patterns
///   - Testing: replay known-good sequences as regression tests
///   - Profiling: measure timing of signal processing
///
/// The recorder uses a ring buffer: when the trace is full, old signals
/// are overwritten. Recording can be started / stopped on the fly.
/// Traces can be filtered by signal kind or source node.
use alloc::vec::Vec;

/// Filter for selective recording
pub struct RecordFilter {
    /// If Some, only record signals of these kinds
    pub kinds: Option<Vec<SignalKind>>,
    /// If Some, only record signals from these source nodes
    pub sources: Option<Vec<u16>>,
    /// Minimum signal strength to record (Q16)
    pub min_strength: i32,
}

impl RecordFilter {
    /// No filter: record everything.
    pub fn all() -> Self {
        RecordFilter {
            kinds: None,
            sources: None,
            min_strength: 0,
        }
    }

    /// Filter to only a specific signal kind.
    pub fn kind_only(kind: SignalKind) -> Self {
        RecordFilter {
            kinds: Some(alloc::vec![kind]),
            sources: None,
            min_strength: 0,
        }
    }

    /// Filter to only a specific source node.
    pub fn source_only(node_id: u16) -> Self {
        RecordFilter {
            kinds: None,
            sources: Some(alloc::vec![node_id]),
            min_strength: 0,
        }
    }

    /// Check if a signal passes this filter.
    pub fn matches(&self, signal: &NeuralSignal) -> bool {
        if signal.strength < self.min_strength {
            return false;
        }
        if let Some(ref kinds) = self.kinds {
            if !kinds.contains(&signal.kind) {
                return false;
            }
        }
        if let Some(ref sources) = self.sources {
            if !sources.contains(&signal.source_node) {
                return false;
            }
        }
        true
    }
}

/// Bookmark into the trace for replay navigation
pub struct TraceBookmark {
    /// Position in the trace
    pub position: usize,
    /// Description of this bookmark
    pub label: u64, // Using u64 instead of String for simplicity
}

/// Records and replays signal traces
pub struct SignalRecorder {
    /// Ring buffer of recorded signals
    pub trace: Vec<NeuralSignal>,
    /// Whether recording is active
    pub recording: bool,
    /// Maximum trace length (ring buffer capacity)
    pub max_trace_len: usize,
    /// Write position in the ring buffer
    write_pos: usize,
    /// Number of signals recorded (may exceed max_trace_len due to wrapping)
    total_recorded: u64,
    /// Total signals dropped by the filter
    filtered_out: u64,
    /// Active recording filter
    filter: RecordFilter,
    /// Bookmarks for replay navigation
    bookmarks: Vec<TraceBookmark>,
    /// Current replay position
    replay_pos: usize,
    /// Whether replay is active
    replaying: bool,
    /// Replay speed multiplier (1 = normal, 2 = double speed, etc.)
    pub replay_speed: u32,
}

impl SignalRecorder {
    /// Create a new recorder with the given maximum trace length.
    pub fn new(max_len: usize) -> Self {
        serial_println!("    [replay] Creating recorder: max_trace={}", max_len);
        SignalRecorder {
            trace: Vec::with_capacity(max_len),
            recording: false,
            max_trace_len: max_len,
            write_pos: 0,
            total_recorded: 0,
            filtered_out: 0,
            filter: RecordFilter::all(),
            bookmarks: Vec::new(),
            replay_pos: 0,
            replaying: false,
            replay_speed: 1,
        }
    }

    /// Start recording.
    pub fn start_recording(&mut self) {
        self.recording = true;
        serial_println!("    [replay] Recording started");
    }

    /// Stop recording.
    pub fn stop_recording(&mut self) {
        self.recording = false;
        serial_println!(
            "    [replay] Recording stopped ({} signals captured)",
            self.len()
        );
    }

    /// Set the recording filter.
    pub fn set_filter(&mut self, filter: RecordFilter) {
        self.filter = filter;
    }

    /// Record a signal into the trace (if recording is active and filter passes).
    pub fn record(&mut self, signal: NeuralSignal) {
        if !self.recording {
            return;
        }

        if !self.filter.matches(&signal) {
            self.filtered_out = self.filtered_out.saturating_add(1);
            return;
        }

        self.total_recorded = self.total_recorded.saturating_add(1);

        if self.trace.len() < self.max_trace_len {
            // Buffer not full yet, just append
            self.trace.push(signal);
            self.write_pos = self.trace.len();
        } else {
            // Ring buffer: overwrite oldest
            let idx = self.write_pos % self.max_trace_len;
            self.trace[idx] = signal;
            self.write_pos += 1;
        }
    }

    /// Get the recorded trace as a slice.
    ///
    /// If the ring has wrapped, returns the oldest-to-newest ordering
    /// within the underlying buffer. For simplicity, returns the raw
    /// buffer (callers should be aware of the ring semantics).
    pub fn replay(&self) -> &[NeuralSignal] {
        &self.trace
    }

    /// Get the number of signals currently in the trace.
    pub fn len(&self) -> usize {
        self.trace.len()
    }

    /// Whether the trace is empty.
    pub fn is_empty(&self) -> bool {
        self.trace.is_empty()
    }

    /// Clear the entire trace.
    pub fn clear(&mut self) {
        self.trace.clear();
        self.write_pos = 0;
        self.bookmarks.clear();
        self.replay_pos = 0;
    }

    /// Get the i-th signal from the trace (ring-buffer-aware).
    pub fn get(&self, index: usize) -> Option<&NeuralSignal> {
        if index >= self.trace.len() {
            return None;
        }
        if self.total_recorded as usize <= self.max_trace_len {
            // Buffer hasn't wrapped
            self.trace.get(index)
        } else {
            // Buffer wrapped: oldest is at write_pos % max_trace_len
            let base = self.write_pos % self.max_trace_len;
            let actual = (base + index) % self.max_trace_len;
            self.trace.get(actual)
        }
    }

    /// Start replay from the beginning.
    pub fn start_replay(&mut self) {
        self.replaying = true;
        self.replay_pos = 0;
        serial_println!("    [replay] Replay started ({} signals)", self.len());
    }

    /// Advance replay by one step. Returns the next signal, or None if done.
    pub fn replay_step(&mut self) -> Option<&NeuralSignal> {
        if !self.replaying || self.replay_pos >= self.trace.len() {
            self.replaying = false;
            return None;
        }
        let pos = self.replay_pos;
        self.replay_pos += 1;
        self.get(pos)
    }

    /// Skip forward in replay.
    pub fn replay_skip(&mut self, count: usize) {
        self.replay_pos = (self.replay_pos + count).min(self.trace.len());
    }

    /// Seek to a specific position in the replay.
    pub fn replay_seek(&mut self, position: usize) {
        self.replay_pos = position.min(self.trace.len());
    }

    /// Stop replay.
    pub fn stop_replay(&mut self) {
        self.replaying = false;
    }

    /// Add a bookmark at the current write position.
    pub fn add_bookmark(&mut self, label: u64) {
        let pos = if self.trace.len() < self.max_trace_len {
            self.trace.len()
        } else {
            self.write_pos % self.max_trace_len
        };
        self.bookmarks.push(TraceBookmark {
            position: pos,
            label,
        });
    }

    /// Get all bookmarks.
    pub fn bookmarks(&self) -> &[TraceBookmark] {
        &self.bookmarks
    }

    /// Filter the trace to only signals of a given kind.
    pub fn filter_kind(&self, kind: SignalKind) -> Vec<&NeuralSignal> {
        self.trace.iter().filter(|s| s.kind == kind).collect()
    }

    /// Filter the trace to only signals from a given source node.
    pub fn filter_source(&self, source: u16) -> Vec<&NeuralSignal> {
        self.trace
            .iter()
            .filter(|s| s.source_node == source)
            .collect()
    }

    /// Count signals by kind in the trace.
    pub fn count_by_kind(&self) -> Vec<(SignalKind, usize)> {
        // Since SignalKind doesn't implement Hash, we'll use a simple linear scan
        let mut counts: Vec<(SignalKind, usize)> = Vec::new();
        for signal in &self.trace {
            let found = counts.iter_mut().find(|(k, _)| *k == signal.kind);
            if let Some((_, count)) = found {
                *count += 1;
            } else {
                counts.push((signal.kind, 1));
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1));
        counts
    }

    /// Get statistics about the trace.
    pub fn stats(&self) -> (u64, u64, usize, usize) {
        (
            self.total_recorded,
            self.filtered_out,
            self.trace.len(),
            self.bookmarks.len(),
        )
    }

    /// Compute the average signal strength in the trace.
    pub fn avg_strength(&self) -> i32 {
        if self.trace.is_empty() {
            return 0;
        }
        let sum: i64 = self.trace.iter().map(|s| s.strength as i64).sum();
        (sum / self.trace.len() as i64) as i32
    }

    /// Find signals in a timestamp range.
    pub fn in_time_range(&self, start: u64, end: u64) -> Vec<&NeuralSignal> {
        self.trace
            .iter()
            .filter(|s| s.timestamp >= start && s.timestamp <= end)
            .collect()
    }
}

// ── Global Singleton ────────────────────────────────────────────────

struct ReplayState {
    recorder: SignalRecorder,
}

static RECORDER: Mutex<Option<ReplayState>> = Mutex::new(None);

const DEFAULT_MAX_TRACE: usize = 4096;

pub fn init() {
    let mut recorder = SignalRecorder::new(DEFAULT_MAX_TRACE);
    recorder.start_recording(); // Start recording by default
    let mut guard = RECORDER.lock();
    *guard = Some(ReplayState { recorder });
    serial_println!(
        "    [replay] Signal recorder subsystem initialised (max_trace={})",
        DEFAULT_MAX_TRACE
    );
}

/// Record a signal in the global recorder.
pub fn record_global(signal: NeuralSignal) {
    let mut guard = RECORDER.lock();
    if let Some(state) = guard.as_mut() {
        state.recorder.record(signal);
    }
}

/// Get the current trace length from the global recorder.
pub fn trace_len_global() -> usize {
    let guard = RECORDER.lock();
    if let Some(state) = guard.as_ref() {
        state.recorder.len()
    } else {
        0
    }
}
