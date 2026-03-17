use crate::sync::Mutex;
/// Global search for Genesis
///
/// Universal search across apps, contacts, messages,
/// files, settings, web, calendar, music.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum SearchDomain {
    Apps,
    Contacts,
    Messages,
    Files,
    Settings,
    Web,
    Calendar,
    Music,
}

#[derive(Clone, Copy)]
struct SearchResult {
    id: u32,
    domain: SearchDomain,
    title_hash: u64,
    snippet_hash: u64,
    relevance_score: u32,
    timestamp: u64,
    icon_hash: u64,
}

struct SearchEngine {
    results: Vec<SearchResult>,
    recent_queries: Vec<u64>,
    max_results: u16,
    search_count: u32,
}

static SEARCH_ENGINE: Mutex<Option<SearchEngine>> = Mutex::new(None);

impl SearchEngine {
    fn new() -> Self {
        SearchEngine {
            results: Vec::new(),
            recent_queries: Vec::new(),
            max_results: 50,
            search_count: 0,
        }
    }

    fn search(&mut self, query_hash: u64, domain: Option<SearchDomain>) -> Vec<u32> {
        self.search_count = self.search_count.saturating_add(1);
        // Add to recent
        if !self.recent_queries.contains(&query_hash) {
            if self.recent_queries.len() >= 20 {
                self.recent_queries.remove(0);
            }
            self.recent_queries.push(query_hash);
        }
        self.results
            .iter()
            .filter(|r| domain.map_or(true, |d| r.domain == d))
            .map(|r| r.id)
            .take(self.max_results as usize)
            .collect()
    }

    fn get_suggestions(&self) -> Vec<u64> {
        self.recent_queries.iter().rev().take(5).copied().collect()
    }

    fn clear_history(&mut self) {
        self.recent_queries.clear();
    }
}

pub fn init() {
    let mut s = SEARCH_ENGINE.lock();
    *s = Some(SearchEngine::new());
    serial_println!("    Global search engine ready");
}
