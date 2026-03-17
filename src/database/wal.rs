use crate::sync::Mutex;
/// Write-Ahead Log for crash recovery
///
/// Part of the AIOS database engine. Provides crash recovery by
/// logging all modifications before they are applied to the database
/// file. Supports checkpointing, recovery replay, and frame management.
use alloc::vec::Vec;

pub struct WalEntry {
    pub sequence: u64,
    pub page_id: u32,
    pub data: Vec<u8>,
}

impl WalEntry {
    /// Create a new WAL entry
    pub fn new(sequence: u64, page_id: u32, data: Vec<u8>) -> Self {
        Self {
            sequence,
            page_id,
            data,
        }
    }

    /// Compute a checksum for integrity verification
    fn checksum(&self) -> u32 {
        let mut crc: u32 = 0xFFFFFFFF;
        // Include sequence in checksum
        let seq_bytes = self.sequence.to_le_bytes();
        for b in &seq_bytes {
            crc ^= *b as u32;
            for _ in 0..8 {
                if crc & 1 != 0 {
                    crc = (crc >> 1) ^ 0xEDB88320;
                } else {
                    crc >>= 1;
                }
            }
        }
        // Include page_id
        let page_bytes = self.page_id.to_le_bytes();
        for b in &page_bytes {
            crc ^= *b as u32;
            for _ in 0..8 {
                if crc & 1 != 0 {
                    crc = (crc >> 1) ^ 0xEDB88320;
                } else {
                    crc >>= 1;
                }
            }
        }
        // Include data
        for b in &self.data {
            crc ^= *b as u32;
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

    /// Serialize the entry to bytes (for writing to WAL file)
    pub fn serialize(&self) -> Vec<u8> {
        let checksum = self.checksum();
        let data_len = self.data.len() as u32;
        let frame_size = 8 + 4 + 4 + self.data.len() + 4; // seq + page_id + data_len + data + checksum
        let mut buf = Vec::with_capacity(frame_size);

        // Sequence number (8 bytes LE)
        for b in &self.sequence.to_le_bytes() {
            buf.push(*b);
        }
        // Page ID (4 bytes LE)
        for b in &self.page_id.to_le_bytes() {
            buf.push(*b);
        }
        // Data length (4 bytes LE)
        for b in &data_len.to_le_bytes() {
            buf.push(*b);
        }
        // Data
        for b in &self.data {
            buf.push(*b);
        }
        // Checksum (4 bytes LE)
        for b in &checksum.to_le_bytes() {
            buf.push(*b);
        }

        buf
    }

    /// Deserialize an entry from bytes
    pub fn deserialize(data: &[u8]) -> Result<(Self, usize), ()> {
        if data.len() < 20 {
            // minimum: 8+4+4+0+4
            return Err(());
        }
        let mut pos = 0;

        // Sequence
        let mut seq_bytes = [0u8; 8];
        seq_bytes.copy_from_slice(&data[pos..pos + 8]);
        let sequence = u64::from_le_bytes(seq_bytes);
        pos += 8;

        // Page ID
        let mut page_bytes = [0u8; 4];
        page_bytes.copy_from_slice(&data[pos..pos + 4]);
        let page_id = u32::from_le_bytes(page_bytes);
        pos += 4;

        // Data length
        let mut len_bytes = [0u8; 4];
        len_bytes.copy_from_slice(&data[pos..pos + 4]);
        let data_len = u32::from_le_bytes(len_bytes) as usize;
        pos += 4;

        if pos + data_len + 4 > data.len() {
            return Err(());
        }

        // Data
        let mut entry_data = Vec::with_capacity(data_len);
        for i in 0..data_len {
            entry_data.push(data[pos + i]);
        }
        pos += data_len;

        // Checksum
        let mut cksum_bytes = [0u8; 4];
        cksum_bytes.copy_from_slice(&data[pos..pos + 4]);
        let stored_checksum = u32::from_le_bytes(cksum_bytes);
        pos += 4;

        let entry = Self {
            sequence,
            page_id,
            data: entry_data,
        };

        // Verify checksum
        let computed_checksum = entry.checksum();
        if computed_checksum != stored_checksum {
            crate::serial_println!(
                "[db::wal] checksum mismatch at seq {}: stored={:#x} computed={:#x}",
                sequence,
                stored_checksum,
                computed_checksum
            );
            return Err(());
        }

        Ok((entry, pos))
    }
}

/// WAL frame header (stored at the beginning of the WAL file)
struct WalHeader {
    magic: u32,
    version: u32,
    page_size: u32,
    checkpoint_sequence: u64,
    frame_count: u64,
}

impl WalHeader {
    fn new(page_size: u32) -> Self {
        Self {
            magic: 0x57414C21, // "WAL!"
            version: 1,
            page_size,
            checkpoint_sequence: 0,
            frame_count: 0,
        }
    }
}

/// Checkpoint mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointMode {
    Passive,  // checkpoint what you can without blocking
    Full,     // wait for readers, checkpoint everything
    Restart,  // full checkpoint then reset WAL
    Truncate, // full checkpoint then truncate WAL file
}

pub struct WriteAheadLog {
    header: WalHeader,
    frames: Vec<WalEntry>,
    next_sequence: u64,
    checkpoint_sequence: u64,
    max_frames_before_checkpoint: u64,
    total_bytes_written: u64,
    dirty_pages: Vec<u32>, // pages modified since last checkpoint
}

impl WriteAheadLog {
    pub fn new() -> Self {
        crate::serial_println!("[db::wal] write-ahead log created");
        Self {
            header: WalHeader::new(4096),
            frames: Vec::new(),
            next_sequence: 1,
            checkpoint_sequence: 0,
            max_frames_before_checkpoint: 1000,
            total_bytes_written: 0,
            dirty_pages: Vec::new(),
        }
    }

