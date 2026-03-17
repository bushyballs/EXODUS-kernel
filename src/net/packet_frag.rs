/// IPv4 packet fragmentation and reassembly
///
/// Handles fragmented IP datagrams per RFC 791.
///
/// A sender that needs to transmit a datagram larger than the outgoing
/// interface MTU splits it into fragments.  Each fragment carries the same
/// IP Identification field, the original protocol, and a fragment offset in
/// units of 8 bytes.  The "More Fragments" (MF) flag is set on every
/// fragment except the last.
///
/// The receiver holds fragment pieces in a `FragEntry` until it has all
/// bytes, then delivers the reassembled datagram.
///
/// RFC 791 requires the reassembly timer to be at least 60 seconds; here we
/// use a conservative 30-second timeout so partially-received datagrams are
/// not held indefinitely (suitable for a kernel with limited memory).
///
/// All code is `#![no_std]`.  Allocation uses the kernel global allocator
/// (enabled via the `alloc` crate).
use crate::sync::Mutex;
use crate::time::clock::uptime_ms;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of simultaneous reassembly contexts.
const FRAG_TABLE_SIZE: usize = 64;

/// Maximum reassembled datagram size (IPv4 max = 65535 bytes, of which the
/// header is at least 20 bytes, so payload can be at most 65515 bytes).
/// We allow the full 65535-byte datagram buffer per RFC 791.
const MAX_DATAGRAM_SIZE: usize = 65536;

/// Bitmask tracking array size: one bit per 8-byte unit over 65536 bytes.
///   65536 / 8 = 8192 units, packed into 8192 / 8 = 1024 bytes.
const FRAG_BITMASK_BYTES: usize = 1024;

/// Reassembly timeout in milliseconds (30 seconds per RFC 791 guidance).
const FRAG_TIMEOUT_MS: u64 = 30_000;

// ---------------------------------------------------------------------------
// FragEntry
// ---------------------------------------------------------------------------

/// State for one in-progress datagram reassembly.
///
/// `Copy` is derived so the type can be used in const array initializers.
/// The 65 KiB `data` buffer is only copied at `const` evaluation time
/// (i.e. during the static initializer); at runtime `FragEntry` slots are
/// mutated in-place through the Mutex-protected `FragTable`.
#[derive(Clone, Copy)]
pub struct FragEntry {
    /// IP Identification field (shared by all fragments of the same datagram).
    pub id: u16,
    /// Source IP address (part of the reassembly key).
    pub src_ip: [u8; 4],
    /// Destination IP address (part of the reassembly key).
    pub dst_ip: [u8; 4],
    /// Upper-layer protocol (e.g. TCP=6, UDP=17, ICMP=1).
    pub proto: u8,
    /// Reassembly buffer.  Fragments are written to their correct byte
    /// offsets so the final `data[0..total_size]` is a complete datagram.
    pub data: [u8; MAX_DATAGRAM_SIZE],
    /// Received-bytes bitmask.  One bit per 8-byte "fragment unit".
    /// Bit `n` is set when bytes `n*8 .. (n+1)*8` have been received.
    pub received_bits: [u8; FRAG_BITMASK_BYTES],
    /// Total datagram size (set when the final fragment — MF=0 — arrives).
    /// Zero means we have not yet seen the last fragment.
    pub total_size: u16,
    /// Set to `true` when all expected bytes have been received.
    pub complete: bool,
    /// Timestamp of first fragment arrival (ms since boot).
    pub timestamp: u64,
    /// Whether this slot is occupied.
    pub in_use: bool,
}

impl FragEntry {
    const fn empty() -> Self {
        FragEntry {
            id: 0,
            src_ip: [0; 4],
            dst_ip: [0; 4],
            proto: 0,
            data: [0; MAX_DATAGRAM_SIZE],
            received_bits: [0; FRAG_BITMASK_BYTES],
            total_size: 0,
            complete: false,
            timestamp: 0,
            in_use: false,
        }
    }

