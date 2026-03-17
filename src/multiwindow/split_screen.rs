use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug)]
pub enum SplitOrientation {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug)]
pub enum SplitRatio {
    Equal,
    OneThirdTwoThirds,
    TwoThirdsOneThird,
    Custom(u8),
}

#[derive(Clone, Copy, Debug)]
pub struct SplitSession {
    pub id: u32,
    pub top_app_id: u32,
    pub bottom_app_id: u32,
    pub orientation: SplitOrientation,
    pub ratio: SplitRatio,
    pub divider_position: u16,
    pub active: bool,
}

pub struct SplitManager {
    sessions: Vec<SplitSession>,
    max_splits: u8,
    total_created: u32,
}

impl SplitManager {
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            max_splits: 4,
            total_created: 0,
        }
    }

    pub fn create_split(
        &mut self,
        top_app_id: u32,
        bottom_app_id: u32,
        orientation: SplitOrientation,
        ratio: SplitRatio,
    ) -> Option<u32> {
        if self.sessions.len() >= self.max_splits as usize {
            return None;
        }

        let divider_position = match ratio {
            SplitRatio::Equal => 50,
            SplitRatio::OneThirdTwoThirds => 33,
            SplitRatio::TwoThirdsOneThird => 67,
            SplitRatio::Custom(percent) => percent as u16,
        };

        self.total_created = self.total_created.saturating_add(1);
        let session = SplitSession {
            id: self.total_created,
            top_app_id,
            bottom_app_id,
            orientation,
            ratio,
            divider_position,
            active: true,
        };

        let id = session.id;
        self.sessions.push(session);
        Some(id)
    }

    pub fn adjust_divider(&mut self, session_id: u32, new_position: u16) -> bool {
        if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id) {
            session.divider_position = new_position.min(90).max(10);
            session.ratio = SplitRatio::Custom(session.divider_position as u8);
            true
        } else {
            false
        }
    }

    pub fn swap_apps(&mut self, session_id: u32) -> bool {
        if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id) {
            let temp = session.top_app_id;
            session.top_app_id = session.bottom_app_id;
            session.bottom_app_id = temp;
            true
        } else {
            false
        }
    }

    pub fn end_split(&mut self, session_id: u32) -> bool {
        if let Some(pos) = self.sessions.iter().position(|s| s.id == session_id) {
            self.sessions.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn get_active_split(&self) -> Option<&SplitSession> {
        self.sessions.iter().find(|s| s.active)
    }
}

static SPLIT_MGR: Mutex<Option<SplitManager>> = Mutex::new(None);

pub fn init() {
    let mut mgr = SPLIT_MGR.lock();
    *mgr = Some(SplitManager::new());
    serial_println!("[split_screen] Split-screen manager initialized (max: 4)");
}

/// Create a new split-screen session
pub fn create_split(
    top_app_id: u32,
    bottom_app_id: u32,
    orientation: SplitOrientation,
    ratio: SplitRatio,
) -> Option<u32> {
    let mut mgr = SPLIT_MGR.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.create_split(top_app_id, bottom_app_id, orientation, ratio)
    } else {
        None
    }
}

/// Adjust the divider position for a split session
pub fn adjust_divider(session_id: u32, new_position: u16) -> bool {
    let mut mgr = SPLIT_MGR.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.adjust_divider(session_id, new_position)
    } else {
        false
    }
}

/// Swap the two apps in a split session
pub fn swap_apps(session_id: u32) -> bool {
    let mut mgr = SPLIT_MGR.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.swap_apps(session_id)
    } else {
        false
    }
}

/// End a split session and return to single-window mode
pub fn end_split(session_id: u32) -> bool {
    let mut mgr = SPLIT_MGR.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.end_split(session_id)
    } else {
        false
    }
}

/// Get the currently active split session
pub fn get_active_split() -> Option<SplitSession> {
    let mgr = SPLIT_MGR.lock();
    if let Some(manager) = mgr.as_ref() {
        manager.get_active_split().copied()
    } else {
        None
    }
}

/// Get all active split sessions
pub fn get_all_splits() -> Vec<SplitSession> {
    let mgr = SPLIT_MGR.lock();
    if let Some(manager) = mgr.as_ref() {
        manager.sessions.clone()
    } else {
        Vec::new()
    }
}

/// Toggle split orientation between horizontal and vertical
pub fn toggle_orientation(session_id: u32) -> bool {
    let mut mgr = SPLIT_MGR.lock();
    if let Some(manager) = mgr.as_mut() {
        if let Some(session) = manager.sessions.iter_mut().find(|s| s.id == session_id) {
            session.orientation = match session.orientation {
                SplitOrientation::Horizontal => SplitOrientation::Vertical,
                SplitOrientation::Vertical => SplitOrientation::Horizontal,
            };
            true
        } else {
            false
        }
    } else {
        false
    }
}
