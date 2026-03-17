use crate::sync::Mutex;
/// Tab manager for Genesis browser
///
/// Manages multiple browser tabs with navigation history,
/// URL tracking, and lifecycle management. Maximum 20 tabs.
/// Each tab owns its own DOM root and navigation stack.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

static TAB_MANAGER: Mutex<Option<TabManager>> = Mutex::new(None);

/// Maximum number of open tabs
const MAX_TABS: usize = 20;

/// Maximum history entries per tab
const MAX_HISTORY: usize = 50;

/// FNV-1a hash for URLs and titles
fn tab_hash(s: &[u8]) -> u64 {
    let mut h: u64 = 0xCBF29CE484222325;
    for &b in s {
        h ^= b as u64;
        h = h.wrapping_mul(0x00000100000001B3);
    }
    h
}

/// Loading state of a tab
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadState {
    Idle,
    Loading,
    Complete,
    Error,
}

/// A history entry in a tab's navigation stack
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub url_hash: u64,
    pub title_hash: u64,
    pub scroll_y: i32, // Q16 scroll position
}

/// A single browser tab
#[derive(Debug, Clone)]
pub struct Tab {
    pub id: u32,
    pub url_hash: u64,
    pub title_hash: u64,
    pub dom_root: u32, // DOM node ID for this tab's document root
    pub active: bool,
    pub loading: LoadState,
    pub history: Vec<HistoryEntry>,
    pub history_index: usize, // current position in history
    pub scroll_y: i32,        // Q16 current scroll position
    pub pinned: bool,
}

impl Tab {
    fn new(id: u32) -> Self {
        Tab {
            id,
            url_hash: 0,
            title_hash: 0,
            dom_root: 0,
            active: false,
            loading: LoadState::Idle,
            history: Vec::new(),
            history_index: 0,
            scroll_y: 0,
            pinned: false,
        }
    }
}

/// The tab manager state
struct TabManager {
    tabs: Vec<Tab>,
    next_id: u32,
    active_tab_id: Option<u32>,
    total_tabs_created: u64,
    total_navigations: u64,
}

impl TabManager {
    fn find_tab(&self, id: u32) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    fn active_index(&self) -> Option<usize> {
        self.active_tab_id.and_then(|id| self.find_tab(id))
    }
}

/// Open a new tab. Returns the tab ID, or None if at max capacity.
pub fn new_tab() -> Option<u32> {
    let mut guard = TAB_MANAGER.lock();
    let mgr = guard.as_mut()?;

    if mgr.tabs.len() >= MAX_TABS {
        return None;
    }

    let id = mgr.next_id;
    mgr.next_id = mgr.next_id.saturating_add(1);
    mgr.total_tabs_created = mgr.total_tabs_created.saturating_add(1);

    // Deactivate current tab
    if let Some(idx) = mgr.active_index() {
        mgr.tabs[idx].active = false;
    }

    let mut tab = Tab::new(id);
    tab.active = true;
    // Create a blank page entry
    tab.history.push(HistoryEntry {
        url_hash: tab_hash(b"about:blank"),
        title_hash: tab_hash(b"New Tab"),
        scroll_y: 0,
    });
    tab.history_index = 0;
    tab.url_hash = tab_hash(b"about:blank");
    tab.title_hash = tab_hash(b"New Tab");

    mgr.tabs.push(tab);
    mgr.active_tab_id = Some(id);

    Some(id)
}

/// Close a tab by ID. Returns true if closed. Cannot close the last tab.
pub fn close_tab(id: u32) -> bool {
    let mut guard = TAB_MANAGER.lock();
    let mgr = guard.as_mut().unwrap();

    // Don't close the last tab
    if mgr.tabs.len() <= 1 {
        return false;
    }

    let idx = match mgr.find_tab(id) {
        Some(i) => i,
        None => return false,
    };

    // Don't close pinned tabs without explicit unpin
    if mgr.tabs[idx].pinned {
        return false;
    }

    let was_active = mgr.tabs[idx].active;
    mgr.tabs.remove(idx);

    if was_active {
        // Activate the nearest tab
        let new_idx = if idx < mgr.tabs.len() {
            idx
        } else {
            mgr.tabs.len() - 1
        };
        mgr.tabs[new_idx].active = true;
        mgr.active_tab_id = Some(mgr.tabs[new_idx].id);
    }

    true
}

/// Switch to a tab by ID. Returns true if switched.
pub fn switch_tab(id: u32) -> bool {
    let mut guard = TAB_MANAGER.lock();
    let mgr = guard.as_mut().unwrap();

    let target = match mgr.find_tab(id) {
        Some(i) => i,
        None => return false,
    };

    // Deactivate current
    if let Some(idx) = mgr.active_index() {
        mgr.tabs[idx].active = false;
    }

    mgr.tabs[target].active = true;
    mgr.active_tab_id = Some(id);
    true
}

/// Navigate the active tab to a new URL (as bytes)
pub fn navigate(url: &[u8]) -> bool {
    let mut guard = TAB_MANAGER.lock();
    let mgr = guard.as_mut().unwrap();

    let idx = match mgr.active_index() {
        Some(i) => i,
        None => return false,
    };

    mgr.total_navigations = mgr.total_navigations.saturating_add(1);
    let url_h = tab_hash(url);

    // Truncate forward history if we navigated back previously
    let current_pos = mgr.tabs[idx].history_index;
    if current_pos + 1 < mgr.tabs[idx].history.len() {
        mgr.tabs[idx].history.truncate(current_pos + 1);
    }

    // Enforce history limit
    if mgr.tabs[idx].history.len() >= MAX_HISTORY {
        mgr.tabs[idx].history.remove(0);
    }

    mgr.tabs[idx].history.push(HistoryEntry {
        url_hash: url_h,
        title_hash: 0, // title updated after page load
        scroll_y: 0,
    });
    mgr.tabs[idx].history_index = mgr.tabs[idx].history.len() - 1;
    mgr.tabs[idx].url_hash = url_h;
    mgr.tabs[idx].title_hash = 0;
    mgr.tabs[idx].loading = LoadState::Loading;
    mgr.tabs[idx].scroll_y = 0;

    true
}

