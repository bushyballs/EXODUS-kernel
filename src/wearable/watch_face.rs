use crate::sync::Mutex;
/// Watch face engine for Genesis
///
/// Customizable watch faces, complications (data slots),
/// always-on display, ambient mode.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum ComplicationType {
    ShortText,
    LongText,
    RangedValue,
    Icon,
    SmallImage,
    LargeImage,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ComplicationData {
    Steps,
    HeartRate,
    Battery,
    Weather,
    Date,
    Timer,
    Sunrise,
    NextAlarm,
    Calories,
    Distance,
}

struct Complication {
    slot: u8,
    complication_type: ComplicationType,
    data_source: ComplicationData,
    position_x: u16,
    position_y: u16,
}

struct WatchFace {
    id: u32,
    name: [u8; 24],
    name_len: usize,
    complications: Vec<Complication>,
    always_on: bool,
    ambient_color: u32, // RGB for ambient mode
    active: bool,
}

struct WatchFaceEngine {
    faces: Vec<WatchFace>,
    active_face: Option<u32>,
    next_id: u32,
    ambient_mode: bool,
}

static WATCH_FACE: Mutex<Option<WatchFaceEngine>> = Mutex::new(None);

impl WatchFaceEngine {
    fn new() -> Self {
        WatchFaceEngine {
            faces: Vec::new(),
            active_face: None,
            next_id: 1,
            ambient_mode: false,
        }
    }

    fn create_face(&mut self, name: &[u8]) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut n = [0u8; 24];
        let nlen = name.len().min(24);
        n[..nlen].copy_from_slice(&name[..nlen]);
        self.faces.push(WatchFace {
            id,
            name: n,
            name_len: nlen,
            complications: Vec::new(),
            always_on: true,
            ambient_color: 0x444444,
            active: false,
        });
        id
    }

    fn set_active(&mut self, face_id: u32) {
        for face in self.faces.iter_mut() {
            face.active = face.id == face_id;
        }
        self.active_face = Some(face_id);
    }

    fn add_complication(
        &mut self,
        face_id: u32,
        slot: u8,
        ctype: ComplicationType,
        data: ComplicationData,
        x: u16,
        y: u16,
    ) {
        if let Some(face) = self.faces.iter_mut().find(|f| f.id == face_id) {
            face.complications.push(Complication {
                slot,
                complication_type: ctype,
                data_source: data,
                position_x: x,
                position_y: y,
            });
        }
    }
}

pub fn init() {
    let mut w = WATCH_FACE.lock();
    *w = Some(WatchFaceEngine::new());
    serial_println!("    Wearable: watch face engine ready");
}

// ── Sensor data stubs ─────────────────────────────────────────────────────────
//
// These stubs provide the sensor API expected by watch-face complications.
// Values are held in static Mutex-guarded state; a hardware sensor driver (or
// BLE sync from the companion) calls the `set_*` functions to update them.

use crate::sync::Mutex as SensorMutex;

/// Live heart-rate reading in beats per minute (0 = no reading yet).
static HEART_RATE_BPM: SensorMutex<u16> = SensorMutex::new(0);

/// Cumulative step count since last reset.
static STEP_COUNT: SensorMutex<u32> = SensorMutex::new(0);

/// Raw accelerometer sample (x, y, z) in milli-g (thousandths of 1 g).
/// Updated at up to 50 Hz by the IMU driver.
static ACCEL_SAMPLE: SensorMutex<(i16, i16, i16)> = SensorMutex::new((0, 0, 0));

/// Update the heart-rate sensor reading.
pub fn set_heart_rate(bpm: u16) {
    *HEART_RATE_BPM.lock() = bpm;
}

/// Read the most recent heart-rate value.
pub fn heart_rate() -> u16 {
    *HEART_RATE_BPM.lock()
}

/// Increment the step counter (called by the pedometer ISR).
pub fn increment_steps(delta: u32) {
    let mut s = STEP_COUNT.lock();
    *s = s.saturating_add(delta);
}

/// Reset the step counter to zero (e.g., at midnight).
pub fn reset_steps() {
    *STEP_COUNT.lock() = 0;
}

/// Read the current step count.
pub fn step_count() -> u32 {
    *STEP_COUNT.lock()
}

/// Update the accelerometer sample (x, y, z in milli-g).
pub fn set_accel(x: i16, y: i16, z: i16) {
    *ACCEL_SAMPLE.lock() = (x, y, z);
}

/// Read the most recent accelerometer sample (x, y, z) in milli-g.
pub fn accel() -> (i16, i16, i16) {
    *ACCEL_SAMPLE.lock()
}

// ── Watch-face rendering ──────────────────────────────────────────────────────

