use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug)]
pub struct FreeformWindow {
    pub id: u32,
    pub app_id: u32,
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
    pub min_width: u16,
    pub min_height: u16,
    pub maximized: bool,
    pub minimized: bool,
    pub z_order: u16,
    pub resizable: bool,
    pub title_hash: u64,
}

pub struct FreeformManager {
    windows: Vec<FreeformWindow>,
    next_id: u32,
    next_z: u16,
    desktop_mode: bool,
}

impl FreeformManager {
    pub fn new() -> Self {
        Self {
            windows: Vec::new(),
            next_id: 1,
            next_z: 1,
            desktop_mode: false,
        }
    }

    pub fn create_window(
        &mut self,
        app_id: u32,
        x: i16,
        y: i16,
        width: u16,
        height: u16,
        resizable: bool,
        title_hash: u64,
    ) -> u32 {
        let window = FreeformWindow {
            id: self.next_id,
            app_id,
            x,
            y,
            width,
            height,
            min_width: 200,
            min_height: 150,
            maximized: false,
            minimized: false,
            z_order: self.next_z,
            resizable,
            title_hash,
        };

        let id = window.id;
        self.next_id = self.next_id.saturating_add(1);
        self.next_z = self.next_z.saturating_add(1);
        self.windows.push(window);
        id
    }

    pub fn move_window(&mut self, window_id: u32, x: i16, y: i16) -> bool {
        if let Some(window) = self.windows.iter_mut().find(|w| w.id == window_id) {
            if !window.maximized {
                window.x = x;
                window.y = y;
            }
            true
        } else {
            false
        }
    }

    pub fn resize_window(&mut self, window_id: u32, width: u16, height: u16) -> bool {
        if let Some(window) = self.windows.iter_mut().find(|w| w.id == window_id) {
            if window.resizable && !window.maximized {
                window.width = width.max(window.min_width);
                window.height = height.max(window.min_height);
            }
            true
        } else {
            false
        }
    }

    pub fn maximize(&mut self, window_id: u32) -> bool {
        if let Some(window) = self.windows.iter_mut().find(|w| w.id == window_id) {
            window.maximized = true;
            window.minimized = false;
            true
        } else {
            false
        }
    }

    pub fn minimize(&mut self, window_id: u32) -> bool {
        if let Some(window) = self.windows.iter_mut().find(|w| w.id == window_id) {
            window.minimized = true;
            window.maximized = false;
            true
        } else {
            false
        }
    }

    pub fn restore(&mut self, window_id: u32) -> bool {
        if let Some(window) = self.windows.iter_mut().find(|w| w.id == window_id) {
            window.maximized = false;
            window.minimized = false;
            true
        } else {
            false
        }
    }

    pub fn bring_to_front(&mut self, window_id: u32) -> bool {
        if let Some(window) = self.windows.iter_mut().find(|w| w.id == window_id) {
            window.z_order = self.next_z;
            self.next_z = self.next_z.saturating_add(1);
            true
        } else {
            false
        }
    }

    pub fn cascade(&mut self) {
        let offset = 30;
        for (i, window) in self.windows.iter_mut().enumerate() {
            window.x = (i as i16) * offset;
            window.y = (i as i16) * offset;
            window.z_order = i as u16;
            window.maximized = false;
            window.minimized = false;
        }
    }

    pub fn tile_all(&mut self) {
        let count = self.windows.len();
        if count == 0 {
            return;
        }

        // Integer sqrt (no f32::sqrt in no_std)
        let mut cols = 1usize;
        while cols * cols < count {
            cols += 1;
        }
        let rows = (count + cols - 1) / cols;
        let tile_width = 1920 / cols as u16;
        let tile_height = 1080 / rows as u16;

        for (i, window) in self.windows.iter_mut().enumerate() {
            let col = i % cols;
            let row = i / cols;
            window.x = (col as u16 * tile_width) as i16;
            window.y = (row as u16 * tile_height) as i16;
            window.width = tile_width;
            window.height = tile_height;
            window.maximized = false;
            window.minimized = false;
        }
    }

