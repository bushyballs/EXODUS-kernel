use crate::sync::Mutex;
/// Database replication for sync
///
/// Part of the AIOS database engine. Implements primary-replica
/// replication using a WAL-shipping approach. The primary streams
/// change records to replicas which apply them in order.
use alloc::string::String;
use alloc::vec::Vec;

pub enum ReplicaRole {
    Primary,
    Replica,
}

/// Replication change record
#[derive(Clone)]
pub struct ChangeRecord {
    pub sequence: u64,
    pub table_name: String,
    pub operation: ChangeOp,
    pub data: Vec<u8>,
    pub checksum: u32,
}

/// Type of change operation
#[derive(Clone, Debug)]
pub enum ChangeOp {
    Insert,
    Update,
    Delete,
}

impl ChangeRecord {
    /// Serialize this change record to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        // Sequence (8 bytes, little-endian)
        for i in 0..8 {
            buf.push(((self.sequence >> (i * 8)) & 0xFF) as u8);
        }
        // Operation type (1 byte)
        buf.push(match self.operation {
            ChangeOp::Insert => 1,
            ChangeOp::Update => 2,
            ChangeOp::Delete => 3,
        });
        // Table name length (2 bytes) + table name
        let name_len = self.table_name.len() as u16;
        buf.push((name_len & 0xFF) as u8);
        buf.push(((name_len >> 8) & 0xFF) as u8);
        for b in self.table_name.as_bytes() {
            buf.push(*b);
        }
        // Data length (4 bytes) + data
        let data_len = self.data.len() as u32;
        for i in 0..4 {
            buf.push(((data_len >> (i * 8)) & 0xFF) as u8);
        }
        for b in &self.data {
            buf.push(*b);
        }
        // Checksum (4 bytes)
        for i in 0..4 {
            buf.push(((self.checksum >> (i * 8)) & 0xFF) as u8);
        }
        buf
    }

    /// Deserialize from bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self, ()> {
        if data.len() < 15 {
            return Err(());
        }
        let mut pos = 0;

        // Sequence
        let mut sequence: u64 = 0;
        for i in 0..8 {
            sequence |= (data[pos + i] as u64) << (i * 8);
        }
        pos += 8;

        // Operation
        let operation = match data[pos] {
            1 => ChangeOp::Insert,
            2 => ChangeOp::Update,
            3 => ChangeOp::Delete,
            _ => return Err(()),
        };
        pos += 1;

        // Table name
        if pos + 2 > data.len() {
            return Err(());
        }
        let name_len = data[pos] as usize | ((data[pos + 1] as usize) << 8);
        pos += 2;
        if pos + name_len > data.len() {
            return Err(());
        }
        let mut table_name = String::new();
        for i in 0..name_len {
            table_name.push(data[pos + i] as char);
        }
        pos += name_len;

        // Data
        if pos + 4 > data.len() {
            return Err(());
        }
        let mut data_len: u32 = 0;
        for i in 0..4 {
            data_len |= (data[pos + i] as u32) << (i * 8);
        }
        pos += 4;
        if pos + data_len as usize > data.len() {
            return Err(());
        }
        let mut rec_data = Vec::with_capacity(data_len as usize);
        for i in 0..data_len as usize {
            rec_data.push(data[pos + i]);
        }
        pos += data_len as usize;

        // Checksum
        let mut checksum: u32 = 0;
        if pos + 4 <= data.len() {
            for i in 0..4 {
                checksum |= (data[pos + i] as u32) << (i * 8);
            }
        }

        Ok(Self {
            sequence,
            table_name,
            operation,
            data: rec_data,
            checksum,
        })
    }

    /// Compute CRC32-like checksum of the record data
    fn compute_checksum(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFFFFFF;
        for byte in data {
            crc ^= *byte as u32;
            for _ in 0..8 {
                if crc & 1 != 0 {
                    crc = (crc >> 1) ^ 0xEDB88320;
                } else {
                    crc >>= 1;
                }
            }
        }
        crc ^ 0xFFFFFFFF
    }
}

/// Peer replication endpoint
struct ReplicationPeer {
    peer_id: u64,
    address: String,
    last_ack_sequence: u64,
    lag_records: u64,
    connected: bool,
}

pub struct ReplicationStream {
    peer_id: u64,
    position: u64,
    buffer: Vec<ChangeRecord>,
    max_buffer_size: usize,
}

impl ReplicationStream {
    fn new(peer_id: u64) -> Self {
        Self {
            peer_id,
            position: 0,
            buffer: Vec::new(),
            max_buffer_size: 1024,
        }
    }

    fn enqueue(&mut self, record: ChangeRecord) -> bool {
        if self.buffer.len() >= self.max_buffer_size {
            return false; // buffer full
        }
        self.buffer.push(record);
        true
    }

    fn drain(&mut self) -> Vec<ChangeRecord> {
        let mut drained = Vec::new();
        core::mem::swap(&mut self.buffer, &mut drained);
        if !drained.is_empty() {
            self.position = drained.last().map(|r| r.sequence).unwrap_or(self.position);
        }
        drained
    }
}

