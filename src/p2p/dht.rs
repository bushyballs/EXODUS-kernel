/// Distributed Hash Table (Kademlia-style) for Genesis
///
/// Provides decentralized key-value storage with XOR-distance routing,
/// k-bucket peer management, iterative lookups, and replication.
///
/// All code is original. No external crates.
use crate::sync::Mutex;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum entries per k-bucket
const K_BUCKET_SIZE: usize = 20;

/// Number of bits in a key (u64)
const KEY_BITS: usize = 64;

/// Maximum entries stored in the local DHT
const MAX_DHT_ENTRIES: usize = 4096;

/// Number of closest nodes to return in a find_node query
const ALPHA: usize = 3;

/// Default entry time-to-live in abstract ticks
const DEFAULT_ENTRY_TTL: u32 = 3600;

/// Replication factor: store on this many closest nodes
const REPLICATION_FACTOR: usize = 3;

/// Maximum number of known peers in the routing table
const MAX_PEERS: usize = 1024;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single entry in the distributed hash table.
#[derive(Clone)]
pub struct DhtEntry {
    pub key: u64,
    pub value_hash: u64,
    pub owner: u64,
    pub timestamp: u64,
    pub ttl: u32,
}

/// A k-bucket holds up to `k` peers, sorted by last-seen time.
/// Each element is (node_id, last_seen_timestamp).
pub struct KBucket {
    pub entries: Vec<(u64, u64)>,
    pub k: usize,
}

impl KBucket {
    /// Create a new empty k-bucket with the given capacity.
    pub fn new(k: usize) -> Self {
        KBucket {
            entries: Vec::new(),
            k,
        }
    }

    /// Insert or update a peer in the bucket.
    /// Returns true if the peer was added or updated.
    pub fn upsert(&mut self, node_id: u64, timestamp: u64) -> bool {
        // Check if already present — move to tail (most recently seen)
        for i in 0..self.entries.len() {
            if self.entries[i].0 == node_id {
                self.entries.remove(i);
                self.entries.push((node_id, timestamp));
                return true;
            }
        }

        // If bucket not full, just add
        if self.entries.len() < self.k {
            self.entries.push((node_id, timestamp));
            return true;
        }

        // Bucket full — ping the least recently seen (head).
        // In a real implementation we'd async-ping; here we evict
        // the head if it's older than the new peer's timestamp.
        if let Some(&(_, head_ts)) = self.entries.first() {
            if timestamp > head_ts {
                self.entries.remove(0);
                self.entries.push((node_id, timestamp));
                return true;
            }
        }

        false
    }

    /// Remove a peer by node id.
    pub fn remove(&mut self, node_id: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|&(id, _)| id != node_id);
        self.entries.len() < before
    }

    /// Check if the bucket contains a given node.
    pub fn contains(&self, node_id: u64) -> bool {
        self.entries.iter().any(|&(id, _)| id == node_id)
    }

    /// Number of entries in the bucket.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the bucket is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Routing table: one k-bucket per bit of key space.
pub struct RoutingTable {
    pub local_id: u64,
    pub buckets: Vec<KBucket>,
}

impl RoutingTable {
    /// Create a new routing table for the given local node id.
    pub fn new(local_id: u64) -> Self {
        let mut buckets = Vec::new();
        for _ in 0..KEY_BITS {
            buckets.push(KBucket::new(K_BUCKET_SIZE));
        }
        RoutingTable { local_id, buckets }
    }

    /// Determine which bucket index a remote node falls into.
    /// This is the position of the highest set bit of (local_id XOR remote_id).
    pub fn bucket_index(&self, remote_id: u64) -> usize {
        let distance = self.local_id ^ remote_id;
        if distance == 0 {
            return 0;
        }
        // Highest set bit position (0-indexed from LSB)
        let mut pos = 0u32;
        let mut d = distance;
        while d > 1 {
            d >>= 1;
            pos += 1;
        }
        pos as usize
    }

    /// Insert or update a peer in the appropriate bucket.
    pub fn update_peer(&mut self, node_id: u64, timestamp: u64) -> bool {
        if node_id == self.local_id {
            return false;
        }
        let idx = self.bucket_index(node_id);
        if idx < self.buckets.len() {
            self.buckets[idx].upsert(node_id, timestamp)
        } else {
            false
        }
    }

    /// Remove a peer from the routing table.
    pub fn remove_peer(&mut self, node_id: u64) -> bool {
        let idx = self.bucket_index(node_id);
        if idx < self.buckets.len() {
            self.buckets[idx].remove(node_id)
        } else {
            false
        }
    }

