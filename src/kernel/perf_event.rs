use crate::kernel::pmu;
/// Software perf_event interface for Genesis
///
/// Provides a file-descriptor-based interface to hardware and software
/// performance counters, mirroring the Linux perf_event_open(2) API
/// (syscall 298) but adapted for bare-metal, no-heap operation.
///
/// ## Hardware events
///
/// Hardware events are routed to one of the four general-purpose PMU
/// counters via `kernel::pmu`.  The mapping is:
///
///   HardwareCycles        → PMU counter 0 (EVENT_CPU_CYCLES)
///   HardwareInstructions  → PMU counter 1 (EVENT_INSTRUCTIONS_RETIRED)
///   HardwareCacheRef      → PMU counter 2 (EVENT_LLC_REFS) — shares with CacheMiss
///   HardwareCacheMiss     → PMU counter 2 (EVENT_LLC_MISSES)
///   HardwareBranchInstr   → PMU counter 3 (EVENT_BRANCH_INSTRUCTIONS)
///   HardwareBranchMiss    → PMU counter 3 (EVENT_BRANCH_MISSES)
///
/// Because there are only four GP counters and more possible events, callers
/// should be aware that re-programming a counter clobbers any previous event
/// on that same slot.
///
/// ## Software events
///
/// Software events are tracked via `AtomicU64` counters incremented by the
/// kernel at the relevant callsite:
///   `sw_inc_page_fault()`   — called from the page-fault handler
///   `sw_inc_ctx_switch()`   — called from the scheduler context-switch path
///
/// ## File descriptors
///
/// Each open event receives a synthetic file descriptor:
///   fd = PERF_FD_BASE + slot_index   (i.e. 4000 … 4063)
///
/// `perf_event_is_fd(fd)` returns true for any fd in that range that
/// corresponds to an active event.
///
/// ## No heap, no panics, no float casts
///
/// All storage is a fixed-size `[PerfEvent; 64]` array guarded by a Mutex.
/// Counter operations use saturating arithmetic or wrapping_add where
/// appropriate.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Software counter globals
// ---------------------------------------------------------------------------

/// Total page faults observed since boot (incremented by the fault handler).
pub static SW_PAGE_FAULTS: AtomicU64 = AtomicU64::new(0);

/// Total context switches since boot (incremented by the scheduler).
pub static SW_CTX_SWITCHES: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Event type
// ---------------------------------------------------------------------------

/// Classification of a performance event, mirroring Linux PERF_TYPE_HARDWARE
/// and PERF_TYPE_SOFTWARE event IDs.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum PerfEventType {
    // Hardware events — backed by PMU general-purpose counters
    HardwareCycles,
    HardwareInstructions,
    HardwareCacheRef,
    HardwareCacheMiss,
    HardwareBranchInstr,
    HardwareBranchMiss,
    // Software events — tracked via atomic globals
    SoftwarePageFaults,
    SoftwareContextSwitches,
    SoftwareCpuMigrations,
    SoftwareTaskClock,
}

// ---------------------------------------------------------------------------
// PerfEvent
// ---------------------------------------------------------------------------

/// State for a single open perf event.
#[derive(Copy, Clone)]
pub struct PerfEvent {
    /// Synthetic file descriptor (PERF_FD_BASE + slot index), or 0 when free.
    pub id: u32,
    /// PID that opened the event (informational; 0 = kernel).
    pub pid: u32,
    /// Event classification.
    pub event_type: PerfEventType,
    /// Which PMU general-purpose counter slot this event is pinned to (0-3).
    /// Unused for software events.
    pub counter_idx: u32,
    /// Last sampled counter value.
    pub value: u64,
    /// Whether the event is currently collecting.
    pub enabled: bool,
    /// Number of counter overflow interrupts received.
    pub overflow_count: u64,
    /// Sampling period (0 = no sampling / counting mode).
    pub period: u64,
    /// Counter value at which the next overflow interrupt should fire.
    pub next_overflow: u64,
}

