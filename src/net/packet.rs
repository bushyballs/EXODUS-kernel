use crate::sync::Mutex;
/// Packet buffer management for Genesis networking
///
/// Provides efficient packet buffers with headroom/tailroom for encapsulation,
/// zero-copy slicing, clone, fragment and reassemble operations.
///
/// Inspired by: Linux sk_buff, FreeBSD mbuf. All code is original.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default headroom reserved for L2/L3/L4 headers
const DEFAULT_HEADROOM: usize = 128;

/// Default tailroom for trailers
const DEFAULT_TAILROOM: usize = 32;

/// Maximum single packet size (jumbo frame)
const MAX_PACKET_SIZE: usize = 9216;

/// Standard MTU
const DEFAULT_MTU: usize = 1500;

/// IP fragment offset granularity (8 bytes)
const FRAGMENT_GRANULARITY: usize = 8;

/// Maximum number of fragments for reassembly
const MAX_FRAGMENTS: usize = 64;

/// Reassembly timeout in "ticks" (arbitrary unit for poll-based check)
const REASSEMBLY_TIMEOUT: u64 = 3000;

// ---------------------------------------------------------------------------
// Packet metadata
// ---------------------------------------------------------------------------

/// Protocol layer that has been parsed up to
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketLayer {
    Raw,
    Link,
    Network,
    Transport,
    Application,
}

/// Packet direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Inbound,
    Outbound,
    Forwarded,
}

/// Packet priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

/// Metadata attached to every packet buffer
#[derive(Debug, Clone)]
pub struct PacketMeta {
    /// Interface index this packet arrived on (or will depart from)
    pub iface_index: u32,
    /// Parsed protocol layer
    pub layer: PacketLayer,
    /// Direction of travel
    pub direction: Direction,
    /// Priority
    pub priority: Priority,
    /// VLAN tag (if any)
    pub vlan_id: Option<u16>,
    /// IP protocol number (if parsed)
    pub ip_protocol: Option<u8>,
    /// Source port (transport layer)
    pub src_port: Option<u16>,
    /// Destination port (transport layer)
    pub dst_port: Option<u16>,
    /// Timestamp (monotonic tick count)
    pub timestamp: u64,
    /// Mark value (for netfilter/QoS)
    pub mark: u32,
    /// Checksum status
    pub checksum_valid: bool,
}