pub struct ReplicationManager {
    role: ReplicaRole,
    peers: Vec<ReplicationPeer>,
    streams: Vec<ReplicationStream>,
    change_log: Vec<ChangeRecord>,
    next_sequence: u64,
    applied_sequence: u64,
    max_log_size: usize,
}

impl ReplicationManager {
    pub fn new(role: ReplicaRole) -> Self {
        let role_name = match role {
            ReplicaRole::Primary => "primary",
            ReplicaRole::Replica => "replica",
        };
        crate::serial_println!("[db::replication] manager created as {}", role_name);
        Self {
            role,
            peers: Vec::new(),
            streams: Vec::new(),
            change_log: Vec::new(),
            next_sequence: 1,
            applied_sequence: 0,
            max_log_size: 10000,
        }
    }

    /// Add a replication peer
    pub fn add_peer(&mut self, peer_id: u64, address: &str) {
        let mut addr = String::new();
        for c in address.chars() {
            addr.push(c);
        }
        self.peers.push(ReplicationPeer {
            peer_id,
            address: addr,
            last_ack_sequence: 0,
            lag_records: 0,
            connected: false,
        });
        self.streams.push(ReplicationStream::new(peer_id));
        crate::serial_println!("[db::replication] peer {} added: {}", peer_id, address);
    }

    /// Record a change (primary only)
    pub fn record_change(&mut self, table_name: &str, op: ChangeOp, data: Vec<u8>) {
        let mut tname = String::new();
        for c in table_name.chars() {
            tname.push(c);
        }
        let checksum = ChangeRecord::compute_checksum(&data);
        let record = ChangeRecord {
            sequence: self.next_sequence,
            table_name: tname,
            operation: op,
            data,
            checksum,
        };
        self.next_sequence = self.next_sequence.saturating_add(1);

        // Add to change log
        if self.change_log.len() >= self.max_log_size {
            // Trim oldest entries
            let trim_count = self.max_log_size / 4;
            for _ in 0..trim_count {
                if !self.change_log.is_empty() {
                    self.change_log.remove(0);
                }
            }
        }
        self.change_log.push(record.clone());

        // Enqueue to all streams
        for stream in &mut self.streams {
            stream.enqueue(record.clone());
        }
    }

    /// Send pending changes to replicas. Returns number of records sent.
    pub fn send_changes(&mut self) -> Result<usize, ()> {
        let mut total_sent = 0usize;
        for (i, stream) in self.streams.iter_mut().enumerate() {
            let records = stream.drain();
            let count = records.len();
            if count > 0 {
                // Serialize and "send" (in real impl, write to network)
                let mut total_bytes = 0usize;
                for record in &records {
                    let bytes = record.to_bytes();
                    total_bytes += bytes.len();
                }
                // Update peer lag tracking
                if i < self.peers.len() {
                    self.peers[i].lag_records = self.next_sequence.saturating_sub(stream.position);
                }
                crate::serial_println!(
                    "[db::replication] sent {} records ({} bytes) to peer {}",
                    count,
                    total_bytes,
                    stream.peer_id
                );
                total_sent += count;
            }
        }
        Ok(total_sent)
    }

    /// Apply incoming changes (replica only). Validates checksums before applying.
    pub fn apply_changes(&mut self, data: &[u8]) -> Result<(), ()> {
        // Parse the data as a series of change records
        let mut pos = 0usize;
        let mut applied = 0u64;

        while pos < data.len() {
            // Try to deserialize a record from remaining bytes
            let remaining = &data[pos..];
            match ChangeRecord::from_bytes(remaining) {
                Ok(record) => {
                    // Verify checksum
                    let computed = ChangeRecord::compute_checksum(&record.data);
                    if computed != record.checksum {
                        crate::serial_println!(
                            "[db::replication] checksum mismatch at seq {}",
                            record.sequence
                        );
                        return Err(());
                    }
                    // Verify sequence ordering
                    if record.sequence <= self.applied_sequence {
                        crate::serial_println!(
                            "[db::replication] duplicate seq {}, skipping",
                            record.sequence
                        );
                    } else {
                        self.applied_sequence = record.sequence;
                        applied += 1;
                    }
                    // Advance position past this record
                    let record_bytes = record.to_bytes();
                    pos += record_bytes.len();
                }
                Err(()) => {
                    crate::serial_println!(
                        "[db::replication] failed to parse record at offset {}",
                        pos
                    );
                    return Err(());
                }
            }
        }

        crate::serial_println!(
            "[db::replication] applied {} change records, seq now at {}",
            applied,
            self.applied_sequence
        );
        Ok(())
    }

    /// Get replication lag for all peers
    pub fn peer_lag(&self) -> Vec<(u64, u64)> {
        let mut result = Vec::new();
        for peer in &self.peers {
            result.push((peer.peer_id, peer.lag_records));
        }
        result
    }

    /// Get current replication sequence number
    pub fn current_sequence(&self) -> u64 {
        self.next_sequence - 1
    }
}

static REPL_MANAGER: Mutex<Option<ReplicationManager>> = Mutex::new(None);

pub fn init() {
    // Default to primary role
    let manager = ReplicationManager::new(ReplicaRole::Primary);
    let mut m = REPL_MANAGER.lock();
    *m = Some(manager);
    crate::serial_println!("[db::replication] replication subsystem initialized");
}