impl PerfEvent {
    /// Return an empty (free) slot.
    pub const fn empty() -> Self {
        PerfEvent {
            id: 0,
            pid: 0,
            event_type: PerfEventType::HardwareCycles,
            counter_idx: 0,
            value: 0,
            enabled: false,
            overflow_count: 0,
            period: 0,
            next_overflow: 0,
        }
    }

    /// Returns `true` if this slot is currently in use.
    #[inline]
    fn is_used(&self) -> bool {
        self.id != 0
    }
}

// ---------------------------------------------------------------------------
// Event table
// ---------------------------------------------------------------------------

/// Maximum simultaneously open perf events.
const MAX_EVENTS: usize = 64;

/// Synthetic fd base for perf events.
pub const PERF_FD_BASE: i32 = 4000;

/// Global table of open perf events.
static PERF_EVENTS: Mutex<[PerfEvent; MAX_EVENTS]> = Mutex::new([PerfEvent::empty(); MAX_EVENTS]);

// ---------------------------------------------------------------------------
// Hardware counter slot assignment
// ---------------------------------------------------------------------------

/// Map a hardware PerfEventType to a PMU counter index (0–3) and configure
/// the counter with the matching event selector.
///
/// Returns the counter index, or `u32::MAX` if the type is a software event
/// or cannot be mapped.
fn hw_counter_for(event_type: PerfEventType) -> u32 {
    match event_type {
        PerfEventType::HardwareCycles => {
            pmu::pmu_reset_counter(0);
            pmu::pmu_enable_counter(0, pmu::EVENT_CPU_CYCLES);
            0
        }
        PerfEventType::HardwareInstructions => {
            pmu::pmu_reset_counter(1);
            pmu::pmu_enable_counter(1, pmu::EVENT_INSTRUCTIONS_RETIRED);
            1
        }
        PerfEventType::HardwareCacheRef => {
            pmu::pmu_reset_counter(2);
            pmu::pmu_enable_counter(2, pmu::EVENT_LLC_REFS);
            2
        }
        PerfEventType::HardwareCacheMiss => {
            pmu::pmu_reset_counter(2);
            pmu::pmu_enable_counter(2, pmu::EVENT_LLC_MISSES);
            2
        }
        PerfEventType::HardwareBranchInstr => {
            pmu::pmu_reset_counter(3);
            pmu::pmu_enable_counter(3, pmu::EVENT_BRANCH_INSTRUCTIONS);
            3
        }
        PerfEventType::HardwareBranchMiss => {
            pmu::pmu_reset_counter(3);
            pmu::pmu_enable_counter(3, pmu::EVENT_BRANCH_MISSES);
            3
        }
        // Software events do not use a PMU counter slot.
        _ => u32::MAX,
    }
}

/// Returns `true` for event types that are tracked by hardware PMU counters.
#[inline]
fn is_hw_event(event_type: PerfEventType) -> bool {
    matches!(
        event_type,
        PerfEventType::HardwareCycles
            | PerfEventType::HardwareInstructions
            | PerfEventType::HardwareCacheRef
            | PerfEventType::HardwareCacheMiss
            | PerfEventType::HardwareBranchInstr
            | PerfEventType::HardwareBranchMiss
    )
}

// ---------------------------------------------------------------------------
// Software counter read
// ---------------------------------------------------------------------------