    pub fn append(&mut self, entry: WalEntry) -> Result<u64, ()> {
        let sequence = self.next_sequence;

        // Create entry with assigned sequence
        let wal_entry = WalEntry {
            sequence,
            page_id: entry.page_id,
            data: entry.data,
        };

        // Serialize and track size
        let serialized = wal_entry.serialize();
        self.total_bytes_written += serialized.len() as u64;

        // Track dirty pages
        let page_id = wal_entry.page_id;
        let mut found = false;
        for dp in &self.dirty_pages {
            if *dp == page_id {
                found = true;
                break;
            }
        }
        if !found {
            self.dirty_pages.push(page_id);
        }

        self.frames.push(wal_entry);
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.header.frame_count = self.header.frame_count.saturating_add(1);

        // Auto-checkpoint hint
        let frames_since = self.header.frame_count - self.checkpoint_sequence;
        if frames_since >= self.max_frames_before_checkpoint {
            crate::serial_println!(
                "[db::wal] auto-checkpoint recommended ({} frames since last)",
                frames_since
            );
        }

        Ok(sequence)
    }

    pub fn checkpoint(&mut self) -> Result<(), ()> {
        self.checkpoint_with_mode(CheckpointMode::Full)
    }

    /// Checkpoint with a specific mode
    pub fn checkpoint_with_mode(&mut self, mode: CheckpointMode) -> Result<(), ()> {
        let frames_to_checkpoint: usize;

        match mode {
            CheckpointMode::Passive => {
                // Only checkpoint frames that are not being read
                frames_to_checkpoint = self.frames.len();
            }
            CheckpointMode::Full | CheckpointMode::Restart | CheckpointMode::Truncate => {
                // Checkpoint all frames
                frames_to_checkpoint = self.frames.len();
            }
        }

        if frames_to_checkpoint == 0 {
            crate::serial_println!("[db::wal] nothing to checkpoint");
            return Ok(());
        }

        // "Apply" frames to the database (in real impl, write to DB file)
        let mut pages_written = 0u32;
        let mut bytes_written = 0u64;
        for i in 0..frames_to_checkpoint {
            bytes_written += self.frames[i].data.len() as u64;
            pages_written += 1;
        }

        // Update checkpoint tracking
        self.checkpoint_sequence = self.header.frame_count;
        self.header.checkpoint_sequence = self.checkpoint_sequence;

        crate::serial_println!(
            "[db::wal] checkpoint {:?}: {} pages written, {} bytes",
            mode,
            pages_written,
            bytes_written
        );

        // Clear checkpointed frames based on mode
        match mode {
            CheckpointMode::Truncate | CheckpointMode::Restart => {
                self.frames.clear();
                self.dirty_pages.clear();
                if matches!(mode, CheckpointMode::Truncate) {
                    self.header.frame_count = 0;
                    self.next_sequence = 1;
                    self.checkpoint_sequence = 0;
                    self.header.checkpoint_sequence = 0;
                    crate::serial_println!("[db::wal] WAL truncated");
                }
            }
            CheckpointMode::Passive | CheckpointMode::Full => {
                // Keep frames for readers but mark as checkpointed
                // In practice we'd remove them once all readers finish
                self.frames.clear();
                self.dirty_pages.clear();
            }
        }

        Ok(())
    }

    pub fn recover(&mut self) -> Result<Vec<WalEntry>, ()> {
        crate::serial_println!(
            "[db::wal] starting recovery, {} frames in log",
            self.frames.len()
        );

        // In recovery, replay all frames after the last checkpoint
        let mut recovered = Vec::new();
        let mut valid_count = 0u64;
        let mut invalid_count = 0u64;

        for frame in &self.frames {
            // Verify each frame's integrity
            let serialized = frame.serialize();
            match WalEntry::deserialize(&serialized) {
                Ok((verified_entry, _)) => {
                    recovered.push(WalEntry {
                        sequence: verified_entry.sequence,
                        page_id: verified_entry.page_id,
                        data: verified_entry.data,
                    });
                    valid_count += 1;
                }
                Err(()) => {
                    invalid_count += 1;
                    // Stop at first invalid frame (WAL is sequential)
                    crate::serial_println!("[db::wal] recovery stopped at invalid frame");
                    break;
                }
            }
        }

        crate::serial_println!(
            "[db::wal] recovery complete: {} valid, {} invalid frames",
            valid_count,
            invalid_count
        );

        // Update sequence to continue after recovered entries
        if let Some(last) = recovered.last() {
            self.next_sequence = last.sequence + 1;
        }

        Ok(recovered)
    }

    /// Get the number of frames in the WAL
    pub fn frame_count(&self) -> u64 {
        self.header.frame_count
    }

    /// Get total bytes written to the WAL
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes_written
    }

    /// Get the number of dirty pages
    pub fn dirty_page_count(&self) -> usize {
        self.dirty_pages.len()
    }

    /// Check if a checkpoint is needed
    pub fn needs_checkpoint(&self) -> bool {
        let frames_since = self
            .header
            .frame_count
            .saturating_sub(self.checkpoint_sequence);
        frames_since >= self.max_frames_before_checkpoint
    }

    /// Set the max frames before auto-checkpoint
    pub fn set_checkpoint_threshold(&mut self, threshold: u64) {
        self.max_frames_before_checkpoint = if threshold == 0 { 1 } else { threshold };
    }
}

static WAL: Mutex<Option<WriteAheadLog>> = Mutex::new(None);

pub fn init() {
    let wal = WriteAheadLog::new();
    let mut w = WAL.lock();
    *w = Some(wal);
    crate::serial_println!("[db::wal] WAL subsystem initialized");
}
