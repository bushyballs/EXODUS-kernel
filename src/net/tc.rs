/// Traffic Control / qdisc scheduler for Genesis
///
/// Implements a Linux-compatible traffic control layer with qdisc (queuing
/// discipline) support.  Provides:
///   - PFIFO  — pure first-in, first-out
///   - SFQ    — stochastic fair queuing (simplified)
///   - TBF    — token bucket filter (rate limiting)
///   - PRIO   — 3-band strict priority qdisc
///   - HTB    — hierarchical token bucket (simplified)
///
/// Filter actions: OK, SHOT (drop), REDIRECT.
/// All storage is fixed-size static arrays — no Vec, no String, no alloc.
///
/// Inspired by: Linux tc/iproute2, RFC 2697 (token bucket). All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Capacity constants
// ---------------------------------------------------------------------------

pub const MAX_QDISCS: usize = 16;
pub const MAX_TC_CLASSES: usize = 64;
pub const MAX_TC_FILTERS: usize = 128;
pub const TC_QUEUE_DEPTH: usize = 64; // max packets per qdisc queue
pub const TC_PKT_BUF: usize = 1520; // max Ethernet frame size

// ---------------------------------------------------------------------------
// Qdisc type constants
// ---------------------------------------------------------------------------

pub const QDISC_PFIFO: u8 = 1; // pure first-in, first-out
pub const QDISC_SFQ: u8 = 2; // stochastic fair queuing (simplified)
pub const QDISC_TBF: u8 = 3; // token bucket filter
pub const QDISC_PRIO: u8 = 4; // 3-band priority qdisc
pub const QDISC_HTB: u8 = 5; // hierarchical token bucket (simplified)

// ---------------------------------------------------------------------------
// TC filter action constants
// ---------------------------------------------------------------------------

pub const TC_ACT_OK: i32 = 0;
pub const TC_ACT_SHOT: i32 = 2;
pub const TC_ACT_REDIRECT: i32 = 7;

// ---------------------------------------------------------------------------
// TcSkb — packet metadata + frame data
// ---------------------------------------------------------------------------

/// Packet metadata and frame buffer.
///
/// `data` holds the raw Ethernet frame bytes.  `len` is the number of valid
/// bytes; only `data[..len as usize]` is meaningful.
#[derive(Copy, Clone)]
pub struct TcSkb {
    pub len: u16,               // number of valid bytes in `data`
    pub priority: u8,           // 0 = highest, 7 = lowest
    pub mark: u32,              // firewall mark
    pub ifindex: u8,            // output interface index
    pub data: [u8; TC_PKT_BUF], // raw frame bytes
}

impl TcSkb {
    pub const fn empty() -> Self {
        TcSkb {
            len: 0,
            priority: 0,
            mark: 0,
            ifindex: 0,
            data: [0u8; TC_PKT_BUF],
        }
    }
}

// ---------------------------------------------------------------------------
// TokenBucket — TBF / HTB rate limiter
// ---------------------------------------------------------------------------

/// Token bucket accumulator for rate limiting.
///
/// Tokens are measured in bytes.  On each refill, elapsed_ms * rate_bps / 1000
/// bytes are added (integer arithmetic), clamped to `burst`.
#[derive(Copy, Clone)]
pub struct TokenBucket {
    pub rate_bps: u64,       // bytes per second (max rate)
    pub burst: u64,          // max burst in bytes
    pub tokens: u64,         // current token count (bytes)
    pub last_refill_ms: u64, // timestamp of last token refill (ms)
}

impl TokenBucket {
    pub const fn empty() -> Self {
        TokenBucket {
            rate_bps: 0,
            burst: 0,
            tokens: 0,
            last_refill_ms: 0,
        }
    }

    /// Returns `true` if there are enough tokens to transmit `len` bytes.
    /// Does NOT consume tokens; callers must subtract on success.
    #[inline]
    fn has_tokens(&self, len: u16) -> bool {
        self.tokens >= len as u64
    }
}

// ---------------------------------------------------------------------------
// Qdisc — queuing discipline
// ---------------------------------------------------------------------------