const FB_STRIDE: u32 = 1920;

const WF_BG: u32 = 0xFF000000;
const WF_AMBER: u32 = 0xFFF59E0B;
const WF_RING: u32 = 0xFF2D2D2D;
const WF_STEP: u32 = 0xFF10B981;
const WF_HR: u32 = 0xFFEF4444;

/// Render the currently active watch face onto the kernel framebuffer.
///
/// Draws a circular bezel, a solid dial background, and a pair of bar
/// indicators for heart rate and step count derived from the live sensor
/// stubs above.  Ambient mode (low-power) dims the background to near-black.
///
/// # Arguments
/// * `fb`     — ARGB framebuffer slice for the full display
/// * `x`, `y` — top-left corner of the watch-face bounding box (pixels)
/// * `w`, `h` — width and height of the bounding box (pixels)
/// * `ambient`— `true` to render in low-power ambient mode
pub fn render(fb: &mut [u32], x: u32, y: u32, w: u32, h: u32, ambient: bool) {
    if w == 0 || h == 0 {
        return;
    }

    let bg = if ambient { 0xFF060606 } else { WF_BG };
    let cx = x.saturating_add(w / 2);
    let cy = y.saturating_add(h / 2);
    let radius = w.min(h) / 2;

    // Fill bounding box with background
    for row in 0..h {
        let py = y.saturating_add(row);
        for col in 0..w {
            let px = x.saturating_add(col);
            let idx = py.saturating_mul(FB_STRIDE).saturating_add(px) as usize;
            if idx < fb.len() {
                fb[idx] = bg;
            }
        }
    }

    // Draw circular bezel ring (1-pixel outline)
    if radius >= 2 {
        draw_ring(fb, cx, cy, radius, WF_RING);
        if radius > 2 {
            draw_ring(fb, cx, cy, radius - 1, WF_RING);
        }
    }

    if !ambient {
        // Heart-rate bar (left side, red)
        let hr = heart_rate().min(220) as u32;
        let hr_bar_h = hr.saturating_mul(h / 2) / 220;
        let bar_x = x.saturating_add(4);
        for row in 0..hr_bar_h {
            let py = y
                .saturating_add(h / 2)
                .saturating_add(h / 2 - 1)
                .saturating_sub(row);
            let idx = py.saturating_mul(FB_STRIDE).saturating_add(bar_x) as usize;
            if idx + 2 < fb.len() {
                fb[idx] = WF_HR;
                fb[idx + 1] = WF_HR;
            }
        }

        // Step-count bar (right side, green); 10 000 steps = full bar
        let steps = step_count().min(10_000);
        let step_bar_h = steps.saturating_mul(h / 2) / 10_000;
        let bar_x2 = x.saturating_add(w).saturating_sub(6);
        for row in 0..step_bar_h {
            let py = y
                .saturating_add(h / 2)
                .saturating_add(h / 2 - 1)
                .saturating_sub(row);
            let idx = py.saturating_mul(FB_STRIDE).saturating_add(bar_x2) as usize;
            if idx + 2 < fb.len() {
                fb[idx] = WF_STEP;
                fb[idx + 1] = WF_STEP;
            }
        }
    }
}

/// Draw a 1-pixel-thick circle outline using the midpoint algorithm.
fn draw_ring(fb: &mut [u32], cx: u32, cy: u32, r: u32, color: u32) {
    let mut x = r;
    let mut y = 0u32;
    let mut err = 0i32;
    while x >= y {
        plot8(fb, cx, cy, x, y, color);
        y = y.saturating_add(1);
        if err <= 0 {
            err += (2 * y as i32) + 1;
        }
        if err > 0 {
            x = x.saturating_sub(1);
            err -= (2 * x as i32) + 1;
        }
    }
}

fn plot8(fb: &mut [u32], cx: u32, cy: u32, dx: u32, dy: u32, color: u32) {
    let pts = [
        (cx.saturating_add(dx), cy.saturating_add(dy)),
        (cx.saturating_sub(dx), cy.saturating_add(dy)),
        (cx.saturating_add(dx), cy.saturating_sub(dy)),
        (cx.saturating_sub(dx), cy.saturating_sub(dy)),
        (cx.saturating_add(dy), cy.saturating_add(dx)),
        (cx.saturating_sub(dy), cy.saturating_add(dx)),
        (cx.saturating_add(dy), cy.saturating_sub(dx)),
        (cx.saturating_sub(dy), cy.saturating_sub(dx)),
    ];
    for (px, py) in pts {
        let idx = py.saturating_mul(FB_STRIDE).saturating_add(px) as usize;
        if idx < fb.len() {
            fb[idx] = color;
        }
    }
}
