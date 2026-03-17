/// MPLS label switching for Genesis — RFC 3031/3032
///
/// Implements a fixed-size Label Forwarding Information Base (LFIB) with
/// Push / Pop / Swap / Drop actions.  No heap, no floats, no panics.
///
/// MPLS label stack entry wire format (32 bits, big-endian):
///   Bits 31:12  — Label  (20 bits)
///   Bits 11:9   — TC/EXP (traffic class, 3 bits)
///   Bit  8      — S      (bottom-of-stack flag)
///   Bits 7:0    — TTL    (8 bits)
///
/// All rules strictly observed:
///   - No heap: no Vec, Box, String, alloc::*
///   - No panics: no unwrap(), expect(), panic!()
///   - No float casts: no as f64, as f32
///   - Saturating arithmetic for counters
///   - Wrapping arithmetic for sequence numbers
///   - Structs in static Mutex are Copy with const fn empty()
///   - Division always guarded (divisor != 0)
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// EtherType for unicast MPLS
pub const MPLS_ETH_TYPE: u16 = 0x8847;

/// EtherType for multicast MPLS
pub const MPLS_ETH_TYPE_MC: u16 = 0x8848;

/// Special label 3 — Implicit NULL (signals PHP: pop at penultimate hop)
pub const MPLS_LABEL_IMPLICIT_NULL: u32 = 3;

/// Special label 1 — Router Alert
pub const MPLS_LABEL_ROUTERALERT: u32 = 1;

/// Maximum valid label value (20-bit label space)
pub const MPLS_LABEL_MAX: u32 = 0xFFFFF;

/// Maximum label stack depth handled per packet
pub const MAX_MPLS_LABELS: usize = 16;

/// Maximum number of entries in the forwarding table (LFIB)
pub const MAX_MPLS_ROUTES: usize = 256;

/// Size of one MPLS label stack entry in bytes
const MPLS_ENTRY_BYTES: usize = 4;

// ---------------------------------------------------------------------------
// Label encode / decode (inline, no heap, no floats)
// ---------------------------------------------------------------------------

/// Encode label, traffic-class, bottom-of-stack flag, and TTL into a
/// 32-bit MPLS label stack entry.
#[inline]
pub fn mpls_label_encode(label: u32, tc: u8, bos: bool, ttl: u8) -> u32 {
    ((label & MPLS_LABEL_MAX) << 12)
        | (((tc & 0x7) as u32) << 9)
        | (if bos { 1 << 8 } else { 0 })
        | (ttl as u32)
}

/// Decode a 32-bit MPLS label stack entry into (label, tc, bos, ttl).
#[inline]
pub fn mpls_label_decode(entry: u32) -> (u32, u8, bool, u8) {
    let label = (entry >> 12) & MPLS_LABEL_MAX;
    let tc = ((entry >> 9) & 0x7) as u8;
    let bos = ((entry >> 8) & 1) == 1;
    let ttl = (entry & 0xFF) as u8;
    (label, tc, bos, ttl)
}

// ---------------------------------------------------------------------------
// Forwarding action
// ---------------------------------------------------------------------------

/// Action to perform on the MPLS label stack when forwarding a packet.
#[derive(Copy, Clone, PartialEq)]
pub enum MplsAction {
    /// Push a new label onto the stack (label stacking / encapsulation)
    Push,
    /// Pop the top label (Penultimate Hop Popping or label-switched path egress)
    Pop,
    /// Replace the top label with `out_label` (label swapping)
    Swap,
    /// Silently discard the packet
    Drop,
}

// ---------------------------------------------------------------------------
// Forwarding table entry
// ---------------------------------------------------------------------------

/// A single LFIB entry: maps an incoming label to an action + next-hop.
#[derive(Copy, Clone)]
pub struct MplsRoute {
    /// Incoming label to match
    pub in_label: u32,
    /// Outgoing label used for Swap and Push actions
    pub out_label: u32,
    /// Forwarding action
    pub action: MplsAction,
    /// IPv4 next-hop address (network byte order)
    pub next_hop: [u8; 4],
    /// Outgoing interface index
    pub ifindex: u8,
    /// Number of packets that matched this entry (saturating)
    pub hit_count: u64,
    /// True when this table slot is occupied
    pub active: bool,
}

