use crate::sync::Mutex;
use crate::{serial_print, serial_println};
/// Integration examples and high-level window management API
///
/// This module provides a unified API that coordinates all the multiwindow
/// subsystems (split-screen, PIP, freeform, layouts, animations, gestures, hotkeys)
use alloc::vec::Vec;

use super::{
    freeform, gestures, hotkeys, layouts, split_screen, Gesture, Layout, Modifiers, SnapZone,
    WindowAction,
};

/// Window manager mode
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WindowMode {
    /// Traditional freeform windows
    Freeform,
    /// Split-screen with two apps
    Split,
    /// Picture-in-picture overlay
    Pip,
    /// Fullscreen single app
    Fullscreen,
    /// Multi-window overview
    Overview,
}

/// Unified window manager state
pub struct WindowManager {
    current_mode: WindowMode,
    active_layout: Option<Layout>,
    animations_enabled: bool,
    gestures_enabled: bool,
}

impl WindowManager {
    pub fn new() -> Self {
        Self {
            current_mode: WindowMode::Freeform,
            active_layout: None,
            animations_enabled: true,
            gestures_enabled: true,
        }
    }

    /// Apply a predefined layout to all windows
    pub fn apply_layout(&mut self, layout: Layout, window_ids: &[u32]) -> bool {
        let config = layouts::get_screen_config();
        let rects = layouts::calculate_layout(layout, window_ids.len(), &config);

        if rects.len() != window_ids.len() {
            return false;
        }

        for (i, &window_id) in window_ids.iter().enumerate() {
            let rect = &rects[i];

            if self.animations_enabled {
                // Animate window to new position
                freeform::move_window(window_id, rect.x, rect.y);
                freeform::resize_window(window_id, rect.width, rect.height);
            } else {
                // Instantly move/resize
                freeform::move_window(window_id, rect.x, rect.y);
                freeform::resize_window(window_id, rect.width, rect.height);
            }
        }

        self.active_layout = Some(layout);
        true
    }

    /// Snap a window to a predefined zone
    pub fn snap_to_zone(&mut self, window_id: u32, zone: SnapZone) -> bool {
        let config = layouts::get_screen_config();
        let rect = layouts::get_snap_zone_rect(zone, &config);

        freeform::move_window(window_id, rect.x, rect.y);
        freeform::resize_window(window_id, rect.width, rect.height);
        true
    }

    /// Handle a gesture and perform corresponding action
    pub fn handle_gesture(&mut self, gesture: Gesture) -> bool {
        if !self.gestures_enabled {
            return false;
        }

        let action = gestures::map_gesture_to_action(gesture);
        self.handle_action(action)
    }

