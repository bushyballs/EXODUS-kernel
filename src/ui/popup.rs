use crate::sync::Mutex;
/// Popup/dialog management
///
/// Part of the AIOS UI layer.
use alloc::string::String;
use alloc::vec::Vec;

/// Popup dialog type
#[derive(Debug, Clone, Copy)]
pub enum PopupType {
    Alert,
    Confirm,
    Prompt,
    Custom,
}

/// A popup dialog instance
pub struct Popup {
    pub popup_type: PopupType,
    pub title: String,
    pub message: String,
    pub visible: bool,
    pub result: Option<bool>,
}

impl Popup {
    /// Create a new popup dialog.
    pub fn new(popup_type: PopupType, title: &str, message: &str) -> Self {
        Popup {
            popup_type,
            title: String::from(title),
            message: String::from(message),
            visible: true,
            result: None,
        }
    }

    /// Create an alert popup (informational, single OK button).
    pub fn alert(title: &str, message: &str) -> Self {
        Self::new(PopupType::Alert, title, message)
    }

    /// Create a confirmation popup (OK/Cancel).
    pub fn confirm(title: &str, message: &str) -> Self {
        Self::new(PopupType::Confirm, title, message)
    }
}

/// Manages popup display and lifecycle.
///
/// Popups are stacked: only the topmost popup receives input.
/// Dismissing the top popup reveals the one below it.
pub struct PopupManager {
    pub stack: Vec<Popup>,
}

impl PopupManager {
    pub fn new() -> Self {
        PopupManager { stack: Vec::new() }
    }

    /// Push a popup onto the stack and display it.
    pub fn show(&mut self, popup: Popup) {
        let type_str = match popup.popup_type {
            PopupType::Alert => "alert",
            PopupType::Confirm => "confirm",
            PopupType::Prompt => "prompt",
            PopupType::Custom => "custom",
        };
        crate::serial_println!("  [popup] show {} \"{}\"", type_str, popup.title);
        self.stack.push(popup);
    }

    /// Dismiss the topmost popup and return it.
    pub fn dismiss_top(&mut self) -> Option<Popup> {
        let popup = self.stack.pop();
        if let Some(ref p) = popup {
            crate::serial_println!("  [popup] dismissed \"{}\"", p.title);
        }
        popup
    }

    /// Dismiss all popups at once.
    pub fn dismiss_all(&mut self) {
        let count = self.stack.len();
        self.stack.clear();
        if count > 0 {
            crate::serial_println!("  [popup] dismissed all ({} popups)", count);
        }
    }

    /// Get a reference to the topmost popup, if any.
    pub fn top(&self) -> Option<&Popup> {
        self.stack.last()
    }

    /// Get a mutable reference to the topmost popup.
    pub fn top_mut(&mut self) -> Option<&mut Popup> {
        self.stack.last_mut()
    }

    /// Check if any popups are currently displayed.
    pub fn has_popups(&self) -> bool {
        !self.stack.is_empty()
    }

    /// Number of popups on the stack.
    pub fn count(&self) -> usize {
        self.stack.len()
    }

    /// Accept the topmost popup (set result to true) and dismiss it.
    pub fn accept_top(&mut self) -> Option<Popup> {
        if let Some(p) = self.stack.last_mut() {
            p.result = Some(true);
        }
        self.dismiss_top()
    }

    /// Reject the topmost popup (set result to false) and dismiss it.
    pub fn reject_top(&mut self) -> Option<Popup> {
        if let Some(p) = self.stack.last_mut() {
            p.result = Some(false);
        }
        self.dismiss_top()
    }
}

static POPUP_MANAGER: Mutex<Option<PopupManager>> = Mutex::new(None);

pub fn init() {
    *POPUP_MANAGER.lock() = Some(PopupManager::new());
    crate::serial_println!("  [popup] Popup manager initialized");
}

/// Show a global popup.
pub fn show(popup: Popup) {
    if let Some(ref mut mgr) = *POPUP_MANAGER.lock() {
        mgr.show(popup);
    }
}

/// Dismiss the topmost global popup.
pub fn dismiss_top() -> Option<Popup> {
    match POPUP_MANAGER.lock().as_mut() {
        Some(mgr) => mgr.dismiss_top(),
        None => None,
    }
}