/// Go back in the active tab's history. Returns true if successful.
pub fn go_back() -> bool {
    let mut guard = TAB_MANAGER.lock();
    let mgr = guard.as_mut().unwrap();

    let idx = match mgr.active_index() {
        Some(i) => i,
        None => return false,
    };

    if mgr.tabs[idx].history_index == 0 {
        return false;
    }

    // Save current scroll position
    let pos = mgr.tabs[idx].history_index;
    mgr.tabs[idx].history[pos].scroll_y = mgr.tabs[idx].scroll_y;

    mgr.tabs[idx].history_index -= 1;
    let entry = mgr.tabs[idx].history[mgr.tabs[idx].history_index].clone();
    mgr.tabs[idx].url_hash = entry.url_hash;
    mgr.tabs[idx].title_hash = entry.title_hash;
    mgr.tabs[idx].scroll_y = entry.scroll_y;
    mgr.tabs[idx].loading = LoadState::Loading;

    true
}

/// Go forward in the active tab's history. Returns true if successful.
pub fn go_forward() -> bool {
    let mut guard = TAB_MANAGER.lock();
    let mgr = guard.as_mut().unwrap();

    let idx = match mgr.active_index() {
        Some(i) => i,
        None => return false,
    };

    if mgr.tabs[idx].history_index + 1 >= mgr.tabs[idx].history.len() {
        return false;
    }

    // Save current scroll position
    let pos = mgr.tabs[idx].history_index;
    mgr.tabs[idx].history[pos].scroll_y = mgr.tabs[idx].scroll_y;

    mgr.tabs[idx].history_index += 1;
    let entry = mgr.tabs[idx].history[mgr.tabs[idx].history_index].clone();
    mgr.tabs[idx].url_hash = entry.url_hash;
    mgr.tabs[idx].title_hash = entry.title_hash;
    mgr.tabs[idx].scroll_y = entry.scroll_y;
    mgr.tabs[idx].loading = LoadState::Loading;

    true
}

/// Refresh the current page (reload from same URL)
pub fn refresh() -> bool {
    let mut guard = TAB_MANAGER.lock();
    let mgr = guard.as_mut().unwrap();

    let idx = match mgr.active_index() {
        Some(i) => i,
        None => return false,
    };

    mgr.tabs[idx].loading = LoadState::Loading;
    mgr.tabs[idx].scroll_y = 0;
    mgr.total_navigations = mgr.total_navigations.saturating_add(1);
    true
}

/// Mark the active tab as done loading with a title
pub fn finish_loading(title: &[u8]) -> bool {
    let mut guard = TAB_MANAGER.lock();
    let mgr = guard.as_mut().unwrap();

    let idx = match mgr.active_index() {
        Some(i) => i,
        None => return false,
    };

    let title_h = tab_hash(title);
    mgr.tabs[idx].loading = LoadState::Complete;
    mgr.tabs[idx].title_hash = title_h;

    // Update history entry title
    let pos = mgr.tabs[idx].history_index;
    if pos < mgr.tabs[idx].history.len() {
        mgr.tabs[idx].history[pos].title_hash = title_h;
    }

    true
}

/// Pin or unpin a tab
pub fn set_pinned(id: u32, pinned: bool) -> bool {
    let mut guard = TAB_MANAGER.lock();
    let mgr = guard.as_mut().unwrap();
    if let Some(idx) = mgr.find_tab(id) {
        mgr.tabs[idx].pinned = pinned;
        true
    } else {
        false
    }
}

/// Set the DOM root node ID for a tab
pub fn set_dom_root(id: u32, dom_root: u32) -> bool {
    let mut guard = TAB_MANAGER.lock();
    let mgr = guard.as_mut().unwrap();
    if let Some(idx) = mgr.find_tab(id) {
        mgr.tabs[idx].dom_root = dom_root;
        true
    } else {
        false
    }
}

/// Get the active tab's ID
pub fn active_tab_id() -> Option<u32> {
    let guard = TAB_MANAGER.lock();
    guard.as_ref().and_then(|mgr| mgr.active_tab_id)
}

/// Get a snapshot of a tab by ID
pub fn get_tab(id: u32) -> Option<Tab> {
    let guard = TAB_MANAGER.lock();
    let mgr = guard.as_ref()?;
    mgr.find_tab(id).map(|idx| mgr.tabs[idx].clone())
}

/// Get all tab IDs in order
pub fn list_tabs() -> Vec<u32> {
    let guard = TAB_MANAGER.lock();
    match guard.as_ref() {
        Some(mgr) => mgr.tabs.iter().map(|t| t.id).collect(),
        None => Vec::new(),
    }
}

/// Get the total number of open tabs
pub fn tab_count() -> usize {
    let guard = TAB_MANAGER.lock();
    match guard.as_ref() {
        Some(mgr) => mgr.tabs.len(),
        None => 0,
    }
}

pub fn init() {
    let mut guard = TAB_MANAGER.lock();
    *guard = Some(TabManager {
        tabs: Vec::new(),
        next_id: 1,
        active_tab_id: None,
        total_tabs_created: 0,
        total_navigations: 0,
    });
    serial_println!("    browser::tabs initialized");
}