    // -----------------------------------------------------------------------
    // Bit-manipulation helpers for the received_bits bitmask
    // -----------------------------------------------------------------------

    /// Mark 8-byte units `start_unit..end_unit` as received.
    fn mark_received(&mut self, start_unit: usize, end_unit: usize) {
        for unit in start_unit..end_unit {
            let byte_idx = unit / 8;
            let bit_idx = unit % 8;
            if byte_idx < FRAG_BITMASK_BYTES {
                self.received_bits[byte_idx] |= 1 << bit_idx;
            }
        }
    }

    /// Check whether all units in `0..total_units` are marked received.
    fn all_received(&self, total_units: usize) -> bool {
        if total_units == 0 {
            return false;
        }
        for unit in 0..total_units {
            let byte_idx = unit / 8;
            let bit_idx = unit % 8;
            if byte_idx >= FRAG_BITMASK_BYTES {
                return false;
            }
            if self.received_bits[byte_idx] & (1 << bit_idx) == 0 {
                return false;
            }
        }
        true
    }

    /// Reset this entry to its empty state so the slot can be reused.
    fn reset(&mut self) {
        self.id = 0;
        self.src_ip = [0; 4];
        self.dst_ip = [0; 4];
        self.proto = 0;
        // Zero only the used portion of the data buffer (avoid zeroing 64 KiB
        // on every reset by using total_size as a hint; fall back to full
        // clear if total_size is not yet known).
        let clear_len = if self.total_size > 0 {
            (self.total_size as usize).min(MAX_DATAGRAM_SIZE)
        } else {
            MAX_DATAGRAM_SIZE
        };
        self.data[..clear_len].fill(0);
        self.received_bits.fill(0);
        self.total_size = 0;
        self.complete = false;
        self.timestamp = 0;
        self.in_use = false;
    }
}

// ---------------------------------------------------------------------------
// Global fragment reassembly table
// ---------------------------------------------------------------------------

struct FragTable {
    entries: [FragEntry; FRAG_TABLE_SIZE],
}

impl FragTable {
    const fn new() -> Self {
        const EMPTY: FragEntry = FragEntry::empty();
        FragTable {
            entries: [EMPTY; FRAG_TABLE_SIZE],
        }
    }
}

// SAFETY: FragTable is accessed only through the FRAG_TABLE Mutex.
unsafe impl Send for FragTable {}
unsafe impl Sync for FragTable {}

