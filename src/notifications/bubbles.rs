use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

/// State of a bubble notification
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BubbleState {
    Collapsed,
    Expanded,
    Minimized,
    Hidden,
}

/// A floating bubble notification
#[derive(Clone, Copy, Debug)]
pub struct Bubble {
    pub id: u32,
    pub app_id: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub state: BubbleState,
    pub icon_hash: u32,
    pub unread_count: u32,
}

impl Bubble {
    pub fn new(id: u32, app_id: u32, x: u32, y: u32, icon_hash: u32) -> Self {
        Self {
            id,
            app_id,
            x,
            y,
            width: 64,
            height: 64,
            state: BubbleState::Collapsed,
            icon_hash,
            unread_count: 0,
        }
    }
}

/// Manages floating bubble notifications
pub struct BubbleManager {
    bubbles: Vec<Bubble>,
    max_bubbles: u8,
    total_created: u64,
}

impl BubbleManager {
    pub fn new() -> Self {
        Self {
            bubbles: vec![],
            max_bubbles: 5,
            total_created: 0,
        }
    }

    /// Create a new bubble notification
    pub fn create_bubble(&mut self, app_id: u32, x: u32, y: u32, icon_hash: u32) -> Option<u32> {
        // Check if we're at max bubbles
        if self.bubbles.len() >= self.max_bubbles as usize {
            serial_println!(
                "[BUBBLE] Cannot create bubble: max limit ({}) reached",
                self.max_bubbles
            );
            return None;
        }

        let id = self.total_created as u32 + 1;
        self.total_created = self.total_created.saturating_add(1);

        let bubble = Bubble::new(id, app_id, x, y, icon_hash);
        self.bubbles.push(bubble);

        serial_println!(
            "[BUBBLE] Created bubble {} for app {} at ({}, {})",
            id,
            app_id,
            x,
            y
        );

        Some(id)
    }

    /// Expand a bubble to show its content
    pub fn expand(&mut self, id: u32) -> bool {
        if let Some(bubble) = self.bubbles.iter_mut().find(|b| b.id == id) {
            bubble.state = BubbleState::Expanded;
            bubble.width = 320;
            bubble.height = 480;
            serial_println!("[BUBBLE] Expanded bubble {}", id);
            true
        } else {
            false
        }
    }

    /// Collapse a bubble back to icon size
    pub fn collapse(&mut self, id: u32) -> bool {
        if let Some(bubble) = self.bubbles.iter_mut().find(|b| b.id == id) {
            bubble.state = BubbleState::Collapsed;
            bubble.width = 64;
            bubble.height = 64;
            serial_println!("[BUBBLE] Collapsed bubble {}", id);
            true
        } else {
            false
        }
    }

    /// Dismiss a bubble
    pub fn dismiss(&mut self, id: u32) -> bool {
        if let Some(pos) = self.bubbles.iter().position(|b| b.id == id) {
            self.bubbles.remove(pos);
            serial_println!("[BUBBLE] Dismissed bubble {}", id);
            true
        } else {
            false
        }
    }

    /// Reposition a bubble
    pub fn reposition(&mut self, id: u32, x: u32, y: u32) -> bool {
        if let Some(bubble) = self.bubbles.iter_mut().find(|b| b.id == id) {
            bubble.x = x;
            bubble.y = y;
            serial_println!("[BUBBLE] Repositioned bubble {} to ({}, {})", id, x, y);
            true
        } else {
            false
        }
    }

    /// Get all visible bubbles
    pub fn get_visible(&self) -> Vec<Bubble> {
        self.bubbles
            .iter()
            .filter(|b| b.state != BubbleState::Hidden)
            .copied()
            .collect()
    }

    /// Update unread count for a bubble
    pub fn update_unread(&mut self, id: u32, count: u32) -> bool {
        if let Some(bubble) = self.bubbles.iter_mut().find(|b| b.id == id) {
            bubble.unread_count = count;
            serial_println!("[BUBBLE] Updated bubble {} unread count to {}", id, count);
            true
        } else {
            false
        }
    }

    /// Get bubble count
    pub fn bubble_count(&self) -> usize {
        self.bubbles.len()
    }

    /// Get bubble by app ID
    pub fn get_by_app(&self, app_id: u32) -> Option<&Bubble> {
        self.bubbles.iter().find(|b| b.app_id == app_id)
    }
}

static BUBBLE_MGR: Mutex<Option<BubbleManager>> = Mutex::new(None);

/// Initialize the bubble manager
pub fn init() {
    let mut lock = BUBBLE_MGR.lock();
    *lock = Some(BubbleManager::new());
    serial_println!("[BUBBLE] Bubble manager initialized");
}

/// Get a reference to the bubble manager
pub fn get_manager() -> &'static Mutex<Option<BubbleManager>> {
    &BUBBLE_MGR
}
