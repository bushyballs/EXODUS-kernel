/// Embedded database engine for Genesis
///
/// A bare-metal, SQLite-inspired relational database with:
///   - B-tree indexes for fast key lookup and range scans
///   - Table storage with typed columns (Int, Text, Bool, Blob)
///   - SQL-like query parser (SELECT, INSERT, UPDATE, DELETE)
///   - ACID transactions with write-ahead logging
///   - Page cache with LRU eviction
///
/// All arithmetic uses Q16 fixed-point (no floats).
/// Designed for embedded/OS-level use without std.
///
/// Inspired by: SQLite (architecture), LevelDB (LSM concepts),
/// PostgreSQL (query planning). All code is original.
pub mod btree;
pub mod cache;
pub mod cursor;
pub mod index;
pub mod query;
pub mod replication;
pub mod schema;
pub mod table;
pub mod transaction;
pub mod vacuum;
pub mod wal;

use crate::{serial_print, serial_println};

use crate::sync::Mutex;
use alloc::string::String;

/// Q16 fixed-point: 1.0 = 65536
const Q16_ONE: i32 = 65536;

/// Database engine statistics
struct DbEngineStats {
    tables_created: u32,
    total_rows: u64,
    queries_executed: u64,
    transactions_committed: u32,
    transactions_rolled_back: u32,
    cache_hits: u64,
    cache_misses: u64,
    btree_nodes_allocated: u32,
}

/// Top-level database engine state
struct DbEngine {
    stats: DbEngineStats,
    initialized: bool,
    next_table_id: u32,
    wal_sequence: u64,
}

static ENGINE: Mutex<Option<DbEngine>> = Mutex::new(None);

impl DbEngine {
    fn new() -> Self {
        DbEngine {
            stats: DbEngineStats {
                tables_created: 0,
                total_rows: 0,
                queries_executed: 0,
                transactions_committed: 0,
                transactions_rolled_back: 0,
                cache_hits: 0,
                cache_misses: 0,
                btree_nodes_allocated: 0,
            },
            initialized: true,
            next_table_id: 1,
            wal_sequence: 0,
        }
    }

    fn allocate_table_id(&mut self) -> u32 {
        let id = self.next_table_id;
        self.next_table_id = self.next_table_id.saturating_add(1);
        self.stats.tables_created = self.stats.tables_created.saturating_add(1);
        id
    }

    fn record_query(&mut self) {
        self.stats.queries_executed = self.stats.queries_executed.saturating_add(1);
    }

    fn record_commit(&mut self) {
        self.stats.transactions_committed = self.stats.transactions_committed.saturating_add(1);
    }

    fn record_rollback(&mut self) {
        self.stats.transactions_rolled_back = self.stats.transactions_rolled_back.saturating_add(1);
    }

    fn record_cache_hit(&mut self) {
        self.stats.cache_hits = self.stats.cache_hits.saturating_add(1);
    }

    fn record_cache_miss(&mut self) {
        self.stats.cache_misses = self.stats.cache_misses.saturating_add(1);
    }

    fn next_wal_seq(&mut self) -> u64 {
        let seq = self.wal_sequence;
        self.wal_sequence = self.wal_sequence.saturating_add(1);
        seq
    }

    /// Cache hit ratio as Q16 fixed-point (0..Q16_ONE = 0%..100%)
    fn cache_hit_ratio_q16(&self) -> i32 {
        let total = self.stats.cache_hits + self.stats.cache_misses;
        if total == 0 {
            return 0;
        }
        (((self.stats.cache_hits as i64) << 16) / (total as i64)) as i32
    }
}

/// Get a formatted summary of engine statistics
pub fn engine_summary() -> String {
    let guard = ENGINE.lock();
    if let Some(ref eng) = *guard {
        let ratio = eng.cache_hit_ratio_q16();
        let pct_whole = ((ratio as i64) * 100) >> 16;
        alloc::format!(
            "tables={} rows={} queries={} commits={} rollbacks={} cache_hit={}%",
            eng.stats.tables_created,
            eng.stats.total_rows,
            eng.stats.queries_executed,
            eng.stats.transactions_committed,
            eng.stats.transactions_rolled_back,
            pct_whole,
        )
    } else {
        String::from("database not initialized")
    }
}

/// Initialize the database engine subsystem
pub fn init() {
    cache::init();
    btree::init();
    table::init();
    transaction::init();
    query::init();

    let mut guard = ENGINE.lock();
    *guard = Some(DbEngine::new());

    serial_println!("  Database engine initialized (B-tree, tables, query, txn, cache)");
}