    /// Get the `count` closest known nodes to a target key.
    pub fn closest_nodes(&self, target: u64, count: usize) -> Vec<u64> {
        let mut all_peers: Vec<(u64, u64)> = Vec::new(); // (distance, node_id)

        for bucket in &self.buckets {
            for &(node_id, _) in &bucket.entries {
                let dist = xor_distance(node_id, target);
                all_peers.push((dist, node_id));
            }
        }

        // Sort by distance (ascending)
        sort_by_distance(&mut all_peers);

        let limit = if count < all_peers.len() {
            count
        } else {
            all_peers.len()
        };
        all_peers[..limit].iter().map(|&(_, id)| id).collect()
    }

    /// Total number of peers across all buckets.
    pub fn peer_count(&self) -> usize {
        self.buckets.iter().map(|b| b.len()).sum()
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static DHT_ENTRIES: Mutex<Option<Vec<DhtEntry>>> = Mutex::new(None);
static DHT_LOCAL_ID: Mutex<Option<u64>> = Mutex::new(None);
static DHT_ROUTING: Mutex<Option<RoutingTable>> = Mutex::new(None);
static DHT_ACTIVE: Mutex<bool> = Mutex::new(false);

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    {
        let mut entries = DHT_ENTRIES.lock();
        *entries = Some(Vec::new());
    }
    serial_println!("    dht: initialized (Kademlia-style, k={})", K_BUCKET_SIZE);
}

// ---------------------------------------------------------------------------
// XOR distance
// ---------------------------------------------------------------------------

/// Compute the XOR distance between two keys.
#[inline]
pub fn xor_distance(a: u64, b: u64) -> u64 {
    a ^ b
}

/// Sort a list of (distance, node_id) pairs by distance ascending.
/// Simple insertion sort — fine for small k-bucket sizes.
fn sort_by_distance(pairs: &mut Vec<(u64, u64)>) {
    let len = pairs.len();
    for i in 1..len {
        let mut j = i;
        while j > 0 && pairs[j].0 < pairs[j - 1].0 {
            pairs.swap(j, j - 1);
            j -= 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Bootstrap / join
// ---------------------------------------------------------------------------

/// Bootstrap the local DHT node with the given id.
pub fn bootstrap(local_id: u64) -> bool {
    {
        let mut id = DHT_LOCAL_ID.lock();
        *id = Some(local_id);
    }
    {
        let mut routing = DHT_ROUTING.lock();
        *routing = Some(RoutingTable::new(local_id));
    }
    {
        let mut active = DHT_ACTIVE.lock();
        *active = true;
    }
    true
}

/// Register a peer in the routing table.
pub fn ping(node_id: u64, timestamp: u64) -> bool {
    let mut routing = DHT_ROUTING.lock();
    if let Some(ref mut rt) = *routing {
        rt.update_peer(node_id, timestamp)
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Store / Lookup
// ---------------------------------------------------------------------------

/// Store a key-value entry in the local DHT.
pub fn store(key: u64, value_hash: u64, owner: u64, timestamp: u64) -> bool {
    let active = DHT_ACTIVE.lock();
    if !*active {
        return false;
    }
    drop(active);

    let mut entries = DHT_ENTRIES.lock();
    if let Some(ref mut list) = *entries {
        // Update existing entry with same key
        for entry in list.iter_mut() {
            if entry.key == key {
                entry.value_hash = value_hash;
                entry.owner = owner;
                entry.timestamp = timestamp;
                entry.ttl = DEFAULT_ENTRY_TTL;
                return true;
            }
        }

        // Reject if at capacity
        if list.len() >= MAX_DHT_ENTRIES {
            // Evict oldest entry
            if !list.is_empty() {
                let mut oldest_idx = 0;
                let mut oldest_ts = list[0].timestamp;
                for (i, entry) in list.iter().enumerate() {
                    if entry.timestamp < oldest_ts {
                        oldest_ts = entry.timestamp;
                        oldest_idx = i;
                    }
                }
                list.remove(oldest_idx);
            }
        }

        list.push(DhtEntry {
            key,
            value_hash,
            owner,
            timestamp,
            ttl: DEFAULT_ENTRY_TTL,
        });
        true
    } else {
        false
    }
}

/// Look up a value by key in the local DHT store.
pub fn lookup(key: u64) -> Option<DhtEntry> {
    let entries = DHT_ENTRIES.lock();
    if let Some(ref list) = *entries {
        for entry in list.iter() {
            if entry.key == key {
                return Some(entry.clone());
            }
        }
    }
    None
}

/// Find the closest known nodes to a given target key.
pub fn find_node(target: u64, count: usize) -> Vec<u64> {
    let routing = DHT_ROUTING.lock();
    if let Some(ref rt) = *routing {
        rt.closest_nodes(target, count)
    } else {
        Vec::new()
    }
}

/// Find a value by key. First check local store, then return closest nodes.
/// Returns (Some(entry), []) if found locally, or (None, closest_nodes) otherwise.
pub fn find_value(key: u64) -> (Option<DhtEntry>, Vec<u64>) {
    // Check local store first
    if let Some(entry) = lookup(key) {
        return (Some(entry), Vec::new());
    }

    // Not found locally — return closest nodes for iterative lookup
    let closest = find_node(key, ALPHA);
    (None, closest)
}

/// Get the closest nodes to a target from the routing table.
pub fn get_closest_nodes(target: u64, count: usize) -> Vec<u64> {
    find_node(target, count)
}

// ---------------------------------------------------------------------------
// Bucket maintenance
// ---------------------------------------------------------------------------

/// Refresh a specific bucket by performing a lookup on a random key in that
/// bucket's range. Returns the number of peers discovered.
pub fn refresh_bucket(bucket_index: usize) -> usize {
    if bucket_index >= KEY_BITS {
        return 0;
    }

    let local_id;
    {
        let id = DHT_LOCAL_ID.lock();
        local_id = match *id {
            Some(lid) => lid,
            None => return 0,
        };
    }

    // Generate a target that would fall into the specified bucket.
    // Flip the bit at `bucket_index` in the local id.
    let target = local_id ^ (1u64 << bucket_index);

    // Perform a find_node for that target to populate the bucket
    let found = find_node(target, K_BUCKET_SIZE);
    found.len()
}

/// Expire entries whose TTL has been exhausted.
/// `elapsed` is the number of ticks since last expiry pass.
pub fn expire_entries(elapsed: u32) -> usize {
    let mut entries = DHT_ENTRIES.lock();
    if let Some(ref mut list) = *entries {
        let before = list.len();
        for entry in list.iter_mut() {
            if entry.ttl > elapsed {
                entry.ttl -= elapsed;
            } else {
                entry.ttl = 0;
            }
        }
        list.retain(|e| e.ttl > 0);
        before - list.len()
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Replication
// ---------------------------------------------------------------------------

/// Replicate a key to the `REPLICATION_FACTOR` closest nodes.
/// Returns the list of node IDs that should store a copy.
pub fn replicate(key: u64) -> Vec<u64> {
    let entry = lookup(key);
    if entry.is_none() {
        return Vec::new();
    }

    let closest = find_node(key, REPLICATION_FACTOR);
    // In a real network, we'd issue store RPCs to each of these nodes.
    // Here we return the target set for the caller to handle.
    closest
}

/// Replicate all local entries to their respective closest nodes.
/// Returns total replication targets generated.
pub fn replicate_all() -> usize {
    let keys: Vec<u64>;
    {
        let entries = DHT_ENTRIES.lock();
        keys = match *entries {
            Some(ref list) => list.iter().map(|e| e.key).collect(),
            None => Vec::new(),
        };
    }

    let mut total = 0;
    for key in keys {
        total += replicate(key).len();
    }
    total
}

// ---------------------------------------------------------------------------
// Stats / queries
// ---------------------------------------------------------------------------

/// Number of entries in the local DHT store.
pub fn entry_count() -> usize {
    let entries = DHT_ENTRIES.lock();
    match *entries {
        Some(ref list) => list.len(),
        None => 0,
    }
}

/// Number of peers in the routing table.
pub fn peer_count() -> usize {
    let routing = DHT_ROUTING.lock();
    match *routing {
        Some(ref rt) => rt.peer_count(),
        None => 0,
    }
}

/// Check whether the DHT is active.
pub fn is_active() -> bool {
    let active = DHT_ACTIVE.lock();
    *active
}

/// Get all entries owned by a specific node.
pub fn entries_by_owner(owner: u64) -> Vec<DhtEntry> {
    let entries = DHT_ENTRIES.lock();
    match *entries {
        Some(ref list) => list.iter().filter(|e| e.owner == owner).cloned().collect(),
        None => Vec::new(),
    }
}

/// Get all entries within a certain XOR distance of a key.
pub fn entries_near(target: u64, max_distance: u64) -> Vec<DhtEntry> {
    let entries = DHT_ENTRIES.lock();
    match *entries {
        Some(ref list) => list
            .iter()
            .filter(|e| xor_distance(e.key, target) <= max_distance)
            .cloned()
            .collect(),
        None => Vec::new(),
    }
}

/// Shutdown the DHT, clearing all state.
pub fn shutdown() {
    {
        let mut entries = DHT_ENTRIES.lock();
        *entries = Some(Vec::new());
    }
    {
        let mut routing = DHT_ROUTING.lock();
        *routing = None;
    }
    {
        let mut id = DHT_LOCAL_ID.lock();
        *id = None;
    }
    {
        let mut active = DHT_ACTIVE.lock();
        *active = false;
    }
}
