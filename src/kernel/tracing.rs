/// Kernel tracing subsystem for Genesis
///
/// An ftrace-inspired tracing framework that records kernel events into a
/// lock-free ring buffer. Supports static tracepoints, dynamic function
/// tracing, event filtering, and multiple output formats.
///
/// Tracepoints are inserted at key kernel locations (scheduler, memory,
/// I/O, syscalls). Each trace event carries a timestamp, CPU, PID, and
/// event-specific payload. The ring buffer is per-CPU to minimize contention.
///
/// Features:
/// - Ring buffer per-CPU trace buffer with overwrite mode
/// - Tracepoint registration (static + dynamic)
/// - Function entry/exit tracing with timestamps
/// - Event filtering by PID, event type, subsystem
/// - Trace output formatting (human-readable and binary)
/// - ftrace-like function graph tracer (indented call depth)
/// - Trace buffer snapshot and export
/// - Statistics: events_lost, buffer_usage
///
/// Inspired by: Linux ftrace/tracepoints (kernel/trace/). All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Maximum entries per ring buffer
const RING_BUFFER_SIZE: usize = 8192;

/// Maximum number of registered tracepoints
const MAX_TRACEPOINTS: usize = 256;

/// Maximum filter rules
const MAX_FILTERS: usize = 32;

/// Maximum per-CPU buffers
const MAX_CPUS: usize = 64;

/// Maximum function graph depth
const MAX_GRAPH_DEPTH: usize = 64;

/// Maximum function graph entries per CPU
const MAX_GRAPH_ENTRIES: usize = 4096;

/// Global tracing enable/disable flag (atomic for fast path check)
static TRACING_ENABLED: AtomicBool = AtomicBool::new(false);

/// Function graph tracer enable flag
static GRAPH_TRACER_ENABLED: AtomicBool = AtomicBool::new(false);

/// Global event sequence counter
static EVENT_SEQ: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Trace event categories and levels
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceCategory {
    Sched,
    Memory,
    Irq,
    Syscall,
    Block,
    Net,
    Fs,
    Power,
    Timer,
    Ipc,
    Custom,
}