impl PacketMeta {
    pub fn new() -> Self {
        PacketMeta {
            iface_index: 0,
            layer: PacketLayer::Raw,
            direction: Direction::Inbound,
            priority: Priority::Normal,
            vlan_id: None,
            ip_protocol: None,
            src_port: None,
            dst_port: None,
            timestamp: 0,
            mark: 0,
            checksum_valid: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Packet buffer
// ---------------------------------------------------------------------------

/// A network packet buffer with headroom and tailroom
///
/// Layout: [headroom | data | tailroom]
///
/// `data_start` and `data_end` index into the backing buffer to identify
/// the actual payload region. Headroom allows prepending headers without
/// copying; tailroom allows appending trailers.
#[derive(Clone)]
pub struct PacketBuf {
    /// Backing storage
    buf: Vec<u8>,
    /// Start of actual data within buf
    data_start: usize,
    /// End of actual data within buf (exclusive)
    data_end: usize,
    /// Metadata
    pub meta: PacketMeta,
}

impl PacketBuf {
    /// Allocate a new packet buffer with specified headroom, data capacity, and tailroom
    pub fn alloc(headroom: usize, capacity: usize, tailroom: usize) -> Self {
        let total = headroom + capacity + tailroom;
        let buf = alloc::vec![0u8; total];
        PacketBuf {
            buf,
            data_start: headroom,
            data_end: headroom,
            meta: PacketMeta::new(),
        }
    }

    /// Create a new packet buffer with default headroom/tailroom, copying in data
    pub fn from_slice(data: &[u8]) -> Self {
        let total = DEFAULT_HEADROOM + data.len() + DEFAULT_TAILROOM;
        let mut buf = alloc::vec![0u8; total];
        buf[DEFAULT_HEADROOM..DEFAULT_HEADROOM + data.len()].copy_from_slice(data);
        PacketBuf {
            buf,
            data_start: DEFAULT_HEADROOM,
            data_end: DEFAULT_HEADROOM + data.len(),
            meta: PacketMeta::new(),
        }
    }

    /// Create a packet buffer wrapping an existing Vec (no headroom)
    pub fn from_vec(v: Vec<u8>) -> Self {
        let len = v.len();
        PacketBuf {
            buf: v,
            data_start: 0,
            data_end: len,
            meta: PacketMeta::new(),
        }
    }

    /// Get the data region as a byte slice
    pub fn data(&self) -> &[u8] {
        &self.buf[self.data_start..self.data_end]
    }

    /// Get the data region as a mutable byte slice
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.buf[self.data_start..self.data_end]
    }

    /// Length of the data region
    pub fn len(&self) -> usize {
        self.data_end - self.data_start
    }

    /// Whether the data region is empty
    pub fn is_empty(&self) -> bool {
        self.data_start == self.data_end
    }

    /// Available headroom (bytes before data start)
    pub fn headroom(&self) -> usize {
        self.data_start
    }

    /// Available tailroom (bytes after data end)
    pub fn tailroom(&self) -> usize {
        self.buf.len() - self.data_end
    }

    /// Total capacity of the backing buffer
    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// Prepend bytes to the front (uses headroom, no copy if sufficient)
    ///
    /// Returns a mutable slice to the newly-prepended region.
    pub fn push_front(&mut self, count: usize) -> Result<&mut [u8], PacketError> {
        if count > self.data_start {
            return Err(PacketError::InsufficientHeadroom);
        }
        self.data_start -= count;
        Ok(&mut self.buf[self.data_start..self.data_start + count])
    }

    /// Append bytes to the end (uses tailroom, no copy if sufficient)
    ///
    /// Returns a mutable slice to the newly-appended region.
    pub fn push_back(&mut self, count: usize) -> Result<&mut [u8], PacketError> {
        if count > self.tailroom() {
            return Err(PacketError::InsufficientTailroom);
        }
        let start = self.data_end;
        self.data_end += count;
        Ok(&mut self.buf[start..self.data_end])
    }

    /// Remove bytes from the front (consume a header)
    pub fn pull_front(&mut self, count: usize) -> Result<(), PacketError> {
        if count > self.len() {
            return Err(PacketError::TooShort);
        }
        self.data_start += count;
        Ok(())
    }

    /// Remove bytes from the end (trim a trailer)
    pub fn trim_back(&mut self, count: usize) -> Result<(), PacketError> {
        if count > self.len() {
            return Err(PacketError::TooShort);
        }
        self.data_end -= count;
        Ok(())
    }

    /// Ensure at least `min` bytes of headroom; reallocates if needed
    pub fn ensure_headroom(&mut self, min: usize) {
        if self.data_start >= min {
            return;
        }
        let extra = min - self.data_start;
        let old_len = self.len();
        let new_total = self.buf.len() + extra;
        let mut new_buf = alloc::vec![0u8; new_total];
        let new_start = self.data_start + extra;
        new_buf[new_start..new_start + old_len]
            .copy_from_slice(&self.buf[self.data_start..self.data_end]);
        self.buf = new_buf;
        self.data_start = new_start;
        self.data_end = new_start + old_len;
    }

    /// Ensure at least `min` bytes of tailroom; reallocates if needed
    pub fn ensure_tailroom(&mut self, min: usize) {
        if self.tailroom() >= min {
            return;
        }
        let extra = min - self.tailroom();
        self.buf.resize(self.buf.len() + extra, 0);
    }

    /// Prepend header data from a slice
    pub fn prepend(&mut self, header: &[u8]) -> Result<(), PacketError> {
        let region = self.push_front(header.len())?;
        region.copy_from_slice(header);
        Ok(())
    }

    /// Append trailer data from a slice
    pub fn append(&mut self, trailer: &[u8]) -> Result<(), PacketError> {
        let region = self.push_back(trailer.len())?;
        region.copy_from_slice(trailer);
        Ok(())
    }

    /// Read a u16 in big-endian at the given offset from data start
    pub fn read_u16_be(&self, offset: usize) -> Result<u16, PacketError> {
        if offset + 2 > self.len() {
            return Err(PacketError::TooShort);
        }
        let d = self.data();
        Ok(u16::from_be_bytes([d[offset], d[offset + 1]]))
    }

    /// Read a u32 in big-endian at the given offset from data start
    pub fn read_u32_be(&self, offset: usize) -> Result<u32, PacketError> {
        if offset + 4 > self.len() {
            return Err(PacketError::TooShort);
        }
        let d = self.data();
        Ok(u32::from_be_bytes([
            d[offset],
            d[offset + 1],
            d[offset + 2],
            d[offset + 3],
        ]))
    }

    /// Write a u16 in big-endian at the given offset from data start
    pub fn write_u16_be(&mut self, offset: usize, val: u16) -> Result<(), PacketError> {
        if offset + 2 > self.len() {
            return Err(PacketError::TooShort);
        }
        let bytes = val.to_be_bytes();
        let d = self.data_mut();
        d[offset] = bytes[0];
        d[offset + 1] = bytes[1];
        Ok(())
    }

    /// Write a u32 in big-endian at the given offset from data start
    pub fn write_u32_be(&mut self, offset: usize, val: u32) -> Result<(), PacketError> {
        if offset + 4 > self.len() {
            return Err(PacketError::TooShort);
        }
        let bytes = val.to_be_bytes();
        let d = self.data_mut();
        d[offset] = bytes[0];
        d[offset + 1] = bytes[1];
        d[offset + 2] = bytes[2];
        d[offset + 3] = bytes[3];
        Ok(())
    }

    /// Extract the data as a new Vec
    pub fn to_vec(&self) -> Vec<u8> {
        self.data().to_vec()
    }

    /// Split the packet at the given offset from data start, returning (head, tail)
    pub fn split_at(&self, offset: usize) -> Result<(PacketBuf, PacketBuf), PacketError> {
        if offset > self.len() {
            return Err(PacketError::TooShort);
        }
        let head = PacketBuf::from_slice(&self.data()[..offset]);
        let mut tail = PacketBuf::from_slice(&self.data()[offset..]);
        tail.meta = self.meta.clone();
        Ok((head, tail))
    }

    /// Concatenate another packet's data to the end of this one
    pub fn concat(&mut self, other: &PacketBuf) -> Result<(), PacketError> {
        self.append(other.data())
    }
}

impl core::fmt::Debug for PacketBuf {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(
            f,
            "PacketBuf(len={}, head={}, tail={}, layer={:?})",
            self.len(),
            self.headroom(),
            self.tailroom(),
            self.meta.layer
        )
    }
}

// ---------------------------------------------------------------------------
// Packet errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketError {
    InsufficientHeadroom,
    InsufficientTailroom,
    TooShort,
    TooLarge,
    FragmentOverlap,
    FragmentMissing,
    FragmentTimeout,
    ReassemblyFull,
    InvalidFragment,
    ChecksumError,
}