    /// Handle a window action (from hotkey or gesture)
    pub fn handle_action(&mut self, action: gestures::GestureAction) -> bool {
        use gestures::GestureAction;

        match action {
            GestureAction::NextWindow => {
                // Get all windows and cycle to next
                let windows = freeform::get_windows();
                if !windows.is_empty() {
                    let top = freeform::get_top_window();
                    if let Some(current) = top {
                        // Find next window
                        if let Some(pos) = windows.iter().position(|w| w.id == current.id) {
                            let next_pos = (pos + 1) % windows.len();
                            freeform::bring_to_front(windows[next_pos].id);
                        }
                    }
                }
                true
            }

            GestureAction::PrevWindow => {
                // Get all windows and cycle to previous
                let windows = freeform::get_windows();
                if !windows.is_empty() {
                    let top = freeform::get_top_window();
                    if let Some(current) = top {
                        if let Some(pos) = windows.iter().position(|w| w.id == current.id) {
                            let prev_pos = if pos == 0 { windows.len() - 1 } else { pos - 1 };
                            freeform::bring_to_front(windows[prev_pos].id);
                        }
                    }
                }
                true
            }

            GestureAction::ShowAllWindows => {
                // Switch to overview mode
                self.current_mode = WindowMode::Overview;
                // Could trigger layout like Grid2x2 or Grid3x3
                let windows = freeform::get_windows();
                let window_ids: Vec<u32> = windows.iter().map(|w| w.id).collect();
                if window_ids.len() <= 4 {
                    self.apply_layout(Layout::Grid2x2, &window_ids);
                } else if window_ids.len() <= 6 {
                    self.apply_layout(Layout::Grid2x3, &window_ids);
                } else {
                    self.apply_layout(Layout::Grid3x3, &window_ids);
                }
                true
            }

            GestureAction::MinimizeWindow => {
                if let Some(top) = freeform::get_top_window() {
                    freeform::minimize(top.id);
                }
                true
            }

            GestureAction::MaximizeWindow => {
                if let Some(top) = freeform::get_top_window() {
                    freeform::maximize(top.id);
                }
                true
            }

            GestureAction::CloseWindow => {
                if let Some(top) = freeform::get_top_window() {
                    freeform::close_window(top.id);
                }
                true
            }

            GestureAction::SnapLeft => {
                if let Some(top) = freeform::get_top_window() {
                    self.snap_to_zone(top.id, SnapZone::Left);
                }
                true
            }

            GestureAction::SnapRight => {
                if let Some(top) = freeform::get_top_window() {
                    self.snap_to_zone(top.id, SnapZone::Right);
                }
                true
            }

            GestureAction::ShowDesktop => {
                // Minimize all windows
                let windows = freeform::get_windows();
                for window in windows {
                    freeform::minimize(window.id);
                }
                true
            }

            GestureAction::EnterSplitScreen => {
                // Enter split screen mode with top 2 windows
                let windows = freeform::get_windows();
                if windows.len() >= 2 {
                    self.current_mode = WindowMode::Split;
                    split_screen::create_split(
                        windows[0].app_id,
                        windows[1].app_id,
                        super::SplitOrientation::Vertical,
                        super::SplitRatio::Equal,
                    );
                }
                true
            }

            GestureAction::ExitSplitScreen => {
                // Exit split screen and restore freeform
                if let Some(session) = split_screen::get_active_split() {
                    split_screen::end_split(session.id);
                    self.current_mode = WindowMode::Freeform;
                }
                true
            }

            GestureAction::None => false,
        }
    }

    /// Handle hotkey press
    pub fn handle_hotkey(&mut self, modifiers: Modifiers, key_code: u8) -> bool {
        if let Some(action) = hotkeys::process_key(modifiers, key_code) {
            self.handle_window_action(action)
        } else {
            false
        }
    }

