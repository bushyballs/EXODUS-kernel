/// PTP/IEEE 1588 hardware timestamping driver for Genesis
///
/// Implements Precision Time Protocol (IEEE 1588) hardware clock management.
/// Provides nanosecond-accurate clock synchronization with external timestamp
/// capture, frequency adjustment, and periodic tick advancement.
///
/// All rules strictly observed:
///   - No heap: no Vec, Box, String, alloc::*
///   - No panics: no unwrap(), expect(), panic!()
///   - No float casts: no as f64, as f32
///   - Saturating arithmetic for counters
///   - Wrapping arithmetic for sequence/ring indices
///   - Structs in static Mutex are Copy with const fn empty()
///   - Division always guarded (divisor != 0)
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of PTP hardware clocks supported
pub const MAX_PTP_CLOCKS: usize = 4;

/// Maximum number of external timestamp events in the ring buffer
pub const MAX_PTP_EVENTS: usize = 64;

/// Ring buffer size mask (MAX_PTP_EVENTS must be a power of two)
const PTP_EVENT_MASK: u32 = (MAX_PTP_EVENTS as u32).wrapping_sub(1);

/// PTP clock capability: pulse-per-second output
pub const PTP_CAP_PPS: u32 = 1 << 0;

/// PTP clock capability: external timestamp input
pub const PTP_CAP_EXT_TS: u32 = 1 << 1;

/// PTP clock capability: periodic output
pub const PTP_CAP_PER_OUT: u32 = 1 << 2;

/// PTP clock capability: programmable pins
pub const PTP_CAP_PIN: u32 = 1 << 3;

/// One billion nanoseconds in a second (used for nsec carry/borrow)
const NSEC_PER_SEC: u32 = 1_000_000_000;

/// One billion as u64 for frequency-adjustment arithmetic
const NSEC_PER_SEC_U64: u64 = 1_000_000_000;

/// One billion as i64 for signed frequency-adjustment arithmetic
const NSEC_PER_SEC_I64: i64 = 1_000_000_000;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Nanosecond-accurate PTP timestamp (no floats — integer seconds + nanoseconds).
#[derive(Copy, Clone)]
pub struct PtpTimestamp {
    /// Seconds component (signed to allow negative adjustments)
    pub sec: i64,
    /// Sub-second nanoseconds component, always in 0..999_999_999
    pub nsec: u32,
}

impl PtpTimestamp {
    /// Canonical zero timestamp.
    pub const fn zero() -> Self {
        Self { sec: 0, nsec: 0 }
    }
}

/// State for a single PTP hardware clock instance.
#[derive(Copy, Clone)]
pub struct PtpClock {
    /// Unique id assigned at registration
    pub id: u32,
    /// Clock name as a fixed-length byte array
    pub name: [u8; 16],
    /// Number of valid bytes in `name`
    pub name_len: u8,
    /// Capability flags (PTP_CAP_*)
    pub caps: u32,
    /// Current hardware clock time
    pub current_time: PtpTimestamp,
    /// Frequency adjustment in parts-per-billion (applied by ptp_tick)
    pub freq_adj_ppb: i32,
    /// Maximum allowed |freq_adj_ppb| (typically 500_000 ppb = ±500 ppm)
    pub max_adj: i32,
    /// True when this slot is occupied
    pub active: bool,
}

impl PtpClock {
    /// Return an empty (unoccupied) clock slot.
    const fn empty() -> Self {
        Self {
            id: 0,
            name: [0u8; 16],
            name_len: 0,
            caps: 0,
            current_time: PtpTimestamp::zero(),
            freq_adj_ppb: 0,
            max_adj: 0,
            active: false,
        }
    }
}

/// A single external timestamp event captured by hardware.
#[derive(Copy, Clone)]
pub struct PtpExtTsEvent {
    /// Id of the clock that captured this event
    pub clock_id: u32,
    /// External timestamp input channel number
    pub channel: u8,
    /// Captured hardware timestamp
    pub ts: PtpTimestamp,
    /// True when this ring slot contains a valid event
    pub valid: bool,
}