/// Read the current value for a software event type.
fn sw_read(event_type: PerfEventType) -> u64 {
    match event_type {
        PerfEventType::SoftwarePageFaults => SW_PAGE_FAULTS.load(Ordering::Relaxed),
        PerfEventType::SoftwareContextSwitches => SW_CTX_SWITCHES.load(Ordering::Relaxed),
        // SoftwareCpuMigrations and SoftwareTaskClock: not yet implemented;
        // return 0 so callers at least don't panic.
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// fd ↔ slot helpers
// ---------------------------------------------------------------------------

/// Convert a file descriptor to a slot index.  Returns `MAX_EVENTS` on error.
#[inline]
fn fd_to_slot(fd: i32) -> usize {
    if fd < PERF_FD_BASE {
        return MAX_EVENTS;
    }
    let idx = (fd - PERF_FD_BASE) as usize;
    if idx >= MAX_EVENTS {
        MAX_EVENTS
    } else {
        idx
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Open a new perf event.
///
/// Finds a free slot in `PERF_EVENTS`, configures the underlying PMU counter
/// (for hardware events), and returns a synthetic file descriptor.
///
/// Returns `-1` if:
///   - all 64 slots are in use, or
///   - `pmu_detect()` returned 0 and the event requires hardware counters.
pub fn perf_event_open(pid: u32, event_type: PerfEventType, period: u64) -> i32 {
    let mut table = PERF_EVENTS.lock();
    // Find first free slot.
    let mut slot = MAX_EVENTS;
    for i in 0..MAX_EVENTS {
        if !table[i].is_used() {
            slot = i;
            break;
        }
    }
    if slot == MAX_EVENTS {
        return -1; // table full
    }

    // For hardware events we need a working PMU.
    let counter_idx = if is_hw_event(event_type) {
        let idx = hw_counter_for(event_type);
        if idx == u32::MAX {
            return -1;
        }
        idx
    } else {
        u32::MAX
    };

    let fd = PERF_FD_BASE + slot as i32;
    table[slot] = PerfEvent {
        id: fd as u32,
        pid,
        event_type,
        counter_idx,
        value: 0,
        enabled: true,
        overflow_count: 0,
        period,
        next_overflow: if period > 0 { period } else { 0 },
    };
    fd
}

/// Read the current counter value for the event identified by `fd`.
///
/// Updates `event.value` in the table and returns the new value.
/// Returns `0` if `fd` is not a valid perf event fd.
pub fn perf_event_read(fd: i32) -> u64 {
    let slot = fd_to_slot(fd);
    if slot == MAX_EVENTS {
        return 0;
    }
    let mut table = PERF_EVENTS.lock();
    if !table[slot].is_used() {
        return 0;
    }
    let val = if is_hw_event(table[slot].event_type) {
        pmu::pmu_read_counter(table[slot].counter_idx)
    } else {
        sw_read(table[slot].event_type)
    };
    table[slot].value = val;
    val
}

/// Reset the underlying counter to 0 for the event identified by `fd`.
///
/// No-op if `fd` is invalid.
pub fn perf_event_reset(fd: i32) {
    let slot = fd_to_slot(fd);
    if slot == MAX_EVENTS {
        return;
    }
    let mut table = PERF_EVENTS.lock();
    if !table[slot].is_used() {
        return;
    }
    if is_hw_event(table[slot].event_type) {
        pmu::pmu_reset_counter(table[slot].counter_idx);
    }
    table[slot].value = 0;
}

/// Enable the event identified by `fd`.
///
/// Re-enables the PMU counter (for hardware events) and marks the event as
/// enabled.  Returns `true` on success, `false` if `fd` is invalid.
pub fn perf_event_enable(fd: i32) -> bool {
    let slot = fd_to_slot(fd);
    if slot == MAX_EVENTS {
        return false;
    }
    let mut table = PERF_EVENTS.lock();
    if !table[slot].is_used() {
        return false;
    }
    if is_hw_event(table[slot].event_type) {
        hw_counter_for(table[slot].event_type);
    }
    table[slot].enabled = true;
    true
}

/// Disable the event identified by `fd` without closing it.
///
/// Disables the underlying PMU counter (for hardware events) and marks the
/// event as disabled.  Returns `true` on success, `false` if `fd` is invalid.
pub fn perf_event_disable(fd: i32) -> bool {
    let slot = fd_to_slot(fd);
    if slot == MAX_EVENTS {
        return false;
    }
    let mut table = PERF_EVENTS.lock();
    if !table[slot].is_used() {
        return false;
    }
    if is_hw_event(table[slot].event_type) {
        pmu::pmu_disable_counter(table[slot].counter_idx);
    }
    table[slot].enabled = false;
    true
}

/// Close the event identified by `fd`.
///
/// Disables the underlying PMU counter (for hardware events) and frees the
/// slot.  No-op if `fd` is invalid.
pub fn perf_event_close(fd: i32) {
    let slot = fd_to_slot(fd);
    if slot == MAX_EVENTS {
        return;
    }
    let mut table = PERF_EVENTS.lock();
    if !table[slot].is_used() {
        return;
    }
    if is_hw_event(table[slot].event_type) {
        pmu::pmu_disable_counter(table[slot].counter_idx);
    }
    table[slot] = PerfEvent::empty();
}

/// Called from the PMI (NMI) interrupt handler when a hardware counter
/// overflows.  Increments `overflow_count` for the event bound to `fd`.
///
/// No-op if `fd` is invalid.
pub fn perf_event_overflow(fd: i32) {
    let slot = fd_to_slot(fd);
    if slot == MAX_EVENTS {
        return;
    }
    let mut table = PERF_EVENTS.lock();
    if !table[slot].is_used() {
        return;
    }
    table[slot].overflow_count = table[slot].overflow_count.saturating_add(1);
    // If sampling, reload the counter to the next period boundary.
    if table[slot].period > 0 {
        let p = table[slot].period;
        if is_hw_event(table[slot].event_type) {
            let cidx = table[slot].counter_idx;
            // Write the two's-complement negative of the period so the counter
            // overflows again after `period` events.  Avoid subtraction
            // underflow by computing with wrapping semantics on u64.
            let reload = 0u64.wrapping_sub(p);
            unsafe {
                core::arch::asm!(
                    "wrmsr",
                    in("ecx") pmu::IA32_PMC0 + cidx,
                    in("eax") (reload & 0xFFFF_FFFF) as u32,
                    in("edx") (reload >> 32) as u32,
                    options(nomem, nostack)
                );
            }
        }
        table[slot].next_overflow = table[slot].next_overflow.wrapping_add(p);
    }
}

/// Returns `true` if `fd` is a valid, in-use perf event file descriptor.
pub fn perf_event_is_fd(fd: i32) -> bool {
    let slot = fd_to_slot(fd);
    if slot == MAX_EVENTS {
        return false;
    }
    let table = PERF_EVENTS.lock();
    table[slot].is_used()
}

// ---------------------------------------------------------------------------
// Software counter increment entry points
// ---------------------------------------------------------------------------

/// Increment the global software page-fault counter.
///
/// Call this from the page-fault handler (interrupt/exception path).
#[inline]
pub fn sw_inc_page_fault() {
    SW_PAGE_FAULTS.fetch_add(1, Ordering::Relaxed);
}

/// Increment the global software context-switch counter.
///
/// Call this from the scheduler at every context switch.
#[inline]
pub fn sw_inc_ctx_switch() {
    SW_CTX_SWITCHES.fetch_add(1, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the perf_event subsystem.
///
/// Calls `pmu::init()` to detect and configure hardware counters, then
/// zeroes the event table.  Safe to call multiple times (pmu::init() is
/// idempotent once PMU_VERSION > 0).
pub fn init() {
    // Ensure the PMU is ready before we accept any perf_event_open calls.
    pmu::init();

    // Zero the event table (it is already zero-initialised as a static, but
    // calling this explicitly makes the initialisation sequence visible and
    // allows future re-init paths).
    {
        let mut table = PERF_EVENTS.lock();
        for i in 0..MAX_EVENTS {
            table[i] = PerfEvent::empty();
        }
    }

    serial_println!(
        "  [perf_event] subsystem ready ({} event slots)",
        MAX_EVENTS
    );
}
