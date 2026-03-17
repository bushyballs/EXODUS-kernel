/// Performance events and hardware counters for Genesis
///
/// Provides access to CPU performance monitoring counters (PMCs) for profiling
/// and performance analysis. Supports hardware counters (cycles, instructions,
/// cache misses, branch mispredictions), software counters, and sampling-based
/// profiling with configurable sample periods.
///
/// Uses x86_64 MSRs: IA32_PERFEVTSELx and IA32_PMCx for counter programming.
/// Sampling generates NMI interrupts on counter overflow for profile collection.
///
/// Features:
/// - Hardware performance counter abstraction (via x86 PMC MSRs)
/// - Software events: context switches, page faults, CPU migrations
/// - Event scheduling (multiplex if more events than HW counters)
/// - Per-CPU and per-process event tracking
/// - Ring buffer for event samples (IP, timestamp, pid, tid)
/// - Sampling with configurable period/frequency
/// - Event group support (read multiple counters atomically)
/// - Counter overflow interrupt handling
///
/// Inspired by: Linux perf_events (kernel/events/). All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Maximum hardware counters per CPU (typical x86_64 has 4-8 general purpose)
const MAX_HW_COUNTERS: usize = 8;

/// Maximum active perf events system-wide
const MAX_EVENTS: usize = 64;

/// Maximum samples in the sample buffer
const MAX_SAMPLES: usize = 4096;

/// Maximum event groups
const MAX_GROUPS: usize = 16;

/// Maximum events per group
const MAX_EVENTS_PER_GROUP: usize = 8;

/// Multiplexing time slice (ms) — how often we rotate multiplexed events
const MULTIPLEX_INTERVAL_MS: u64 = 10;

/// Maximum per-CPU event state entries
const MAX_CPUS: usize = 64;

/// MSR addresses for performance monitoring
mod msr {
    pub const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;
    pub const IA32_PERF_GLOBAL_STATUS: u32 = 0x38E;
    pub const IA32_PERF_GLOBAL_OVF_CTRL: u32 = 0x390;
    pub const IA32_FIXED_CTR_CTRL: u32 = 0x38D;
    pub const IA32_PERFEVTSEL0: u32 = 0x186;
    pub const IA32_PMC0: u32 = 0x0C1;
    pub const IA32_FIXED_CTR0: u32 = 0x309;
}

// ---------------------------------------------------------------------------
// Event types
// ---------------------------------------------------------------------------

/// Hardware event types (maps to x86 architectural performance events)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HwEventType {
    CpuCycles,
    Instructions,
    CacheReferences,
    CacheMisses,
    BranchInstructions,
    BranchMisses,
    BusCycles,
    RefCycles,
}

impl HwEventType {
    /// Get the x86 architectural event select + unit mask
    fn to_event_select(self) -> (u8, u8) {
        match self {
            HwEventType::CpuCycles => (0x3C, 0x00),
            HwEventType::Instructions => (0x00, 0x01),
            HwEventType::CacheReferences => (0x2E, 0x4F),
            HwEventType::CacheMisses => (0x2E, 0x41),
            HwEventType::BranchInstructions => (0xC4, 0x00),
            HwEventType::BranchMisses => (0xC5, 0x00),
            HwEventType::BusCycles => (0x3C, 0x01),
            HwEventType::RefCycles => (0x00, 0x03),
        }
    }

    fn name(self) -> &'static str {
        match self {
            HwEventType::CpuCycles => "cpu-cycles",
            HwEventType::Instructions => "instructions",
            HwEventType::CacheReferences => "cache-references",
            HwEventType::CacheMisses => "cache-misses",
            HwEventType::BranchInstructions => "branch-instructions",
            HwEventType::BranchMisses => "branch-misses",
            HwEventType::BusCycles => "bus-cycles",
            HwEventType::RefCycles => "ref-cycles",
        }
    }
}

/// Software event types (kernel-maintained counters)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwEventType {
    ContextSwitches,
    PageFaults,
    PageFaultMinor,
    PageFaultMajor,
    CpuMigrations,
    AlignmentFaults,
}

impl SwEventType {
    fn name(self) -> &'static str {
        match self {
            SwEventType::ContextSwitches => "context-switches",
            SwEventType::PageFaults => "page-faults",
            SwEventType::PageFaultMinor => "page-faults:minor",
            SwEventType::PageFaultMajor => "page-faults:major",
            SwEventType::CpuMigrations => "cpu-migrations",
            SwEventType::AlignmentFaults => "alignment-faults",
        }
    }
}

/// Event source
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventSource {
    Hardware(HwEventType),
    Software(SwEventType),
}

// ---------------------------------------------------------------------------
// Event configuration and state
// ---------------------------------------------------------------------------

/// Perf event configuration
#[derive(Debug, Clone, Copy)]
pub struct EventConfig {
    pub source: EventSource,
    /// Sample period (0 = counting mode, >0 = sampling every N events)
    pub sample_period: u64,
    /// Sample frequency (Hz, alternative to sample_period; 0 = use period)
    pub sample_freq: u64,
    /// Whether to count in kernel mode
    pub kernel: bool,
    /// Whether to count in user mode
    pub user: bool,
    /// Specific CPU to monitor (-1 for all)
    pub cpu: i32,
    /// Process ID to monitor (0 for all)
    pub pid: u32,
    /// Enable on creation
    pub enabled: bool,
    /// Event group ID (0 = no group)
    pub group_id: u32,
    /// Whether this is the group leader
    pub is_group_leader: bool,
    /// Whether to include callchain in samples
    pub sample_callchain: bool,
}