impl PtpExtTsEvent {
    /// Return an empty (invalid) event slot.
    const fn empty() -> Self {
        Self {
            clock_id: 0,
            channel: 0,
            ts: PtpTimestamp::zero(),
            valid: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static PTP_CLOCKS: Mutex<[PtpClock; MAX_PTP_CLOCKS]> =
    Mutex::new([PtpClock::empty(); MAX_PTP_CLOCKS]);

static PTP_EVENTS: Mutex<[PtpExtTsEvent; MAX_PTP_EVENTS]> =
    Mutex::new([PtpExtTsEvent::empty(); MAX_PTP_EVENTS]);

/// Ring-buffer write pointer (tail — next slot to write into)
static PTP_EVENT_TAIL: AtomicU32 = AtomicU32::new(0);

/// Ring-buffer read pointer (head — oldest unconsumed event)
static PTP_EVENT_HEAD: AtomicU32 = AtomicU32::new(0);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Find the array index of an active clock with the given `id`, or `None`.
fn find_clock_idx(clocks: &[PtpClock; MAX_PTP_CLOCKS], id: u32) -> Option<usize> {
    let mut i: usize = 0;
    while i < MAX_PTP_CLOCKS {
        if clocks[i].active && clocks[i].id == id {
            return Some(i);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Normalise a timestamp so that `nsec` is always in 0..999_999_999.
///
/// If `nsec >= NSEC_PER_SEC`, carries the excess into `sec`.
/// Does not handle the case where a single call could cause `nsec` to carry
/// more than once (callers ensure delta_ns is applied in chunks if needed).
#[inline]
fn normalise_timestamp(ts: &mut PtpTimestamp) {
    if ts.nsec >= NSEC_PER_SEC {
        let carry = (ts.nsec / NSEC_PER_SEC) as i64;
        ts.nsec %= NSEC_PER_SEC;
        ts.sec = ts.sec.saturating_add(carry);
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a new PTP hardware clock.
///
/// Copies up to 16 bytes from `name` into the clock's name buffer.
/// Returns the assigned clock id on success, or `None` if the table is full
/// or `max_adj` is zero or negative (which would make clamping nonsensical).
pub fn ptp_clock_register(name: &[u8], caps: u32, max_adj: i32) -> Option<u32> {
    if max_adj <= 0 {
        return None;
    }
    let mut clocks = PTP_CLOCKS.lock();
    let mut i: usize = 0;
    while i < MAX_PTP_CLOCKS {
        if !clocks[i].active {
            let id = i as u32;
            clocks[i] = PtpClock::empty();
            clocks[i].id = id;
            clocks[i].caps = caps;
            clocks[i].max_adj = max_adj;
            clocks[i].active = true;

            // Copy name bytes, capped at 16
            let copy_len = if name.len() < 16 { name.len() } else { 16 };
            let mut j: usize = 0;
            while j < copy_len {
                clocks[i].name[j] = name[j];
                j = j.saturating_add(1);
            }
            clocks[i].name_len = copy_len as u8;

            return Some(id);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Unregister a PTP clock by id.
///
/// Returns `true` on success, `false` if the clock id is not found.
pub fn ptp_clock_unregister(clock_id: u32) -> bool {
    let mut clocks = PTP_CLOCKS.lock();
    if let Some(idx) = find_clock_idx(&*clocks, clock_id) {
        clocks[idx] = PtpClock::empty();
        return true;
    }
    false
}

/// Read the current time of a PTP clock.
///
/// Returns `Some(PtpTimestamp)` on success, `None` if not found.
pub fn ptp_gettime(clock_id: u32) -> Option<PtpTimestamp> {
    let clocks = PTP_CLOCKS.lock();
    if let Some(idx) = find_clock_idx(&*clocks, clock_id) {
        return Some(clocks[idx].current_time);
    }
    None
}

/// Set the current time of a PTP clock.
///
/// Returns `true` on success, `false` if the clock id is not found.
pub fn ptp_settime(clock_id: u32, ts: PtpTimestamp) -> bool {
    let mut clocks = PTP_CLOCKS.lock();
    if let Some(idx) = find_clock_idx(&*clocks, clock_id) {
        clocks[idx].current_time = ts;
        // Ensure the stored time is normalised
        normalise_timestamp(&mut clocks[idx].current_time);
        return true;
    }
    false
}

/// Adjust the current time of a PTP clock by `delta_ns` nanoseconds.
///
/// Handles nanosecond overflow/underflow using integer arithmetic only.
/// No floats used.
///
/// Returns `true` on success, `false` if the clock id is not found.
pub fn ptp_adjtime(clock_id: u32, delta_ns: i64) -> bool {
    let mut clocks = PTP_CLOCKS.lock();
    if let Some(idx) = find_clock_idx(&*clocks, clock_id) {
        let ts = &mut clocks[idx].current_time;

        // Decompose delta_ns into whole seconds and remainder nanoseconds
        // so we can avoid any temporary > i64 arithmetic.
        let delta_sec: i64 = delta_ns / NSEC_PER_SEC_I64;
        let delta_nsec: i64 = delta_ns % NSEC_PER_SEC_I64;

        // Apply the whole-second part
        ts.sec = ts.sec.saturating_add(delta_sec);

        // Apply the nanosecond remainder.  The sum of ts.nsec (u32, always
        // < 10^9) and delta_nsec (i64, |value| < 10^9) fits in an i64.
        let new_nsec: i64 = ts.nsec as i64 + delta_nsec;

        if new_nsec < 0 {
            // Borrow one second
            ts.sec = ts.sec.saturating_sub(1);
            ts.nsec = (new_nsec + NSEC_PER_SEC_I64) as u32;
        } else if new_nsec >= NSEC_PER_SEC_I64 {
            // Carry one second
            ts.sec = ts.sec.saturating_add(1);
            ts.nsec = (new_nsec - NSEC_PER_SEC_I64) as u32;
        } else {
            ts.nsec = new_nsec as u32;
        }

        return true;
    }
    false
}

/// Set the frequency adjustment for a PTP clock.
///
/// `ppb` is clamped to ±`max_adj` before being stored.
/// Returns `true` on success, `false` if the clock id is not found.
pub fn ptp_adjfine(clock_id: u32, ppb: i32) -> bool {
    let mut clocks = PTP_CLOCKS.lock();
    if let Some(idx) = find_clock_idx(&*clocks, clock_id) {
        let max = clocks[idx].max_adj;
        let clamped = if ppb > max {
            max
        } else if ppb < -max {
            -max
        } else {
            ppb
        };
        clocks[idx].freq_adj_ppb = clamped;
        return true;
    }
    false
}

/// Enable or disable an external timestamp input channel.
///
/// Stub implementation — verifies the clock exists and always succeeds.
/// Returns `true` on success, `false` if the clock id is not found.
pub fn ptp_enable_extts(clock_id: u32, _channel: u8, _enable: bool) -> bool {
    let clocks = PTP_CLOCKS.lock();
    find_clock_idx(&*clocks, clock_id).is_some()
}

/// Push an external timestamp event into the ring buffer.
///
/// If the ring is full the oldest entry is silently overwritten (head advances).
pub fn ptp_push_extts(clock_id: u32, channel: u8, ts: PtpTimestamp) {
    let tail = PTP_EVENT_TAIL.load(Ordering::Relaxed);
    let head = PTP_EVENT_HEAD.load(Ordering::Relaxed);

    // Check whether the ring is full: (tail + 1) & MASK == head
    let next_tail = tail.wrapping_add(1) & PTP_EVENT_MASK;
    if next_tail == head {
        // Ring full — advance head to drop the oldest entry
        PTP_EVENT_HEAD.store(head.wrapping_add(1) & PTP_EVENT_MASK, Ordering::Relaxed);
    }

    let slot = (tail & PTP_EVENT_MASK) as usize;
    {
        let mut events = PTP_EVENTS.lock();
        events[slot] = PtpExtTsEvent {
            clock_id,
            channel,
            ts,
            valid: true,
        };
    }
    PTP_EVENT_TAIL.store(next_tail, Ordering::Release);
}

/// Pop the oldest external timestamp event from the ring buffer.
///
/// Returns `Some(PtpExtTsEvent)` if an event is available, `None` if empty.
pub fn ptp_pop_extts() -> Option<PtpExtTsEvent> {
    let head = PTP_EVENT_HEAD.load(Ordering::Acquire);
    let tail = PTP_EVENT_TAIL.load(Ordering::Relaxed);

    if head == tail {
        // Ring is empty
        return None;
    }

    let slot = (head & PTP_EVENT_MASK) as usize;
    let event = {
        let mut events = PTP_EVENTS.lock();
        let ev = events[slot];
        events[slot] = PtpExtTsEvent::empty();
        ev
    };

    PTP_EVENT_HEAD.store(head.wrapping_add(1) & PTP_EVENT_MASK, Ordering::Release);

    if event.valid {
        Some(event)
    } else {
        None
    }
}

/// Advance the hardware clock by `elapsed_ns` nanoseconds, applying the
/// current frequency adjustment.
///
/// The adjusted elapsed time is:
///   adj_ns = elapsed_ns + (elapsed_ns * freq_adj_ppb) / 1_000_000_000
///
/// Integer-only arithmetic; divisor guard ensures no division by zero
/// (the divisor is the constant 1_000_000_000 which is never zero, but we
/// guard it explicitly to satisfy the kernel rule).
pub fn ptp_tick(clock_id: u32, elapsed_ns: u64) {
    let mut clocks = PTP_CLOCKS.lock();
    let idx = match find_clock_idx(&*clocks, clock_id) {
        Some(i) => i,
        None => return,
    };

    // Calculate frequency-adjusted elapsed time using i64 arithmetic.
    // elapsed_ns fits in i64 for any reasonable tick interval (<= ~9.2 s).
    let elapsed_i64: i64 = elapsed_ns as i64;
    let ppb: i64 = clocks[idx].freq_adj_ppb as i64;

    // Guard: NSEC_PER_SEC_I64 is always 1_000_000_000 != 0, but we check
    // explicitly to satisfy the no-division-without-guard rule.
    let adj_ns: i64 = if NSEC_PER_SEC_I64 != 0 {
        elapsed_i64 + (elapsed_i64 * ppb) / NSEC_PER_SEC_I64
    } else {
        elapsed_i64
    };

    let ts = &mut clocks[idx].current_time;

    // Decompose adj_ns into seconds + nanoseconds
    let adj_sec: i64 = adj_ns / NSEC_PER_SEC_I64;
    let adj_nsec: i64 = adj_ns % NSEC_PER_SEC_I64;

    ts.sec = ts.sec.saturating_add(adj_sec);

    let new_nsec: i64 = ts.nsec as i64 + adj_nsec;
    if new_nsec < 0 {
        ts.sec = ts.sec.saturating_sub(1);
        ts.nsec = (new_nsec + NSEC_PER_SEC_I64) as u32;
    } else if new_nsec >= NSEC_PER_SEC_I64 {
        ts.sec = ts.sec.saturating_add(1);
        ts.nsec = (new_nsec - NSEC_PER_SEC_I64) as u32;
    } else {
        ts.nsec = new_nsec as u32;
    }
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the PTP driver.
///
/// Registers a simulated PTP hardware clock named "genesis-ptp0" with
/// PPS + external-timestamp capabilities and a ±500 000 ppb max adjustment.
pub fn init() {
    let name = b"genesis-ptp0";
    let caps = PTP_CAP_PPS | PTP_CAP_EXT_TS;
    let max_adj: i32 = 500_000;

    if ptp_clock_register(name, caps, max_adj).is_some() {
        serial_println!("[ptp] PTP hardware clock initialized");
    } else {
        serial_println!("[ptp] PTP hardware clock initialization failed (table full)");
    }
}
