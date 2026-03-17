use crate::sync::Mutex;
/// UI accessibility — screen reader, focus management
///
/// Part of the AIOS UI layer.
use alloc::string::String;
use alloc::vec::Vec;

/// An accessible UI element
pub struct AccessibleNode {
    pub label: String,
    pub role: String,
    pub focusable: bool,
    pub children: Vec<usize>,
    pub actions: Vec<String>,
}

/// Manages UI accessibility tree and focus
pub struct AccessibilityManager {
    pub nodes: Vec<AccessibleNode>,
    pub focus_index: usize,
    pub screen_reader_active: bool,
    announcements: Vec<String>,
}

impl AccessibilityManager {
    pub fn new() -> Self {
        AccessibilityManager {
            nodes: Vec::new(),
            focus_index: 0,
            screen_reader_active: false,
            announcements: Vec::new(),
        }
    }

    /// Register an accessible node and return its index.
    pub fn register_node(&mut self, label: &str, role: &str, focusable: bool) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(AccessibleNode {
            label: String::from(label),
            role: String::from(role),
            focusable,
            children: Vec::new(),
            actions: Vec::new(),
        });
        idx
    }

    /// Remove a node by index, shifting subsequent indices.
    pub fn remove_node(&mut self, index: usize) {
        if index < self.nodes.len() {
            self.nodes.remove(index);
            // Fix focus index if it was pointing at or past the removed node
            if self.focus_index >= self.nodes.len() && !self.nodes.is_empty() {
                self.focus_index = self.nodes.len() - 1;
            }
        }
    }

    /// Move keyboard/screen-reader focus forward or backward among focusable nodes.
    pub fn move_focus(&mut self, forward: bool) {
        if self.nodes.is_empty() {
            return;
        }

        let count = self.nodes.len();
        let mut idx = self.focus_index;
        // Walk through nodes to find the next focusable one
        for _ in 0..count {
            if forward {
                idx = (idx + 1) % count;
            } else {
                idx = if idx == 0 { count - 1 } else { idx - 1 };
            }
            if self.nodes[idx].focusable {
                self.focus_index = idx;
                if self.screen_reader_active {
                    let label = self.nodes[idx].label.clone();
                    let role = self.nodes[idx].role.clone();
                    crate::serial_println!("  [a11y] focus -> [{}] {} ({})", idx, label, role);
                }
                return;
            }
        }
        // No focusable node found; stay where we are
    }

    /// Get the currently focused node, if any.
    pub fn focused_node(&self) -> Option<&AccessibleNode> {
        self.nodes.get(self.focus_index).filter(|n| n.focusable)
    }

    /// Announce text through the screen reader.
    pub fn announce(&self, text: &str) {
        if self.screen_reader_active {
            crate::serial_println!("  [a11y] announce: {}", text);
        }
    }

    /// Queue an announcement for later delivery.
    pub fn queue_announcement(&mut self, text: &str) {
        self.announcements.push(String::from(text));
    }

    /// Flush and deliver all queued announcements.
    pub fn flush_announcements(&mut self) {
        if self.screen_reader_active {
            for msg in self.announcements.drain(..) {
                crate::serial_println!("  [a11y] announce: {}", msg);
            }
        } else {
            self.announcements.clear();
        }
    }

    /// Enable or disable the screen reader.
    pub fn set_screen_reader(&mut self, active: bool) {
        self.screen_reader_active = active;
        crate::serial_println!(
            "  [a11y] screen reader {}",
            if active { "enabled" } else { "disabled" }
        );
    }

    /// Total number of registered accessible nodes.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

static A11Y_MANAGER: Mutex<Option<AccessibilityManager>> = Mutex::new(None);

pub fn init() {
    *A11Y_MANAGER.lock() = Some(AccessibilityManager::new());
    crate::serial_println!("  [a11y] Accessibility manager initialized");
}

/// Move focus globally.
pub fn move_focus(forward: bool) {
    if let Some(ref mut mgr) = *A11Y_MANAGER.lock() {
        mgr.move_focus(forward);
    }
}

/// Global announcement.
pub fn announce(text: &str) {
    if let Some(ref mgr) = *A11Y_MANAGER.lock() {
        mgr.announce(text);
    }
}
