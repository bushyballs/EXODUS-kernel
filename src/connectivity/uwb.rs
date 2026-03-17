/// Ultra-Wideband (UWB) for Genesis
///
/// Precise ranging, spatial awareness, device finding,
/// and secure access (digital car key, smart locks).
///
/// Inspired by: Android UWB, Apple U1. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// UWB ranging result
pub struct RangingResult {
    pub peer_id: u32,
    pub distance_cm: u32,
    pub azimuth_deg: i16,   // -180 to 180
    pub elevation_deg: i16, // -90 to 90
    pub confidence: u8,     // 0-100
    pub timestamp: u64,
}

/// UWB session type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionType {
    Ranging,
    DataTransfer,
    ControleeOnly,
    ControllerOnly,
}

/// UWB session
pub struct UwbSession {
    pub id: u32,
    pub session_type: SessionType,
    pub active: bool,
    pub peers: Vec<u32>,
    pub results: Vec<RangingResult>,
    pub channel: u8,
    pub update_rate_hz: u8,
}

/// UWB controller
pub struct UwbController {
    pub enabled: bool,
    pub sessions: Vec<UwbSession>,
    pub next_session_id: u32,
    pub device_role: String,
    pub supported_channels: Vec<u8>,
}

impl UwbController {
    const fn new() -> Self {
        UwbController {
            enabled: false,
            sessions: Vec::new(),
            next_session_id: 1,
            device_role: String::new(),
            supported_channels: Vec::new(),
        }
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }
    pub fn disable(&mut self) {
        self.enabled = false;
    }

    pub fn create_session(&mut self, session_type: SessionType, channel: u8) -> u32 {
        let id = self.next_session_id;
        self.next_session_id = self.next_session_id.saturating_add(1);
        self.sessions.push(UwbSession {
            id,
            session_type,
            active: false,
            peers: Vec::new(),
            results: Vec::new(),
            channel,
            update_rate_hz: 10,
        });
        id
    }

    pub fn start_session(&mut self, id: u32) -> bool {
        if let Some(session) = self.sessions.iter_mut().find(|s| s.id == id) {
            session.active = true;
            true
        } else {
            false
        }
    }

    pub fn stop_session(&mut self, id: u32) {
        if let Some(session) = self.sessions.iter_mut().find(|s| s.id == id) {
            session.active = false;
        }
    }

    pub fn on_ranging_result(&mut self, session_id: u32, result: RangingResult) {
        if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id) {
            if session.results.len() >= 100 {
                session.results.remove(0);
            }
            session.results.push(result);
        }
    }

    pub fn get_distance(&self, session_id: u32, peer_id: u32) -> Option<u32> {
        self.sessions
            .iter()
            .find(|s| s.id == session_id)
            .and_then(|s| s.results.iter().rev().find(|r| r.peer_id == peer_id))
            .map(|r| r.distance_cm)
    }
}

static UWB: Mutex<UwbController> = Mutex::new(UwbController::new());

pub fn init() {
    let mut uwb = UWB.lock();
    uwb.supported_channels = alloc::vec![5, 6, 8, 9];
    crate::serial_println!("  [connectivity] UWB controller initialized");
}