// ---------------------------------------------------------------------------
// Fragmentation
// ---------------------------------------------------------------------------

/// Fragment a packet into MTU-sized pieces with IP fragmentation headers
///
/// Each fragment gets a copy of the first `header_len` bytes of the original
/// packet as a pseudo-header, plus the fragment offset and MF flag encoded
/// into the returned metadata.
pub fn fragment_packet(
    pkt: &PacketBuf,
    mtu: usize,
    ip_header_len: usize,
) -> Result<Vec<FragmentInfo>, PacketError> {
    let data = pkt.data();
    if data.len() <= mtu {
        // No fragmentation needed
        return Ok(alloc::vec![FragmentInfo {
            data: PacketBuf::from_slice(data),
            offset: 0,
            more_fragments: false,
        }]);
    }

    if ip_header_len > data.len() || ip_header_len > mtu {
        return Err(PacketError::TooShort);
    }

    let header = &data[..ip_header_len];
    let payload = &data[ip_header_len..];
    let max_payload_per_frag =
        ((mtu - ip_header_len) / FRAGMENT_GRANULARITY) * FRAGMENT_GRANULARITY;

    if max_payload_per_frag == 0 {
        return Err(PacketError::TooLarge);
    }

    let mut fragments = Vec::new();
    let mut offset = 0usize;

    while offset < payload.len() {
        let end = (offset + max_payload_per_frag).min(payload.len());
        let more = end < payload.len();

        let mut frag = PacketBuf::alloc(
            DEFAULT_HEADROOM,
            ip_header_len + (end - offset),
            DEFAULT_TAILROOM,
        );
        // Copy header
        {
            let region = frag
                .push_back(ip_header_len)
                .map_err(|_| PacketError::TooLarge)?;
            region.copy_from_slice(header);
        }
        // Copy payload slice
        {
            let region = frag
                .push_back(end - offset)
                .map_err(|_| PacketError::TooLarge)?;
            region.copy_from_slice(&payload[offset..end]);
        }

        frag.meta = pkt.meta.clone();

        fragments.push(FragmentInfo {
            data: frag,
            offset: (offset / FRAGMENT_GRANULARITY) as u16,
            more_fragments: more,
        });

        offset = end;
    }

    Ok(fragments)
}

