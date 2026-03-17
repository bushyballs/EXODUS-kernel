/// Linux perf_events interface for hardware and software performance counters.
///
/// Provides `perf_event_open`-style semantics as a bare-metal, no-heap
/// implementation.  All storage lives in a fixed-size static array guarded by
/// a Mutex.  Every counter operation uses saturating or wrapping arithmetic;
/// no floats, no heap, no panics.
///
/// ## Event taxonomy
///
/// | PERF_TYPE_*      | value | backing             |
/// |------------------|-------|---------------------|
/// | HARDWARE         | 0     | RDTSC / PMU MSRs    |
/// | SOFTWARE         | 1     | in-kernel atomics   |
/// | TRACEPOINT       | 2     | stub (returns 0)    |
/// | HW_CACHE         | 3     | stub (returns 0)    |
/// | RAW              | 4     | stub (returns 0)    |
///
/// ## File descriptors
///
/// Each open event is assigned a synthetic id in the range
/// `[1 .. MAX_PERF_EVENTS]`.  `perf_event_open` returns `Some(id)`.
///
/// ## Syscall
///
/// `sys_perf_event_open` implements SYS_PERF_EVENT_OPEN (298).  It treats
/// `attr_addr` as a pointer to a two-u32 record: `[event_type, config_lo]`.
/// The first field selects PERF_TYPE_* and the second is cast to a u64
/// config word.  Returns `fd = event_id + 100` on success, -1 on failure.
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Public constants — mirroring linux/perf_event.h
// ---------------------------------------------------------------------------

pub const MAX_PERF_EVENTS: usize = 64;
pub const MAX_PERF_GROUPS: usize = 8;
pub const PERF_SAMPLE_BUF: usize = 256;

// Event type constants
pub const PERF_TYPE_HARDWARE: u32 = 0;
pub const PERF_TYPE_SOFTWARE: u32 = 1;
pub const PERF_TYPE_TRACEPOINT: u32 = 2;
pub const PERF_TYPE_HW_CACHE: u32 = 3;
pub const PERF_TYPE_RAW: u32 = 4;

// Hardware event IDs (config field when event_type == PERF_TYPE_HARDWARE)
pub const PERF_COUNT_HW_CPU_CYCLES: u64 = 0;
pub const PERF_COUNT_HW_INSTRUCTIONS: u64 = 1;
pub const PERF_COUNT_HW_CACHE_REFERENCES: u64 = 2;
pub const PERF_COUNT_HW_CACHE_MISSES: u64 = 3;
pub const PERF_COUNT_HW_BRANCH_INSTRUCTIONS: u64 = 4;
pub const PERF_COUNT_HW_BRANCH_MISSES: u64 = 5;
pub const PERF_COUNT_HW_BUS_CYCLES: u64 = 6;
pub const PERF_COUNT_HW_STALLED_CYCLES_FRONTEND: u64 = 7;
pub const PERF_COUNT_HW_STALLED_CYCLES_BACKEND: u64 = 8;

// Software event IDs (config field when event_type == PERF_TYPE_SOFTWARE)
pub const PERF_COUNT_SW_CPU_CLOCK: u64 = 0;
pub const PERF_COUNT_SW_TASK_CLOCK: u64 = 1;
pub const PERF_COUNT_SW_PAGE_FAULTS: u64 = 2;
pub const PERF_COUNT_SW_CONTEXT_SWITCHES: u64 = 3;
pub const PERF_COUNT_SW_CPU_MIGRATIONS: u64 = 4;
pub const PERF_COUNT_SW_PAGE_FAULTS_MIN: u64 = 5;
pub const PERF_COUNT_SW_PAGE_FAULTS_MAJ: u64 = 6;

// Perf event flags
pub const PERF_FLAG_DISABLED: u32 = 1 << 0;
pub const PERF_FLAG_INHERIT: u32 = 1 << 1;
pub const PERF_FLAG_PINNED: u32 = 1 << 2;
pub const PERF_FLAG_SAMPLE_PERIOD: u32 = 1 << 10;

// ---------------------------------------------------------------------------
// PerfSample — one entry in a per-event ring buffer
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct PerfSample {
    pub cpu: u32,
    pub pid: u32,
    pub ip: u64,        // instruction pointer at the time of the sample
    pub period: u64,    // number of events elapsed since last sample
    pub timestamp: u64, // nanosecond timestamp
}