impl MplsRoute {
    /// Return an empty (unoccupied) route slot.
    const fn empty() -> Self {
        Self {
            in_label: 0,
            out_label: 0,
            action: MplsAction::Drop,
            next_hop: [0u8; 4],
            ifindex: 0,
            hit_count: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static MPLS_ROUTES: Mutex<[MplsRoute; MAX_MPLS_ROUTES]> =
    Mutex::new([MplsRoute::empty(); MAX_MPLS_ROUTES]);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Find the array index of an active route matching `in_label`, or `None`.
fn find_route_idx(routes: &[MplsRoute; MAX_MPLS_ROUTES], in_label: u32) -> Option<usize> {
    let mut i: usize = 0;
    while i < MAX_MPLS_ROUTES {
        if routes[i].active && routes[i].in_label == in_label {
            return Some(i);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Read a big-endian u32 from `buf` starting at `offset`.
/// Returns 0 if there are not enough bytes.
#[inline]
fn read_u32_be(buf: &[u8; 1514], offset: usize) -> u32 {
    if offset.saturating_add(3) >= 1514 {
        return 0;
    }
    ((buf[offset] as u32) << 24)
        | ((buf[offset + 1] as u32) << 16)
        | ((buf[offset + 2] as u32) << 8)
        | (buf[offset + 3] as u32)
}

/// Write a big-endian u32 into `buf` starting at `offset`.
/// No-op if there are not enough bytes.
#[inline]
fn write_u32_be(buf: &mut [u8; 1514], offset: usize, val: u32) {
    if offset.saturating_add(3) >= 1514 {
        return;
    }
    buf[offset] = ((val >> 24) & 0xFF) as u8;
    buf[offset + 1] = ((val >> 16) & 0xFF) as u8;
    buf[offset + 2] = ((val >> 8) & 0xFF) as u8;
    buf[offset + 3] = (val & 0xFF) as u8;
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Add (or replace) an MPLS forwarding table entry.
///
/// If an entry for `in_label` already exists it is replaced.
/// Returns `true` on success, `false` if the table is full.
pub fn mpls_route_add(
    in_label: u32,
    action: MplsAction,
    out_label: u32,
    next_hop: [u8; 4],
    ifindex: u8,
) -> bool {
    let mut routes = MPLS_ROUTES.lock();

    // Replace existing entry for the same in_label if one exists.
    if let Some(idx) = find_route_idx(&*routes, in_label) {
        routes[idx].out_label = out_label;
        routes[idx].action = action;
        routes[idx].next_hop = next_hop;
        routes[idx].ifindex = ifindex;
        return true;
    }

    // Find a free slot.
    let mut i: usize = 0;
    while i < MAX_MPLS_ROUTES {
        if !routes[i].active {
            routes[i] = MplsRoute {
                in_label,
                out_label,
                action,
                next_hop,
                ifindex,
                hit_count: 0,
                active: true,
            };
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Remove an MPLS forwarding entry by incoming label.
///
/// Returns `true` on success, `false` if no matching entry exists.
pub fn mpls_route_del(in_label: u32) -> bool {
    let mut routes = MPLS_ROUTES.lock();
    if let Some(idx) = find_route_idx(&*routes, in_label) {
        routes[idx] = MplsRoute::empty();
        return true;
    }
    false
}

/// Look up an MPLS route by incoming label.
///
/// Returns a copy of the matching `MplsRoute` on success, `None` if not found.
pub fn mpls_lookup(in_label: u32) -> Option<MplsRoute> {
    let routes = MPLS_ROUTES.lock();
    if let Some(idx) = find_route_idx(&*routes, in_label) {
        return Some(routes[idx]);
    }
    None
}

/// Process an incoming MPLS packet.
///
/// `packet` starts at the MPLS label stack (Ethernet header already stripped).
/// `len` is the total number of valid bytes in `packet`.
///
/// Parses the label stack entries (4 bytes each) until the bottom-of-stack
/// bit is found, then looks up the top label and applies the forwarding action
/// (Pop or Swap) to modify the label stack in-place.  Push and Drop actions
/// are noted but the packet buffer is not further modified here (the caller
/// is responsible for forwarding).
///
/// Returns `(bytes_consumed, new_len)`:
///   - `bytes_consumed`: bytes removed from the front of `packet` (for Pop)
///   - `new_len`: new valid length of `packet` after modification
pub fn mpls_input(packet: &mut [u8; 1514], len: usize) -> (usize, usize) {
    if len < MPLS_ENTRY_BYTES {
        return (0, len);
    }

    // Walk the label stack to find the top label (first entry) and count
    // total stack bytes.
    let mut stack_depth: usize = 0;
    let mut top_entry: u32 = 0;
    let mut offset: usize = 0;

    while offset.saturating_add(MPLS_ENTRY_BYTES) <= len && stack_depth < MAX_MPLS_LABELS {
        let entry = read_u32_be(packet, offset);
        if stack_depth == 0 {
            top_entry = entry;
        }
        offset = offset.saturating_add(MPLS_ENTRY_BYTES);
        stack_depth = stack_depth.saturating_add(1);

        let (_label, _tc, bos, _ttl) = mpls_label_decode(entry);
        if bos {
            break;
        }
    }

    let (top_label, top_tc, top_bos, top_ttl) = mpls_label_decode(top_entry);
    let _ = top_bos; // used only for stack walk logic above

    // Look up the top label in the LFIB and record a hit.
    let route = {
        let mut routes = MPLS_ROUTES.lock();
        match find_route_idx(&*routes, top_label) {
            Some(idx) => {
                routes[idx].hit_count = routes[idx].hit_count.saturating_add(1);
                routes[idx]
            }
            None => return (0, len),
        }
    };

    match route.action {
        MplsAction::Drop => {
            // Signal caller to drop: return zero new_len
            (0, 0)
        }

        MplsAction::Pop => {
            // Remove the top 4-byte label stack entry by shifting left.
            let payload_start: usize = MPLS_ENTRY_BYTES;
            let payload_len: usize = if len > payload_start {
                len - payload_start
            } else {
                0
            };
            // Shift remaining bytes to front (no alloc — in-place copy).
            let mut i: usize = 0;
            while i < payload_len && payload_start.saturating_add(i) < 1514 {
                packet[i] = packet[payload_start + i];
                i = i.saturating_add(1);
            }
            (MPLS_ENTRY_BYTES, payload_len)
        }

        MplsAction::Swap => {
            // Replace the top label stack entry in-place.
            // Preserve tc and ttl from the incoming entry; use out_label.
            let new_entry = mpls_label_encode(
                route.out_label,
                top_tc,
                stack_depth == 1, // preserve BOS: true when only one label
                top_ttl.saturating_sub(1),
            );
            write_u32_be(packet, 0, new_entry);
            (0, len)
        }

        MplsAction::Push => {
            // Caller should use mpls_push_label for encapsulation.
            // Nothing to consume from the front.
            (0, len)
        }
    }
}

/// Prepend a single MPLS label stack entry to a packet.
///
/// Copies `pkt_len` bytes from `packet` into `out` shifted right by 4 bytes,
/// then writes the new label stack entry at the front.
///
/// Returns the new total length, or 0 if the result would exceed 1514 bytes.
pub fn mpls_push_label(
    packet: &[u8],
    pkt_len: usize,
    label: u32,
    tc: u8,
    ttl: u8,
    out: &mut [u8; 1514],
) -> usize {
    let new_len = pkt_len.saturating_add(MPLS_ENTRY_BYTES);
    if new_len > 1514 {
        return 0;
    }

    // Write the new label stack entry at offset 0.
    // BOS is set only when the inner packet has no MPLS label stack; callers
    // are responsible for setting BOS correctly when stacking multiple labels.
    let entry = mpls_label_encode(label, tc, true, ttl);
    out[0] = ((entry >> 24) & 0xFF) as u8;
    out[1] = ((entry >> 16) & 0xFF) as u8;
    out[2] = ((entry >> 8) & 0xFF) as u8;
    out[3] = (entry & 0xFF) as u8;

    // Copy the original packet payload after the new label.
    let copy_len = if pkt_len < packet.len() {
        pkt_len
    } else {
        packet.len()
    };
    let mut i: usize = 0;
    while i < copy_len {
        let dst = MPLS_ENTRY_BYTES.saturating_add(i);
        if dst >= 1514 {
            break;
        }
        out[dst] = packet[i];
        i = i.saturating_add(1);
    }

    new_len
}

/// Return aggregate forwarding statistics.
///
/// Returns `(total_routes, total_hits)` where `total_routes` is the number
/// of active LFIB entries and `total_hits` is the sum of all hit counters
/// (both saturating).
pub fn mpls_get_stats() -> (u64, u64) {
    let routes = MPLS_ROUTES.lock();
    let mut total_routes: u64 = 0;
    let mut total_hits: u64 = 0;
    let mut i: usize = 0;
    while i < MAX_MPLS_ROUTES {
        if routes[i].active {
            total_routes = total_routes.saturating_add(1);
            total_hits = total_hits.saturating_add(routes[i].hit_count);
        }
        i = i.saturating_add(1);
    }
    (total_routes, total_hits)
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the MPLS label switching subsystem.
///
/// Adds a default route: label 100 → Swap to label 200, next-hop 10.0.0.1,
/// interface index 1.
pub fn init() {
    mpls_route_add(100, MplsAction::Swap, 200, [10, 0, 0, 1], 1);
    serial_println!("[mpls] MPLS label switching initialized");
}
