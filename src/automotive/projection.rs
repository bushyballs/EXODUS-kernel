/// Car display projection for Genesis
///
/// Screen mirroring to car head unit, touch input routing,
/// audio routing, media controls, navigation display.
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum ProjectionState {
    Disconnected,
    Connecting,
    Connected,
    Projecting,
    Error,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ConnectionType {
    UsbWired,
    WifiDirect,
    Bluetooth,
}

struct CarProjection {
    state: ProjectionState,
    connection: ConnectionType,
    car_display_width: u16,
    car_display_height: u16,
    touch_enabled: bool,
    audio_routed: bool,
    night_mode: bool,
    fps: u8,
    latency_ms: u32,
    sessions: u32,
}

static PROJECTION: Mutex<Option<CarProjection>> = Mutex::new(None);

impl CarProjection {
    fn new() -> Self {
        CarProjection {
            state: ProjectionState::Disconnected,
            connection: ConnectionType::UsbWired,
            car_display_width: 1280,
            car_display_height: 720,
            touch_enabled: true,
            audio_routed: false,
            night_mode: false,
            fps: 30,
            latency_ms: 0,
            sessions: 0,
        }
    }

    fn connect(&mut self, conn_type: ConnectionType) {
        self.state = ProjectionState::Connecting;
        self.connection = conn_type;
        // Handshake
        self.state = ProjectionState::Connected;
        self.sessions = self.sessions.saturating_add(1);
    }

    fn start_projection(&mut self) {
        if self.state == ProjectionState::Connected {
            self.state = ProjectionState::Projecting;
            self.audio_routed = true;
        }
    }

    fn disconnect(&mut self) {
        self.state = ProjectionState::Disconnected;
        self.audio_routed = false;
    }
}

pub fn init() {
    let mut p = PROJECTION.lock();
    *p = Some(CarProjection::new());
    serial_println!("    Automotive: car projection ready");
}
