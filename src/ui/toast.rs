use crate::sync::Mutex;
/// Toast notification display
///
/// Part of the AIOS UI layer.
use alloc::string::String;
use alloc::vec::Vec;

/// Toast severity level
#[derive(Debug, Clone, Copy)]
pub enum ToastLevel {
    Info,
    Success,
    Warning,
    Error,
}

/// A toast notification
pub struct Toast {
    pub message: String,
    pub level: ToastLevel,
    pub duration_ms: u32,
    pub remaining_ms: u32,
}

/// Default toast display duration in milliseconds
const DEFAULT_TOAST_DURATION_MS: u32 = 3000;

/// Manages toast notification queue and display
pub struct ToastManager {
    pub queue: Vec<Toast>,
    pub max_visible: usize,
}

impl ToastManager {
    pub fn new(max_visible: usize) -> Self {
        ToastManager {
            queue: Vec::new(),
            max_visible,
        }
    }

    /// Show a toast with the given message and severity level.
    ///
    /// The toast is added to the queue with the default duration.
    pub fn show(&mut self, message: &str, level: ToastLevel) {
        self.show_with_duration(message, level, DEFAULT_TOAST_DURATION_MS);
    }

    /// Show a toast with a custom duration.
    pub fn show_with_duration(&mut self, message: &str, level: ToastLevel, duration_ms: u32) {
        let level_str = match level {
            ToastLevel::Info => "INFO",
            ToastLevel::Success => "OK",
            ToastLevel::Warning => "WARN",
            ToastLevel::Error => "ERR",
        };

        crate::serial_println!("  [toast] [{}] {}", level_str, message);

        self.queue.push(Toast {
            message: String::from(message),
            level,
            duration_ms,
            remaining_ms: duration_ms,
        });

        // If the queue exceeds a reasonable limit, drop oldest non-visible toasts
        let max_queued = self.max_visible * 3;
        while self.queue.len() > max_queued {
            self.queue.remove(0);
        }
    }

    /// Advance time for all active toasts and remove expired ones.
    pub fn tick(&mut self, dt_ms: u32) {
        for toast in self.queue.iter_mut() {
            toast.remaining_ms = toast.remaining_ms.saturating_sub(dt_ms);
        }
        // Remove expired toasts
        self.queue.retain(|t| t.remaining_ms > 0);
    }

    /// Get the currently visible toasts (up to max_visible).
    pub fn visible(&self) -> &[Toast] {
        let count = if self.queue.len() < self.max_visible {
            self.queue.len()
        } else {
            self.max_visible
        };
        &self.queue[..count]
    }

    /// Number of active toasts in the queue.
    pub fn active_count(&self) -> usize {
        self.queue.len()
    }

    /// Dismiss all active toasts immediately.
    pub fn dismiss_all(&mut self) {
        self.queue.clear();
    }
}

static TOAST_MANAGER: Mutex<Option<ToastManager>> = Mutex::new(None);

/// Default max visible toasts on screen
const DEFAULT_MAX_VISIBLE: usize = 3;

pub fn init() {
    *TOAST_MANAGER.lock() = Some(ToastManager::new(DEFAULT_MAX_VISIBLE));
    crate::serial_println!("  [toast] Toast manager initialized");
}

/// Show a global toast notification.
pub fn show(message: &str, level: ToastLevel) {
    if let Some(ref mut mgr) = *TOAST_MANAGER.lock() {
        mgr.show(message, level);
    }
}

/// Tick global toast timers.
pub fn tick(dt_ms: u32) {
    if let Some(ref mut mgr) = *TOAST_MANAGER.lock() {
        mgr.tick(dt_ms);
    }
}