impl EventConfig {
    pub fn counting(source: EventSource) -> Self {
        EventConfig {
            source,
            sample_period: 0,
            sample_freq: 0,
            kernel: true,
            user: true,
            cpu: -1,
            pid: 0,
            enabled: true,
            group_id: 0,
            is_group_leader: false,
            sample_callchain: false,
        }
    }

    pub fn sampling(source: EventSource, period: u64) -> Self {
        EventConfig {
            source,
            sample_period: period,
            sample_freq: 0,
            kernel: true,
            user: true,
            cpu: -1,
            pid: 0,
            enabled: true,
            group_id: 0,
            is_group_leader: false,
            sample_callchain: false,
        }
    }

    pub fn counting_for_pid(source: EventSource, pid: u32) -> Self {
        EventConfig {
            source,
            sample_period: 0,
            sample_freq: 0,
            kernel: true,
            user: true,
            cpu: -1,
            pid,
            enabled: true,
            group_id: 0,
            is_group_leader: false,
            sample_callchain: false,
        }
    }

    pub fn counting_for_cpu(source: EventSource, cpu: i32) -> Self {
        EventConfig {
            source,
            sample_period: 0,
            sample_freq: 0,
            kernel: true,
            user: true,
            cpu,
            pid: 0,
            enabled: true,
            group_id: 0,
            is_group_leader: false,
            sample_callchain: false,
        }
    }
}

/// A single performance event instance
pub struct PerfEvent {
    /// Unique event ID
    pub id: u32,
    /// Configuration
    pub config: EventConfig,
    /// Counter value (cumulative)
    pub count: u64,
    /// Hardware counter index assigned (-1 if software or not yet assigned)
    pub hw_counter_idx: i32,
    /// Whether event is currently active (counting)
    pub active: bool,
    /// Overflow count (for sampling)
    pub overflow_count: u64,
    /// Time enabled (in ms)
    pub time_enabled_ms: u64,
    /// Time running (in ms, may be < enabled if multiplexed)
    pub time_running_ms: u64,
    /// Timestamp when event was last started
    pub start_time_ms: u64,
    /// Accumulated count from previous multiplex intervals
    pub saved_count: u64,
    /// Whether this event is waiting for a HW counter (multiplexed)
    pub pending: bool,
    /// Counter value at last overflow (for sampling period)
    pub overflow_threshold: u64,
}

// ---------------------------------------------------------------------------
// Sample record
// ---------------------------------------------------------------------------

/// A sample record captured during profiling
#[derive(Debug, Clone)]
pub struct PerfSample {
    pub event_id: u32,
    pub ip: u64,
    pub pid: u32,
    pub tid: u32,
    pub cpu: u32,
    pub timestamp_ms: u64,
    pub counter_value: u64,
    /// Callchain (list of return addresses, if callchain sampling enabled)
    pub callchain: Vec<u64>,
    /// Weight (for latency-weighted sampling)
    pub weight: u64,
}

/// Per-CPU sample ring buffer
struct SampleRingBuffer {
    samples: Vec<PerfSample>,
    head: usize,
    capacity: usize,
    lost: u64,
}

impl SampleRingBuffer {
    const fn new() -> Self {
        SampleRingBuffer {
            samples: Vec::new(),
            head: 0,
            capacity: MAX_SAMPLES,
            lost: 0,
        }
    }

    fn push(&mut self, sample: PerfSample) {
        if self.samples.len() < self.capacity {
            self.samples.push(sample);
        } else {
            let idx = self.head % self.capacity;
            self.samples[idx] = sample;
            self.head = self.head.wrapping_add(1);
            self.lost = self.lost.saturating_add(1);
        }
    }

    fn drain(&mut self) -> Vec<PerfSample> {
        let result = core::mem::take(&mut self.samples);
        self.head = 0;
        result
    }

    fn len(&self) -> usize {
        self.samples.len()
    }

    fn read_recent(&self, max: usize) -> Vec<PerfSample> {
        if self.samples.is_empty() {
            return Vec::new();
        }
        let n = max.min(self.samples.len());
        let start = self.samples.len().saturating_sub(n);
        self.samples[start..].to_vec()
    }

    fn clear(&mut self) {
        self.samples.clear();
        self.head = 0;
    }
}

// ---------------------------------------------------------------------------
// Event groups
// ---------------------------------------------------------------------------

/// An event group — multiple events read atomically.
struct EventGroup {
    id: u32,
    /// Event IDs in this group (first is the leader)
    event_ids: Vec<u32>,
    /// Whether the group is active
    active: bool,
}

// ---------------------------------------------------------------------------
// Perf profile (aggregated output)
// ---------------------------------------------------------------------------

/// Perf profile (aggregated sampling data)
#[derive(Debug, Clone)]
pub struct PerfProfile {
    pub name: String,
    pub total_samples: u64,
    pub duration_ms: u64,
    pub ip_histogram: Vec<(u64, u64)>,
    pub cpu_samples: Vec<(u32, u64)>,
    pub pid_samples: Vec<(u32, u64)>,
}