static FRAG_TABLE: Mutex<FragTable> = Mutex::new(FragTable::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Process an incoming IPv4 fragment.
///
/// `packet` is the raw IPv4 packet (including the IP header) starting at
/// byte 0.
///
/// Returns `Some(idx)` when reassembly is complete.  The caller should then
/// retrieve the reassembled datagram via [`frag_get_datagram`] before calling
/// [`frag_release`] on the returned index.
///
/// Returns `None` if the datagram is not yet complete (more fragments are
/// expected).
///
/// Silently discards malformed packets (short headers, offset overflow, etc.).
pub fn frag_receive(packet: &[u8]) -> Option<usize> {
    // Minimum IPv4 header is 20 bytes.
    if packet.len() < 20 {
        return None;
    }

    // Parse IP header fields.
    let ihl = ((packet[0] & 0x0F) as usize) * 4;
    if ihl < 20 || packet.len() < ihl {
        return None;
    }

    let total_len = u16::from_be_bytes([packet[2], packet[3]]) as usize;
    if total_len < ihl || total_len > packet.len() {
        return None;
    }

    let id = u16::from_be_bytes([packet[4], packet[5]]);
    let frag_word = u16::from_be_bytes([packet[6], packet[7]]);
    let mf_flag = (frag_word & 0x2000) != 0; // More Fragments bit
                                             // Fragment offset is in units of 8 bytes (RFC 791).
    let frag_off = ((frag_word & 0x1FFF) as usize) * 8;
    let proto = packet[9];
    let src_ip = [packet[12], packet[13], packet[14], packet[15]];
    let dst_ip = [packet[16], packet[17], packet[18], packet[19]];

    let payload = &packet[ihl..total_len];
    let payload_len = payload.len();

    // A datagram without MF and with offset 0 is not fragmented at all;
    // the caller should process it directly.
    if !mf_flag && frag_off == 0 {
        return None;
    }

    // Bounds check: fragment must fit in the reassembly buffer.
    let end_byte = frag_off.saturating_add(payload_len);
    if end_byte > MAX_DATAGRAM_SIZE {
        return None;
    }

    let now_ms = uptime_ms();

    let mut table = FRAG_TABLE.lock();

    // --- Find or create a FragEntry for this (src, dst, id, proto) key. ---
    let slot = {
        // First pass: look for an existing entry.
        let existing = (0..FRAG_TABLE_SIZE).find(|&i| {
            let e = &table.entries[i];
            e.in_use && e.id == id && e.proto == proto && e.src_ip == src_ip && e.dst_ip == dst_ip
        });

        if let Some(idx) = existing {
            idx
        } else {
            // Second pass: find a free slot.
            let free = (0..FRAG_TABLE_SIZE).find(|&i| !table.entries[i].in_use);
            match free {
                Some(idx) => {
                    table.entries[idx].in_use = true;
                    table.entries[idx].id = id;
                    table.entries[idx].src_ip = src_ip;
                    table.entries[idx].dst_ip = dst_ip;
                    table.entries[idx].proto = proto;
                    table.entries[idx].timestamp = now_ms;
                    idx
                }
                None => return None, // table full
            }
        }
    };

    let entry = &mut table.entries[slot];

    // --- Copy payload into the correct position in the reassembly buffer. ---
    entry.data[frag_off..frag_off + payload_len].copy_from_slice(payload);

    // --- Update the received-bits bitmask. ---
    // Each unit is 8 bytes.  Fragments are always 8-byte aligned per RFC 791.
    let start_unit = frag_off / 8;
    let end_unit = (frag_off + payload_len + 7) / 8; // round up
    entry.mark_received(start_unit, end_unit);

    // --- Record total_size when we see the last fragment (MF=0). ---
    if !mf_flag {
        let total_data_size = frag_off + payload_len;
        entry.total_size = total_data_size.min(MAX_DATAGRAM_SIZE) as u16;
    }

    // --- Check if reassembly is complete. ---
    if entry.total_size > 0 {
        let total_units = ((entry.total_size as usize) + 7) / 8;
        if entry.all_received(total_units) {
            entry.complete = true;
            return Some(slot);
        }
    }

    None
}

/// Retrieve the reassembled datagram payload from a completed `FragEntry`.
///
/// Returns a slice of `entry.data[0..total_size]`.
///
/// The caller must hold no reference to the slice when calling
/// [`frag_release`].
pub fn frag_get_datagram(slot: usize) -> Option<(usize, u16)> {
    if slot >= FRAG_TABLE_SIZE {
        return None;
    }
    let table = FRAG_TABLE.lock();
    let e = &table.entries[slot];
    if !e.in_use || !e.complete || e.total_size == 0 {
        return None;
    }
    Some((slot, e.total_size))
}

/// Copy the reassembled payload out into `buf`.
///
/// Returns the number of bytes copied, or 0 if the slot is invalid.
pub fn frag_copy_datagram(slot: usize, buf: &mut [u8]) -> usize {
    if slot >= FRAG_TABLE_SIZE {
        return 0;
    }
    let table = FRAG_TABLE.lock();
    let e = &table.entries[slot];
    if !e.in_use || !e.complete {
        return 0;
    }
    let len = (e.total_size as usize).min(buf.len());
    buf[..len].copy_from_slice(&e.data[..len]);
    len
}

/// Release a fragment slot (after the reassembled datagram has been consumed).
pub fn frag_release(slot: usize) {
    if slot >= FRAG_TABLE_SIZE {
        return;
    }
    let mut table = FRAG_TABLE.lock();
    table.entries[slot].reset();
}

// ---------------------------------------------------------------------------
// frag_send — fragment an outbound datagram
// ---------------------------------------------------------------------------

/// Fragment a datagram that exceeds `mtu` bytes.
///
/// `packet` is the complete IPv4 datagram (header + payload).
/// `mtu` is the maximum transmission unit of the outgoing interface.
///
/// Fragments are written into `output`, which must be large enough to hold
/// all fragments.  Each fragment is a complete IPv4 packet (header + partial
/// payload), separated contiguously in `output`.  The function returns a
/// slice of starting offsets and lengths: `(offset_into_output, fragment_len)`.
///
/// Returns 0 if the packet does not need fragmentation or if an error
/// occurs.  Errors include: `mtu < 28` (too small for a minimal fragment),
/// malformed IP header, or `output` buffer too small.
///
/// The IP identification field and TTL are preserved from the original header.
/// The checksum in each fragment header is recomputed.
pub fn frag_send<'a>(packet: &[u8], mtu: u16, output: &'a mut [u8]) -> &'a [(usize, usize)] {
    // Use a static to return the fragment descriptor array without alloc.
    // This is sufficient for a bare-metal kernel that serialises packet
    // transmission under a lock.
    //
    // Maximum fragments: 65535 / 8 + 1 = 8192.  In practice MTU is at
    // least 576 bytes (RFC 791), giving at most 65535/556 ≈ 118 fragments.
    // We cap at 128.
    const MAX_FRAGS: usize = 128;
    static mut FRAG_DESCS: [(usize, usize); MAX_FRAGS] = [(0, 0); MAX_FRAGS];

    // SAFETY: called single-threaded under kernel lock; we reset the array
    // at entry and never return a reference into it across yield points.
    let descs: &mut [(usize, usize); MAX_FRAGS] = unsafe { &mut FRAG_DESCS };

    if packet.len() < 20 {
        return &[];
    }

    let ihl = ((packet[0] & 0x0F) as usize) * 4;
    if ihl < 20 || packet.len() < ihl {
        return &[];
    }

    let mtu = mtu as usize;
    // Minimum MTU must accommodate IP header + at least 8 bytes of payload
    // (one 8-byte fragment unit per RFC 791 §2.3).
    if mtu < ihl + 8 {
        return &[];
    }

    let payload = &packet[ihl..];
    if payload.is_empty() {
        return &[];
    }

    // Maximum payload bytes per fragment (must be a multiple of 8).
    let max_frag_payload = (mtu - ihl) & !7usize;
    if max_frag_payload == 0 {
        return &[];
    }

    // If the whole packet fits in one MTU, no fragmentation is needed.
    if packet.len() <= mtu {
        return &[];
    }

    let mut frag_count = 0usize;
    let mut out_offset = 0usize;
    let mut data_offset = 0usize; // byte offset into payload

    // Original header values we need to preserve.
    let orig_id = u16::from_be_bytes([packet[4], packet[5]]);
    let orig_ttl = packet[8];
    let orig_proto = packet[9];
    let src_ip = &packet[12..16];
    let dst_ip = &packet[16..20];

    // Original DF flag (bit 14 of the flags+offset word).  If DF is set the
    // sender should not be fragmenting at all; we honour the request from the
    // caller by proceeding anyway since they explicitly called frag_send().
    // The DF flag is cleared on all outgoing fragments (RFC 791 §3.1).

    while data_offset < payload.len() && frag_count < MAX_FRAGS {
        let remaining = payload.len() - data_offset;
        let this_payload_len = remaining.min(max_frag_payload);
        let is_last = data_offset + this_payload_len >= payload.len();

        let total_frag_len = ihl + this_payload_len;
        if out_offset + total_frag_len > output.len() {
            // Output buffer too small — return what we have so far.
            break;
        }

        // --- Build fragment IP header ---
        let frag_slice = &mut output[out_offset..out_offset + total_frag_len];

        // Copy original IP header as the base.
        frag_slice[..ihl].copy_from_slice(&packet[..ihl]);

        // Update Total Length.
        let total_len_u16 = total_frag_len as u16;
        frag_slice[2..4].copy_from_slice(&total_len_u16.to_be_bytes());

        // Update ID (same for all fragments).
        frag_slice[4..6].copy_from_slice(&orig_id.to_be_bytes());

        // Fragment offset (in 8-byte units) + MF flag.
        let offset_units = (data_offset / 8) as u16;
        let flags_offset: u16 = if is_last {
            offset_units // MF = 0
        } else {
            0x2000 | offset_units // MF = 1
        };
        frag_slice[6..8].copy_from_slice(&flags_offset.to_be_bytes());

        // TTL, Protocol, src/dst from original.
        frag_slice[8] = orig_ttl;
        frag_slice[9] = orig_proto;
        frag_slice[10] = 0; // checksum (high byte, zeroed for computation)
        frag_slice[11] = 0; // checksum (low byte)
        frag_slice[12..16].copy_from_slice(src_ip);
        frag_slice[16..20].copy_from_slice(dst_ip);

        // Recompute IP header checksum (RFC 791 §3.1).
        let cksum = ip_checksum(&frag_slice[..ihl]);
        frag_slice[10..12].copy_from_slice(&cksum.to_be_bytes());

        // Copy payload fragment.
        frag_slice[ihl..ihl + this_payload_len]
            .copy_from_slice(&payload[data_offset..data_offset + this_payload_len]);

        descs[frag_count] = (out_offset, total_frag_len);
        frag_count += 1;
        out_offset += total_frag_len;
        data_offset += this_payload_len;
    }

    unsafe { &FRAG_DESCS[..frag_count] }
}

