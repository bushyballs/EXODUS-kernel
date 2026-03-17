use crate::serial_println;
use crate::sync::Mutex;
/// Filesystem journaling engine for crash recovery
///
/// Part of the AIOS filesystem layer.
///
/// Provides a write-ahead log (WAL) that records filesystem metadata
/// changes before they are committed to the main area, enabling crash
/// recovery by replaying the journal after an unclean shutdown.
///
/// Design:
///   - Circular log buffer stored in memory (can be backed by a journal
///     partition or file on disk).
///   - Three modes: Writeback (metadata only, data written directly),
///     Ordered (data written before metadata commit), Full (all data
///     goes through journal).
///   - Transactions are identified by monotonically increasing sequence
///     numbers. A transaction consists of a set of JournalBlock records.
///   - Checkpoint: mark committed transactions as fully written to disk.
///   - Replay: on recovery, scan the journal for committed but
///     un-checkpointed transactions and re-apply them.
///
/// Inspired by: Linux jbd2 (fs/jbd2), ext4 journaling. All code is original.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default journal log capacity (number of records)
const DEFAULT_LOG_CAPACITY: usize = 4096;

/// Magic number for journal superblock identification
const JOURNAL_MAGIC: u32 = 0x4A524E4C; // "JRNL"

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Journaling mode.
#[derive(Clone, Copy, PartialEq)]
pub enum JournalMode {
    /// Only metadata goes through the journal; data is written directly.
    Writeback,
    /// Data is written to its final location before the metadata commit.
    Ordered,
    /// Both data and metadata go through the journal.
    Full,
}

/// State of a transaction.
#[derive(Clone, Copy, PartialEq)]
enum TxnState {
    /// Accumulating writes.
    Running,
    /// All writes recorded, commit record pending.
    Committing,
    /// Commit record written to journal.
    Committed,
    /// Data written to final location; journal space can be reclaimed.
    Checkpointed,
}

/// A single journal record (one modified block).
#[derive(Clone)]
struct JournalBlock {
    /// Block number on the filesystem being modified
    block_nr: u64,
    /// Snapshot of the block data before/after (for undo/redo)
    data: Vec<u8>,
}

/// A transaction groups multiple block writes into an atomic unit.
#[derive(Clone)]
struct Transaction {
    seq: u64,
    state: TxnState,
    blocks: Vec<JournalBlock>,
}

/// Inner journal state.
struct Inner {
    mode: JournalMode,
    /// All transactions (circular buffer semantic, indexed by seq)
    log: Vec<Transaction>,
    /// Next sequence number
    next_seq: u64,
    /// Index of the active (running) transaction, if any
    active_txn: Option<usize>,
    /// Sequence number of the last checkpointed transaction
    checkpoint_seq: u64,
    /// Total committed bytes (for stats)
    committed_bytes: u64,
    /// Number of replayed transactions (for stats)
    replayed_count: u64,
}

// ---------------------------------------------------------------------------
// Inner implementation
// ---------------------------------------------------------------------------

impl Inner {
    fn new(mode: JournalMode) -> Self {
        Inner {
            mode,
            log: Vec::new(),
            next_seq: 1,
            active_txn: None,
            checkpoint_seq: 0,
            committed_bytes: 0,
            replayed_count: 0,
        }
    }

    /// Begin a new transaction, returning its sequence number.
    fn begin(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        let txn = Transaction {
            seq,
            state: TxnState::Running,
            blocks: Vec::new(),
        };
        self.log.push(txn);
        self.active_txn = Some(self.log.len() - 1);
        seq
    }

    /// Record a block write in the current (or specified) transaction.
    fn log_block(&mut self, txn_seq: u64, block_nr: u64, data: &[u8]) -> Result<(), ()> {
        let txn = self.find_txn_mut(txn_seq).ok_or(())?;
        if txn.state != TxnState::Running {
            return Err(()); // Can only write to running transactions
        }
        txn.blocks.push(JournalBlock {
            block_nr,
            data: Vec::from(data),
        });
        Ok(())
    }

    /// Commit a transaction: mark it as committed so it can survive a crash.
    fn commit(&mut self, txn_seq: u64) -> Result<(), ()> {
        let txn = self.find_txn_mut(txn_seq).ok_or(())?;
        if txn.state != TxnState::Running {
            return Err(());
        }
        txn.state = TxnState::Committing;
        // In a real implementation, we would write the commit record to disk here.
        // Simulate the commit by advancing to Committed state.
        let byte_count: u64 = txn.blocks.iter().map(|b| b.data.len() as u64).sum();
        txn.state = TxnState::Committed;
        self.committed_bytes = self.committed_bytes.saturating_add(byte_count);
        if let Some(ref mut idx) = self.active_txn {
            if self.log.get(*idx).map_or(false, |t| t.seq == txn_seq) {
                self.active_txn = None;
            }
        }
        Ok(())
    }