impl PerfSample {
    pub const fn empty() -> Self {
        PerfSample {
            cpu: 0,
            pid: 0,
            ip: 0,
            period: 0,
            timestamp: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// PerfEvent — state for one open event
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct PerfEvent {
    /// Non-zero id means slot is occupied.  id == slot_index + 1.
    pub id: u32,
    pub event_type: u32, // PERF_TYPE_*
    pub config: u64,     // event-specific selector
    pub pid: i32,        // -1 = all processes
    pub cpu: i32,        // -1 = all CPUs
    pub group_fd: i32,   // group leader id, -1 = standalone
    pub flags: u32,
    pub sample_period: u64, // sample every N events (0 = disabled)
    pub count: u64,         // accumulated event count (saturating)
    pub enabled: bool,
    pub active: bool,
    pub sample_buf: [PerfSample; PERF_SAMPLE_BUF],
    pub sample_head: u32, // next write position (wrapping)
    pub sample_tail: u32, // next read position  (wrapping)
}

impl PerfEvent {
    pub const fn empty() -> Self {
        PerfEvent {
            id: 0,
            event_type: PERF_TYPE_HARDWARE,
            config: 0,
            pid: -1,
            cpu: -1,
            group_fd: -1,
            flags: 0,
            sample_period: 0,
            count: 0,
            enabled: false,
            active: false,
            sample_buf: [PerfSample::empty(); PERF_SAMPLE_BUF],
            sample_head: 0,
            sample_tail: 0,
        }
    }

    #[inline]
    fn is_used(&self) -> bool {
        self.id != 0
    }
}

// ---------------------------------------------------------------------------
// Global event table
// ---------------------------------------------------------------------------

static PERF_EVENTS: Mutex<[PerfEvent; MAX_PERF_EVENTS]> =
    Mutex::new([PerfEvent::empty(); MAX_PERF_EVENTS]);

// ---------------------------------------------------------------------------
// PMU hardware counter read
// ---------------------------------------------------------------------------

/// Read a hardware counter value.
///
/// For `PERF_COUNT_HW_CPU_CYCLES` (config == 0) we use RDTSC which is always
/// available on x86-64.  All other hardware selectors are stubbed to 0 until
/// the PMU MSR layer maps them.
fn read_hw_counter(config: u64) -> u64 {
    match config {
        PERF_COUNT_HW_CPU_CYCLES => {
            // RDTSC: EDX:EAX — no float, no heap
            let lo: u32;
            let hi: u32;
            unsafe {
                core::arch::asm!(
                    "rdtsc",
                    out("eax") lo,
                    out("edx") hi,
                    options(nostack, nomem)
                );
            }
            ((hi as u64) << 32) | (lo as u64)
        }
        // All other HW selectors: stub returns 0
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Open a new perf event and return its id.
///
/// `flags` may include `PERF_FLAG_DISABLED` to open the event in a disabled
/// state.  Returns `None` if the table is full.
pub fn perf_event_open(
    event_type: u32,
    config: u64,
    pid: i32,
    cpu: i32,
    flags: u32,
) -> Option<u32> {
    let mut table = PERF_EVENTS.lock();

    // Find a free slot
    let mut slot = MAX_PERF_EVENTS;
    for i in 0..MAX_PERF_EVENTS {
        if !table[i].is_used() {
            slot = i;
            break;
        }
    }
    if slot == MAX_PERF_EVENTS {
        return None; // table full
    }

    let id = (slot as u32).saturating_add(1); // id is 1-based; 0 means free
    let enabled = (flags & PERF_FLAG_DISABLED) == 0;

    table[slot] = PerfEvent {
        id,
        event_type,
        config,
        pid,
        cpu,
        group_fd: -1,
        flags,
        sample_period: 0,
        count: 0,
        enabled,
        active: enabled,
        sample_buf: [PerfSample::empty(); PERF_SAMPLE_BUF],
        sample_head: 0,
        sample_tail: 0,
    };
    Some(id)
}

/// Close (free) a perf event by id.  Returns `true` on success.
pub fn perf_event_close(event_id: u32) -> bool {
    if event_id == 0 {
        return false;
    }
    let slot = (event_id as usize).saturating_sub(1);
    if slot >= MAX_PERF_EVENTS {
        return false;
    }
    let mut table = PERF_EVENTS.lock();
    if !table[slot].is_used() {
        return false;
    }
    table[slot] = PerfEvent::empty();
    true
}

/// Enable a previously opened (or disabled) event.  Returns `true` on success.
pub fn perf_event_enable(event_id: u32) -> bool {
    if event_id == 0 {
        return false;
    }
    let slot = (event_id as usize).saturating_sub(1);
    if slot >= MAX_PERF_EVENTS {
        return false;
    }
    let mut table = PERF_EVENTS.lock();
    if !table[slot].is_used() {
        return false;
    }
    table[slot].enabled = true;
    table[slot].active = true;
    true
}

/// Disable an event without closing it.  Returns `true` on success.
pub fn perf_event_disable(event_id: u32) -> bool {
    if event_id == 0 {
        return false;
    }
    let slot = (event_id as usize).saturating_sub(1);
    if slot >= MAX_PERF_EVENTS {
        return false;
    }
    let mut table = PERF_EVENTS.lock();
    if !table[slot].is_used() {
        return false;
    }
    table[slot].enabled = false;
    table[slot].active = false;
    true
}

/// Read the current accumulated count for `event_id`.
///
/// For HARDWARE events the counter is polled from the PMU via
/// `read_hw_counter`.  For SOFTWARE events the stored `count` is returned
/// directly (it is updated via `perf_sw_inc`).
pub fn perf_event_read(event_id: u32) -> Option<u64> {
    if event_id == 0 {
        return None;
    }
    let slot = (event_id as usize).saturating_sub(1);
    if slot >= MAX_PERF_EVENTS {
        return None;
    }
    let mut table = PERF_EVENTS.lock();
    if !table[slot].is_used() {
        return None;
    }
    let val = if table[slot].event_type == PERF_TYPE_HARDWARE {
        read_hw_counter(table[slot].config)
    } else {
        table[slot].count
    };
    table[slot].count = val;
    Some(val)
}

/// Reset the accumulated count to zero.  Returns `true` on success.
pub fn perf_event_reset(event_id: u32) -> bool {
    if event_id == 0 {
        return false;
    }
    let slot = (event_id as usize).saturating_sub(1);
    if slot >= MAX_PERF_EVENTS {
        return false;
    }
    let mut table = PERF_EVENTS.lock();
    if !table[slot].is_used() {
        return false;
    }
    table[slot].count = 0;
    true
}

/// Push a sample into the event's ring buffer.
///
/// The ring head advances with `wrapping_add`; if the buffer is full the
/// oldest entry is silently overwritten (standard perf ring semantics).
pub fn perf_push_sample(event_id: u32, cpu: u32, pid: u32, ip: u64, timestamp: u64) {
    if event_id == 0 {
        return;
    }
    let slot = (event_id as usize).saturating_sub(1);
    if slot >= MAX_PERF_EVENTS {
        return;
    }
    let mut table = PERF_EVENTS.lock();
    if !table[slot].is_used() {
        return;
    }

    let period = table[slot].count;
    let head = table[slot].sample_head as usize;
    if head < PERF_SAMPLE_BUF {
        table[slot].sample_buf[head] = PerfSample {
            cpu,
            pid,
            ip,
            period,
            timestamp,
        };
    }
    let buf_len = PERF_SAMPLE_BUF as u32;
    // Guard: buf_len is a compile-time constant > 0; safe.
    table[slot].sample_head = table[slot].sample_head.wrapping_add(1) % buf_len;
}

/// Dequeue the oldest sample from the event's ring buffer.
///
/// Returns `None` when the buffer is empty (head == tail).
pub fn perf_pop_sample(event_id: u32) -> Option<PerfSample> {
    if event_id == 0 {
        return None;
    }
    let slot = (event_id as usize).saturating_sub(1);
    if slot >= MAX_PERF_EVENTS {
        return None;
    }
    let mut table = PERF_EVENTS.lock();
    if !table[slot].is_used() {
        return None;
    }
    if table[slot].sample_tail == table[slot].sample_head {
        return None; // empty
    }
    let tail = table[slot].sample_tail as usize;
    let sample = if tail < PERF_SAMPLE_BUF {
        table[slot].sample_buf[tail]
    } else {
        return None;
    };
    let buf_len = PERF_SAMPLE_BUF as u32;
    // Guard: buf_len is a compile-time constant > 0; safe.
    table[slot].sample_tail = table[slot].sample_tail.wrapping_add(1) % buf_len;
    Some(sample)
}

/// Returns `true` if the accumulated count has reached or exceeded the
/// configured `sample_period` (and `sample_period` is non-zero).
pub fn perf_overflow_check(event_id: u32) -> bool {
    if event_id == 0 {
        return false;
    }
    let slot = (event_id as usize).saturating_sub(1);
    if slot >= MAX_PERF_EVENTS {
        return false;
    }
    let table = PERF_EVENTS.lock();
    if !table[slot].is_used() {
        return false;
    }
    let sp = table[slot].sample_period;
    // Guard: sample_period == 0 means no overflow checking
    if sp == 0 {
        return false;
    }
    table[slot].count >= sp
}

/// Increment the count on every enabled SOFTWARE event whose `config`
/// matches `sw_event_id`.
///
/// Call this from the relevant kernel path (e.g. page-fault handler for
/// `PERF_COUNT_SW_PAGE_FAULTS`, scheduler for `PERF_COUNT_SW_CONTEXT_SWITCHES`).
pub fn perf_sw_inc(sw_event_id: u64) {
    let mut table = PERF_EVENTS.lock();
    for i in 0..MAX_PERF_EVENTS {
        if table[i].is_used()
            && table[i].enabled
            && table[i].event_type == PERF_TYPE_SOFTWARE
            && table[i].config == sw_event_id
        {
            table[i].count = table[i].count.saturating_add(1);
        }
    }
}

/// Poll all enabled HARDWARE events, refresh their counts from the PMU, and
/// update the stored value.
///
/// Intended to be called from a periodic timer tick or the scheduler.
/// `current_ms` is the current uptime in milliseconds and is accepted for
/// future use (e.g. multiplexing) but not stored.
pub fn perf_tick(_current_ms: u64) {
    let mut table = PERF_EVENTS.lock();
    for i in 0..MAX_PERF_EVENTS {
        if table[i].is_used() && table[i].enabled && table[i].event_type == PERF_TYPE_HARDWARE {
            let val = read_hw_counter(table[i].config);
            table[i].count = val;
        }
    }
}

// ---------------------------------------------------------------------------
// Syscall stub — SYS_PERF_EVENT_OPEN = 298
// ---------------------------------------------------------------------------

/// Kernel implementation of `perf_event_open(2)` (SYS_PERF_EVENT_OPEN = 298).
///
/// `attr_addr` is treated as a pointer to two consecutive little-endian u32
/// words: `[event_type: u32, config_lo: u32]`.  This is a minimal subset of
/// the real `perf_event_attr` structure sufficient to identify the event kind
/// and its hardware/software selector.
///
/// If `attr_addr` is 0 or the pointer is clearly invalid (high-canonical
/// kernel address guard not enforced here — trust the syscall gate) a default
/// HARDWARE/CPU_CYCLES event is created.
///
/// Returns the synthetic fd `event_id + 100` on success, or `-1` on failure.
pub fn sys_perf_event_open(attr_addr: u64, pid: i32, cpu: i32, _group_fd: i32, flags: u64) -> i64 {
    // Parse event_type and config_lo from the attr struct.
    // Layout: bytes 0-3 = event_type (u32 LE), bytes 4-7 = config_lo (u32 LE).
    let (event_type, config): (u32, u64) = if attr_addr != 0 {
        // Safety: caller (syscall gate) has validated the pointer region.
        let event_type_raw = unsafe { core::ptr::read_volatile(attr_addr as *const u32) };
        let config_raw =
            unsafe { core::ptr::read_volatile(attr_addr.saturating_add(4) as *const u32) };
        (event_type_raw, config_raw as u64)
    } else {
        // Fallback: CPU_CYCLES hardware event
        (PERF_TYPE_HARDWARE, PERF_COUNT_HW_CPU_CYCLES)
    };

    // Map the flags u64 to our internal u32 flag word (low 32 bits).
    let perf_flags = (flags & 0xFFFF_FFFF) as u32;

    match perf_event_open(event_type, config, pid, cpu, perf_flags) {
        Some(id) => {
            // Synthetic fd = id + 100 to avoid collision with real fds 0-99
            (id as i64).saturating_add(100)
        }
        None => -1,
    }
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the perf subsystem.
///
/// Creates two default monitoring events:
///   1. HARDWARE / CPU_CYCLES  — measures TSC-based cycles
///   2. SOFTWARE / PAGE_FAULTS — counts page faults via `perf_sw_inc`
pub fn init() {
    // Zero the event table (static is already zero-initialised but explicit
    // zeroing makes the init sequence auditable).
    {
        let mut table = PERF_EVENTS.lock();
        for i in 0..MAX_PERF_EVENTS {
            table[i] = PerfEvent::empty();
        }
    }

    // Default event 1: hardware CPU cycles
    let _ = perf_event_open(
        PERF_TYPE_HARDWARE,
        PERF_COUNT_HW_CPU_CYCLES,
        -1, // all processes
        -1, // all CPUs
        0,  // enabled immediately
    );

    // Default event 2: software page faults
    let _ = perf_event_open(PERF_TYPE_SOFTWARE, PERF_COUNT_SW_PAGE_FAULTS, -1, -1, 0);

    serial_println!("[perf] perf_events profiling subsystem initialized");
}
