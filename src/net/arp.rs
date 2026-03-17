use super::{Ipv4Addr, MacAddr, NetworkDriver};
use crate::sync::Mutex;
/// ARP (Address Resolution Protocol) for Genesis
///
/// Maps IPv4 addresses to MAC addresses on the local network.
/// Maintains a cache of known mappings with aging and state transitions.
///
/// ARP packet format (28 bytes for IPv4-over-Ethernet):
///   Hardware type (2B), Protocol type (2B), HW addr len (1B),
///   Proto addr len (1B), Operation (2B), Sender HW addr (6B),
///   Sender proto addr (4B), Target HW addr (6B), Target proto addr (4B)
///
/// Features:
///   - ARP cache with aging (tick-based TTL)
///   - ARP entry states: Incomplete, Reachable, Stale, Delay
///   - Packet queue for entries in Incomplete state
///   - ARP request building and sending
///   - ARP reply handling and cache update
///   - Gratuitous ARP for IP conflict detection
///   - Cache statistics
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// ARP operations
pub const ARP_REQUEST: u16 = 1;
pub const ARP_REPLY: u16 = 2;

/// Hardware type: Ethernet
pub const HW_ETHERNET: u16 = 1;

/// Protocol type: IPv4
pub const PROTO_IPV4: u16 = 0x0800;

/// ARP cache timeout for Reachable entries (20 minutes in ticks, 1 tick = 1ms)
const REACHABLE_TIMEOUT: u64 = 20 * 60 * 1000;

/// ARP cache timeout for Stale entries (60 minutes)
const STALE_TIMEOUT: u64 = 60 * 60 * 1000;

/// ARP cache timeout for Incomplete entries (3 seconds)
const INCOMPLETE_TIMEOUT: u64 = 3_000;

/// Delay before transitioning from Delay to Probe (5 seconds)
const DELAY_TIMEOUT: u64 = 5_000;

/// Maximum ARP retries for incomplete entries
const MAX_ARP_RETRIES: u32 = 5;

/// Maximum packets queued per incomplete entry
const MAX_PENDING_PACKETS: usize = 8;

/// Maximum ARP cache size
const MAX_CACHE_SIZE: usize = 512;

/// ARP cache expiry time (300 seconds = 5 minutes, measured in timer ticks)
/// Kept for backward compatibility — used as a default if not otherwise specified
pub const ARP_CACHE_TIMEOUT: u64 = 300 * 100;

// ---------------------------------------------------------------------------
// ARP entry states
// ---------------------------------------------------------------------------

/// States for an ARP cache entry (modeled after Linux neighbor cache)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArpEntryState {
    /// We sent an ARP request and are waiting for a reply
    Incomplete,
    /// We have a confirmed MAC address (recently validated)
    Reachable,
    /// The entry has not been used recently — may be stale
    Stale,
    /// A packet was sent to a Stale entry — waiting to see if it's still valid
    Delay,
    /// Static entry — never expires
    Static,
}

// ---------------------------------------------------------------------------
// ARP packet
// ---------------------------------------------------------------------------

/// ARP packet header
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct ArpPacket {
    pub hw_type: [u8; 2],    // Hardware type (big-endian)
    pub proto_type: [u8; 2], // Protocol type (big-endian)
    pub hw_len: u8,          // Hardware address length (6 for Ethernet)
    pub proto_len: u8,       // Protocol address length (4 for IPv4)
    pub operation: [u8; 2],  // Operation (big-endian)
    pub sender_hw: [u8; 6],  // Sender MAC
    pub sender_ip: [u8; 4],  // Sender IP
    pub target_hw: [u8; 6],  // Target MAC
    pub target_ip: [u8; 4],  // Target IP
}

impl ArpPacket {
    pub fn operation_u16(&self) -> u16 {
        u16::from_be_bytes(self.operation)
    }

    pub fn sender_mac(&self) -> MacAddr {
        MacAddr(self.sender_hw)
    }

    pub fn sender_ipv4(&self) -> Ipv4Addr {
        Ipv4Addr(self.sender_ip)
    }

    pub fn target_ipv4(&self) -> Ipv4Addr {
        Ipv4Addr(self.target_ip)
    }