impl TraceCategory {
    fn name(self) -> &'static str {
        match self {
            TraceCategory::Sched => "sched",
            TraceCategory::Memory => "memory",
            TraceCategory::Irq => "irq",
            TraceCategory::Syscall => "syscall",
            TraceCategory::Block => "block",
            TraceCategory::Net => "net",
            TraceCategory::Fs => "fs",
            TraceCategory::Power => "power",
            TraceCategory::Timer => "timer",
            TraceCategory::Ipc => "ipc",
            TraceCategory::Custom => "custom",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TraceLevel {
    Critical = 0,
    Error = 1,
    Warning = 2,
    Info = 3,
    Debug = 4,
    Verbose = 5,
}

// ---------------------------------------------------------------------------
// Trace events
// ---------------------------------------------------------------------------

/// A single trace event record
#[derive(Clone)]
pub struct TraceEvent {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub cpu: u32,
    pub pid: u32,
    pub category: TraceCategory,
    pub level: TraceLevel,
    /// Tracepoint name (e.g., "sched_switch", "page_fault")
    pub name: String,
    /// Event-specific data fields (key=value pairs)
    pub fields: Vec<(String, String)>,
    /// Raw numeric args (up to 4, for fast tracepoints)
    pub args: [u64; 4],
}

// ---------------------------------------------------------------------------
// Function graph tracer
// ---------------------------------------------------------------------------

/// Function graph entry — records function entry/exit with call depth.
#[derive(Clone)]
pub struct GraphEntry {
    /// Timestamp (ms)
    pub timestamp_ms: u64,
    /// CPU
    pub cpu: u32,
    /// PID
    pub pid: u32,
    /// Function address
    pub func_addr: u64,
    /// Function name (if resolved)
    pub func_name: String,
    /// Whether this is an entry (true) or exit (false) record
    pub is_entry: bool,
    /// Call depth (indentation level)
    pub depth: u32,
    /// Duration (only valid on exit, in microseconds)
    pub duration_us: u64,
}

/// Per-CPU function graph state
struct GraphState {
    /// Current call depth for this CPU
    depth: u32,
    /// Stack of entry timestamps (for computing duration on exit)
    entry_times: Vec<u64>,
    /// Recorded graph entries
    entries: Vec<GraphEntry>,
}

impl GraphState {
    const fn new() -> Self {
        GraphState {
            depth: 0,
            entry_times: Vec::new(),
            entries: Vec::new(),
        }
    }

    fn record_entry(&mut self, func_addr: u64, func_name: &str) {
        let now = crate::time::clock::uptime_ms();
        let cpu = crate::smp::current_cpu();
        let pid = crate::process::getpid();

        if self.entries.len() >= MAX_GRAPH_ENTRIES {
            // Overwrite oldest
            self.entries.remove(0);
        }

        self.entries.push(GraphEntry {
            timestamp_ms: now,
            cpu,
            pid,
            func_addr,
            func_name: String::from(func_name),
            is_entry: true,
            depth: self.depth,
            duration_us: 0,
        });

        self.entry_times.push(now);
        if self.depth < MAX_GRAPH_DEPTH as u32 {
            self.depth += 1;
        }
    }

    fn record_exit(&mut self, func_addr: u64, func_name: &str) {
        let now = crate::time::clock::uptime_ms();
        let cpu = crate::smp::current_cpu();
        let pid = crate::process::getpid();

        self.depth = self.depth.saturating_sub(1);

        let duration_us = if let Some(entry_time) = self.entry_times.pop() {
            now.saturating_sub(entry_time) * 1000 // ms -> us
        } else {
            0
        };

        if self.entries.len() >= MAX_GRAPH_ENTRIES {
            self.entries.remove(0);
        }

        self.entries.push(GraphEntry {
            timestamp_ms: now,
            cpu,
            pid,
            func_addr,
            func_name: String::from(func_name),
            is_entry: false,
            depth: self.depth,
            duration_us,
        });
    }
}

// ---------------------------------------------------------------------------
// Tracepoints
// ---------------------------------------------------------------------------

/// A registered tracepoint
pub struct Tracepoint {
    pub name: String,
    pub category: TraceCategory,
    pub enabled: bool,
    pub hit_count: u64,
    pub description: String,
    /// Whether this is a static (compiled-in) or dynamic tracepoint
    pub is_dynamic: bool,
    /// Callback function address (for dynamic tracepoints)
    pub callback_addr: Option<u64>,
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Filter rule for trace events
#[derive(Clone)]
pub struct TraceFilter {
    pub id: u32,
    pub category: Option<TraceCategory>,
    pub min_level: TraceLevel,
    pub pid: u32,
    pub cpu: i32,
    pub name_match: String,
    pub allow: bool,
}

// ---------------------------------------------------------------------------
// Per-CPU ring buffer
// ---------------------------------------------------------------------------

struct RingBuffer {
    events: Vec<TraceEvent>,
    head: usize,
    count: usize,
    capacity: usize,
    dropped: u64,
    /// Total bytes written (approximate)
    bytes_written: u64,
}

impl RingBuffer {
    const fn new() -> Self {
        RingBuffer {
            events: Vec::new(),
            head: 0,
            count: 0,
            capacity: RING_BUFFER_SIZE,
            dropped: 0,
            bytes_written: 0,
        }
    }

    fn ensure_capacity(&mut self) {
        if self.events.is_empty() {
            self.events.reserve(self.capacity);
        }
    }

    fn push(&mut self, event: TraceEvent) {
        self.ensure_capacity();
        // Approximate size of the event for stats
        let approx_size = 64 + event.name.len() + event.fields.len() * 32;
        self.bytes_written += approx_size as u64;

        if self.events.len() < self.capacity {
            self.events.push(event);
            self.count += 1;
        } else {
            // Overwrite oldest entry (ring buffer mode)
            let idx = self.head % self.capacity;
            self.events[idx] = event;
            self.head = (self.head + 1) % self.capacity;
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    fn drain_all(&mut self) -> Vec<TraceEvent> {
        let result = core::mem::take(&mut self.events);
        self.head = 0;
        self.count = 0;
        result
    }

    fn read_recent(&self, max_count: usize) -> Vec<TraceEvent> {
        if self.events.is_empty() {
            return Vec::new();
        }
        let n = max_count.min(self.events.len());
        let start = self.events.len().saturating_sub(n);
        self.events[start..].to_vec()
    }

    fn clear(&mut self) {
        self.events.clear();
        self.head = 0;
        self.count = 0;
    }

    fn usage_percent(&self) -> u32 {
        if self.capacity == 0 {
            return 0;
        }
        ((self.events.len() * 100) / self.capacity) as u32
    }
}

// ---------------------------------------------------------------------------
// Snapshot — frozen copy of trace buffers
// ---------------------------------------------------------------------------

/// A snapshot of all trace buffers at a point in time.
pub struct TraceSnapshot {
    pub timestamp_ms: u64,
    pub events: Vec<TraceEvent>,
    pub total_events: u64,
    pub total_dropped: u64,
}

// ---------------------------------------------------------------------------
// Binary trace format
// ---------------------------------------------------------------------------

/// Binary trace event header (for efficient export/import)
#[repr(C, packed)]
struct BinaryEventHeader {
    seq: u64,
    timestamp_ms: u64,
    cpu: u32,
    pid: u32,
    category: u8,
    level: u8,
    name_len: u16,
    num_fields: u16,
    num_args: u8,
    _pad: u8,
}

// ---------------------------------------------------------------------------
// Tracing subsystem state
// ---------------------------------------------------------------------------

struct TracingSubsystem {
    /// Per-CPU ring buffers
    buffers: Vec<RingBuffer>,
    /// Per-CPU function graph states
    graph_states: Vec<GraphState>,
    /// Registered tracepoints
    tracepoints: Vec<Tracepoint>,
    /// Active filters
    filters: Vec<TraceFilter>,
    /// Next filter ID
    next_filter_id: u32,
    /// Current trace level threshold
    level_threshold: TraceLevel,
    /// Number of CPUs
    num_cpus: usize,
    /// Total events recorded
    total_events: u64,
    /// Total events dropped
    total_dropped: u64,
    /// Saved snapshots
    snapshots: Vec<TraceSnapshot>,
    /// Maximum snapshots to keep
    max_snapshots: usize,
}

impl TracingSubsystem {
    const fn new() -> Self {
        TracingSubsystem {
            buffers: Vec::new(),
            graph_states: Vec::new(),
            tracepoints: Vec::new(),
            filters: Vec::new(),
            next_filter_id: 1,
            level_threshold: TraceLevel::Info,
            num_cpus: 1,
            total_events: 0,
            total_dropped: 0,
            snapshots: Vec::new(),
            max_snapshots: 4,
        }
    }

    fn init_buffers(&mut self, ncpus: usize) {
        self.num_cpus = ncpus;
        for _ in 0..ncpus {
            self.buffers.push(RingBuffer::new());
            self.graph_states.push(GraphState::new());
        }
    }

    // ------- Tracepoints -------

    fn register_tracepoint(&mut self, name: &str, category: TraceCategory, desc: &str) -> bool {
        if self.tracepoints.len() >= MAX_TRACEPOINTS {
            return false;
        }
        if self.tracepoints.iter().any(|tp| tp.name == name) {
            return false;
        }
        self.tracepoints.push(Tracepoint {
            name: String::from(name),
            category,
            enabled: true,
            hit_count: 0,
            description: String::from(desc),
            is_dynamic: false,
            callback_addr: None,
        });
        true
    }

    /// Register a dynamic tracepoint (created at runtime, e.g., for modules).
    fn register_dynamic_tracepoint(
        &mut self,
        name: &str,
        category: TraceCategory,
        desc: &str,
        callback: u64,
    ) -> bool {
        if self.tracepoints.len() >= MAX_TRACEPOINTS {
            return false;
        }
        if self.tracepoints.iter().any(|tp| tp.name == name) {
            return false;
        }
        self.tracepoints.push(Tracepoint {
            name: String::from(name),
            category,
            enabled: true,
            hit_count: 0,
            description: String::from(desc),
            is_dynamic: true,
            callback_addr: Some(callback),
        });
        true
    }

    /// Unregister a dynamic tracepoint.
    fn unregister_dynamic_tracepoint(&mut self, name: &str) -> bool {
        if let Some(idx) = self
            .tracepoints
            .iter()
            .position(|tp| tp.name == name && tp.is_dynamic)
        {
            self.tracepoints.remove(idx);
            true
        } else {
            false
        }
    }

    fn set_tracepoint_enabled(&mut self, name: &str, enabled: bool) -> bool {
        if let Some(tp) = self.tracepoints.iter_mut().find(|tp| tp.name == name) {
            tp.enabled = enabled;
            true
        } else {
            false
        }
    }

    /// Enable/disable all tracepoints in a category.
    fn set_category_enabled(&mut self, category: TraceCategory, enabled: bool) -> usize {
        let mut count = 0;
        for tp in &mut self.tracepoints {
            if tp.category == category {
                tp.enabled = enabled;
                count += 1;
            }
        }
        count
    }

    // ------- Filtering -------

    fn passes_filters(&self, event: &TraceEvent) -> bool {
        if event.level > self.level_threshold {
            return false;
        }
        if self.filters.is_empty() {
            return true;
        }
        for filter in &self.filters {
            let matches_category = filter.category.map_or(true, |c| c == event.category);
            let matches_level = event.level <= filter.min_level;
            let matches_pid = filter.pid == 0 || filter.pid == event.pid;
            let matches_cpu = filter.cpu < 0 || filter.cpu as u32 == event.cpu;
            let matches_name =
                filter.name_match.is_empty() || event.name.contains(filter.name_match.as_str());

            let matched =
                matches_category && matches_level && matches_pid && matches_cpu && matches_name;

            if matched && !filter.allow {
                return false;
            }
        }
        true
    }

    // ------- Event recording -------

    fn record_event(&mut self, mut event: TraceEvent) {
        event.seq = EVENT_SEQ.fetch_add(1, Ordering::Relaxed);
        event.timestamp_ms = crate::time::clock::uptime_ms();
        event.cpu = crate::smp::current_cpu();
        event.pid = crate::process::getpid();

        // Check tracepoint enabled
        if let Some(tp) = self.tracepoints.iter_mut().find(|tp| tp.name == event.name) {
            if !tp.enabled {
                return;
            }
            tp.hit_count = tp.hit_count.saturating_add(1);
        }

        if !self.passes_filters(&event) {
            return;
        }

        let cpu_idx = event.cpu as usize;
        if cpu_idx < self.buffers.len() {
            self.buffers[cpu_idx].push(event);
        } else if !self.buffers.is_empty() {
            self.buffers[0].push(event);
        }

        self.total_events = self.total_events.saturating_add(1);
    }

    // ------- Function graph tracer -------

    fn graph_entry(&mut self, func_addr: u64, func_name: &str) {
        let cpu = crate::smp::current_cpu() as usize;
        if cpu < self.graph_states.len() {
            self.graph_states[cpu].record_entry(func_addr, func_name);
        }
    }

    fn graph_exit(&mut self, func_addr: u64, func_name: &str) {
        let cpu = crate::smp::current_cpu() as usize;
        if cpu < self.graph_states.len() {
            self.graph_states[cpu].record_exit(func_addr, func_name);
        }
    }

    /// Format function graph output (ftrace-style indented display).
    fn format_graph(&self) -> String {
        let mut all_entries: Vec<GraphEntry> = Vec::new();
        for gs in &self.graph_states {
            all_entries.extend(gs.entries.clone());
        }
        all_entries.sort_by_key(|e| e.timestamp_ms);

        let mut s = String::from("# FUNCTION GRAPH TRACER\n");
        s.push_str(&format!("# CPU  DURATION        FUNCTION\n"));
        s.push_str(&format!("#  |     |             |\n"));

        for entry in &all_entries {
            let indent = "  ".repeat(entry.depth as usize);
            if entry.is_entry {
                s.push_str(&format!(
                    "  {:>3}               {} {}() {{\n",
                    entry.cpu, indent, entry.func_name
                ));
            } else {
                let dur_str = if entry.duration_us > 0 {
                    if entry.duration_us >= 1000 {
                        format!("{:>6} ms", entry.duration_us / 1000)
                    } else {
                        format!("{:>6} us", entry.duration_us)
                    }
                } else {
                    format!("       -")
                };
                s.push_str(&format!("  {:>3} {} {} }}\n", entry.cpu, dur_str, indent));
            }
        }
        s
    }

    /// Clear graph tracer entries.
    fn clear_graph(&mut self) {
        for gs in &mut self.graph_states {
            gs.entries.clear();
            gs.depth = 0;
            gs.entry_times.clear();
        }
    }

    // ------- Filter management -------

    fn add_filter(&mut self, filter: TraceFilter) -> Result<u32, TracingError> {
        if self.filters.len() >= MAX_FILTERS {
            return Err(TracingError::TooManyFilters);
        }
        let id = self.next_filter_id;
        self.next_filter_id = self.next_filter_id.saturating_add(1);
        let mut f = filter;
        f.id = id;
        self.filters.push(f);
        Ok(id)
    }

    fn remove_filter(&mut self, filter_id: u32) -> bool {
        if let Some(idx) = self.filters.iter().position(|f| f.id == filter_id) {
            self.filters.remove(idx);
            true
        } else {
            false
        }
    }

    fn clear_filters(&mut self) {
        self.filters.clear();
    }

    // ------- Reading / exporting -------

    fn read_recent(&self, max_count: usize) -> Vec<TraceEvent> {
        let mut all: Vec<TraceEvent> = Vec::new();
        for buf in &self.buffers {
            all.extend(buf.read_recent(max_count));
        }
        all.sort_by_key(|e| e.seq);
        if all.len() > max_count {
            all.split_off(all.len() - max_count)
        } else {
            all
        }
    }

    fn drain_all(&mut self) -> Vec<TraceEvent> {
        let mut all: Vec<TraceEvent> = Vec::new();
        for buf in &mut self.buffers {
            all.extend(buf.drain_all());
        }
        all.sort_by_key(|e| e.seq);
        all
    }

    fn clear_all(&mut self) {
        for buf in &mut self.buffers {
            buf.clear();
        }
        self.total_events = 0;
        self.total_dropped = 0;
    }

    fn set_level(&mut self, level: TraceLevel) {
        self.level_threshold = level;
    }

    // ------- Snapshots -------

    /// Take a snapshot of all trace buffers.
    fn take_snapshot(&mut self) -> usize {
        let now = crate::time::clock::uptime_ms();
        let mut events: Vec<TraceEvent> = Vec::new();
        let mut total_dropped: u64 = 0;

        for buf in &self.buffers {
            events.extend(buf.events.clone());
            total_dropped += buf.dropped;
        }

        events.sort_by_key(|e| e.seq);

        let snapshot = TraceSnapshot {
            timestamp_ms: now,
            events,
            total_events: self.total_events,
            total_dropped,
        };

        if self.snapshots.len() >= self.max_snapshots {
            self.snapshots.remove(0);
        }
        self.snapshots.push(snapshot);
        self.snapshots.len() - 1
    }

    /// Get a snapshot by index.
    fn get_snapshot(&self, index: usize) -> Option<&TraceSnapshot> {
        self.snapshots.get(index)
    }

    /// List snapshots.
    fn list_snapshots(&self) -> Vec<(usize, u64, u64, u64)> {
        self.snapshots
            .iter()
            .enumerate()
            .map(|(i, s)| (i, s.timestamp_ms, s.total_events, s.events.len() as u64))
            .collect()
    }

    // ------- Formatting -------

    fn format_event(event: &TraceEvent) -> String {
        let mut s = format!(
            "[{:>10}] cpu={} pid={:<5} {}/{}: ",
            event.timestamp_ms,
            event.cpu,
            event.pid,
            event.category.name(),
            event.name,
        );
        for (k, v) in &event.fields {
            s.push_str(&format!("{}={} ", k, v));
        }
        // Append raw args if any are non-zero
        let has_args = event.args.iter().any(|&a| a != 0);
        if has_args {
            s.push_str(&format!(
                "[{:#x},{:#x},{:#x},{:#x}]",
                event.args[0], event.args[1], event.args[2], event.args[3]
            ));
        }
        s
    }

    /// Format events into binary representation (for efficient export).
    fn export_binary(&self) -> Vec<u8> {
        let mut data: Vec<u8> = Vec::new();
        // Header: magic + version + event count
        data.extend_from_slice(b"GTRC"); // Genesis TRaCe
        data.extend_from_slice(&1u32.to_le_bytes()); // version
        let events = self.read_recent(RING_BUFFER_SIZE);
        data.extend_from_slice(&(events.len() as u32).to_le_bytes());

        for event in &events {
            // Write binary header
            data.extend_from_slice(&event.seq.to_le_bytes());
            data.extend_from_slice(&event.timestamp_ms.to_le_bytes());
            data.extend_from_slice(&event.cpu.to_le_bytes());
            data.extend_from_slice(&event.pid.to_le_bytes());
            data.push(event.category as u8);
            data.push(event.level as u8);
            let name_bytes = event.name.as_bytes();
            data.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            data.extend_from_slice(name_bytes);
            data.extend_from_slice(&(event.fields.len() as u16).to_le_bytes());
            for (k, v) in &event.fields {
                let kb = k.as_bytes();
                let vb = v.as_bytes();
                data.extend_from_slice(&(kb.len() as u16).to_le_bytes());
                data.extend_from_slice(kb);
                data.extend_from_slice(&(vb.len() as u16).to_le_bytes());
                data.extend_from_slice(vb);
            }
            for arg in &event.args {
                data.extend_from_slice(&arg.to_le_bytes());
            }
        }
        data
    }

    // ------- Statistics -------

    fn status(&self) -> String {
        let enabled = TRACING_ENABLED.load(Ordering::Relaxed);
        let graph_enabled = GRAPH_TRACER_ENABLED.load(Ordering::Relaxed);
        let mut total_buf_events: usize = 0;
        let mut total_buf_dropped: u64 = 0;
        let mut total_bytes: u64 = 0;

        for buf in &self.buffers {
            total_buf_events += buf.events.len();
            total_buf_dropped += buf.dropped;
            total_bytes += buf.bytes_written;
        }

        let total_graph_entries: usize = self.graph_states.iter().map(|gs| gs.entries.len()).sum();

        format!(
            "Tracing: {}\n\
             Graph tracer: {}\n\
             Level: {:?}\n\
             Buffers: {} (per-CPU)\n\
             Events recorded: {}\n\
             Events in buffer: {}\n\
             Events dropped: {}\n\
             Bytes written: {}\n\
             Graph entries: {}\n\
             Tracepoints: {} ({} enabled, {} dynamic)\n\
             Filters: {}\n\
             Snapshots: {}/{}\n",
            if enabled { "ENABLED" } else { "DISABLED" },
            if graph_enabled { "ENABLED" } else { "DISABLED" },
            self.level_threshold,
            self.buffers.len(),
            self.total_events,
            total_buf_events,
            total_buf_dropped,
            total_bytes,
            total_graph_entries,
            self.tracepoints.len(),
            self.tracepoints.iter().filter(|tp| tp.enabled).count(),
            self.tracepoints.iter().filter(|tp| tp.is_dynamic).count(),
            self.filters.len(),
            self.snapshots.len(),
            self.max_snapshots,
        )
    }

    /// Per-CPU buffer statistics.
    fn per_cpu_stats(&self) -> Vec<(usize, usize, u64, u32, u64)> {
        self.buffers
            .iter()
            .enumerate()
            .map(|(i, buf)| {
                (
                    i,
                    buf.events.len(),
                    buf.dropped,
                    buf.usage_percent(),
                    buf.bytes_written,
                )
            })
            .collect()
    }

    fn list_tracepoints(&self) -> Vec<(String, TraceCategory, bool, u64)> {
        self.tracepoints
            .iter()
            .map(|tp| (tp.name.clone(), tp.category, tp.enabled, tp.hit_count))
            .collect()
    }

    /// Get top N tracepoints by hit count.
    fn top_tracepoints(&self, n: usize) -> Vec<(String, u64)> {
        let mut sorted: Vec<(String, u64)> = self
            .tracepoints
            .iter()
            .map(|tp| (tp.name.clone(), tp.hit_count))
            .collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        sorted.truncate(n);
        sorted
    }
}

// ---------------------------------------------------------------------------
// Tracing errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum TracingError {
    TooManyFilters,
    NotFound,
    BufferFull,
}

// ---------------------------------------------------------------------------
// Global tracing subsystem and public API
// ---------------------------------------------------------------------------

static TRACING: Mutex<TracingSubsystem> = Mutex::new(TracingSubsystem::new());

// --- Enable / disable ---
pub fn enable() {
    TRACING_ENABLED.store(true, Ordering::Release);
}
pub fn disable() {
    TRACING_ENABLED.store(false, Ordering::Release);
}
#[inline]
pub fn is_enabled() -> bool {
    TRACING_ENABLED.load(Ordering::Relaxed)
}

pub fn enable_graph_tracer() {
    GRAPH_TRACER_ENABLED.store(true, Ordering::Release);
}
pub fn disable_graph_tracer() {
    GRAPH_TRACER_ENABLED.store(false, Ordering::Release);
}
#[inline]
pub fn is_graph_tracer_enabled() -> bool {
    GRAPH_TRACER_ENABLED.load(Ordering::Relaxed)
}

// --- Tracepoint management ---
pub fn register_tracepoint(name: &str, category: TraceCategory, desc: &str) -> bool {
    TRACING.lock().register_tracepoint(name, category, desc)
}
pub fn register_dynamic_tracepoint(
    name: &str,
    category: TraceCategory,
    desc: &str,
    callback: u64,
) -> bool {
    TRACING
        .lock()
        .register_dynamic_tracepoint(name, category, desc, callback)
}
pub fn unregister_dynamic_tracepoint(name: &str) -> bool {
    TRACING.lock().unregister_dynamic_tracepoint(name)
}
pub fn set_tracepoint_enabled(name: &str, enabled: bool) -> bool {
    TRACING.lock().set_tracepoint_enabled(name, enabled)
}
pub fn set_category_enabled(category: TraceCategory, enabled: bool) -> usize {
    TRACING.lock().set_category_enabled(category, enabled)
}

// --- Event recording ---
pub fn trace_event(
    name: &str,
    category: TraceCategory,
    level: TraceLevel,
    fields: Vec<(String, String)>,
    args: [u64; 4],
) {
    if !is_enabled() {
        return;
    }
    let event = TraceEvent {
        seq: 0,
        timestamp_ms: 0,
        cpu: 0,
        pid: 0,
        category,
        level,
        name: String::from(name),
        fields,
        args,
    };
    TRACING.lock().record_event(event);
}

pub fn trace_simple(name: &str, category: TraceCategory, level: TraceLevel) {
    trace_event(name, category, level, Vec::new(), [0; 4]);
}

// --- Function graph tracer ---
pub fn graph_entry(func_addr: u64, func_name: &str) {
    if !is_graph_tracer_enabled() {
        return;
    }
    TRACING.lock().graph_entry(func_addr, func_name);
}
pub fn graph_exit(func_addr: u64, func_name: &str) {
    if !is_graph_tracer_enabled() {
        return;
    }
    TRACING.lock().graph_exit(func_addr, func_name);
}
pub fn format_graph() -> String {
    TRACING.lock().format_graph()
}
pub fn clear_graph() {
    TRACING.lock().clear_graph();
}

// --- Filters ---
pub fn add_filter(filter: TraceFilter) -> Result<u32, TracingError> {
    TRACING.lock().add_filter(filter)
}
pub fn remove_filter(filter_id: u32) -> bool {
    TRACING.lock().remove_filter(filter_id)
}
pub fn clear_filters() {
    TRACING.lock().clear_filters();
}
pub fn set_level(level: TraceLevel) {
    TRACING.lock().set_level(level);
}

// --- Reading / exporting ---
pub fn read_recent(max_count: usize) -> Vec<TraceEvent> {
    TRACING.lock().read_recent(max_count)
}
pub fn drain_all() -> Vec<TraceEvent> {
    TRACING.lock().drain_all()
}
pub fn clear_all() {
    TRACING.lock().clear_all();
}

// --- Snapshots ---
pub fn take_snapshot() -> usize {
    TRACING.lock().take_snapshot()
}
pub fn list_snapshots() -> Vec<(usize, u64, u64, u64)> {
    TRACING.lock().list_snapshots()
}

// --- Export ---
pub fn export_binary() -> Vec<u8> {
    TRACING.lock().export_binary()
}

// --- Status and statistics ---
pub fn status() -> String {
    TRACING.lock().status()
}
pub fn per_cpu_stats() -> Vec<(usize, usize, u64, u32, u64)> {
    TRACING.lock().per_cpu_stats()
}
pub fn list_tracepoints() -> Vec<(String, TraceCategory, bool, u64)> {
    TRACING.lock().list_tracepoints()
}
pub fn top_tracepoints(n: usize) -> Vec<(String, u64)> {
    TRACING.lock().top_tracepoints(n)
}
pub fn format_event(event: &TraceEvent) -> String {
    TracingSubsystem::format_event(event)
}

pub fn init() {
    let ncpus = crate::smp::num_cpus() as usize;
    let ncpus = if ncpus == 0 { 1 } else { ncpus };

    let mut tracing = TRACING.lock();
    tracing.init_buffers(ncpus);

    // Register built-in tracepoints
    tracing.register_tracepoint(
        "sched_switch",
        TraceCategory::Sched,
        "Context switch between tasks",
    );
    tracing.register_tracepoint("sched_wakeup", TraceCategory::Sched, "Task woken up");
    tracing.register_tracepoint("sched_fork", TraceCategory::Sched, "New task created");
    tracing.register_tracepoint("sched_exit", TraceCategory::Sched, "Task exiting");
    tracing.register_tracepoint(
        "sched_migrate",
        TraceCategory::Sched,
        "Task migrated between CPUs",
    );
    tracing.register_tracepoint(
        "page_fault",
        TraceCategory::Memory,
        "Page fault handler entry",
    );
    tracing.register_tracepoint("page_alloc", TraceCategory::Memory, "Page frame allocated");
    tracing.register_tracepoint("page_free", TraceCategory::Memory, "Page frame freed");
    tracing.register_tracepoint("kmalloc", TraceCategory::Memory, "Kernel memory allocation");
    tracing.register_tracepoint("kfree", TraceCategory::Memory, "Kernel memory free");
    tracing.register_tracepoint("irq_entry", TraceCategory::Irq, "Interrupt handler entry");
    tracing.register_tracepoint("irq_exit", TraceCategory::Irq, "Interrupt handler exit");
    tracing.register_tracepoint(
        "softirq_entry",
        TraceCategory::Irq,
        "Soft IRQ handler entry",
    );
    tracing.register_tracepoint("softirq_exit", TraceCategory::Irq, "Soft IRQ handler exit");
    tracing.register_tracepoint("syscall_enter", TraceCategory::Syscall, "System call entry");
    tracing.register_tracepoint("syscall_exit", TraceCategory::Syscall, "System call exit");
    tracing.register_tracepoint(
        "block_read",
        TraceCategory::Block,
        "Block device read request",
    );
    tracing.register_tracepoint(
        "block_write",
        TraceCategory::Block,
        "Block device write request",
    );
    tracing.register_tracepoint(
        "block_complete",
        TraceCategory::Block,
        "Block IO completion",
    );
    tracing.register_tracepoint("net_rx", TraceCategory::Net, "Network packet received");
    tracing.register_tracepoint("net_tx", TraceCategory::Net, "Network packet transmitted");
    tracing.register_tracepoint("net_drop", TraceCategory::Net, "Network packet dropped");
    tracing.register_tracepoint("timer_tick", TraceCategory::Timer, "Timer interrupt tick");
    tracing.register_tracepoint(
        "timer_expire",
        TraceCategory::Timer,
        "Timer expiration callback",
    );
    tracing.register_tracepoint("cpu_idle", TraceCategory::Power, "CPU entering idle state");
    tracing.register_tracepoint(
        "cpu_frequency",
        TraceCategory::Power,
        "CPU frequency change",
    );

    let tp_count = tracing.tracepoints.len();
    drop(tracing);

    TRACING_ENABLED.store(false, Ordering::Release);

    serial_println!(
        "  [tracing] Kernel tracing initialized ({} per-CPU buffers, {} tracepoints, graph tracer ready)",
        ncpus, tp_count,
    );
}