// ---------------------------------------------------------------------------
// Hardware counter state per CPU
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct HwCounterState {
    /// Which event ID is using each counter slot (0 = free)
    assigned: [u32; MAX_HW_COUNTERS],
    /// Number of counters available
    num_counters: usize,
}

impl HwCounterState {
    const fn new() -> Self {
        HwCounterState {
            assigned: [0; MAX_HW_COUNTERS],
            num_counters: 4,
        }
    }
}

/// Per-CPU perf state
struct PerCpuPerfState {
    hw_state: HwCounterState,
    sample_buffer: SampleRingBuffer,
    /// Events currently scheduled (active) on this CPU
    active_events: Vec<u32>,
    /// Events waiting to be scheduled (multiplexed)
    pending_events: Vec<u32>,
    /// Last multiplex rotation timestamp
    last_multiplex_ms: u64,
}

impl PerCpuPerfState {
    const fn new() -> Self {
        PerCpuPerfState {
            hw_state: HwCounterState::new(),
            sample_buffer: SampleRingBuffer::new(),
            active_events: Vec::new(),
            pending_events: Vec::new(),
            last_multiplex_ms: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Main subsystem
// ---------------------------------------------------------------------------

struct PerfEventSubsystem {
    events: Vec<PerfEvent>,
    groups: Vec<EventGroup>,
    per_cpu: Vec<PerCpuPerfState>,
    next_id: u32,
    next_group_id: u32,
    /// PMU version detected from CPUID
    pmu_version: u8,
    num_gp_counters: u8,
    gp_counter_width: u8,
    num_fixed_counters: u8,
    /// Global software counters (indexed by SwEventType ordinal)
    sw_counters: [u64; 8],
    /// Per-PID software counters: (pid, sw_type_idx, count)
    per_pid_sw: Vec<(u32, usize, u64)>,
    profiling_active: bool,
    /// Total overflow interrupts handled
    total_overflows: u64,
}

impl PerfEventSubsystem {
    const fn new() -> Self {
        PerfEventSubsystem {
            events: Vec::new(),
            groups: Vec::new(),
            per_cpu: Vec::new(),
            next_id: 1,
            next_group_id: 1,
            pmu_version: 0,
            num_gp_counters: 0,
            gp_counter_width: 0,
            num_fixed_counters: 0,
            sw_counters: [0; 8],
            per_pid_sw: Vec::new(),
            profiling_active: false,
            total_overflows: 0,
        }
    }

    /// Detect PMU capabilities via CPUID leaf 0x0A
    fn detect_pmu(&mut self) {
        let eax: u32;
        let edx: u32;

        unsafe {
            core::arch::asm!(
                "xchg rsi, rbx",
                "cpuid",
                "xchg rsi, rbx",
                inout("eax") 0x0A_u32 => eax,
                out("ecx") _,
                out("edx") edx,
                out("rsi") _,
            );
        }

        self.pmu_version = (eax & 0xFF) as u8;
        self.num_gp_counters = ((eax >> 8) & 0xFF) as u8;
        self.gp_counter_width = ((eax >> 16) & 0xFF) as u8;

        if self.pmu_version >= 2 {
            self.num_fixed_counters = (edx & 0x1F) as u8;
        }

        // Initialize per-CPU state
        let ncpus = crate::smp::num_cpus().max(1) as usize;
        let usable = (self.num_gp_counters as usize).min(MAX_HW_COUNTERS);
        for _ in 0..ncpus {
            let mut state = PerCpuPerfState::new();
            state.hw_state.num_counters = usable;
            self.per_cpu.push(state);
        }
    }

    /// Read an MSR
    unsafe fn read_msr(msr: u32) -> u64 {
        let lo: u32;
        let hi: u32;
        core::arch::asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") lo,
            out("edx") hi,
        );
        ((hi as u64) << 32) | (lo as u64)
    }

    /// Write an MSR
    unsafe fn write_msr(msr: u32, value: u64) {
        let lo = value as u32;
        let hi = (value >> 32) as u32;
        core::arch::asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") lo,
            in("edx") hi,
        );
    }

    /// Program a hardware counter
    fn program_hw_counter(
        &self,
        cpu: usize,
        idx: usize,
        event_sel: u8,
        unit_mask: u8,
        kernel: bool,
        user: bool,
    ) {
        if cpu >= self.per_cpu.len() {
            return;
        }
        if idx >= self.per_cpu[cpu].hw_state.num_counters {
            return;
        }

        let mut val: u64 = (event_sel as u64) | ((unit_mask as u64) << 8);
        if kernel {
            val |= 1 << 17;
        }
        if user {
            val |= 1 << 16;
        }
        val |= 1 << 22; // EN bit

        unsafe {
            Self::write_msr(msr::IA32_PMC0 + idx as u32, 0);
            Self::write_msr(msr::IA32_PERFEVTSEL0 + idx as u32, val);
        }
    }

    /// Program a hardware counter for sampling (set initial count for overflow).
    fn program_hw_counter_sampling(
        &self,
        cpu: usize,
        idx: usize,
        event_sel: u8,
        unit_mask: u8,
        kernel: bool,
        user: bool,
        period: u64,
    ) {
        if cpu >= self.per_cpu.len() {
            return;
        }
        if idx >= self.per_cpu[cpu].hw_state.num_counters {
            return;
        }

        let mut val: u64 = (event_sel as u64) | ((unit_mask as u64) << 8);
        if kernel {
            val |= 1 << 17;
        }
        if user {
            val |= 1 << 16;
        }
        val |= 1 << 22; // EN bit
        val |= 1 << 20; // INT bit — enable interrupt on overflow

        // Set counter to -(period) so it overflows after `period` events
        let counter_width_mask = if self.gp_counter_width > 0 {
            (1u64 << self.gp_counter_width) - 1
        } else {
            0xFFFF_FFFF_FFFF
        };
        let initial = counter_width_mask.wrapping_sub(period) & counter_width_mask;

        unsafe {
            Self::write_msr(msr::IA32_PMC0 + idx as u32, initial);
            Self::write_msr(msr::IA32_PERFEVTSEL0 + idx as u32, val);
        }
    }

    /// Read a hardware counter value
    fn read_hw_counter(&self, idx: usize) -> u64 {
        if self.per_cpu.is_empty() {
            return 0;
        }
        let cpu = crate::smp::current_cpu() as usize;
        if cpu >= self.per_cpu.len() {
            return 0;
        }
        if idx >= self.per_cpu[cpu].hw_state.num_counters {
            return 0;
        }
        unsafe { Self::read_msr(msr::IA32_PMC0 + idx as u32) }
    }

    /// Stop a hardware counter
    fn stop_hw_counter(&self, cpu: usize, idx: usize) {
        if cpu >= self.per_cpu.len() {
            return;
        }
        if idx >= self.per_cpu[cpu].hw_state.num_counters {
            return;
        }
        unsafe {
            Self::write_msr(msr::IA32_PERFEVTSEL0 + idx as u32, 0);
        }
    }

    /// Allocate a hardware counter slot on a specific CPU
    fn alloc_hw_counter(&mut self, cpu: usize) -> Option<usize> {
        if cpu >= self.per_cpu.len() {
            return None;
        }
        for i in 0..self.per_cpu[cpu].hw_state.num_counters {
            if self.per_cpu[cpu].hw_state.assigned[i] == 0 {
                return Some(i);
            }
        }
        None
    }

    /// Free a hardware counter slot
    fn free_hw_counter(&mut self, cpu: usize, idx: usize) {
        if cpu >= self.per_cpu.len() {
            return;
        }
        if idx < MAX_HW_COUNTERS {
            self.per_cpu[cpu].hw_state.assigned[idx] = 0;
            self.stop_hw_counter(cpu, idx);
        }
    }

    // ------- Event group management -------

    /// Create a new event group. Returns the group ID.
    fn create_group(&mut self) -> u32 {
        let id = self.next_group_id;
        self.next_group_id = self.next_group_id.saturating_add(1);
        self.groups.push(EventGroup {
            id,
            event_ids: Vec::new(),
            active: false,
        });
        id
    }

    /// Read all events in a group atomically.
    fn read_group(&mut self, group_id: u32) -> Vec<(u32, u64)> {
        let event_ids: Vec<u32> = self
            .groups
            .iter()
            .find(|g| g.id == group_id)
            .map(|g| g.event_ids.clone())
            .unwrap_or_default();

        let mut results = Vec::new();
        for eid in &event_ids {
            if let Ok(val) = self.read_event(*eid) {
                results.push((*eid, val));
            }
        }
        results
    }

    /// Enable all events in a group.
    fn enable_group(&mut self, group_id: u32) -> Result<(), PerfError> {
        let event_ids: Vec<u32> = self
            .groups
            .iter()
            .find(|g| g.id == group_id)
            .map(|g| g.event_ids.clone())
            .ok_or(PerfError::NotFound)?;

        for eid in &event_ids {
            let _ = self.enable_event(*eid);
        }
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == group_id) {
            g.active = true;
        }
        Ok(())
    }

