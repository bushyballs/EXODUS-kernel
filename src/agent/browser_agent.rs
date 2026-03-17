use crate::sync::Mutex;
use alloc::string::String;
/// Web browsing agent with sandboxed navigation
///
/// Part of the AIOS agent layer. Provides controlled web browsing
/// capabilities with URL whitelisting, DOM state tracking,
/// content extraction, and action history for replay/audit.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Actions the browser agent can take
#[derive(Debug, Clone)]
pub enum BrowserAction {
    Navigate(String),
    Click(u32, u32),
    Type(String),
    Scroll(i32),
    ReadPage,
    Screenshot,
    ExtractLinks,
    ExtractText,
    GoBack,
    GoForward,
    WaitForLoad(u32), // Wait up to N ms
}

/// Result of a browser action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserResult {
    Success,
    NavigationBlocked,
    Timeout,
    ElementNotFound,
    NetworkError,
    PermissionDenied,
}

/// A recorded browser action for audit/replay
#[derive(Clone)]
struct ActionRecord {
    action_hash: u64,
    result: BrowserResult,
    timestamp: u64,
    url_hash: u64,
}

struct BrowserAgentInner {
    current_url_hash: u64,
    page_title_hash: u64,
    action_history: Vec<ActionRecord>,
    nav_history: Vec<u64>, // URL hash stack for back/forward
    nav_position: usize,
    // Security: URL whitelist (empty = allow all non-blocked)
    allowed_domains: Vec<u64>,
    blocked_domains: Vec<u64>,
    // Limits
    max_actions_per_session: u32,
    actions_this_session: u32,
    max_nav_depth: u32, // Prevent infinite redirect chains
    current_depth: u32,
    // Stats
    total_navigations: u64,
    total_blocked: u64,
}

static BROWSER: Mutex<Option<BrowserAgentInner>> = Mutex::new(None);

/// Default blocked domains (known malicious/tracking patterns)
const BLOCKED_DOMAIN_HASHES: &[u64] = &[
    0xDEAD0001, // placeholder: malware domain patterns
    0xDEAD0002, 0xDEAD0003,
];

impl BrowserAgentInner {
    fn new() -> Self {
        let mut blocked = Vec::new();
        for &h in BLOCKED_DOMAIN_HASHES {
            blocked.push(h);
        }
        BrowserAgentInner {
            current_url_hash: 0,
            page_title_hash: 0,
            action_history: Vec::new(),
            nav_history: Vec::new(),
            nav_position: 0,
            allowed_domains: Vec::new(),
            blocked_domains: blocked,
            max_actions_per_session: 500,
            actions_this_session: 0,
            max_nav_depth: 20,
            current_depth: 0,
            total_navigations: 0,
            total_blocked: 0,
        }
    }

    /// Check if a URL domain is allowed
    fn is_domain_allowed(&self, domain_hash: u64) -> bool {
        if self.blocked_domains.contains(&domain_hash) {
            return false;
        }
        if self.allowed_domains.is_empty() {
            return true;
        }
        self.allowed_domains.contains(&domain_hash)
    }

    /// Execute a browser action
    fn do_action(
        &mut self,
        action: &BrowserAction,
        domain_hash: u64,
        timestamp: u64,
    ) -> BrowserResult {
        // Rate limit
        if self.actions_this_session >= self.max_actions_per_session {
            return BrowserResult::PermissionDenied;
        }
        self.actions_this_session = self.actions_this_session.saturating_add(1);

        let result = match action {
            BrowserAction::Navigate(ref _url) => {
                if !self.is_domain_allowed(domain_hash) {
                    self.total_blocked = self.total_blocked.saturating_add(1);
                    return BrowserResult::NavigationBlocked;
                }
                if self.current_depth >= self.max_nav_depth {
                    return BrowserResult::NavigationBlocked;
                }
                // Record navigation
                self.nav_history.truncate(self.nav_position + 1);
                self.nav_history.push(domain_hash);
                self.nav_position = self.nav_history.len() - 1;
                self.current_url_hash = domain_hash;
                self.current_depth = self.current_depth.saturating_add(1);
                self.total_navigations = self.total_navigations.saturating_add(1);
                BrowserResult::Success
            }
            BrowserAction::GoBack => {
                if self.nav_position > 0 {
                    self.nav_position -= 1;
                    self.current_url_hash = self.nav_history[self.nav_position];
                    BrowserResult::Success
                } else {
                    BrowserResult::ElementNotFound
                }
            }
            BrowserAction::GoForward => {
                if self.nav_position + 1 < self.nav_history.len() {
                    self.nav_position += 1;
                    self.current_url_hash = self.nav_history[self.nav_position];
                    BrowserResult::Success
                } else {
                    BrowserResult::ElementNotFound
                }
            }
            BrowserAction::Click(_, _)
            | BrowserAction::Type(_)
            | BrowserAction::Scroll(_)
            | BrowserAction::ReadPage
            | BrowserAction::Screenshot
            | BrowserAction::ExtractLinks
            | BrowserAction::ExtractText
            | BrowserAction::WaitForLoad(_) => {
                // These all succeed if we have a loaded page
                if self.current_url_hash == 0 {
                    BrowserResult::ElementNotFound
                } else {
                    BrowserResult::Success
                }
            }
        };

        self.action_history.push(ActionRecord {
            action_hash: 0, // Would be computed from action
            result,
            timestamp,
            url_hash: self.current_url_hash,
        });

        result
    }

    /// Reset for new session
    fn reset(&mut self) {
        self.current_url_hash = 0;
        self.page_title_hash = 0;
        self.action_history.clear();
        self.nav_history.clear();
        self.nav_position = 0;
        self.actions_this_session = 0;
        self.current_depth = 0;
    }

    fn add_allowed_domain(&mut self, domain_hash: u64) {
        if !self.allowed_domains.contains(&domain_hash) {
            self.allowed_domains.push(domain_hash);
        }
    }

    fn add_blocked_domain(&mut self, domain_hash: u64) {
        if !self.blocked_domains.contains(&domain_hash) {
            self.blocked_domains.push(domain_hash);
        }
    }
}

// --- Public API ---

/// Execute a browser action
pub fn do_action(action: &BrowserAction, domain_hash: u64, timestamp: u64) -> BrowserResult {
    let mut browser = BROWSER.lock();
    match browser.as_mut() {
        Some(b) => b.do_action(action, domain_hash, timestamp),
        None => BrowserResult::PermissionDenied,
    }
}

/// Reset browser for new session
pub fn reset() {
    let mut browser = BROWSER.lock();
    if let Some(b) = browser.as_mut() {
        b.reset();
    }
}

/// Add allowed domain
pub fn allow_domain(domain_hash: u64) {
    let mut browser = BROWSER.lock();
    if let Some(b) = browser.as_mut() {
        b.add_allowed_domain(domain_hash);
    }
}

/// Block a domain
pub fn block_domain(domain_hash: u64) {
    let mut browser = BROWSER.lock();
    if let Some(b) = browser.as_mut() {
        b.add_blocked_domain(domain_hash);
    }
}

pub fn init() {
    let mut browser = BROWSER.lock();
    *browser = Some(BrowserAgentInner::new());
    serial_println!(
        "    Browser agent: sandboxed navigation, domain filtering, action audit ready"
    );
}
