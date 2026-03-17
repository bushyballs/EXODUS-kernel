use crate::sync::Mutex;
/// Miracast / screen mirroring for Genesis
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum CastProtocol {
    Miracast,
    ChromeCast,
    AirPlay,
    Custom,
}

#[derive(Clone, Copy, PartialEq)]
pub enum CastState {
    Idle,
    Discovering,
    Connecting,
    Connected,
    Streaming,
    Error,
}

#[derive(Clone, Copy)]
struct CastDevice {
    id: u32,
    name_hash: u64,
    protocol: CastProtocol,
    ip_hash: u64,
    supports_audio: bool,
    supports_4k: bool,
    latency_ms: u16,
}

struct ScreenCaster {
    devices: Vec<CastDevice>,
    active_cast: Option<u32>,
    state: CastState,
    resolution_w: u16,
    resolution_h: u16,
    fps: u8,
    next_id: u32,
}

static SCREEN_CAST: Mutex<Option<ScreenCaster>> = Mutex::new(None);

impl ScreenCaster {
    fn new() -> Self {
        ScreenCaster {
            devices: Vec::new(),
            active_cast: None,
            state: CastState::Idle,
            resolution_w: 1920,
            resolution_h: 1080,
            fps: 30,
            next_id: 1,
        }
    }

    fn discover_devices(&mut self) {
        self.state = CastState::Discovering;
    }

    fn connect(&mut self, device_id: u32) -> bool {
        if self.devices.iter().any(|d| d.id == device_id) {
            self.active_cast = Some(device_id);
            self.state = CastState::Connected;
            true
        } else {
            false
        }
    }

    fn disconnect(&mut self) {
        self.active_cast = None;
        self.state = CastState::Idle;
    }

    fn set_resolution(&mut self, w: u16, h: u16, fps: u8) {
        self.resolution_w = w;
        self.resolution_h = h;
        self.fps = fps;
    }

    fn is_casting(&self) -> bool {
        self.state == CastState::Streaming || self.state == CastState::Connected
    }
}

pub fn init() {
    let mut sc = SCREEN_CAST.lock();
    *sc = Some(ScreenCaster::new());
    serial_println!("    Screen cast: Miracast/ChromeCast/AirPlay ready");
}
