pub mod deep_links;
/// Search framework for Genesis
///
/// Global search, content indexing, deep links.
pub mod global_search;
pub mod indexer;

/// Initialize the search subsystem
pub fn init() {
    global_search::init();
    indexer::init();
    deep_links::init();
}
