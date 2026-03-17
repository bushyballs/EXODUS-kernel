use crate::sync::Mutex;
/// Content indexer for Genesis search
///
/// Indexes content across apps, files, messages
/// for fast retrieval by the global search engine.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy)]
struct IndexEntry {
    content_hash: u64,
    domain: u8,
    app_id: u32,
    timestamp: u64,
    word_count: u16,
    language: u8,
}

struct SearchIndex {
    entries: Vec<IndexEntry>,
    total_indexed: u64,
    last_index_time: u64,
    index_size_kb: u32,
}

static INDEXER: Mutex<Option<SearchIndex>> = Mutex::new(None);

impl SearchIndex {
    fn new() -> Self {
        SearchIndex {
            entries: Vec::new(),
            total_indexed: 0,
            last_index_time: 0,
            index_size_kb: 0,
        }
    }

    fn index_content(
        &mut self,
        content_hash: u64,
        domain: u8,
        app_id: u32,
        word_count: u16,
        timestamp: u64,
    ) {
        self.entries.push(IndexEntry {
            content_hash,
            domain,
            app_id,
            timestamp,
            word_count,
            language: 0,
        });
        self.total_indexed = self.total_indexed.saturating_add(1);
        self.last_index_time = timestamp;
        self.index_size_kb += (word_count as u32) / 10 + 1;
    }

    fn remove_from_index(&mut self, content_hash: u64) {
        self.entries.retain(|e| e.content_hash != content_hash);
    }

    fn reindex_all(&mut self) {
        self.last_index_time = 0; // Force full reindex on next pass
    }

    fn prune_old(&mut self, before_timestamp: u64) {
        self.entries.retain(|e| e.timestamp >= before_timestamp);
    }

    fn get_stats(&self) -> (u64, u32) {
        (self.total_indexed, self.index_size_kb)
    }
}

pub fn init() {
    let mut idx = INDEXER.lock();
    *idx = Some(SearchIndex::new());
    serial_println!("    Content indexer ready");
}
