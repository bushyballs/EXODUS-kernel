use crate::sync::Mutex;
/// Drag and drop framework
///
/// Part of the AIOS UI layer.
use alloc::string::String;
use alloc::vec::Vec;

/// Current drag-and-drop state
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DragState {
    Idle,
    Dragging,
    OverTarget,
    Dropped,
}

/// A registered drop target region
struct DropTarget {
    id: u64,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    accepted_mime: String,
}

/// Manages system-wide drag and drop operations
pub struct DragDropManager {
    pub state: DragState,
    pub source_id: u64,
    pub mime_type: String,
    pub x: i32,
    pub y: i32,
    targets: Vec<DropTarget>,
    hover_target: Option<u64>,
}

impl DragDropManager {
    pub fn new() -> Self {
        DragDropManager {
            state: DragState::Idle,
            source_id: 0,
            mime_type: String::new(),
            x: 0,
            y: 0,
            targets: Vec::new(),
            hover_target: None,
        }
    }

    /// Register a drop target region.
    pub fn register_target(
        &mut self,
        id: u64,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        accepted_mime: &str,
    ) {
        self.targets.push(DropTarget {
            id,
            x,
            y,
            width,
            height,
            accepted_mime: String::from(accepted_mime),
        });
    }

    /// Unregister a drop target by ID.
    pub fn unregister_target(&mut self, id: u64) {
        self.targets.retain(|t| t.id != id);
    }

    /// Begin a drag operation from a source element.
    pub fn begin_drag(&mut self, source_id: u64, mime: &str) {
        self.source_id = source_id;
        self.mime_type = String::from(mime);
        self.state = DragState::Dragging;
        self.hover_target = None;
        crate::serial_println!(
            "  [drag_drop] drag started: source={}, mime={}",
            source_id,
            mime
        );
    }

    /// Update the drag position during a drag operation.
    pub fn update_position(&mut self, x: i32, y: i32) {
        if self.state != DragState::Dragging && self.state != DragState::OverTarget {
            return;
        }

        self.x = x;
        self.y = y;

        // Check if we are over any registered drop target
        let mut found_target: Option<u64> = None;
        for target in &self.targets {
            if x >= target.x
                && x < target.x + target.width
                && y >= target.y
                && y < target.y + target.height
            {
                // Check MIME type compatibility (empty accepted_mime means accept all)
                if target.accepted_mime.is_empty() || target.accepted_mime == self.mime_type {
                    found_target = Some(target.id);
                    break;
                }
            }
        }

        match found_target {
            Some(tid) => {
                if self.state != DragState::OverTarget {
                    crate::serial_println!("  [drag_drop] over target {}", tid);
                }
                self.state = DragState::OverTarget;
                self.hover_target = Some(tid);
            }
            None => {
                self.state = DragState::Dragging;
                self.hover_target = None;
            }
        }
    }

    /// Attempt to drop at the current position.
    ///
    /// Returns true if the drop was accepted by a target.
    pub fn drop_at(&mut self, x: i32, y: i32) -> bool {
        self.update_position(x, y);

        if self.state == DragState::OverTarget {
            let tid = self.hover_target.unwrap_or(0);
            crate::serial_println!(
                "  [drag_drop] dropped on target {} (mime={})",
                tid,
                self.mime_type
            );
            self.state = DragState::Dropped;
            self.cancel();
            true
        } else {
            crate::serial_println!("  [drag_drop] drop cancelled (no target)");
            self.cancel();
            false
        }
    }

    /// Cancel the current drag operation.
    pub fn cancel(&mut self) {
        self.state = DragState::Idle;
        self.source_id = 0;
        self.mime_type.clear();
        self.hover_target = None;
    }

    /// Check whether a drag is currently active.
    pub fn is_dragging(&self) -> bool {
        self.state == DragState::Dragging || self.state == DragState::OverTarget
    }
}

static DRAG_DROP: Mutex<Option<DragDropManager>> = Mutex::new(None);

pub fn init() {
    *DRAG_DROP.lock() = Some(DragDropManager::new());
    crate::serial_println!("  [drag_drop] Drag-and-drop manager initialized");
}

/// Begin a global drag operation.
pub fn begin_drag(source_id: u64, mime: &str) {
    if let Some(ref mut mgr) = *DRAG_DROP.lock() {
        mgr.begin_drag(source_id, mime);
    }
}

/// Drop at a global position.
pub fn drop_at(x: i32, y: i32) -> bool {
    match DRAG_DROP.lock().as_mut() {
        Some(mgr) => mgr.drop_at(x, y),
        None => false,
    }
}
