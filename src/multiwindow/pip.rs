use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug)]
pub struct PipWindow {
    pub id: u32,
    pub app_id: u32,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub aspect_ratio_x: u8,
    pub aspect_ratio_y: u8,
    pub always_on_top: bool,
    pub opacity: u8,
}

pub struct PipManager {
    windows: Vec<PipWindow>,
    max_pip: u8,
    next_id: u32,
}

impl PipManager {
    pub fn new() -> Self {
        Self {
            windows: Vec::new(),
            max_pip: 3,
            next_id: 1,
        }
    }

    pub fn create_pip(
        &mut self,
        app_id: u32,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        aspect_ratio_x: u8,
        aspect_ratio_y: u8,
    ) -> Option<u32> {
        if self.windows.len() >= self.max_pip as usize {
            return None;
        }

        let window = PipWindow {
            id: self.next_id,
            app_id,
            x,
            y,
            width,
            height,
            aspect_ratio_x,
            aspect_ratio_y,
            always_on_top: true,
            opacity: 255,
        };

        let id = window.id;
        self.next_id = self.next_id.saturating_add(1);
        self.windows.push(window);
        Some(id)
    }

    pub fn move_pip(&mut self, pip_id: u32, x: u16, y: u16) -> bool {
        if let Some(window) = self.windows.iter_mut().find(|w| w.id == pip_id) {
            window.x = x;
            window.y = y;
            true
        } else {
            false
        }
    }

    pub fn resize_pip(&mut self, pip_id: u32, width: u16, _height: u16) -> bool {
        if let Some(window) = self.windows.iter_mut().find(|w| w.id == pip_id) {
            let aspect = (window.aspect_ratio_x as f32) / (window.aspect_ratio_y as f32);
            let new_width = width;
            let new_height = (new_width as f32 / aspect) as u16;

            window.width = new_width;
            window.height = new_height;
            true
        } else {
            false
        }
    }

    pub fn close_pip(&mut self, pip_id: u32) -> bool {
        if let Some(pos) = self.windows.iter().position(|w| w.id == pip_id) {
            self.windows.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn toggle_always_on_top(&mut self, pip_id: u32) -> bool {
        if let Some(window) = self.windows.iter_mut().find(|w| w.id == pip_id) {
            window.always_on_top = !window.always_on_top;
            true
        } else {
            false
        }
    }

    pub fn get_visible(&self) -> &[PipWindow] {
        &self.windows
    }
}

static PIP_MGR: Mutex<Option<PipManager>> = Mutex::new(None);

pub fn init() {
    let mut mgr = PIP_MGR.lock();
    *mgr = Some(PipManager::new());
    serial_println!("[pip] Picture-in-picture manager initialized (max: 3)");
}

/// Create a new PIP window
pub fn create_pip(
    app_id: u32,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    aspect_ratio_x: u8,
    aspect_ratio_y: u8,
) -> Option<u32> {
    let mut mgr = PIP_MGR.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.create_pip(app_id, x, y, width, height, aspect_ratio_x, aspect_ratio_y)
    } else {
        None
    }
}

/// Move a PIP window to a new position
pub fn move_pip(pip_id: u32, x: u16, y: u16) -> bool {
    let mut mgr = PIP_MGR.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.move_pip(pip_id, x, y)
    } else {
        false
    }
}

/// Resize a PIP window (maintains aspect ratio)
pub fn resize_pip(pip_id: u32, width: u16, height: u16) -> bool {
    let mut mgr = PIP_MGR.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.resize_pip(pip_id, width, height)
    } else {
        false
    }
}

/// Close a PIP window
pub fn close_pip(pip_id: u32) -> bool {
    let mut mgr = PIP_MGR.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.close_pip(pip_id)
    } else {
        false
    }
}

/// Toggle always-on-top for a PIP window
pub fn toggle_always_on_top(pip_id: u32) -> bool {
    let mut mgr = PIP_MGR.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.toggle_always_on_top(pip_id)
    } else {
        false
    }
}

/// Set opacity for a PIP window (0-255)
pub fn set_opacity(pip_id: u32, opacity: u8) -> bool {
    let mut mgr = PIP_MGR.lock();
    if let Some(manager) = mgr.as_mut() {
        if let Some(window) = manager.windows.iter_mut().find(|w| w.id == pip_id) {
            window.opacity = opacity;
            true
        } else {
            false
        }
    } else {
        false
    }
}

/// Get all visible PIP windows
pub fn get_visible() -> Vec<PipWindow> {
    let mgr = PIP_MGR.lock();
    if let Some(manager) = mgr.as_ref() {
        manager.get_visible().to_vec()
    } else {
        Vec::new()
    }
}

/// Snap PIP window to corner (0=top-left, 1=top-right, 2=bottom-right, 3=bottom-left)
pub fn snap_to_corner(pip_id: u32, corner: u8, margin: u16) -> bool {
    let mut mgr = PIP_MGR.lock();
    if let Some(manager) = mgr.as_mut() {
        if let Some(window) = manager.windows.iter_mut().find(|w| w.id == pip_id) {
            // Assume screen size 1920x1080 (could be parameterized)
            let (new_x, new_y) = match corner {
                0 => (margin, margin),                                              // top-left
                1 => (1920 - window.width - margin, margin),                        // top-right
                2 => (1920 - window.width - margin, 1080 - window.height - margin), // bottom-right
                3 => (margin, 1080 - window.height - margin),                       // bottom-left
                _ => return false,
            };
            window.x = new_x;
            window.y = new_y;
            true
        } else {
            false
        }
    } else {
        false
    }
}