/// Fragment metadata
#[derive(Debug, Clone)]
pub struct FragmentInfo {
    pub data: PacketBuf,
    /// Fragment offset in units of 8 bytes
    pub offset: u16,
    /// More Fragments flag
    pub more_fragments: bool,
}

// ---------------------------------------------------------------------------
// Reassembly
// ---------------------------------------------------------------------------

/// Key for identifying a fragment group
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReassemblyKey {
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    identification: u16,
    protocol: u8,
}

/// A single received fragment awaiting reassembly
#[derive(Debug, Clone)]
struct FragmentEntry {
    /// Fragment offset in bytes (not in 8-byte units)
    offset: usize,
    /// Fragment payload data (without IP header)
    data: Vec<u8>,
    /// More Fragments flag
    more_fragments: bool,
}

/// State for reassembling one fragmented datagram
struct ReassemblySlot {
    key: ReassemblyKey,
    fragments: Vec<FragmentEntry>,
    /// IP header from the first fragment
    ip_header: Vec<u8>,
    /// Total expected length (known once we see the last fragment)
    total_len: Option<usize>,
    /// Timestamp when first fragment arrived
    created_tick: u64,
}

/// Global reassembly table
struct ReassemblyTable {
    slots: Vec<ReassemblySlot>,
    tick: u64,
}

static REASSEMBLY: Mutex<Option<ReassemblyTable>> = Mutex::new(None);

/// Initialize the packet subsystem
pub fn init() {
    *REASSEMBLY.lock() = Some(ReassemblyTable {
        slots: Vec::new(),
        tick: 0,
    });
    serial_println!("  Net: packet buffer subsystem initialized");
}

/// Submit a fragment for reassembly. Returns the completed packet if all
/// fragments have been received, or None if still waiting.
pub fn submit_fragment(
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    identification: u16,
    protocol: u8,
    ip_header: &[u8],
    fragment_offset_bytes: usize,
    more_fragments: bool,
    payload: &[u8],
) -> Result<Option<PacketBuf>, PacketError> {
    let mut guard = REASSEMBLY.lock();
    let table = guard.as_mut().ok_or(PacketError::ReassemblyFull)?;

    // Advance tick for timeout tracking
    table.tick = table.tick.saturating_add(1);

    // Expire old entries
    table
        .slots
        .retain(|slot| table.tick - slot.created_tick < REASSEMBLY_TIMEOUT);

    let key = ReassemblyKey {
        src_ip,
        dst_ip,
        identification,
        protocol,
    };

    // Find or create slot
    let slot_idx = match table.slots.iter().position(|s| s.key == key) {
        Some(idx) => idx,
        None => {
            if table.slots.len() >= MAX_FRAGMENTS {
                return Err(PacketError::ReassemblyFull);
            }
            table.slots.push(ReassemblySlot {
                key: key.clone(),
                fragments: Vec::new(),
                ip_header: ip_header.to_vec(),
                total_len: None,
                created_tick: table.tick,
            });
            table.slots.len() - 1
        }
    };

    let slot = &mut table.slots[slot_idx];

    // Check for overlap with existing fragments
    for existing in &slot.fragments {
        let ex_start = existing.offset;
        let ex_end = ex_start + existing.data.len();
        let new_start = fragment_offset_bytes;
        let new_end = new_start + payload.len();
        if new_start < ex_end && new_end > ex_start {
            return Err(PacketError::FragmentOverlap);
        }
    }

    // Record total length if this is the last fragment
    if !more_fragments {
        slot.total_len = Some(fragment_offset_bytes + payload.len());
    }

    slot.fragments.push(FragmentEntry {
        offset: fragment_offset_bytes,
        data: payload.to_vec(),
        more_fragments,
    });

    // Check if we have all fragments
    if let Some(total) = slot.total_len {
        // Sort fragments by offset
        slot.fragments.sort_by_key(|f| f.offset);

        // Verify contiguous coverage
        let mut expected_offset = 0usize;
        for frag in &slot.fragments {
            if frag.offset != expected_offset {
                return Ok(None); // gap — still waiting
            }
            expected_offset += frag.data.len();
        }

        if expected_offset != total {
            return Ok(None);
        }

        // Reassemble
        let header = slot.ip_header.clone();
        let mut assembled =
            PacketBuf::alloc(DEFAULT_HEADROOM, header.len() + total, DEFAULT_TAILROOM);
        assembled
            .append(&header)
            .map_err(|_| PacketError::TooLarge)?;
        for frag in &slot.fragments {
            assembled
                .append(&frag.data)
                .map_err(|_| PacketError::TooLarge)?;
        }

        // Remove the slot
        table.slots.swap_remove(slot_idx);

        return Ok(Some(assembled));
    }

    Ok(None)
}