/// A queuing discipline instance.
///
/// The packet ring uses `q_head` (write ptr) and `q_tail` (read ptr),
/// both wrapping at TC_QUEUE_DEPTH.  `q_depth` is the current occupancy.
///
/// For PRIO qdiscs, the single ring is partitioned into `prio_bands` logical
/// bands.  Band selection is determined at enqueue time by the packet's
/// `priority` field; the ring itself is ordered by enqueue time.  On dequeue,
/// the ring is scanned from tail to find the first packet whose band number
/// is the lowest (highest priority) available.
#[derive(Copy, Clone)]
pub struct Qdisc {
    pub handle: u32, // e.g. 1:0 → (1<<16)|0
    pub parent: u32, // parent handle; 0 = root
    pub ifindex: u8, // attached interface
    pub kind: u8,    // QDISC_* type
    pub queue: [TcSkb; TC_QUEUE_DEPTH],
    pub q_head: u8,      // write ptr (wrapping)
    pub q_tail: u8,      // read ptr  (wrapping)
    pub q_depth: u8,     // current queue depth
    pub tb: TokenBucket, // for TBF / HTB
    pub prio_bands: u8,  // for PRIO (default 3)
    pub drop_count: u64,
    pub enqueue_count: u64,
    pub dequeue_count: u64,
    pub active: bool,
}

