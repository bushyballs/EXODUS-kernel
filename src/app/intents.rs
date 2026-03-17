/// Intent system for Genesis — inter-app communication
///
/// Apps communicate through intents — structured messages that request
/// actions from other apps. Supports explicit intents (target app known)
/// and implicit intents (system finds the right handler).
///
/// Inspired by: Android Intents, iOS URL schemes, D-Bus. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Standard intent actions
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    View,
    Edit,
    Share,
    Send,
    Pick,
    Create,
    Delete,
    Search,
    Open,
    Settings,
    Custom(String),
}

/// Intent data types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataType {
    Text,
    Image,
    Video,
    Audio,
    Pdf,
    Uri,
    Contact,
    Location,
    Custom(String),
}

/// An intent — a message requesting an action
pub struct Intent {
    pub action: Action,
    pub data_type: Option<DataType>,
    pub data: Vec<u8>,
    pub extras: BTreeMap<String, String>,
    pub source_app: String,
    pub target_app: Option<String>, // None = implicit
    pub category: Option<String>,
    pub flags: u32,
}

/// Intent filter — declares what intents an app can handle
pub struct IntentFilter {
    pub app_id: String,
    pub actions: Vec<Action>,
    pub data_types: Vec<DataType>,
    pub categories: Vec<String>,
    pub priority: i8,
}

/// Intent resolution result
pub struct ResolveResult {
    pub app_id: String,
    pub priority: i8,
}

/// Intent manager
pub struct IntentManager {
    filters: Vec<IntentFilter>,
    /// Pending intent results (for startActivityForResult pattern)
    pending_results: BTreeMap<u32, Vec<u8>>,
    next_request_id: u32,
}

impl IntentManager {
    const fn new() -> Self {
        IntentManager {
            filters: Vec::new(),
            pending_results: BTreeMap::new(),
            next_request_id: 1,
        }
    }

    /// Register an intent filter for an app
    pub fn register_filter(&mut self, filter: IntentFilter) {
        self.filters.push(filter);
    }

    /// Resolve an intent to matching apps
    pub fn resolve(&self, intent: &Intent) -> Vec<ResolveResult> {
        // Explicit intent — target app specified
        if let Some(ref target) = intent.target_app {
            return alloc::vec![ResolveResult {
                app_id: target.clone(),
                priority: 0,
            }];
        }

        // Implicit intent — find matching filters
        let mut results = Vec::new();
        for filter in &self.filters {
            // Check action match
            let action_match = filter.actions.contains(&intent.action);
            if !action_match {
                continue;
            }

            // Check data type match
            let type_match = match (&intent.data_type, filter.data_types.is_empty()) {
                (None, true) => true,
                (Some(dt), false) => filter.data_types.contains(dt),
                (None, false) => false,
                (Some(_), true) => true,
            };
            if !type_match {
                continue;
            }

            // Check category
            let cat_match = match (&intent.category, filter.categories.is_empty()) {
                (None, _) => true,
                (Some(cat), false) => filter.categories.contains(cat),
                (Some(_), true) => true,
            };
            if !cat_match {
                continue;
            }

            results.push(ResolveResult {
                app_id: filter.app_id.clone(),
                priority: filter.priority,
            });
        }

        // Sort by priority (highest first)
        results.sort_by(|a, b| b.priority.cmp(&a.priority));
        results
    }

    /// Send an intent
    pub fn send(&mut self, intent: Intent) -> Option<u32> {
        let results = self.resolve(&intent);
        if results.is_empty() {
            return None;
        }

        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);

        // In a real system, this would launch the target app
        // with the intent data. For now, just return the request ID.
        Some(request_id)
    }

    /// Set result for a pending intent
    pub fn set_result(&mut self, request_id: u32, data: Vec<u8>) {
        self.pending_results.insert(request_id, data);
    }

    /// Get result for a pending intent
    pub fn get_result(&mut self, request_id: u32) -> Option<Vec<u8>> {
        self.pending_results.remove(&request_id)
    }
}

static INTENT_MANAGER: Mutex<IntentManager> = Mutex::new(IntentManager::new());

pub fn init() {
    crate::serial_println!("  [intents] Intent system initialized");
}

pub fn register_filter(filter: IntentFilter) {
    INTENT_MANAGER.lock().register_filter(filter);
}

pub fn send(intent: Intent) -> Option<u32> {
    INTENT_MANAGER.lock().send(intent)
}