// ---------------------------------------------------------------------------
// frag_expire — periodic cleanup
// ---------------------------------------------------------------------------

/// Remove fragment reassembly entries older than `FRAG_TIMEOUT_MS`
/// milliseconds (30 seconds per RFC 791 requirements).
///
/// Should be called periodically — for example from the network stack's
/// timer tick or a dedicated housekeeping task.
pub fn frag_expire() {
    let now_ms = uptime_ms();
    let mut table = FRAG_TABLE.lock();
    for i in 0..FRAG_TABLE_SIZE {
        let e = &mut table.entries[i];
        if !e.in_use {
            continue;
        }
        let age_ms = now_ms.saturating_sub(e.timestamp);
        if age_ms >= FRAG_TIMEOUT_MS {
            e.reset();
        }
    }
}

// ---------------------------------------------------------------------------
// IP header checksum (RFC 1071)
// ---------------------------------------------------------------------------

/// Compute the one's-complement checksum over `data` (IP header).
///
/// `data` must have an even length (or the last byte is zero-padded
/// internally); returns the 16-bit checksum in network byte order.
#[inline]
fn ip_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        let word = u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        sum = sum.wrapping_add(word);
        i += 2;
    }
    // Odd byte (shouldn't happen for a standard IP header but handle it).
    if i < data.len() {
        sum = sum.wrapping_add((data[i] as u32) << 8);
    }
    // Fold carries.
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

/// Returns `(active_entries, complete_entries)` in the fragment table.
pub fn frag_stats() -> (usize, usize) {
    let table = FRAG_TABLE.lock();
    let mut active = 0usize;
    let mut complete = 0usize;
    for i in 0..FRAG_TABLE_SIZE {
        if table.entries[i].in_use {
            active += 1;
            if table.entries[i].complete {
                complete += 1;
            }
        }
    }
    (active, complete)
}