    /// Parse an ARP packet from raw bytes
    pub fn parse(data: &[u8]) -> Option<&ArpPacket> {
        if data.len() < core::mem::size_of::<ArpPacket>() {
            return None;
        }
        let pkt = unsafe { &*(data.as_ptr() as *const ArpPacket) };
        // Validate hardware type and protocol type
        if u16::from_be_bytes(pkt.hw_type) != HW_ETHERNET {
            return None;
        }
        if u16::from_be_bytes(pkt.proto_type) != PROTO_IPV4 {
            return None;
        }
        if pkt.hw_len != 6 || pkt.proto_len != 4 {
            return None;
        }
        Some(pkt)
    }

    /// Serialize this ARP packet into bytes
    pub fn to_bytes(&self) -> [u8; 28] {
        let mut buf = [0u8; 28];
        buf[0..2].copy_from_slice(&self.hw_type);
        buf[2..4].copy_from_slice(&self.proto_type);
        buf[4] = self.hw_len;
        buf[5] = self.proto_len;
        buf[6..8].copy_from_slice(&self.operation);
        buf[8..14].copy_from_slice(&self.sender_hw);
        buf[14..18].copy_from_slice(&self.sender_ip);
        buf[18..24].copy_from_slice(&self.target_hw);
        buf[24..28].copy_from_slice(&self.target_ip);
        buf
    }
}

// ---------------------------------------------------------------------------
// ARP cache entry
// ---------------------------------------------------------------------------

/// ARP cache entry
#[derive(Clone)]
struct ArpEntry {
    /// Resolved MAC address (valid when state is Reachable, Stale, Delay, or Static)
    mac: MacAddr,
    /// Current state
    state: ArpEntryState,
    /// Tick when this entry was created or last confirmed
    created_tick: u64,
    /// Tick when the state was last changed
    state_tick: u64,
    /// Number of ARP requests sent (for Incomplete state)
    retries: u32,
    /// Packets queued while waiting for ARP resolution
    pending_packets: Vec<Vec<u8>>,
}

impl ArpEntry {
    fn new_incomplete() -> Self {
        let now = crate::time::clock::uptime_ms();
        ArpEntry {
            mac: MacAddr::ZERO,
            state: ArpEntryState::Incomplete,
            created_tick: now,
            state_tick: now,
            retries: 0,
            pending_packets: Vec::new(),
        }
    }

    fn new_reachable(mac: MacAddr) -> Self {
        let now = crate::time::clock::uptime_ms();
        ArpEntry {
            mac,
            state: ArpEntryState::Reachable,
            created_tick: now,
            state_tick: now,
            retries: 0,
            pending_packets: Vec::new(),
        }
    }