/// Expire old reassembly entries (call periodically)
pub fn reassembly_tick() {
    let mut guard = REASSEMBLY.lock();
    if let Some(table) = guard.as_mut() {
        table.tick = table.tick.saturating_add(1);
        let tick = table.tick;
        let before = table.slots.len();
        table
            .slots
            .retain(|slot| tick - slot.created_tick < REASSEMBLY_TIMEOUT);
        let expired = before - table.slots.len();
        if expired > 0 {
            serial_println!("  Net: reassembly expired {} incomplete datagrams", expired);
        }
    }
}

/// Get reassembly queue depth
pub fn reassembly_queue_depth() -> usize {
    let guard = REASSEMBLY.lock();
    match guard.as_ref() {
        Some(table) => table.slots.len(),
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Packet pool (pre-allocated buffers for fast allocation)
// ---------------------------------------------------------------------------

/// A pool of pre-allocated packet buffers
struct PacketPool {
    free: Vec<PacketBuf>,
    capacity: usize,
    alloc_count: u64,
    free_count: u64,
}

static POOL: Mutex<Option<PacketPool>> = Mutex::new(None);

/// Initialize the packet pool with a given number of pre-allocated buffers
pub fn init_pool(count: usize, buf_size: usize) {
    let mut free = Vec::with_capacity(count);
    for _ in 0..count {
        free.push(PacketBuf::alloc(
            DEFAULT_HEADROOM,
            buf_size,
            DEFAULT_TAILROOM,
        ));
    }
    *POOL.lock() = Some(PacketPool {
        free,
        capacity: count,
        alloc_count: 0,
        free_count: 0,
    });
    serial_println!(
        "  Net: packet pool initialized ({} x {} bytes)",
        count,
        buf_size
    );
}

/// Allocate a packet buffer from the pool (falls back to heap if pool is empty)
pub fn pool_alloc(min_size: usize) -> PacketBuf {
    let mut guard = POOL.lock();
    if let Some(pool) = guard.as_mut() {
        pool.alloc_count = pool.alloc_count.saturating_add(1);
        if let Some(mut pkt) = pool.free.pop() {
            // Reset the packet
            pkt.data_start = DEFAULT_HEADROOM;
            pkt.data_end = DEFAULT_HEADROOM;
            pkt.meta = PacketMeta::new();
            return pkt;
        }
    }
    // Fallback to fresh allocation
    PacketBuf::alloc(DEFAULT_HEADROOM, min_size, DEFAULT_TAILROOM)
}

/// Return a packet buffer to the pool
pub fn pool_free(pkt: PacketBuf) {
    let mut guard = POOL.lock();
    if let Some(pool) = guard.as_mut() {
        pool.free_count = pool.free_count.saturating_add(1);
        if pool.free.len() < pool.capacity {
            pool.free.push(pkt);
        }
        // else: drop it (pool full)
    }
}

/// Get pool statistics
pub fn pool_stats() -> (u64, u64, usize) {
    let guard = POOL.lock();
    match guard.as_ref() {
        Some(pool) => (pool.alloc_count, pool.free_count, pool.free.len()),
        None => (0, 0, 0),
    }
}

// ---------------------------------------------------------------------------
// Checksum helpers
// ---------------------------------------------------------------------------

/// Compute an internet checksum over a byte slice (RFC 1071)
pub fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        i = i.saturating_add(2);
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// Verify an internet checksum (returns true if valid)
pub fn verify_checksum(data: &[u8]) -> bool {
    internet_checksum(data) == 0
}