    /// Abort a running transaction, discarding all its records.
    fn abort(&mut self, txn_seq: u64) -> Result<(), ()> {
        let idx = self.find_txn_index(txn_seq).ok_or(())?;
        if self.log[idx].state != TxnState::Running {
            return Err(());
        }
        self.log.remove(idx);
        if self.active_txn == Some(idx) {
            self.active_txn = None;
        }
        Ok(())
    }

    /// Checkpoint: mark committed transactions up to `up_to_seq` as fully
    /// written to their final locations. Their journal space can be reclaimed.
    fn checkpoint(&mut self, up_to_seq: u64) {
        for txn in self.log.iter_mut() {
            if txn.seq <= up_to_seq && txn.state == TxnState::Committed {
                txn.state = TxnState::Checkpointed;
            }
        }
        if up_to_seq > self.checkpoint_seq {
            self.checkpoint_seq = up_to_seq;
        }
        // Reclaim checkpointed entries from the front of the log
        while let Some(first) = self.log.first() {
            if first.state == TxnState::Checkpointed {
                self.log.remove(0);
            } else {
                break;
            }
        }
    }

    /// Replay: find all committed-but-not-checkpointed transactions and
    /// return their block writes for re-application.
    fn replay(&mut self) -> Vec<(u64, Vec<u8>)> {
        let mut writes = Vec::new();
        for txn in self.log.iter_mut() {
            if txn.state == TxnState::Committed {
                for block in txn.blocks.iter() {
                    writes.push((block.block_nr, block.data.clone()));
                }
                txn.state = TxnState::Checkpointed;
                self.replayed_count = self.replayed_count.saturating_add(1);
            }
        }
        // Clean up replayed entries
        self.log.retain(|t| t.state != TxnState::Checkpointed);
        writes
    }

    /// Check if there are any transactions needing recovery.
    fn needs_recovery(&self) -> bool {
        self.log.iter().any(|t| t.state == TxnState::Committed)
    }

    /// Return the number of active (running + committing) transactions.
    fn active_count(&self) -> usize {
        self.log
            .iter()
            .filter(|t| t.state == TxnState::Running || t.state == TxnState::Committing)
            .count()
    }

    fn find_txn_mut(&mut self, seq: u64) -> Option<&mut Transaction> {
        self.log.iter_mut().find(|t| t.seq == seq)
    }

    fn find_txn_index(&self, seq: u64) -> Option<usize> {
        self.log.iter().position(|t| t.seq == seq)
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static JOURNAL: Mutex<Option<Inner>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Begin a new journal transaction. Returns the transaction sequence number.
pub fn begin_transaction() -> Result<u64, ()> {
    let mut guard = JOURNAL.lock();
    guard.as_mut().map(|inner| inner.begin()).ok_or(())
}

/// Record a block write in a transaction.
pub fn log_block(txn_seq: u64, block_nr: u64, data: &[u8]) -> Result<(), ()> {
    let mut guard = JOURNAL.lock();
    guard
        .as_mut()
        .ok_or(())
        .and_then(|inner| inner.log_block(txn_seq, block_nr, data))
}

/// Commit a transaction.
pub fn commit(txn_seq: u64) -> Result<(), ()> {
    let mut guard = JOURNAL.lock();
    guard
        .as_mut()
        .ok_or(())
        .and_then(|inner| inner.commit(txn_seq))
}

/// Abort a transaction.
pub fn abort(txn_seq: u64) -> Result<(), ()> {
    let mut guard = JOURNAL.lock();
    guard
        .as_mut()
        .ok_or(())
        .and_then(|inner| inner.abort(txn_seq))
}

/// Checkpoint committed transactions up to the given sequence number.
pub fn checkpoint(up_to_seq: u64) {
    let mut guard = JOURNAL.lock();
    if let Some(inner) = guard.as_mut() {
        inner.checkpoint(up_to_seq);
    }
}

/// Replay the journal for crash recovery. Returns block writes to re-apply.
pub fn replay() -> Vec<(u64, Vec<u8>)> {
    let mut guard = JOURNAL.lock();
    guard.as_mut().map_or_else(Vec::new, |inner| inner.replay())
}

/// Check whether recovery is needed.
pub fn needs_recovery() -> bool {
    let guard = JOURNAL.lock();
    guard.as_ref().map_or(false, |inner| inner.needs_recovery())
}

/// Return (committed_bytes, replayed_count, active_txn_count).
pub fn stats() -> (u64, u64, usize) {
    let guard = JOURNAL.lock();
    guard.as_ref().map_or((0, 0, 0), |inner| {
        (
            inner.committed_bytes,
            inner.replayed_count,
            inner.active_count(),
        )
    })
}

/// Initialize the journaling subsystem with a given mode.
pub fn init_with_mode(mode: JournalMode) {
    let mut guard = JOURNAL.lock();
    *guard = Some(Inner::new(mode));
    serial_println!(
        "    journal: initialized (mode={})",
        match mode {
            JournalMode::Writeback => "writeback",
            JournalMode::Ordered => "ordered",
            JournalMode::Full => "full",
        }
    );
}

/// Initialize with default ordered mode.
pub fn init() {
    init_with_mode(JournalMode::Ordered);
}