    fn new_static(mac: MacAddr) -> Self {
        let now = crate::time::clock::uptime_ms();
        ArpEntry {
            mac,
            state: ArpEntryState::Static,
            created_tick: now,
            state_tick: now,
            retries: 0,
            pending_packets: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// ARP cache
// ---------------------------------------------------------------------------

/// ARP cache — maps IPv4 addresses (as u32) to entries
static ARP_CACHE: Mutex<BTreeMap<u32, ArpEntry>> = Mutex::new(BTreeMap::new());

/// ARP statistics
struct ArpStats {
    requests_sent: u64,
    requests_received: u64,
    replies_sent: u64,
    replies_received: u64,
    cache_hits: u64,
    cache_misses: u64,
    cache_evictions: u64,
    conflicts_detected: u64,
}

static ARP_STATS: Mutex<ArpStats> = Mutex::new(ArpStats {
    requests_sent: 0,
    requests_received: 0,
    replies_sent: 0,
    replies_received: 0,
    cache_hits: 0,
    cache_misses: 0,
    cache_evictions: 0,
    conflicts_detected: 0,
});

/// Initialize ARP subsystem
pub fn init() {
    // Cache starts empty — broadcast MAC is implicit
}

// ---------------------------------------------------------------------------
// Cache operations
// ---------------------------------------------------------------------------

/// Look up a MAC address for an IP. Returns None if not resolved yet.
pub fn lookup(ip: Ipv4Addr) -> Option<MacAddr> {
    // Broadcast address always maps to broadcast MAC
    if ip == Ipv4Addr::BROADCAST {
        return Some(MacAddr::BROADCAST);
    }

    let mut cache = ARP_CACHE.lock();
    if let Some(entry) = cache.get_mut(&ip.to_u32()) {
        match entry.state {
            ArpEntryState::Reachable | ArpEntryState::Static => {
                {
                    let mut s = ARP_STATS.lock();
                    s.cache_hits = s.cache_hits.saturating_add(1);
                }
                Some(entry.mac)
            }
            ArpEntryState::Stale => {
                // Transition to Delay — still usable but may need revalidation
                entry.state = ArpEntryState::Delay;
                entry.state_tick = crate::time::clock::uptime_ms();
                {
                    let mut s = ARP_STATS.lock();
                    s.cache_hits = s.cache_hits.saturating_add(1);
                }
                Some(entry.mac)
            }
            ArpEntryState::Delay => {
                {
                    let mut s = ARP_STATS.lock();
                    s.cache_hits = s.cache_hits.saturating_add(1);
                }
                Some(entry.mac)
            }
            ArpEntryState::Incomplete => {
                {
                    let mut s = ARP_STATS.lock();
                    s.cache_misses = s.cache_misses.saturating_add(1);
                }
                None
            }
        }
    } else {
        {
            let mut s = ARP_STATS.lock();
            s.cache_misses = s.cache_misses.saturating_add(1);
        }
        None
    }
}

/// Add or update an entry to the ARP cache (Reachable state).
pub fn insert(ip: Ipv4Addr, mac: MacAddr) {
    let key = ip.to_u32();
    let mut cache = ARP_CACHE.lock();

    // Evict if cache is full
    if !cache.contains_key(&key) && cache.len() >= MAX_CACHE_SIZE {
        evict_oldest(&mut cache);
    }

    if let Some(entry) = cache.get_mut(&key) {
        // Update existing entry
        let had_pending = !entry.pending_packets.is_empty();
        entry.mac = mac;
        entry.state = ArpEntryState::Reachable;
        entry.state_tick = crate::time::clock::uptime_ms();
        entry.retries = 0;
        // Pending packets will be drained by the caller
        if had_pending {
            // Leave pending_packets for drain_pending() to pick up
        }
    } else {
        cache.insert(key, ArpEntry::new_reachable(mac));
    }
}

/// Add a static ARP entry (never expires).
pub fn insert_static(ip: Ipv4Addr, mac: MacAddr) {
    let key = ip.to_u32();
    let mut cache = ARP_CACHE.lock();
    cache.insert(key, ArpEntry::new_static(mac));
}

/// Remove an entry from the cache.
pub fn remove(ip: Ipv4Addr) {
    ARP_CACHE.lock().remove(&ip.to_u32());
}

/// Flush the entire ARP cache (except static entries).
pub fn flush() {
    ARP_CACHE
        .lock()
        .retain(|_, entry| entry.state == ArpEntryState::Static);
}

/// Flush all entries including static ones.
pub fn flush_all() {
    ARP_CACHE.lock().clear();
}

/// Evict the oldest non-static entry.
fn evict_oldest(cache: &mut BTreeMap<u32, ArpEntry>) {
    let mut oldest_key: Option<u32> = None;
    let mut oldest_tick = u64::MAX;

    for (&key, entry) in cache.iter() {
        if entry.state != ArpEntryState::Static && entry.state_tick < oldest_tick {
            oldest_tick = entry.state_tick;
            oldest_key = Some(key);
        }
    }

    if let Some(key) = oldest_key {
        cache.remove(&key);
        {
            let mut s = ARP_STATS.lock();
            s.cache_evictions = s.cache_evictions.saturating_add(1);
        }
    }
}

// ---------------------------------------------------------------------------
// Pending packet queue
// ---------------------------------------------------------------------------

/// Queue a packet to be sent once ARP resolution completes.
/// Creates an Incomplete entry if one doesn't exist.
/// Returns true if the packet was queued, false if the queue is full.
pub fn queue_packet(ip: Ipv4Addr, packet: Vec<u8>) -> bool {
    let key = ip.to_u32();
    let mut cache = ARP_CACHE.lock();

    let entry = cache.entry(key).or_insert_with(ArpEntry::new_incomplete);

    if entry.pending_packets.len() >= MAX_PENDING_PACKETS {
        return false; // Queue full — drop packet
    }

    entry.pending_packets.push(packet);
    true
}

/// Drain pending packets for a resolved IP.
/// Returns the packets that were queued.
pub fn drain_pending(ip: Ipv4Addr) -> Vec<Vec<u8>> {
    let key = ip.to_u32();
    let mut cache = ARP_CACHE.lock();

    if let Some(entry) = cache.get_mut(&key) {
        let packets = core::mem::take(&mut entry.pending_packets);
        packets
    } else {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// ARP processing
// ---------------------------------------------------------------------------

/// Process an incoming ARP packet.
/// Returns an ARP reply packet if we need to respond.
pub fn process_arp(data: &[u8], our_ip: Ipv4Addr, our_mac: MacAddr) -> Option<ArpPacket> {
    let packet = ArpPacket::parse(data)?;

    match packet.operation_u16() {
        ARP_REQUEST => {
            {
                let mut s = ARP_STATS.lock();
                s.requests_received = s.requests_received.saturating_add(1);
            }

            // Always update cache with sender info (even if not for us)
            insert(packet.sender_ipv4(), packet.sender_mac());

            // Is this request for our IP?
            if packet.target_ipv4() == our_ip {
                {
                    let mut s = ARP_STATS.lock();
                    s.replies_sent = s.replies_sent.saturating_add(1);
                }
                // Build ARP reply
                Some(ArpPacket {
                    hw_type: HW_ETHERNET.to_be_bytes(),
                    proto_type: PROTO_IPV4.to_be_bytes(),
                    hw_len: 6,
                    proto_len: 4,
                    operation: ARP_REPLY.to_be_bytes(),
                    sender_hw: our_mac.0,
                    sender_ip: our_ip.0,
                    target_hw: packet.sender_hw,
                    target_ip: packet.sender_ip,
                })
            } else {
                None
            }
        }
        ARP_REPLY => {
            {
                let mut s = ARP_STATS.lock();
                s.replies_received = s.replies_received.saturating_add(1);
            }

            // Update cache with the reply
            insert(packet.sender_ipv4(), packet.sender_mac());

            // Check for IP conflict: if sender claims our IP with a different MAC
            if packet.sender_ipv4() == our_ip && packet.sender_mac() != our_mac {
                {
                    let mut s = ARP_STATS.lock();
                    s.conflicts_detected = s.conflicts_detected.saturating_add(1);
                }
                crate::serial_println!(
                    "  ARP: IP CONFLICT! {} claimed by {} (we are {})",
                    our_ip,
                    packet.sender_mac(),
                    our_mac
                );
            }

            None
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// ARP request/reply building
// ---------------------------------------------------------------------------

/// Build an ARP request for an IP address.
pub fn build_request(our_mac: MacAddr, our_ip: Ipv4Addr, target_ip: Ipv4Addr) -> ArpPacket {
    {
        let mut s = ARP_STATS.lock();
        s.requests_sent = s.requests_sent.saturating_add(1);
    }
    ArpPacket {
        hw_type: HW_ETHERNET.to_be_bytes(),
        proto_type: PROTO_IPV4.to_be_bytes(),
        hw_len: 6,
        proto_len: 4,
        operation: ARP_REQUEST.to_be_bytes(),
        sender_hw: our_mac.0,
        sender_ip: our_ip.0,
        target_hw: [0; 6], // unknown
        target_ip: target_ip.0,
    }
}

/// Build a gratuitous ARP (announce our IP/MAC, detect conflicts).
/// A gratuitous ARP is an ARP request where sender and target IP are the same.
pub fn build_gratuitous(our_mac: MacAddr, our_ip: Ipv4Addr) -> ArpPacket {
    ArpPacket {
        hw_type: HW_ETHERNET.to_be_bytes(),
        proto_type: PROTO_IPV4.to_be_bytes(),
        hw_len: 6,
        proto_len: 4,
        operation: ARP_REQUEST.to_be_bytes(),
        sender_hw: our_mac.0,
        sender_ip: our_ip.0,
        target_hw: [0xFF; 6], // broadcast target
        target_ip: our_ip.0,  // same as sender (gratuitous)
    }
}

/// Build a gratuitous ARP reply (update all hosts on the network).
pub fn build_gratuitous_reply(our_mac: MacAddr, our_ip: Ipv4Addr) -> ArpPacket {
    ArpPacket {
        hw_type: HW_ETHERNET.to_be_bytes(),
        proto_type: PROTO_IPV4.to_be_bytes(),
        hw_len: 6,
        proto_len: 4,
        operation: ARP_REPLY.to_be_bytes(),
        sender_hw: our_mac.0,
        sender_ip: our_ip.0,
        target_hw: [0xFF; 6],
        target_ip: our_ip.0,
    }
}

// ---------------------------------------------------------------------------
// Cache aging / timer
// ---------------------------------------------------------------------------

/// Periodic ARP cache maintenance. Should be called every ~1 second.
/// Returns a list of IPs that need ARP requests (Incomplete entries).
pub fn timer_tick() -> Vec<Ipv4Addr> {
    let now = crate::time::clock::uptime_ms();
    let mut cache = ARP_CACHE.lock();
    let mut needs_request = Vec::new();
    let mut to_remove = Vec::new();

    for (&key, entry) in cache.iter_mut() {
        match entry.state {
            ArpEntryState::Incomplete => {
                let elapsed = now.saturating_sub(entry.state_tick);
                if elapsed >= INCOMPLETE_TIMEOUT {
                    if entry.retries < MAX_ARP_RETRIES {
                        entry.retries = entry.retries.saturating_add(1);
                        entry.state_tick = now;
                        needs_request.push(Ipv4Addr::from_u32(key));
                    } else {
                        // Give up — drop pending packets
                        to_remove.push(key);
                    }
                }
            }
            ArpEntryState::Reachable => {
                let elapsed = now.saturating_sub(entry.state_tick);
                if elapsed >= REACHABLE_TIMEOUT {
                    entry.state = ArpEntryState::Stale;
                    entry.state_tick = now;
                }
            }
            ArpEntryState::Stale => {
                let elapsed = now.saturating_sub(entry.state_tick);
                if elapsed >= STALE_TIMEOUT {
                    to_remove.push(key);
                }
            }
            ArpEntryState::Delay => {
                let elapsed = now.saturating_sub(entry.state_tick);
                if elapsed >= DELAY_TIMEOUT {
                    // Transition to Incomplete (need to re-validate)
                    entry.state = ArpEntryState::Incomplete;
                    entry.state_tick = now;
                    entry.retries = 0;
                    needs_request.push(Ipv4Addr::from_u32(key));
                }
            }
            ArpEntryState::Static => {
                // Never expires
            }
        }
    }

    for key in to_remove {
        cache.remove(&key);
    }

    needs_request
}

// ---------------------------------------------------------------------------
// Cache inspection
// ---------------------------------------------------------------------------

/// Get the state of a cache entry.
pub fn get_state(ip: Ipv4Addr) -> Option<ArpEntryState> {
    ARP_CACHE.lock().get(&ip.to_u32()).map(|e| e.state)
}

/// List all ARP cache entries: (IP, MAC, state, age_ms)
pub fn list_cache() -> Vec<(Ipv4Addr, MacAddr, ArpEntryState, u64)> {
    let now = crate::time::clock::uptime_ms();
    let cache = ARP_CACHE.lock();
    cache
        .iter()
        .map(|(&key, entry)| {
            (
                Ipv4Addr::from_u32(key),
                entry.mac,
                entry.state,
                now.saturating_sub(entry.created_tick),
            )
        })
        .collect()
}

/// Get the number of entries in the cache.
pub fn cache_size() -> usize {
    ARP_CACHE.lock().len()
}

/// Get ARP statistics.
pub fn get_stats() -> (u64, u64, u64, u64, u64, u64, u64, u64) {
    let s = ARP_STATS.lock();
    (
        s.requests_sent,
        s.requests_received,
        s.replies_sent,
        s.replies_received,
        s.cache_hits,
        s.cache_misses,
        s.cache_evictions,
        s.conflicts_detected,
    )
}

/// Reset ARP statistics.
pub fn reset_stats() {
    let mut s = ARP_STATS.lock();
    s.requests_sent = 0;
    s.requests_received = 0;
    s.replies_sent = 0;
    s.replies_received = 0;
    s.cache_hits = 0;
    s.cache_misses = 0;
    s.cache_evictions = 0;
    s.conflicts_detected = 0;
}

// ---------------------------------------------------------------------------
// High-level public API (task-specification wrappers)
// ---------------------------------------------------------------------------

/// Check the ARP cache for `ip`; if not present, broadcast an ARP request and
/// spin-wait up to ~100 000 iterations for a reply.
///
/// Returns the resolved MAC address or `None` on timeout.
///
/// This is a bare-metal busy-wait resolver suitable for kernel use before
/// threading is available.
pub fn arp_request(ip: Ipv4Addr) -> Option<MacAddr> {
    // Fast path: already in cache
    if let Some(mac) = lookup(ip) {
        return Some(mac);
    }

    // Get our own MAC and IP from the first configured interface.
    let (our_mac, our_ip) = {
        let ifaces = crate::net::INTERFACES.lock();
        match ifaces.first() {
            Some(i) => (i.mac, i.ipv4?),
            None => return None,
        }
    };

    // Build and broadcast an ARP request.
    let req = build_request(our_mac, our_ip, ip);
    let req_bytes = req.to_bytes();

    // Wrap in an Ethernet frame (broadcast destination) and send.
    {
        let mut frame = [0u8; 60];
        // Destination: broadcast
        frame[0..6].copy_from_slice(&[0xFF; 6]);
        // Source: our MAC
        frame[6..12].copy_from_slice(&our_mac.0);
        // EtherType: ARP = 0x0806
        frame[12] = 0x08;
        frame[13] = 0x06;
        // ARP payload
        frame[14..14 + 28].copy_from_slice(&req_bytes);
        let driver = crate::drivers::e1000::driver().lock();
        if let Some(ref nic) = *driver {
            let _ = nic.send(&frame);
        }
    }

    // Mark an Incomplete entry in the cache so process_arp can fill it.
    {
        let key = ip.to_u32();
        let mut cache = ARP_CACHE.lock();
        cache.entry(key).or_insert_with(ArpEntry::new_incomplete);
    }

    // Spin-wait for the reply to be processed by the network stack.
    for _ in 0u32..100_000 {
        crate::net::poll();
        if let Some(mac) = lookup(ip) {
            return Some(mac);
        }
        core::hint::spin_loop();
    }

    None
}

/// Add an IP → MAC mapping to the ARP cache (Reachable state).
///
/// This is an alias for `insert()` with the expected task-specification name.
#[inline(always)]
pub fn arp_cache_add(ip: [u8; 4], mac: [u8; 6]) {
    insert(Ipv4Addr(ip), MacAddr(mac));
}

/// Send a gratuitous ARP on the primary interface to announce our IP/MAC and
/// detect IP conflicts.
///
/// A gratuitous ARP request is broadcast with sender IP == target IP (our IP),
/// which causes all hosts on the segment to update their ARP caches.
pub fn arp_gratuitous(our_ip: [u8; 4], our_mac: [u8; 6]) {
    let ip = Ipv4Addr(our_ip);
    let mac = MacAddr(our_mac);
    let pkt = build_gratuitous(mac, ip);
    let bytes = pkt.to_bytes();

    let mut frame = [0u8; 60];
    frame[0..6].copy_from_slice(&[0xFF; 6]); // broadcast dst
    frame[6..12].copy_from_slice(&our_mac); // our src MAC
    frame[12] = 0x08;
    frame[13] = 0x06; // EtherType ARP
    frame[14..14 + 28].copy_from_slice(&bytes);

    let driver = crate::drivers::e1000::driver().lock();
    if let Some(ref nic) = *driver {
        let _ = nic.send(&frame);
    }
    crate::serial_println!("  ARP: gratuitous ARP sent for {}", ip);
}