    /// Disable all events in a group.
    fn disable_group(&mut self, group_id: u32) -> Result<(), PerfError> {
        let event_ids: Vec<u32> = self
            .groups
            .iter()
            .find(|g| g.id == group_id)
            .map(|g| g.event_ids.clone())
            .ok_or(PerfError::NotFound)?;

        for eid in &event_ids {
            let _ = self.disable_event(*eid);
        }
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == group_id) {
            g.active = false;
        }
        Ok(())
    }

    // ------- Event management -------

    /// Create a new perf event
    fn create_event(&mut self, config: EventConfig) -> Result<u32, PerfError> {
        if self.events.len() >= MAX_EVENTS {
            return Err(PerfError::TooManyEvents);
        }

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let cpu = if config.cpu >= 0 {
            config.cpu as usize
        } else {
            crate::smp::current_cpu() as usize
        };
        let mut hw_idx: i32 = -1;
        let mut pending = false;

        // For hardware events, allocate a PMC
        if let EventSource::Hardware(hw_type) = config.source {
            match self.alloc_hw_counter(cpu) {
                Some(idx) => {
                    hw_idx = idx as i32;
                    self.per_cpu[cpu].hw_state.assigned[idx] = id;
                    if config.enabled {
                        let (event_sel, unit_mask) = hw_type.to_event_select();
                        if config.sample_period > 0 {
                            self.program_hw_counter_sampling(
                                cpu,
                                idx,
                                event_sel,
                                unit_mask,
                                config.kernel,
                                config.user,
                                config.sample_period,
                            );
                        } else {
                            self.program_hw_counter(
                                cpu,
                                idx,
                                event_sel,
                                unit_mask,
                                config.kernel,
                                config.user,
                            );
                        }
                    }
                }
                None => {
                    // No free counter — add to multiplex queue
                    pending = true;
                    if cpu < self.per_cpu.len() {
                        self.per_cpu[cpu].pending_events.push(id);
                    }
                }
            }
        }

        let now = crate::time::clock::uptime_ms();
        let event = PerfEvent {
            id,
            config,
            count: 0,
            hw_counter_idx: hw_idx,
            active: config.enabled && !pending,
            overflow_count: 0,
            time_enabled_ms: 0,
            time_running_ms: 0,
            start_time_ms: if config.enabled && !pending { now } else { 0 },
            saved_count: 0,
            pending,
            overflow_threshold: config.sample_period,
        };

        self.events.push(event);

        // Add to group if specified
        if config.group_id > 0 {
            if let Some(g) = self.groups.iter_mut().find(|g| g.id == config.group_id) {
                g.event_ids.push(id);
            }
        }

        Ok(id)
    }

    /// Read event counter value
    fn read_event(&mut self, event_id: u32) -> Result<u64, PerfError> {
        let idx = self
            .events
            .iter()
            .position(|e| e.id == event_id)
            .ok_or(PerfError::NotFound)?;

        let (source, hw_idx, saved) = {
            let event = &self.events[idx];
            (event.config.source, event.hw_counter_idx, event.saved_count)
        };

        let new_count = match source {
            EventSource::Hardware(_) => {
                if hw_idx >= 0 {
                    saved + self.read_hw_counter(hw_idx as usize)
                } else {
                    saved // multiplexed, not currently running
                }
            }
            EventSource::Software(sw_type) => self.sw_counters[sw_type as usize],
        };

        self.events[idx].count = new_count;
        Ok(new_count)
    }

    /// Enable an event
    fn enable_event(&mut self, event_id: u32) -> Result<(), PerfError> {
        let now = crate::time::clock::uptime_ms();
        let idx = self
            .events
            .iter()
            .position(|e| e.id == event_id)
            .ok_or(PerfError::NotFound)?;

        let (hw_idx, kernel, user, source, already_active, sample_period) = {
            let event = &self.events[idx];
            (
                event.hw_counter_idx,
                event.config.kernel,
                event.config.user,
                event.config.source,
                event.active,
                event.config.sample_period,
            )
        };

        if already_active {
            return Ok(());
        }

        if let EventSource::Hardware(hw_type) = source {
            if hw_idx >= 0 {
                let cpu = crate::smp::current_cpu() as usize;
                let (event_sel, unit_mask) = hw_type.to_event_select();
                if sample_period > 0 {
                    self.program_hw_counter_sampling(
                        cpu,
                        hw_idx as usize,
                        event_sel,
                        unit_mask,
                        kernel,
                        user,
                        sample_period,
                    );
                } else {
                    self.program_hw_counter(
                        cpu,
                        hw_idx as usize,
                        event_sel,
                        unit_mask,
                        kernel,
                        user,
                    );
                }
            }
        }

        let event = &mut self.events[idx];
        event.active = true;
        event.start_time_ms = now;
        Ok(())
    }

    /// Disable an event
    fn disable_event(&mut self, event_id: u32) -> Result<(), PerfError> {
        let now = crate::time::clock::uptime_ms();
        let _ = self.read_event(event_id);

        let idx = self
            .events
            .iter()
            .position(|e| e.id == event_id)
            .ok_or(PerfError::NotFound)?;

        let (hw_idx, active, start_time_ms) = {
            let event = &self.events[idx];
            (event.hw_counter_idx, event.active, event.start_time_ms)
        };

        if !active {
            return Ok(());
        }

        if hw_idx >= 0 {
            let cpu = crate::smp::current_cpu() as usize;
            self.stop_hw_counter(cpu, hw_idx as usize);
        }

        let elapsed = now.saturating_sub(start_time_ms);
        let event = &mut self.events[idx];
        event.active = false;
        event.time_enabled_ms += elapsed;
        event.time_running_ms += elapsed;
        Ok(())
    }

    /// Destroy an event
    fn destroy_event(&mut self, event_id: u32) -> Result<(), PerfError> {
        let idx = self
            .events
            .iter()
            .position(|e| e.id == event_id)
            .ok_or(PerfError::NotFound)?;

        let event = &self.events[idx];
        if event.hw_counter_idx >= 0 {
            let cpu = if event.config.cpu >= 0 {
                event.config.cpu as usize
            } else {
                0
            };
            self.free_hw_counter(cpu, event.hw_counter_idx as usize);
        }

        // Remove from any group
        let eid = event_id;
        for g in &mut self.groups {
            g.event_ids.retain(|&id| id != eid);
        }

        // Remove from per-CPU pending
        for state in &mut self.per_cpu {
            state.pending_events.retain(|&id| id != eid);
            state.active_events.retain(|&id| id != eid);
        }

        self.events.remove(idx);

        // Try to schedule a pending event into the freed counter
        self.try_schedule_pending();

        Ok(())
    }

    /// Try to schedule pending events into free HW counters (multiplex rotation).
    fn try_schedule_pending(&mut self) {
        for cpu_idx in 0..self.per_cpu.len() {
            while let Some(free_slot) = {
                let state = &self.per_cpu[cpu_idx];
                (0..state.hw_state.num_counters).find(|&i| state.hw_state.assigned[i] == 0)
            } {
                // Pop first pending event for this CPU
                let pending_id = {
                    let state = &mut self.per_cpu[cpu_idx];
                    if state.pending_events.is_empty() {
                        break;
                    }
                    state.pending_events.remove(0)
                };

                // Assign and program the counter — extract config before calling methods
                let hw_info =
                    if let Some(event) = self.events.iter_mut().find(|e| e.id == pending_id) {
                        event.hw_counter_idx = free_slot as i32;
                        event.pending = false;
                        event.active = true;
                        event.start_time_ms = crate::time::clock::uptime_ms();

                        self.per_cpu[cpu_idx].hw_state.assigned[free_slot] = pending_id;

                        if let EventSource::Hardware(hw_type) = event.config.source {
                            let (event_sel, unit_mask) = hw_type.to_event_select();
                            Some((
                                event_sel,
                                unit_mask,
                                event.config.kernel,
                                event.config.user,
                                event.config.sample_period,
                            ))
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                // Now program the counter without conflicting borrows
                if let Some((event_sel, unit_mask, kernel, user, sample_period)) = hw_info {
                    if sample_period > 0 {
                        self.program_hw_counter_sampling(
                            cpu_idx,
                            free_slot,
                            event_sel,
                            unit_mask,
                            kernel,
                            user,
                            sample_period,
                        );
                    } else {
                        self.program_hw_counter(
                            cpu_idx, free_slot, event_sel, unit_mask, kernel, user,
                        );
                    }
                }
            }
        }
    }

    /// Perform multiplexing rotation — called periodically from timer tick.
    fn multiplex_rotate(&mut self) {
        let now = crate::time::clock::uptime_ms();

        for cpu_idx in 0..self.per_cpu.len() {
            if now.saturating_sub(self.per_cpu[cpu_idx].last_multiplex_ms) < MULTIPLEX_INTERVAL_MS {
                continue;
            }
            self.per_cpu[cpu_idx].last_multiplex_ms = now;

            // Find events that are active on this CPU
            let active_hw_events: Vec<u32> = self
                .events
                .iter()
                .filter(|e| e.active && e.hw_counter_idx >= 0 && !e.pending)
                .filter(|e| e.config.cpu < 0 || e.config.cpu == cpu_idx as i32)
                .map(|e| e.id)
                .collect();

            let pending: Vec<u32> = self.per_cpu[cpu_idx].pending_events.clone();
            if pending.is_empty() {
                continue;
            }

            // Swap: save current counter values, stop, switch in pending
            if let Some(&evict_id) = active_hw_events.last() {
                // Extract the hw_counter_idx first without holding a mutable borrow
                let hw_idx = self
                    .events
                    .iter()
                    .find(|e| e.id == evict_id)
                    .map(|e| e.hw_counter_idx)
                    .unwrap_or(-1);

                if hw_idx >= 0 {
                    // Read the counter value before mutably borrowing events
                    let count = self.read_hw_counter(hw_idx as usize);

                    // Now mutably borrow the event to update it
                    if let Some(event) = self.events.iter_mut().find(|e| e.id == evict_id) {
                        event.saved_count += count;
                        event.count = event.saved_count;
                        let elapsed = now.saturating_sub(event.start_time_ms);
                        event.time_running_ms += elapsed;
                        event.active = false;
                        event.pending = true;
                    }

                    // Free the counter
                    self.per_cpu[cpu_idx].hw_state.assigned[hw_idx as usize] = 0;
                    self.stop_hw_counter(cpu_idx, hw_idx as usize);

                    // Move evicted event to pending
                    self.per_cpu[cpu_idx].pending_events.push(evict_id);
                }
            }

            // Try to schedule pending events
            self.try_schedule_pending();
        }
    }

    // ------- Sampling and overflow -------

    /// Handle counter overflow interrupt (called from NMI handler).
    fn handle_overflow(&mut self, cpu: usize) {
        self.total_overflows = self.total_overflows.saturating_add(1);

        // Check global overflow status MSR
        let status = unsafe { Self::read_msr(msr::IA32_PERF_GLOBAL_STATUS) };

        if cpu >= self.per_cpu.len() {
            return;
        }

        for idx in 0..self.per_cpu[cpu].hw_state.num_counters {
            if (status & (1u64 << idx)) == 0 {
                continue;
            }

            let event_id = self.per_cpu[cpu].hw_state.assigned[idx];
            if event_id == 0 {
                continue;
            }

            // Record a sample
            let ip = 0u64; // In real implementation, read from interrupt frame
            let pid = crate::process::getpid();
            let ts = crate::time::clock::uptime_ms();

            let sample = PerfSample {
                event_id,
                ip,
                pid,
                tid: pid,
                cpu: cpu as u32,
                timestamp_ms: ts,
                counter_value: 0,
                callchain: Vec::new(),
                weight: 0,
            };
            self.per_cpu[cpu].sample_buffer.push(sample);

            // Re-arm counter for next period — extract config before calling methods
            let rearm_info = if let Some(event) = self.events.iter_mut().find(|e| e.id == event_id)
            {
                event.overflow_count = event.overflow_count.saturating_add(1);
                if event.config.sample_period > 0 {
                    if let EventSource::Hardware(hw_type) = event.config.source {
                        let (es, um) = hw_type.to_event_select();
                        Some((
                            es,
                            um,
                            event.config.kernel,
                            event.config.user,
                            event.config.sample_period,
                        ))
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };
            if let Some((es, um, kernel, user, period)) = rearm_info {
                self.program_hw_counter_sampling(cpu, idx, es, um, kernel, user, period);
            }
        }

        // Clear overflow status
        unsafe {
            Self::write_msr(msr::IA32_PERF_GLOBAL_OVF_CTRL, status);
        }
    }

    /// Record a sample (called from NMI handler or software path)
    fn record_sample(&mut self, event_id: u32, ip: u64) {
        let cpu = crate::smp::current_cpu() as usize;
        let sample = PerfSample {
            event_id,
            ip,
            pid: crate::process::getpid(),
            tid: crate::process::getpid(),
            cpu: cpu as u32,
            timestamp_ms: crate::time::clock::uptime_ms(),
            counter_value: 0,
            callchain: Vec::new(),
            weight: 0,
        };

        if cpu < self.per_cpu.len() {
            self.per_cpu[cpu].sample_buffer.push(sample);
        }

        if let Some(event) = self.events.iter_mut().find(|e| e.id == event_id) {
            event.overflow_count = event.overflow_count.saturating_add(1);
        }
    }

    /// Increment a software counter
    fn sw_counter_inc(&mut self, sw_type: SwEventType) {
        let idx = sw_type as usize;
        if idx < self.sw_counters.len() {
            self.sw_counters[idx] = self.sw_counters[idx].saturating_add(1);
        }
        // Also track per-PID
        let pid = crate::process::getpid();
        if let Some(entry) = self
            .per_pid_sw
            .iter_mut()
            .find(|e| e.0 == pid && e.1 == idx)
        {
            entry.2 += 1;
        } else {
            self.per_pid_sw.push((pid, idx, 1));
        }
    }

    /// Increment a software counter for a specific PID.
    fn sw_counter_inc_for_pid(&mut self, sw_type: SwEventType, pid: u32) {
        let idx = sw_type as usize;
        if idx < self.sw_counters.len() {
            self.sw_counters[idx] = self.sw_counters[idx].saturating_add(1);
        }
        if let Some(entry) = self
            .per_pid_sw
            .iter_mut()
            .find(|e| e.0 == pid && e.1 == idx)
        {
            entry.2 += 1;
        } else {
            self.per_pid_sw.push((pid, idx, 1));
        }
    }

    /// Get software counter value for a specific PID.
    fn sw_counter_for_pid(&self, sw_type: SwEventType, pid: u32) -> u64 {
        let idx = sw_type as usize;
        self.per_pid_sw
            .iter()
            .find(|e| e.0 == pid && e.1 == idx)
            .map(|e| e.2)
            .unwrap_or(0)
    }

    // ------- Profiling and reporting -------

    /// Generate a profile from collected samples (across all CPUs)
    fn generate_profile(&self, name: &str) -> PerfProfile {
        let mut all_samples: Vec<&PerfSample> = Vec::new();
        for state in &self.per_cpu {
            for s in &state.sample_buffer.samples {
                all_samples.push(s);
            }
        }

        let mut ip_map: Vec<(u64, u64)> = Vec::new();
        let mut cpu_map: Vec<(u32, u64)> = Vec::new();
        let mut pid_map: Vec<(u32, u64)> = Vec::new();

        for sample in &all_samples {
            if let Some(entry) = ip_map.iter_mut().find(|(ip, _)| *ip == sample.ip) {
                entry.1 += 1;
            } else {
                ip_map.push((sample.ip, 1));
            }
            if let Some(entry) = cpu_map.iter_mut().find(|(cpu, _)| *cpu == sample.cpu) {
                entry.1 += 1;
            } else {
                cpu_map.push((sample.cpu, 1));
            }
            if let Some(entry) = pid_map.iter_mut().find(|(pid, _)| *pid == sample.pid) {
                entry.1 += 1;
            } else {
                pid_map.push((sample.pid, 1));
            }
        }

        ip_map.sort_by(|a, b| b.1.cmp(&a.1));

        let first_ts = all_samples.first().map(|s| s.timestamp_ms).unwrap_or(0);
        let last_ts = all_samples.last().map(|s| s.timestamp_ms).unwrap_or(0);

        PerfProfile {
            name: String::from(name),
            total_samples: all_samples.len() as u64,
            duration_ms: last_ts.saturating_sub(first_ts),
            ip_histogram: ip_map,
            cpu_samples: cpu_map,
            pid_samples: pid_map,
        }
    }

    /// Format a stat report (like `perf stat` output)
    fn stat_report(&mut self) -> String {
        let mut s = String::from("Performance counter stats:\n\n");

        for event in &mut self.events {
            if let EventSource::Hardware(_) = event.config.source {
                if event.hw_counter_idx >= 0 && event.active {
                    let hw_val =
                        unsafe { Self::read_msr(msr::IA32_PMC0 + event.hw_counter_idx as u32) };
                    event.count = event.saved_count + hw_val;
                }
            }

            let name = match event.config.source {
                EventSource::Hardware(hw) => hw.name(),
                EventSource::Software(sw) => sw.name(),
            };

            let mux_pct = if event.time_enabled_ms > 0 {
                (event.time_running_ms * 100) / event.time_enabled_ms
            } else {
                100
            };

            s.push_str(&format!(
                "  {:>16}  {:<24} ({:>3}% time running)\n",
                event.count, name, mux_pct
            ));
        }

        s.push_str(&format!(
            "\n  {} events active, {} pending multiplex\n",
            self.events.iter().filter(|e| e.active).count(),
            self.events.iter().filter(|e| e.pending).count()
        ));
        s.push_str(&format!(
            "  {} total overflow interrupts\n",
            self.total_overflows
        ));

        let total_samples: usize = self.per_cpu.iter().map(|s| s.sample_buffer.len()).sum();
        let total_lost: u64 = self.per_cpu.iter().map(|s| s.sample_buffer.lost).sum();
        s.push_str(&format!(
            "  {} samples buffered, {} lost\n",
            total_samples, total_lost
        ));

        s
    }

    /// Clear all samples
    fn clear_samples(&mut self) {
        for state in &mut self.per_cpu {
            state.sample_buffer.clear();
        }
    }

    /// Get number of active events
    fn active_count(&self) -> usize {
        self.events.iter().filter(|e| e.active).count()
    }

    /// List all events
    fn list_events(&self) -> Vec<(u32, &'static str, bool, u64)> {
        self.events
            .iter()
            .map(|e| {
                let name = match e.config.source {
                    EventSource::Hardware(hw) => hw.name(),
                    EventSource::Software(sw) => sw.name(),
                };
                (e.id, name, e.active, e.count)
            })
            .collect()
    }

    /// List event groups
    fn list_groups(&self) -> Vec<(u32, Vec<u32>, bool)> {
        self.groups
            .iter()
            .map(|g| (g.id, g.event_ids.clone(), g.active))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Perf errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum PerfError {
    NotFound,
    TooManyEvents,
    NoCounterAvailable,
    NotSupported,
    AlreadyActive,
    GroupFull,
}

// ---------------------------------------------------------------------------
// Global perf event subsystem and public API
// ---------------------------------------------------------------------------

static PERF: Mutex<PerfEventSubsystem> = Mutex::new(PerfEventSubsystem::new());

pub fn create_event(config: EventConfig) -> Result<u32, PerfError> {
    PERF.lock().create_event(config)
}
pub fn read_event(event_id: u32) -> Result<u64, PerfError> {
    PERF.lock().read_event(event_id)
}
pub fn enable_event(event_id: u32) -> Result<(), PerfError> {
    PERF.lock().enable_event(event_id)
}
pub fn disable_event(event_id: u32) -> Result<(), PerfError> {
    PERF.lock().disable_event(event_id)
}
pub fn destroy_event(event_id: u32) -> Result<(), PerfError> {
    PERF.lock().destroy_event(event_id)
}
pub fn record_sample(event_id: u32, ip: u64) {
    PERF.lock().record_sample(event_id, ip);
}
pub fn sw_counter_inc(sw_type: SwEventType) {
    PERF.lock().sw_counter_inc(sw_type);
}
pub fn sw_counter_inc_for_pid(sw_type: SwEventType, pid: u32) {
    PERF.lock().sw_counter_inc_for_pid(sw_type, pid);
}
pub fn sw_counter_for_pid(sw_type: SwEventType, pid: u32) -> u64 {
    PERF.lock().sw_counter_for_pid(sw_type, pid)
}
pub fn generate_profile(name: &str) -> PerfProfile {
    PERF.lock().generate_profile(name)
}
pub fn stat_report() -> String {
    PERF.lock().stat_report()
}
pub fn clear_samples() {
    PERF.lock().clear_samples();
}
pub fn list_events() -> Vec<(u32, &'static str, bool, u64)> {
    PERF.lock().list_events()
}
pub fn active_count() -> usize {
    PERF.lock().active_count()
}

// Event groups
pub fn create_group() -> u32 {
    PERF.lock().create_group()
}
pub fn read_group(group_id: u32) -> Vec<(u32, u64)> {
    PERF.lock().read_group(group_id)
}
pub fn enable_group(group_id: u32) -> Result<(), PerfError> {
    PERF.lock().enable_group(group_id)
}
pub fn disable_group(group_id: u32) -> Result<(), PerfError> {
    PERF.lock().disable_group(group_id)
}

// Multiplexing and overflow (called from timer/NMI)
pub fn multiplex_rotate() {
    PERF.lock().multiplex_rotate();
}
pub fn handle_overflow(cpu: usize) {
    PERF.lock().handle_overflow(cpu);
}

pub fn init() {
    let mut perf = PERF.lock();
    perf.detect_pmu();
    serial_println!(
        "  [perf] Performance events initialized (PMU v{}, {} GP counters, {} fixed, {}-bit, multiplex)",
        perf.pmu_version,
        perf.num_gp_counters,
        perf.num_fixed_counters,
        perf.gp_counter_width,
    );
}