    /// Handle window action from hotkey
    fn handle_window_action(&mut self, action: WindowAction) -> bool {
        match action {
            WindowAction::CloseWindow => {
                if let Some(top) = freeform::get_top_window() {
                    freeform::close_window(top.id);
                }
                true
            }

            WindowAction::MinimizeWindow => {
                if let Some(top) = freeform::get_top_window() {
                    freeform::minimize(top.id);
                }
                true
            }

            WindowAction::MaximizeWindow => {
                if let Some(top) = freeform::get_top_window() {
                    if top.maximized {
                        freeform::restore(top.id);
                    } else {
                        freeform::maximize(top.id);
                    }
                }
                true
            }

            WindowAction::SnapLeft => {
                if let Some(top) = freeform::get_top_window() {
                    self.snap_to_zone(top.id, SnapZone::Left);
                }
                true
            }

            WindowAction::SnapRight => {
                if let Some(top) = freeform::get_top_window() {
                    self.snap_to_zone(top.id, SnapZone::Right);
                }
                true
            }

            WindowAction::SnapTop => {
                if let Some(top) = freeform::get_top_window() {
                    self.snap_to_zone(top.id, SnapZone::Top);
                }
                true
            }

            WindowAction::SnapBottom => {
                if let Some(top) = freeform::get_top_window() {
                    self.snap_to_zone(top.id, SnapZone::Bottom);
                }
                true
            }

            WindowAction::SnapTopLeft => {
                if let Some(top) = freeform::get_top_window() {
                    self.snap_to_zone(top.id, SnapZone::TopLeft);
                }
                true
            }

            WindowAction::SnapTopRight => {
                if let Some(top) = freeform::get_top_window() {
                    self.snap_to_zone(top.id, SnapZone::TopRight);
                }
                true
            }

            WindowAction::SnapBottomLeft => {
                if let Some(top) = freeform::get_top_window() {
                    self.snap_to_zone(top.id, SnapZone::BottomLeft);
                }
                true
            }

            WindowAction::SnapBottomRight => {
                if let Some(top) = freeform::get_top_window() {
                    self.snap_to_zone(top.id, SnapZone::BottomRight);
                }
                true
            }

            WindowAction::CascadeWindows => {
                freeform::cascade();
                true
            }

            WindowAction::TileWindows => {
                freeform::tile_all();
                true
            }

            WindowAction::ShowDesktop => {
                let windows = freeform::get_windows();
                for window in windows {
                    freeform::minimize(window.id);
                }
                true
            }

            WindowAction::NextWindow => {
                let windows = freeform::get_windows();
                if !windows.is_empty() {
                    let top = freeform::get_top_window();
                    if let Some(current) = top {
                        if let Some(pos) = windows.iter().position(|w| w.id == current.id) {
                            let next_pos = (pos + 1) % windows.len();
                            freeform::bring_to_front(windows[next_pos].id);
                        }
                    }
                }
                true
            }

            WindowAction::PrevWindow => {
                let windows = freeform::get_windows();
                if !windows.is_empty() {
                    let top = freeform::get_top_window();
                    if let Some(current) = top {
                        if let Some(pos) = windows.iter().position(|w| w.id == current.id) {
                            let prev_pos = if pos == 0 { windows.len() - 1 } else { pos - 1 };
                            freeform::bring_to_front(windows[prev_pos].id);
                        }
                    }
                }
                true
            }

            WindowAction::ShowAllWindows => {
                self.current_mode = WindowMode::Overview;
                let windows = freeform::get_windows();
                let window_ids: Vec<u32> = windows.iter().map(|w| w.id).collect();
                if window_ids.len() <= 4 {
                    self.apply_layout(Layout::Grid2x2, &window_ids);
                } else {
                    self.apply_layout(Layout::Grid3x3, &window_ids);
                }
                true
            }

            _ => false,
        }
    }

    /// Set whether animations are enabled
    pub fn set_animations_enabled(&mut self, enabled: bool) {
        self.animations_enabled = enabled;
    }

    /// Set whether gestures are enabled
    pub fn set_gestures_enabled(&mut self, enabled: bool) {
        self.gestures_enabled = enabled;
    }

    /// Get current window mode
    pub fn get_mode(&self) -> WindowMode {
        self.current_mode
    }
}

static WINDOW_MGR: Mutex<Option<WindowManager>> = Mutex::new(None);

pub fn init() {
    let mut mgr = WINDOW_MGR.lock();
    *mgr = Some(WindowManager::new());
    serial_println!("[integration] Unified window manager initialized");
}

/// Apply a layout to windows
pub fn apply_layout(layout: Layout, window_ids: &[u32]) -> bool {
    let mut mgr = WINDOW_MGR.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.apply_layout(layout, window_ids)
    } else {
        false
    }
}

/// Snap window to zone
pub fn snap_to_zone(window_id: u32, zone: SnapZone) -> bool {
    let mut mgr = WINDOW_MGR.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.snap_to_zone(window_id, zone)
    } else {
        false
    }
}

/// Handle gesture
pub fn handle_gesture(gesture: Gesture) -> bool {
    let mut mgr = WINDOW_MGR.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.handle_gesture(gesture)
    } else {
        false
    }
}

/// Handle hotkey
pub fn handle_hotkey(modifiers: Modifiers, key_code: u8) -> bool {
    let mut mgr = WINDOW_MGR.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.handle_hotkey(modifiers, key_code)
    } else {
        false
    }
}