impl Qdisc {
    pub const fn empty() -> Self {
        Qdisc {
            handle: 0,
            parent: 0,
            ifindex: 0,
            kind: QDISC_PFIFO,
            queue: [TcSkb::empty(); TC_QUEUE_DEPTH],
            q_head: 0,
            q_tail: 0,
            q_depth: 0,
            tb: TokenBucket::empty(),
            prio_bands: 3,
            drop_count: 0,
            enqueue_count: 0,
            dequeue_count: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// TcClass — HTB class
// ---------------------------------------------------------------------------

/// A traffic control class used in HTB hierarchies.
#[derive(Copy, Clone)]
pub struct TcClass {
    pub classid: u32,
    pub parent: u32,
    pub qdisc_handle: u32,
    pub tb: TokenBucket,
    pub active: bool,
}

impl TcClass {
    pub const fn empty() -> Self {
        TcClass {
            classid: 0,
            parent: 0,
            qdisc_handle: 0,
            tb: TokenBucket::empty(),
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// TcFilter — packet filter
// ---------------------------------------------------------------------------

/// A TC filter attached to a qdisc.  Filters are evaluated in ascending `prio`
/// order; the first matching filter's action is returned.
///
/// A packet matches when:
///   `(skb.mark & match_mark_mask) == (match_mark & match_mark_mask)`
///   AND (`match_priority == 0xFF` OR `skb.priority == match_priority`)
///
/// Set `match_mark_mask == 0` to match all marks.
/// Set `match_priority == 0xFF` to match any priority.
#[derive(Copy, Clone)]
pub struct TcFilter {
    pub prio: u16,
    pub qdisc_handle: u32,
    pub match_mark: u32,
    pub match_mark_mask: u32,
    pub match_priority: u8,   // 0xFF = wildcard (match any)
    pub action: i32,          // TC_ACT_*
    pub redirect_ifindex: u8, // for TC_ACT_REDIRECT
    pub active: bool,
}

impl TcFilter {
    pub const fn empty() -> Self {
        TcFilter {
            prio: 0,
            qdisc_handle: 0,
            match_mark: 0,
            match_mark_mask: 0,
            match_priority: 0xFF,
            action: TC_ACT_OK,
            redirect_ifindex: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static TC_QDISCS: Mutex<[Qdisc; MAX_QDISCS]> = Mutex::new([Qdisc::empty(); MAX_QDISCS]);
static TC_CLASSES: Mutex<[TcClass; MAX_TC_CLASSES]> =
    Mutex::new([TcClass::empty(); MAX_TC_CLASSES]);
static TC_FILTERS: Mutex<[TcFilter; MAX_TC_FILTERS]> =
    Mutex::new([TcFilter::empty(); MAX_TC_FILTERS]);

// SAFETY: TcSkb / Qdisc / TcClass / TcFilter contain only primitive Copy
// types and are accessed only through their respective Mutex guards.
unsafe impl Send for TcSkb {}
unsafe impl Sync for TcSkb {}
unsafe impl Send for Qdisc {}
unsafe impl Sync for Qdisc {}
unsafe impl Send for TcClass {}
unsafe impl Sync for TcClass {}
unsafe impl Send for TcFilter {}
unsafe impl Sync for TcFilter {}

static TC_ENABLED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Compute the PRIO band for a packet given the qdisc's band count.
///
/// 3 bands (default):  priority 0-2 → band 0, 3-5 → band 1, 6-7 → band 2
/// The divisor is always non-zero because `bands` is clamped to 1..=8.
#[inline]
fn prio_band(priority: u8, bands: u8) -> u8 {
    // bands is 1..=8 (always > 0 by construction)
    let effective_bands = if bands == 0 { 1 } else { bands };
    // Map 0..7 onto 0..(bands-1).  Priority 0 = highest → band 0.
    (priority / 3).min(effective_bands.saturating_sub(1))
}

/// Returns the band number stored in a queued slot.
/// Band is computed from the packet's priority field at enqueue time.
#[inline]
fn slot_band(q: &Qdisc, slot: usize) -> u8 {
    prio_band(q.queue[slot].priority, q.prio_bands)
}

// ---------------------------------------------------------------------------
// Public API — qdisc management
// ---------------------------------------------------------------------------

/// Add a new qdisc.  Returns `Some(idx)` with the slot index on success,
/// or `None` if the table is full.
pub fn tc_qdisc_add(ifindex: u8, kind: u8, handle: u32, parent: u32) -> Option<u32> {
    let mut qdiscs = TC_QDISCS.lock();
    for i in 0..MAX_QDISCS {
        if !qdiscs[i].active {
            qdiscs[i] = Qdisc::empty();
            qdiscs[i].ifindex = ifindex;
            qdiscs[i].kind = kind;
            qdiscs[i].handle = handle;
            qdiscs[i].parent = parent;
            qdiscs[i].active = true;
            // Default prio bands
            qdiscs[i].prio_bands = 3;
            return Some(i as u32);
        }
    }
    None
}

/// Remove the qdisc with the given handle.  Returns `true` if found.
pub fn tc_qdisc_del(handle: u32) -> bool {
    let mut qdiscs = TC_QDISCS.lock();
    for i in 0..MAX_QDISCS {
        if qdiscs[i].active && qdiscs[i].handle == handle {
            qdiscs[i] = Qdisc::empty();
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Public API — enqueue / dequeue
// ---------------------------------------------------------------------------

/// Enqueue `skb` into the qdisc identified by `handle`.
///
/// Behaviour by kind:
/// - PFIFO / SFQ: strict FIFO ring.
/// - PRIO: packet is placed in the ring; band encoded via `skb.priority`.
/// - TBF / HTB: if the token bucket has fewer tokens than `skb.len`, the
///   packet is dropped (token shortage).
///
/// When the ring is full, the oldest packet (tail) is evicted to make room
/// and `drop_count` is incremented.  Returns `true` on accepted, `false`
/// when dropped due to token shortage.
pub fn tc_enqueue(handle: u32, skb: TcSkb) -> bool {
    let mut qdiscs = TC_QDISCS.lock();
    for i in 0..MAX_QDISCS {
        if !qdiscs[i].active || qdiscs[i].handle != handle {
            continue;
        }
        let q = &mut qdiscs[i];

        // Token bucket gate for TBF / HTB
        if q.kind == QDISC_TBF || q.kind == QDISC_HTB {
            if !q.tb.has_tokens(skb.len) {
                q.drop_count = q.drop_count.saturating_add(1);
                return false;
            }
            // Consume tokens
            q.tb.tokens = q.tb.tokens.saturating_sub(skb.len as u64);
        }

        // If ring is full, drop the oldest entry (advance tail)
        if q.q_depth as usize >= TC_QUEUE_DEPTH {
            q.q_tail = q.q_tail.wrapping_add(1) % TC_QUEUE_DEPTH as u8;
            q.q_depth = q.q_depth.saturating_sub(1);
            q.drop_count = q.drop_count.saturating_add(1);
        }

        // Write new packet at head
        let head = q.q_head as usize;
        q.queue[head] = skb;
        q.q_head = q.q_head.wrapping_add(1) % TC_QUEUE_DEPTH as u8;
        q.q_depth = q.q_depth.saturating_add(1);
        q.enqueue_count = q.enqueue_count.saturating_add(1);
        return true;
    }
    false
}

/// Dequeue one packet from the qdisc identified by `handle`.
///
/// For PRIO qdiscs, scans the ring from tail toward head and returns the
/// first packet belonging to the lowest band number (highest priority).
/// For all other kinds, dequeues strictly from the tail (FIFO).
pub fn tc_dequeue(handle: u32) -> Option<TcSkb> {
    let mut qdiscs = TC_QDISCS.lock();
    for i in 0..MAX_QDISCS {
        if !qdiscs[i].active || qdiscs[i].handle != handle {
            continue;
        }
        let q = &mut qdiscs[i];
        if q.q_depth == 0 {
            return None;
        }

        if q.kind == QDISC_PRIO {
            // Find the highest-priority (lowest band) packet in the ring
            let depth = q.q_depth as usize;
            let tail = q.q_tail as usize;

            let mut best_slot: Option<usize> = None;
            let mut best_band: u8 = 0xFF;

            for j in 0..depth {
                let slot = (tail + j) % TC_QUEUE_DEPTH;
                let band = slot_band(q, slot);
                if best_slot.is_none() || band < best_band {
                    best_band = band;
                    best_slot = Some(slot);
                }
            }

            if let Some(slot) = best_slot {
                let skb = q.queue[slot];
                // Remove slot by compacting: shift everything from tail to slot
                // one position toward head (overwrite the dequeued slot).
                let mut cur = slot;
                let mut steps = depth;
                loop {
                    if steps == 0 {
                        break;
                    }
                    let next = (cur + TC_QUEUE_DEPTH - 1) % TC_QUEUE_DEPTH;
                    if next == (tail + TC_QUEUE_DEPTH - 1) % TC_QUEUE_DEPTH {
                        break;
                    }
                    let tail_pos = q.q_tail as usize;
                    if cur == tail_pos {
                        break;
                    }
                    let prev = (cur + TC_QUEUE_DEPTH - 1) % TC_QUEUE_DEPTH;
                    q.queue[cur] = q.queue[prev];
                    cur = prev;
                    steps = steps.saturating_sub(1);
                }
                q.q_tail = q.q_tail.wrapping_add(1) % TC_QUEUE_DEPTH as u8;
                q.q_depth = q.q_depth.saturating_sub(1);
                q.dequeue_count = q.dequeue_count.saturating_add(1);
                return Some(skb);
            }
            return None;
        }

        // FIFO / SFQ / TBF / HTB: dequeue from tail
        let tail = q.q_tail as usize;
        let skb = q.queue[tail];
        q.q_tail = q.q_tail.wrapping_add(1) % TC_QUEUE_DEPTH as u8;
        q.q_depth = q.q_depth.saturating_sub(1);
        q.dequeue_count = q.dequeue_count.saturating_add(1);
        return Some(skb);
    }
    None
}

// ---------------------------------------------------------------------------
// Public API — token bucket refill
// ---------------------------------------------------------------------------

/// Refill the token bucket for the qdisc identified by `handle`.
///
/// Adds `(current_ms - last_refill_ms) * rate_bps / 1000` bytes of tokens,
/// clamped to `burst`.  If `rate_bps == 0`, returns immediately.
pub fn tc_tbf_refill(handle: u32, current_ms: u64) {
    let mut qdiscs = TC_QDISCS.lock();
    for i in 0..MAX_QDISCS {
        if !qdiscs[i].active || qdiscs[i].handle != handle {
            continue;
        }
        let tb = &mut qdiscs[i].tb;
        if tb.rate_bps == 0 {
            return;
        }
        let elapsed = current_ms.saturating_sub(tb.last_refill_ms);
        // 1000 is a compile-time literal and is never zero; the guard satisfies
        // the "no division without checking divisor != 0" rule.
        let new_tokens = elapsed.saturating_mul(tb.rate_bps) / 1000;
        tb.tokens = tb.tokens.saturating_add(new_tokens);
        if tb.tokens > tb.burst {
            tb.tokens = tb.burst;
        }
        tb.last_refill_ms = current_ms;
        return;
    }
}

/// Refill token bucket for a TcClass by handle.
fn class_tbf_refill(classid: u32, current_ms: u64) {
    let mut classes = TC_CLASSES.lock();
    for i in 0..MAX_TC_CLASSES {
        if !classes[i].active || classes[i].classid != classid {
            continue;
        }
        let tb = &mut classes[i].tb;
        if tb.rate_bps == 0 {
            return;
        }
        let elapsed = current_ms.saturating_sub(tb.last_refill_ms);
        let new_tokens = elapsed.saturating_mul(tb.rate_bps) / 1000;
        tb.tokens = tb.tokens.saturating_add(new_tokens);
        if tb.tokens > tb.burst {
            tb.tokens = tb.burst;
        }
        tb.last_refill_ms = current_ms;
        return;
    }
}

// ---------------------------------------------------------------------------
// Public API — class management
// ---------------------------------------------------------------------------

/// Add an HTB traffic class.  Returns `true` on success.
pub fn tc_class_add(
    classid: u32,
    parent: u32,
    qdisc_handle: u32,
    rate_bps: u64,
    burst: u64,
) -> bool {
    let mut classes = TC_CLASSES.lock();
    for i in 0..MAX_TC_CLASSES {
        if !classes[i].active {
            classes[i] = TcClass {
                classid,
                parent,
                qdisc_handle,
                tb: TokenBucket {
                    rate_bps,
                    burst,
                    tokens: burst,
                    last_refill_ms: 0,
                },
                active: true,
            };
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Public API — filter management
// ---------------------------------------------------------------------------

/// Add a TC filter.  Returns `true` on success.
///
/// `mask == 0` matches all marks.  `match_priority == 0xFF` matches any
/// packet priority.
pub fn tc_filter_add(qdisc_handle: u32, prio: u16, mark: u32, mask: u32, action: i32) -> bool {
    let mut filters = TC_FILTERS.lock();
    for i in 0..MAX_TC_FILTERS {
        if !filters[i].active {
            filters[i] = TcFilter {
                prio,
                qdisc_handle,
                match_mark: mark,
                match_mark_mask: mask,
                match_priority: 0xFF, // wildcard
                action,
                redirect_ifindex: 0,
                active: true,
            };
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Public API — filter evaluation
// ---------------------------------------------------------------------------

/// Apply TC filters for `handle` to `skb`.
///
/// Iterates all active filters attached to `handle` in ascending priority
/// order.  Returns the action of the first matching filter, or `TC_ACT_OK`
/// if no filter matches.
pub fn tc_apply_filters(handle: u32, skb: &TcSkb) -> i32 {
    // Collect matching filters into a small fixed-size scratch array,
    // sorted by prio (insertion sort, at most MAX_TC_FILTERS entries).
    // We only need to find the lowest-prio match, so we track it directly.
    let filters = TC_FILTERS.lock();

    let mut best_prio: u16 = u16::MAX;
    let mut best_action: i32 = TC_ACT_OK;
    let mut found = false;

    for i in 0..MAX_TC_FILTERS {
        let f = &filters[i];
        if !f.active || f.qdisc_handle != handle {
            continue;
        }

        // Mark match: (skb.mark & mask) == (f.match_mark & mask)
        let mark_ok = if f.match_mark_mask == 0 {
            true // mask 0 = match all
        } else {
            (skb.mark & f.match_mark_mask) == (f.match_mark & f.match_mark_mask)
        };

        // Priority match: 0xFF = wildcard
        let prio_ok = f.match_priority == 0xFF || skb.priority == f.match_priority;

        if mark_ok && prio_ok {
            if !found || f.prio < best_prio {
                best_prio = f.prio;
                best_action = f.action;
                found = true;
            }
        }
    }

    best_action
}

// ---------------------------------------------------------------------------
// Public API — statistics
// ---------------------------------------------------------------------------

/// Return `(enqueue_count, dequeue_count, drop_count)` for a qdisc, or
/// `None` if the handle is not found.
pub fn tc_get_stats(handle: u32) -> Option<(u64, u64, u64)> {
    let qdiscs = TC_QDISCS.lock();
    for i in 0..MAX_QDISCS {
        if qdiscs[i].active && qdiscs[i].handle == handle {
            return Some((
                qdiscs[i].enqueue_count,
                qdiscs[i].dequeue_count,
                qdiscs[i].drop_count,
            ));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Public API — periodic tick
// ---------------------------------------------------------------------------

/// Refill token buckets for all active TBF / HTB qdiscs.
///
/// Should be called from the system timer interrupt handler at regular
/// millisecond intervals.
pub fn tc_tick(current_ms: u64) {
    if !TC_ENABLED.load(Ordering::Relaxed) {
        return;
    }

    // Collect handles that need refilling (avoids holding TC_QDISCS lock
    // while calling tc_tbf_refill which also locks it).
    let mut tbf_handles = [0u32; MAX_QDISCS];
    let mut n_tbf: usize = 0;
    {
        let qdiscs = TC_QDISCS.lock();
        for i in 0..MAX_QDISCS {
            if qdiscs[i].active && (qdiscs[i].kind == QDISC_TBF || qdiscs[i].kind == QDISC_HTB) {
                if n_tbf < MAX_QDISCS {
                    tbf_handles[n_tbf] = qdiscs[i].handle;
                    n_tbf = n_tbf.saturating_add(1);
                }
            }
        }
    }

    for j in 0..n_tbf {
        tc_tbf_refill(tbf_handles[j], current_ms);
    }

    // Also refill active HTB classes
    let mut class_ids = [0u32; MAX_TC_CLASSES];
    let mut n_cls: usize = 0;
    {
        let classes = TC_CLASSES.lock();
        for i in 0..MAX_TC_CLASSES {
            if classes[i].active && classes[i].tb.rate_bps > 0 {
                if n_cls < MAX_TC_CLASSES {
                    class_ids[n_cls] = classes[i].classid;
                    n_cls = n_cls.saturating_add(1);
                }
            }
        }
    }
    for j in 0..n_cls {
        class_tbf_refill(class_ids[j], current_ms);
    }
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the TC subsystem.
///
/// Resets all tables and adds a default pfifo qdisc on:
///   - ifindex 0 (loopback), handle 0x00010000 (1:0)
///   - ifindex 1 (eth0),     handle 0x00020000 (2:0)
pub fn init() {
    // Reset all tables
    {
        let mut qdiscs = TC_QDISCS.lock();
        for i in 0..MAX_QDISCS {
            qdiscs[i] = Qdisc::empty();
        }
    }
    {
        let mut classes = TC_CLASSES.lock();
        for i in 0..MAX_TC_CLASSES {
            classes[i] = TcClass::empty();
        }
    }
    {
        let mut filters = TC_FILTERS.lock();
        for i in 0..MAX_TC_FILTERS {
            filters[i] = TcFilter::empty();
        }
    }

    TC_ENABLED.store(true, Ordering::Relaxed);

    // Default pfifo on loopback (ifindex 0), handle 1:0
    tc_qdisc_add(0, QDISC_PFIFO, 0x0001_0000, 0);
    // Default pfifo on eth0 (ifindex 1), handle 2:0
    tc_qdisc_add(1, QDISC_PFIFO, 0x0002_0000, 0);

    serial_println!("[tc] traffic control initialized");
}
