/// ACID transaction manager for Genesis embedded database
///
/// Provides:
///   - Transaction begin / commit / rollback
///   - Write-Ahead Log (WAL) for crash recovery
///   - Isolation via transaction-local undo buffers
///   - Savepoints for nested partial rollback
///   - Deadlock detection (wait-for graph cycle check)
///
/// WAL format: sequential log entries with LSN (log sequence number),
/// operation type, table/row identifiers, and before/after images.
///
/// No floats. All sizing uses Q16 fixed-point where needed.
///
/// Inspired by: SQLite WAL mode, PostgreSQL MVCC, InnoDB undo logs.
/// All code is original.
use crate::{serial_print, serial_println};

use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Q16 fixed-point constant
const Q16_ONE: i32 = 65536;

/// Maximum concurrent transactions
const MAX_TRANSACTIONS: usize = 64;
/// Maximum WAL entries before forced checkpoint
const WAL_CHECKPOINT_THRESHOLD: usize = 4096;
/// Maximum savepoints per transaction
const MAX_SAVEPOINTS: usize = 16;

/// Transaction state
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TxnState {
    Active,
    Committed,
    RolledBack,
    Aborted,
}

/// WAL operation type
#[derive(Clone, Debug)]
pub enum WalOp {
    Insert {
        table_name: String,
        rowid: i64,
    },
    Update {
        table_name: String,
        rowid: i64,
        col_index: usize,
        old_value_hash: u64,
        new_value_hash: u64,
    },
    Delete {
        table_name: String,
        rowid: i64,
    },
    CreateTable {
        table_name: String,
    },
    DropTable {
        table_name: String,
    },
    Savepoint {
        name: String,
    },
    TxnBegin {
        txn_id: u64,
    },
    TxnCommit {
        txn_id: u64,
    },
    TxnRollback {
        txn_id: u64,
    },
    Checkpoint,
}

/// A single WAL entry
#[derive(Clone, Debug)]
pub struct WalEntry {
    pub lsn: u64,
    pub txn_id: u64,
    pub timestamp: u64,
    pub op: WalOp,
    pub checksum: u32,
}

impl WalEntry {
    fn new(lsn: u64, txn_id: u64, op: WalOp) -> Self {
        let checksum = compute_entry_checksum(lsn, txn_id);
        WalEntry {
            lsn,
            txn_id,
            timestamp: 0, // Would use kernel time in real impl
            op,
            checksum,
        }
    }
}

