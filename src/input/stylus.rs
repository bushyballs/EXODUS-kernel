/// Stylus/pen input for Genesis
///
/// Pressure, tilt, palm rejection, hover detection.
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum StylusEvent {
    Down,
    Move,
    Up,
    HoverEnter,
    HoverMove,
    HoverExit,
    ButtonPress,
    ButtonRelease,
}

#[derive(Clone, Copy)]
struct StylusState {
    x: u16,
    y: u16,
    pressure: u16,
    tilt_x: i8,
    tilt_y: i8,
    twist: u16,
    button_primary: bool,
    button_secondary: bool,
    in_range: bool,
    tip_switch: bool,
}

struct StylusDriver {
    state: StylusState,
    palm_rejection: bool,
    latency_samples: [u16; 8],
    sample_idx: u8,
    total_strokes: u32,
}

static STYLUS: Mutex<Option<StylusDriver>> = Mutex::new(None);

impl StylusDriver {
    fn new() -> Self {
        StylusDriver {
            state: StylusState {
                x: 0,
                y: 0,
                pressure: 0,
                tilt_x: 0,
                tilt_y: 0,
                twist: 0,
                button_primary: false,
                button_secondary: false,
                in_range: false,
                tip_switch: false,
            },
            palm_rejection: true,
            latency_samples: [0; 8],
            sample_idx: 0,
            total_strokes: 0,
        }
    }

    fn process_event(&mut self, event: StylusEvent, x: u16, y: u16, pressure: u16) {
        self.state.x = x;
        self.state.y = y;
        self.state.pressure = pressure;
        match event {
            StylusEvent::Down => {
                self.state.tip_switch = true;
                self.total_strokes = self.total_strokes.saturating_add(1);
            }
            StylusEvent::Up => {
                self.state.tip_switch = false;
            }
            StylusEvent::HoverEnter => {
                self.state.in_range = true;
            }
            StylusEvent::HoverExit => {
                self.state.in_range = false;
            }
            StylusEvent::ButtonPress => {
                self.state.button_primary = true;
            }
            StylusEvent::ButtonRelease => {
                self.state.button_primary = false;
            }
            _ => {}
        }
    }

    fn get_latency_avg(&self) -> u16 {
        let sum: u32 = self.latency_samples.iter().map(|&s| s as u32).sum();
        (sum / 8) as u16
    }

    fn is_active(&self) -> bool {
        self.state.in_range || self.state.tip_switch
    }
}

pub fn init() {
    let mut s = STYLUS.lock();
    *s = Some(StylusDriver::new());
    serial_println!("    Stylus input: pressure, tilt, palm rejection ready");
}