    pub fn close_window(&mut self, window_id: u32) -> bool {
        if let Some(pos) = self.windows.iter().position(|w| w.id == window_id) {
            self.windows.remove(pos);
            true
        } else {
            false
        }
    }
}

static FREEFORM: Mutex<Option<FreeformManager>> = Mutex::new(None);

pub fn init() {
    let mut mgr = FREEFORM.lock();
    *mgr = Some(FreeformManager::new());
    serial_println!("[freeform] Freeform window manager initialized (desktop mode)");
}

/// Create a new freeform window
pub fn create_window(
    app_id: u32,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    resizable: bool,
    title_hash: u64,
) -> u32 {
    let mut mgr = FREEFORM.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.create_window(app_id, x, y, width, height, resizable, title_hash)
    } else {
        0
    }
}

/// Move a window to a new position
pub fn move_window(window_id: u32, x: i16, y: i16) -> bool {
    let mut mgr = FREEFORM.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.move_window(window_id, x, y)
    } else {
        false
    }
}

/// Resize a window
pub fn resize_window(window_id: u32, width: u16, height: u16) -> bool {
    let mut mgr = FREEFORM.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.resize_window(window_id, width, height)
    } else {
        false
    }
}

/// Maximize a window
pub fn maximize(window_id: u32) -> bool {
    let mut mgr = FREEFORM.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.maximize(window_id)
    } else {
        false
    }
}

/// Minimize a window
pub fn minimize(window_id: u32) -> bool {
    let mut mgr = FREEFORM.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.minimize(window_id)
    } else {
        false
    }
}

/// Restore a window from maximized or minimized state
pub fn restore(window_id: u32) -> bool {
    let mut mgr = FREEFORM.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.restore(window_id)
    } else {
        false
    }
}

/// Bring a window to the front of the Z-order
pub fn bring_to_front(window_id: u32) -> bool {
    let mut mgr = FREEFORM.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.bring_to_front(window_id)
    } else {
        false
    }
}

/// Cascade all windows
pub fn cascade() {
    let mut mgr = FREEFORM.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.cascade();
    }
}

/// Tile all windows in a grid
pub fn tile_all() {
    let mut mgr = FREEFORM.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.tile_all();
    }
}

/// Close a window
pub fn close_window(window_id: u32) -> bool {
    let mut mgr = FREEFORM.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.close_window(window_id)
    } else {
        false
    }
}

/// Get all windows sorted by Z-order (front to back)
pub fn get_windows() -> Vec<FreeformWindow> {
    let mgr = FREEFORM.lock();
    if let Some(manager) = mgr.as_ref() {
        let mut windows = manager.windows.clone();
        windows.sort_by(|a, b| b.z_order.cmp(&a.z_order));
        windows
    } else {
        Vec::new()
    }
}

/// Get the topmost window
pub fn get_top_window() -> Option<FreeformWindow> {
    let mgr = FREEFORM.lock();
    if let Some(manager) = mgr.as_ref() {
        manager
            .windows
            .iter()
            .filter(|w| !w.minimized)
            .max_by_key(|w| w.z_order)
            .copied()
    } else {
        None
    }
}

/// Enable or disable desktop mode
pub fn set_desktop_mode(enabled: bool) {
    let mut mgr = FREEFORM.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.desktop_mode = enabled;
    }
}

/// Check if a point is inside a window
pub fn hit_test(x: i16, y: i16) -> Option<u32> {
    let mgr = FREEFORM.lock();
    if let Some(manager) = mgr.as_ref() {
        manager
            .windows
            .iter()
            .filter(|w| !w.minimized)
            .filter(|w| x >= w.x && x < w.x + w.width as i16 && y >= w.y && y < y + w.height as i16)
            .max_by_key(|w| w.z_order)
            .map(|w| w.id)
    } else {
        None
    }
}