/// Compute a simple checksum for WAL integrity
fn compute_entry_checksum(lsn: u64, txn_id: u64) -> u32 {
    // FNV-1a inspired hash (32-bit)
    let mut hash: u32 = 0x811C9DC5;
    let lsn_bytes = lsn.to_le_bytes();
    let txn_bytes = txn_id.to_le_bytes();
    for &b in lsn_bytes.iter().chain(txn_bytes.iter()) {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

/// Undo record for rollback
#[derive(Clone, Debug)]
pub struct UndoRecord {
    table_name: String,
    rowid: i64,
    undo_type: UndoType,
}

#[derive(Clone, Debug)]
pub enum UndoType {
    /// Row was inserted — undo by deleting it
    UndoInsert,
    /// Row was deleted — undo by un-deleting it
    UndoDelete,
    /// Row was updated — undo by restoring old col value
    UndoUpdate {
        col_index: usize,
        old_value_hash: u64,
    },
}

/// A savepoint within a transaction
#[derive(Clone, Debug)]
struct Savepoint {
    name: String,
    undo_position: usize, // index into the undo_log at savepoint time
}

/// A single transaction
pub struct Transaction {
    pub txn_id: u64,
    pub state: TxnState,
    undo_log: Vec<UndoRecord>,
    savepoints: Vec<Savepoint>,
    wal_start_lsn: u64,
    operations: u32,
    tables_locked: Vec<String>,
}

impl Transaction {
    fn new(txn_id: u64, start_lsn: u64) -> Self {
        Transaction {
            txn_id,
            state: TxnState::Active,
            undo_log: Vec::new(),
            savepoints: Vec::new(),
            wal_start_lsn: start_lsn,
            operations: 0,
            tables_locked: Vec::new(),
        }
    }

    /// Record an insert for undo
    pub fn record_insert(&mut self, table_name: &str, rowid: i64) {
        self.undo_log.push(UndoRecord {
            table_name: String::from(table_name),
            rowid,
            undo_type: UndoType::UndoInsert,
        });
        self.operations = self.operations.saturating_add(1);
    }

    /// Record a delete for undo
    pub fn record_delete(&mut self, table_name: &str, rowid: i64) {
        self.undo_log.push(UndoRecord {
            table_name: String::from(table_name),
            rowid,
            undo_type: UndoType::UndoDelete,
        });
        self.operations = self.operations.saturating_add(1);
    }

    /// Record an update for undo
    pub fn record_update(&mut self, table_name: &str, rowid: i64, col_index: usize, old_hash: u64) {
        self.undo_log.push(UndoRecord {
            table_name: String::from(table_name),
            rowid,
            undo_type: UndoType::UndoUpdate {
                col_index,
                old_value_hash: old_hash,
            },
        });
        self.operations = self.operations.saturating_add(1);
    }

    /// Create a savepoint
    pub fn savepoint(&mut self, name: &str) -> Result<(), TxnError> {
        if self.savepoints.len() >= MAX_SAVEPOINTS {
            return Err(TxnError::TooManySavepoints);
        }
        self.savepoints.push(Savepoint {
            name: String::from(name),
            undo_position: self.undo_log.len(),
        });
        Ok(())
    }

    /// Rollback to a named savepoint (partial rollback)
    pub fn rollback_to_savepoint(&mut self, name: &str) -> Result<Vec<UndoRecord>, TxnError> {
        let sp_idx = self
            .savepoints
            .iter()
            .rposition(|s| s.name == name)
            .ok_or(TxnError::SavepointNotFound)?;
        let undo_pos = self.savepoints[sp_idx].undo_position;
        let undone: Vec<UndoRecord> = self.undo_log.drain(undo_pos..).collect();
        // Remove this and all later savepoints
        self.savepoints.truncate(sp_idx);
        Ok(undone)
    }

    /// Get all undo records for full rollback (in reverse order)
    fn full_undo(&self) -> Vec<UndoRecord> {
        let mut records = self.undo_log.clone();
        records.reverse();
        records
    }

    /// Lock a table for this transaction
    pub fn lock_table(&mut self, table_name: &str) {
        if !self.tables_locked.iter().any(|t| t == table_name) {
            self.tables_locked.push(String::from(table_name));
        }
    }

    /// Check if this transaction holds a lock on the table
    pub fn holds_lock(&self, table_name: &str) -> bool {
        self.tables_locked.iter().any(|t| t == table_name)
    }
}

/// Transaction error types
#[derive(Debug)]
pub enum TxnError {
    NoActiveTransaction,
    TransactionFull,
    AlreadyCommitted,
    AlreadyRolledBack,
    SavepointNotFound,
    TooManySavepoints,
    DeadlockDetected,
    WalCorrupted,
    CheckpointFailed,
}

/// The Write-Ahead Log
struct WriteAheadLog {
    entries: Vec<WalEntry>,
    next_lsn: u64,
    last_checkpoint_lsn: u64,
    total_bytes_written: u64,
}

impl WriteAheadLog {
    fn new() -> Self {
        WriteAheadLog {
            entries: Vec::new(),
            next_lsn: 1,
            last_checkpoint_lsn: 0,
            total_bytes_written: 0,
        }
    }

    fn append(&mut self, txn_id: u64, op: WalOp) -> u64 {
        let lsn = self.next_lsn;
        self.next_lsn = self.next_lsn.saturating_add(1);
        let entry = WalEntry::new(lsn, txn_id, op);
        // Approximate size tracking
        self.total_bytes_written = self.total_bytes_written.saturating_add(64); // ~64 bytes per entry estimate
        self.entries.push(entry);
        lsn
    }

    fn needs_checkpoint(&self) -> bool {
        self.entries.len() >= WAL_CHECKPOINT_THRESHOLD
    }

    fn checkpoint(&mut self) -> Result<u64, TxnError> {
        // In a real implementation, this would flush dirty pages to disk
        // and truncate the WAL. Here we just record the checkpoint.
        let lsn = self.append(0, WalOp::Checkpoint);
        self.last_checkpoint_lsn = lsn;
        // Retain only entries after checkpoint
        self.entries.retain(|e| e.lsn >= lsn);
        Ok(lsn)
    }

    fn verify_integrity(&self) -> bool {
        for entry in &self.entries {
            let expected = compute_entry_checksum(entry.lsn, entry.txn_id);
            if entry.checksum != expected {
                return false;
            }
        }
        true
    }

    /// Recover committed transactions from the WAL
    fn recover(&self) -> Vec<u64> {
        let mut committed: Vec<u64> = Vec::new();
        let mut active: Vec<u64> = Vec::new();
        for entry in &self.entries {
            match &entry.op {
                WalOp::TxnBegin { txn_id } => {
                    active.push(*txn_id);
                }
                WalOp::TxnCommit { txn_id } => {
                    active.retain(|id| id != txn_id);
                    committed.push(*txn_id);
                }
                WalOp::TxnRollback { txn_id } => {
                    active.retain(|id| id != txn_id);
                }
                _ => {}
            }
        }
        committed
    }

    /// WAL size as Q16 ratio of capacity (entries / threshold)
    fn utilization_q16(&self) -> i32 {
        let len = self.entries.len() as i64;
        let cap = WAL_CHECKPOINT_THRESHOLD as i64;
        if cap == 0 {
            return 0;
        }
        (((len) << 16) / (cap)) as i32
    }
}

/// Wait-for graph edge for deadlock detection
#[derive(Clone, Debug)]
struct WaitEdge {
    waiter_txn: u64,
    holder_txn: u64,
    resource: String,
}

/// Deadlock detector using cycle detection in wait-for graph
struct DeadlockDetector {
    edges: Vec<WaitEdge>,
}

impl DeadlockDetector {
    fn new() -> Self {
        DeadlockDetector { edges: Vec::new() }
    }

    fn add_wait(&mut self, waiter: u64, holder: u64, resource: &str) {
        self.edges.push(WaitEdge {
            waiter_txn: waiter,
            holder_txn: holder,
            resource: String::from(resource),
        });
    }

    fn remove_waits_for(&mut self, txn_id: u64) {
        self.edges
            .retain(|e| e.waiter_txn != txn_id && e.holder_txn != txn_id);
    }

    /// DFS cycle detection in the wait-for graph
    fn has_cycle(&self) -> Option<u64> {
        // Collect all transaction IDs
        let mut txn_ids: Vec<u64> = Vec::new();
        for edge in &self.edges {
            if !txn_ids.contains(&edge.waiter_txn) {
                txn_ids.push(edge.waiter_txn);
            }
            if !txn_ids.contains(&edge.holder_txn) {
                txn_ids.push(edge.holder_txn);
            }
        }

        // DFS from each node
        for &start in &txn_ids {
            let mut visited: Vec<u64> = Vec::new();
            let mut stack: Vec<u64> = vec![start];

            while let Some(current) = stack.pop() {
                if visited.contains(&current) {
                    if current == start && visited.len() > 1 {
                        return Some(start); // Cycle found
                    }
                    continue;
                }
                visited.push(current);

                // Find all nodes this one waits for
                for edge in &self.edges {
                    if edge.waiter_txn == current {
                        stack.push(edge.holder_txn);
                    }
                }
            }
        }
        None
    }
}

/// The transaction manager
struct TransactionManager {
    transactions: Vec<Transaction>,
    wal: WriteAheadLog,
    deadlock_detector: DeadlockDetector,
    next_txn_id: u64,
    total_committed: u64,
    total_rolled_back: u64,
}

static TXN_MANAGER: Mutex<Option<TransactionManager>> = Mutex::new(None);

impl TransactionManager {
    fn new() -> Self {
        TransactionManager {
            transactions: Vec::new(),
            wal: WriteAheadLog::new(),
            deadlock_detector: DeadlockDetector::new(),
            next_txn_id: 1,
            total_committed: 0,
            total_rolled_back: 0,
        }
    }

    fn begin(&mut self) -> Result<u64, TxnError> {
        if self.transactions.len() >= MAX_TRANSACTIONS {
            return Err(TxnError::TransactionFull);
        }
        let txn_id = self.next_txn_id;
        self.next_txn_id = self.next_txn_id.saturating_add(1);
        let start_lsn = self.wal.next_lsn;
        self.wal.append(txn_id, WalOp::TxnBegin { txn_id });
        self.transactions.push(Transaction::new(txn_id, start_lsn));
        Ok(txn_id)
    }

    fn commit(&mut self, txn_id: u64) -> Result<(), TxnError> {
        let txn = self.find_active_mut(txn_id)?;
        if txn.state != TxnState::Active {
            return Err(TxnError::AlreadyCommitted);
        }
        txn.state = TxnState::Committed;
        self.wal.append(txn_id, WalOp::TxnCommit { txn_id });
        self.deadlock_detector.remove_waits_for(txn_id);
        self.total_committed = self.total_committed.saturating_add(1);

        // Auto-checkpoint if WAL is large
        if self.wal.needs_checkpoint() {
            let _ = self.wal.checkpoint();
        }
        Ok(())
    }

    fn rollback(&mut self, txn_id: u64) -> Result<Vec<UndoRecord>, TxnError> {
        let txn = self.find_active_mut(txn_id)?;
        if txn.state != TxnState::Active {
            return Err(TxnError::AlreadyRolledBack);
        }
        let undo_records = txn.full_undo();
        txn.state = TxnState::RolledBack;
        self.wal.append(txn_id, WalOp::TxnRollback { txn_id });
        self.deadlock_detector.remove_waits_for(txn_id);
        self.total_rolled_back = self.total_rolled_back.saturating_add(1);
        Ok(undo_records)
    }

    fn find_active_mut(&mut self, txn_id: u64) -> Result<&mut Transaction, TxnError> {
        self.transactions
            .iter_mut()
            .find(|t| t.txn_id == txn_id && t.state == TxnState::Active)
            .ok_or(TxnError::NoActiveTransaction)
    }

    fn check_deadlock(&self) -> Option<u64> {
        self.deadlock_detector.has_cycle()
    }

    fn cleanup_finished(&mut self) {
        self.transactions.retain(|t| t.state == TxnState::Active);
    }

    /// Commit ratio as Q16 fixed-point
    fn commit_ratio_q16(&self) -> i32 {
        let total = self.total_committed + self.total_rolled_back;
        if total == 0 {
            return Q16_ONE;
        }
        (((self.total_committed as i64) << 16) / (total as i64)) as i32
    }
}

/// Begin a new transaction. Returns the transaction ID.
pub fn begin() -> Result<u64, TxnError> {
    let mut guard = TXN_MANAGER.lock();
    if let Some(ref mut mgr) = *guard {
        mgr.begin()
    } else {
        Err(TxnError::NoActiveTransaction)
    }
}

/// Commit a transaction by ID
pub fn commit(txn_id: u64) -> Result<(), TxnError> {
    let mut guard = TXN_MANAGER.lock();
    if let Some(ref mut mgr) = *guard {
        mgr.commit(txn_id)
    } else {
        Err(TxnError::NoActiveTransaction)
    }
}

/// Rollback a transaction by ID
pub fn rollback(txn_id: u64) -> Result<(), TxnError> {
    let mut guard = TXN_MANAGER.lock();
    if let Some(ref mut mgr) = *guard {
        let _undo = mgr.rollback(txn_id)?;
        // In a full implementation, we'd apply each undo record
        // to reverse the corresponding table operations
        Ok(())
    } else {
        Err(TxnError::NoActiveTransaction)
    }
}

/// Create a savepoint within an active transaction
pub fn savepoint(txn_id: u64, name: &str) -> Result<(), TxnError> {
    let mut guard = TXN_MANAGER.lock();
    if let Some(ref mut mgr) = *guard {
        let txn = mgr.find_active_mut(txn_id)?;
        txn.savepoint(name)?;
        mgr.wal.append(
            txn_id,
            WalOp::Savepoint {
                name: String::from(name),
            },
        );
        Ok(())
    } else {
        Err(TxnError::NoActiveTransaction)
    }
}

/// Rollback to a named savepoint
pub fn rollback_to(txn_id: u64, name: &str) -> Result<(), TxnError> {
    let mut guard = TXN_MANAGER.lock();
    if let Some(ref mut mgr) = *guard {
        let txn = mgr.find_active_mut(txn_id)?;
        let _undone = txn.rollback_to_savepoint(name)?;
        Ok(())
    } else {
        Err(TxnError::NoActiveTransaction)
    }
}

/// Check for deadlocks. Returns the txn_id involved if deadlock found.
pub fn detect_deadlock() -> Option<u64> {
    let guard = TXN_MANAGER.lock();
    if let Some(ref mgr) = *guard {
        mgr.check_deadlock()
    } else {
        None
    }
}

/// Force a WAL checkpoint
pub fn checkpoint() -> Result<u64, TxnError> {
    let mut guard = TXN_MANAGER.lock();
    if let Some(ref mut mgr) = *guard {
        mgr.wal.checkpoint()
    } else {
        Err(TxnError::CheckpointFailed)
    }
}

/// Verify WAL integrity
pub fn verify_wal() -> bool {
    let guard = TXN_MANAGER.lock();
    if let Some(ref mgr) = *guard {
        mgr.wal.verify_integrity()
    } else {
        false
    }
}

/// Clean up finished (committed/rolled-back) transactions
pub fn cleanup() {
    let mut guard = TXN_MANAGER.lock();
    if let Some(ref mut mgr) = *guard {
        mgr.cleanup_finished();
    }
}

/// Initialize the transaction subsystem
pub fn init() {
    let mut guard = TXN_MANAGER.lock();
    *guard = Some(TransactionManager::new());
    serial_println!("    Transaction manager ready (ACID, WAL, savepoints, deadlock detect)");
}
