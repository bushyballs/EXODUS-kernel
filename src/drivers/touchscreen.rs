use crate::sync::Mutex;
/// Touchscreen driver for Genesis — multi-touch, gesture recognition, palm rejection
///
/// Provides a complete touchscreen input stack:
///   - Multi-touch point tracking (up to 10 simultaneous contacts)
///   - Gesture recognition (tap, double-tap, swipe, pinch, rotate, long-press)
///   - Palm rejection via contact size thresholds
///   - Pressure sensitivity with configurable curves
///   - Screen-to-touch calibration (3-point affine transform, Q16 fixed-point)
///   - I2C-based controller communication (FT5x06/GT911 style)
///
/// Inspired by: Linux multi-touch protocol (Documentation/input/multi-touch-protocol.rst),
/// Android InputReader, libinput gesture handling. All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::VecDeque;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers (16 fractional bits)
// ---------------------------------------------------------------------------

const Q16_SHIFT: i32 = 16;
const Q16_ONE: i32 = 1 << Q16_SHIFT;

/// Multiply two Q16 values
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> Q16_SHIFT) as i32
}

// ---------------------------------------------------------------------------
// I2C helpers — bit-bang over GPIO pins (controller-specific addresses)
// ---------------------------------------------------------------------------

const I2C_TOUCH_ADDR: u8 = 0x38; // FT5x06 default
const I2C_STATUS_PORT: u16 = 0xC100;
const I2C_DATA_PORT: u16 = 0xC104;
const I2C_CTRL_PORT: u16 = 0xC108;

fn i2c_wait_ready() {
    for _ in 0..5000 {
        let status = crate::io::inb(I2C_STATUS_PORT);
        if status & 0x01 != 0 {
            return;
        }
        core::hint::spin_loop();
    }
}

fn i2c_read_reg(addr: u8, reg: u8) -> u8 {
    // Start condition + slave address + write bit
    crate::io::outb(I2C_CTRL_PORT, 0x01); // START
    crate::io::outb(I2C_DATA_PORT, (addr << 1) | 0x00);
    i2c_wait_ready();
    crate::io::outb(I2C_DATA_PORT, reg);
    i2c_wait_ready();
    // Repeated start + read
    crate::io::outb(I2C_CTRL_PORT, 0x01);
    crate::io::outb(I2C_DATA_PORT, (addr << 1) | 0x01);
    i2c_wait_ready();
    let val = crate::io::inb(I2C_DATA_PORT);
    crate::io::outb(I2C_CTRL_PORT, 0x02); // STOP
    val
}

fn i2c_write_reg(addr: u8, reg: u8, val: u8) {
    crate::io::outb(I2C_CTRL_PORT, 0x01); // START
    crate::io::outb(I2C_DATA_PORT, (addr << 1) | 0x00);
    i2c_wait_ready();
    crate::io::outb(I2C_DATA_PORT, reg);
    i2c_wait_ready();
    crate::io::outb(I2C_DATA_PORT, val);
    i2c_wait_ready();
    crate::io::outb(I2C_CTRL_PORT, 0x02); // STOP
}

// ---------------------------------------------------------------------------
// Touch point and contact data
// ---------------------------------------------------------------------------

/// Maximum simultaneous touch contacts
const MAX_CONTACTS: usize = 10;

/// Pressure threshold below which contact is ignored
const MIN_PRESSURE: i32 = 5;

/// Contact size threshold for palm rejection (Q16: 40.0)
const PALM_SIZE_THRESHOLD: i32 = 40 * Q16_ONE;

/// Long-press duration in milliseconds
const LONG_PRESS_MS: u64 = 500;

/// Double-tap window in milliseconds
const DOUBLE_TAP_MS: u64 = 300;

/// Swipe minimum distance (pixels)
const SWIPE_MIN_DIST: i32 = 50;

/// A single touch contact point
#[derive(Debug, Clone, Copy)]
pub struct TouchContact {
    /// Tracking ID (-1 = inactive)
    pub tracking_id: i32,
    /// X position in screen coordinates
    pub x: i32,
    /// Y position in screen coordinates
    pub y: i32,
    /// Pressure (0-255)
    pub pressure: i32,
    /// Major axis of contact ellipse (Q16)
    pub major: i32,
    /// Minor axis of contact ellipse (Q16)
    pub minor: i32,
    /// Whether this contact is classified as a palm
    pub is_palm: bool,
    /// Timestamp when contact began (ms)
    pub start_time: u64,
    /// Initial X when contact began
    pub start_x: i32,
    /// Initial Y when contact began
    pub start_y: i32,
}

impl TouchContact {
    const fn inactive() -> Self {
        TouchContact {
            tracking_id: -1,
            x: 0,
            y: 0,
            pressure: 0,
            major: 0,
            minor: 0,
            is_palm: false,
            start_time: 0,
            start_x: 0,
            start_y: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Gesture types
// ---------------------------------------------------------------------------

/// Recognized gesture
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    None,
    Tap {
        x: i32,
        y: i32,
    },
    DoubleTap {
        x: i32,
        y: i32,
    },
    LongPress {
        x: i32,
        y: i32,
    },
    SwipeUp {
        x: i32,
        y: i32,
        distance: i32,
    },
    SwipeDown {
        x: i32,
        y: i32,
        distance: i32,
    },
    SwipeLeft {
        x: i32,
        y: i32,
        distance: i32,
    },
    SwipeRight {
        x: i32,
        y: i32,
        distance: i32,
    },
    PinchIn {
        center_x: i32,
        center_y: i32,
        scale_q16: i32,
    },
    PinchOut {
        center_x: i32,
        center_y: i32,
        scale_q16: i32,
    },
    Rotate {
        center_x: i32,
        center_y: i32,
        angle_q16: i32,
    },
}

/// Touch event for the event queue
#[derive(Debug, Clone, Copy)]
pub struct TouchEvent {
    pub contact: TouchContact,
    pub event_type: TouchEventType,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchEventType {
    Down,
    Move,
    Up,
    Cancel,
}

// ---------------------------------------------------------------------------
// Calibration — 3-point affine transform (Q16)
// ---------------------------------------------------------------------------

/// Calibration matrix: maps raw touch coordinates to screen coordinates
/// screen_x = a*raw_x + b*raw_y + c
/// screen_y = d*raw_x + e*raw_y + f
/// All values in Q16 fixed-point
#[derive(Debug, Clone, Copy)]
pub struct CalibrationMatrix {
    pub a: i32,
    pub b: i32,
    pub c: i32,
    pub d: i32,
    pub e: i32,
    pub f: i32,
}

impl CalibrationMatrix {
    /// Identity calibration (1:1 mapping)
    const fn identity() -> Self {
        CalibrationMatrix {
            a: Q16_ONE,
            b: 0,
            c: 0,
            d: 0,
            e: Q16_ONE,
            f: 0,
        }
    }

    /// Apply calibration to raw touch coordinates
    fn apply(&self, raw_x: i32, raw_y: i32) -> (i32, i32) {
        let sx = q16_mul(self.a, raw_x)
            .saturating_add(q16_mul(self.b, raw_y))
            .saturating_add(self.c);
        let sy = q16_mul(self.d, raw_x)
            .saturating_add(q16_mul(self.e, raw_y))
            .saturating_add(self.f);
        // Convert back from Q16 to integer pixels
        (sx >> Q16_SHIFT, sy >> Q16_SHIFT)
    }
}

/// Calibration point pair (screen target, raw touch reading)
#[derive(Debug, Clone, Copy)]
pub struct CalibrationPoint {
    pub screen_x: i32,
    pub screen_y: i32,
    pub raw_x: i32,
    pub raw_y: i32,
}

/// Compute calibration matrix from 3 reference points
/// Uses the standard 3-point affine calibration algorithm
pub fn compute_calibration(pts: &[CalibrationPoint; 3]) -> CalibrationMatrix {
    // Determinant of the raw-coordinate matrix (Q16)
    let det = (pts[0].raw_x - pts[2].raw_x) * (pts[1].raw_y - pts[2].raw_y)
        - (pts[1].raw_x - pts[2].raw_x) * (pts[0].raw_y - pts[2].raw_y);

    if det == 0 {
        return CalibrationMatrix::identity();
    }

    // Compute matrix coefficients scaled to Q16
    let a = (((pts[0].screen_x - pts[2].screen_x) * (pts[1].raw_y - pts[2].raw_y)
        - (pts[1].screen_x - pts[2].screen_x) * (pts[0].raw_y - pts[2].raw_y))
        << Q16_SHIFT)
        / det;

    let b = (((pts[1].screen_x - pts[2].screen_x) * (pts[0].raw_x - pts[2].raw_x)
        - (pts[0].screen_x - pts[2].screen_x) * (pts[1].raw_x - pts[2].raw_x))
        << Q16_SHIFT)
        / det;

    let c = (pts[0].screen_x << Q16_SHIFT)
        - q16_mul(a, pts[0].raw_x << Q16_SHIFT)
        - q16_mul(b, pts[0].raw_y << Q16_SHIFT);

    let d = (((pts[0].screen_y - pts[2].screen_y) * (pts[1].raw_y - pts[2].raw_y)
        - (pts[1].screen_y - pts[2].screen_y) * (pts[0].raw_y - pts[2].raw_y))
        << Q16_SHIFT)
        / det;

    let e = (((pts[1].screen_y - pts[2].screen_y) * (pts[0].raw_x - pts[2].raw_x)
        - (pts[0].screen_y - pts[2].screen_y) * (pts[1].raw_x - pts[2].raw_x))
        << Q16_SHIFT)
        / det;

    let f = (pts[0].screen_y << Q16_SHIFT)
        - q16_mul(d, pts[0].raw_x << Q16_SHIFT)
        - q16_mul(e, pts[0].raw_y << Q16_SHIFT);

    CalibrationMatrix { a, b, c, d, e, f }
}

// ---------------------------------------------------------------------------
// Pressure sensitivity curve
// ---------------------------------------------------------------------------

/// Pressure curve type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressureCurve {
    /// Linear mapping (raw pressure = output pressure)
    Linear,
    /// Soft curve — lighter touches register more easily (square root approx)
    Soft,
    /// Firm curve — requires harder press (square approx)
    Firm,
}

/// Apply pressure curve mapping (input 0-255, output 0-255)
fn apply_pressure_curve(raw: i32, curve: PressureCurve) -> i32 {
    let clamped = raw.clamp(0, 255);
    match curve {
        PressureCurve::Linear => clamped,
        PressureCurve::Soft => {
            // Approximate sqrt via Q16: sqrt(x/255)*255
            // Newton's method: 2 iterations from x/2
            let x_q16 = (clamped << Q16_SHIFT) / 255;
            let mut guess = x_q16 / 2 + Q16_ONE / 2;
            if guess <= 0 {
                guess = 1;
            }
            // Two Newton iterations for sqrt
            guess = (guess + q16_mul(Q16_ONE, x_q16) / (guess >> Q16_SHIFT).max(1)) / 2;
            guess = (guess + q16_mul(Q16_ONE, x_q16) / (guess >> Q16_SHIFT).max(1)) / 2;
            (guess.saturating_mul(255) >> Q16_SHIFT).clamp(0, 255)
        }
        PressureCurve::Firm => {
            // Square curve: (x/255)^2 * 255
            let x_q16 = (clamped << Q16_SHIFT) / 255;
            let sq = q16_mul(x_q16, x_q16);
            (sq.saturating_mul(255) >> Q16_SHIFT).clamp(0, 255)
        }
    }
}

// ---------------------------------------------------------------------------
// Touchscreen controller state
// ---------------------------------------------------------------------------

/// Controller chip type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerType {
    FT5x06,
    GT911,
    Unknown,
}

pub struct TouchscreenState {
    /// Controller chip type
    controller: ControllerType,
    /// I2C address of the controller
    i2c_addr: u8,
    /// Whether controller is initialized
    initialized: bool,
    /// Screen dimensions for clamping
    screen_w: i32,
    screen_h: i32,
    /// Active contacts
    contacts: [TouchContact; MAX_CONTACTS],
    /// Previous frame contacts (for gesture detection)
    prev_contacts: [TouchContact; MAX_CONTACTS],
    /// Number of active contacts
    active_count: usize,
    /// Calibration matrix
    calibration: CalibrationMatrix,
    /// Pressure curve
    pressure_curve: PressureCurve,
    /// Palm rejection enabled
    palm_rejection: bool,
    /// Last tap time and position (for double-tap detection)
    last_tap_time: u64,
    last_tap_x: i32,
    last_tap_y: i32,
    /// Gesture event queue
    gesture_queue: VecDeque<Gesture>,
    /// Touch event queue
    event_queue: VecDeque<TouchEvent>,
    /// Next tracking ID
    next_tracking_id: i32,
    /// Two-finger pinch/rotate initial distance (Q16)
    pinch_initial_dist: i32,
}

impl TouchscreenState {
    const fn new() -> Self {
        const EMPTY: TouchContact = TouchContact::inactive();
        TouchscreenState {
            controller: ControllerType::Unknown,
            i2c_addr: I2C_TOUCH_ADDR,
            initialized: false,
            screen_w: 1024,
            screen_h: 768,
            contacts: [EMPTY; MAX_CONTACTS],
            prev_contacts: [EMPTY; MAX_CONTACTS],
            active_count: 0,
            calibration: CalibrationMatrix::identity(),
            pressure_curve: PressureCurve::Linear,
            palm_rejection: true,
            last_tap_time: 0,
            last_tap_x: 0,
            last_tap_y: 0,
            gesture_queue: VecDeque::new(),
            event_queue: VecDeque::new(),
            next_tracking_id: 1,
            pinch_initial_dist: 0,
        }
    }

    /// Detect which controller chip is present
    fn detect_controller(&mut self) {
        // Try FT5x06 (addr 0x38) — read chip ID register 0xA3
        let chip_id = i2c_read_reg(0x38, 0xA3);
        if chip_id == 0x55 || chip_id == 0x08 || chip_id == 0x06 {
            self.controller = ControllerType::FT5x06;
            self.i2c_addr = 0x38;
            return;
        }
        // Try GT911 (addr 0x5D) — read product ID register 0x8140
        let prod_id = i2c_read_reg(0x5D, 0x81);
        if prod_id == 0x39 {
            self.controller = ControllerType::GT911;
            self.i2c_addr = 0x5D;
            return;
        }
        self.controller = ControllerType::Unknown;
    }

    /// Initialize the controller chip
    fn init_controller(&mut self) -> bool {
        self.detect_controller();
        match self.controller {
            ControllerType::FT5x06 => {
                // Set device mode to normal (reg 0x00 = 0)
                i2c_write_reg(self.i2c_addr, 0x00, 0x00);
                // Set interrupt mode to polling (reg 0xA4 = 0)
                i2c_write_reg(self.i2c_addr, 0xA4, 0x00);
                // Set active touch threshold (reg 0x80)
                i2c_write_reg(self.i2c_addr, 0x80, 0x1E);
                true
            }
            ControllerType::GT911 => {
                // GT911 configuration write at 0x8047
                i2c_write_reg(self.i2c_addr, 0x80, 0x01); // Enable touch
                true
            }
            ControllerType::Unknown => false,
        }
    }

    /// Read raw touch data from the controller
    fn read_raw_contacts(&mut self) -> usize {
        let count = match self.controller {
            ControllerType::FT5x06 => {
                // Register 0x02 holds touch point count
                let n = i2c_read_reg(self.i2c_addr, 0x02) & 0x0F;
                n.min(MAX_CONTACTS as u8) as usize
            }
            ControllerType::GT911 => {
                let status = i2c_read_reg(self.i2c_addr, 0x81);
                if status & 0x80 == 0 {
                    return 0;
                }
                let n = status & 0x0F;
                // Clear status
                i2c_write_reg(self.i2c_addr, 0x81, 0x00);
                n.min(MAX_CONTACTS as u8) as usize
            }
            ControllerType::Unknown => 0,
        };

        let now = crate::time::clock::uptime_ms();

        // Save previous frame
        self.prev_contacts = self.contacts;

        for i in 0..count {
            let base_reg = match self.controller {
                ControllerType::FT5x06 => 0x03u8.saturating_add((i as u8).saturating_mul(6)),
                ControllerType::GT911 => 0x82u8.saturating_add((i as u8).saturating_mul(8)),
                ControllerType::Unknown => 0,
            };

            // Read raw X, Y, pressure
            let xh = i2c_read_reg(self.i2c_addr, base_reg) as i32;
            let xl = i2c_read_reg(self.i2c_addr, base_reg + 1) as i32;
            let yh = i2c_read_reg(self.i2c_addr, base_reg + 2) as i32;
            let yl = i2c_read_reg(self.i2c_addr, base_reg + 3) as i32;
            let raw_pressure = i2c_read_reg(self.i2c_addr, base_reg + 4) as i32;
            let contact_size = i2c_read_reg(self.i2c_addr, base_reg + 5) as i32;

            let raw_x = ((xh & 0x0F) << 8) | xl;
            let raw_y = ((yh & 0x0F) << 8) | yl;

            // Apply calibration
            let (screen_x, screen_y) = self.calibration.apply(raw_x, raw_y);
            let sx = screen_x.clamp(0, self.screen_w - 1);
            let sy = screen_y.clamp(0, self.screen_h - 1);

            // Apply pressure curve
            let pressure = apply_pressure_curve(raw_pressure, self.pressure_curve);

            // Contact size for palm rejection (Q16)
            let major_q16 = contact_size << Q16_SHIFT;

            // Determine if this is a new contact or continuation
            let is_new = self.contacts[i].tracking_id < 0;
            let tracking_id = if is_new {
                let id = self.next_tracking_id;
                self.next_tracking_id = self.next_tracking_id.saturating_add(1);
                id
            } else {
                self.contacts[i].tracking_id
            };

            let is_palm = self.palm_rejection && major_q16 > PALM_SIZE_THRESHOLD;

            self.contacts[i] = TouchContact {
                tracking_id,
                x: sx,
                y: sy,
                pressure,
                major: major_q16,
                minor: major_q16 / 2,
                is_palm,
                start_time: if is_new {
                    now
                } else {
                    self.contacts[i].start_time
                },
                start_x: if is_new { sx } else { self.contacts[i].start_x },
                start_y: if is_new { sy } else { self.contacts[i].start_y },
            };

            // Emit touch event
            if !is_palm && pressure > MIN_PRESSURE {
                let etype = if is_new {
                    TouchEventType::Down
                } else {
                    TouchEventType::Move
                };
                self.event_queue.push_back(TouchEvent {
                    contact: self.contacts[i],
                    event_type: etype,
                    timestamp: now,
                });
            }
        }

        // Mark released contacts
        for i in count..MAX_CONTACTS {
            if self.contacts[i].tracking_id >= 0 {
                let now = crate::time::clock::uptime_ms();
                self.event_queue.push_back(TouchEvent {
                    contact: self.contacts[i],
                    event_type: TouchEventType::Up,
                    timestamp: now,
                });
                self.contacts[i] = TouchContact::inactive();
            }
        }

        self.active_count = count;
        count
    }

    /// Integer square root approximation
    fn isqrt(val: i32) -> i32 {
        if val <= 0 {
            return 0;
        }
        let mut x = val;
        let mut y = (x + 1) / 2;
        while y < x {
            x = y;
            y = (x + val / x) / 2;
        }
        x
    }

    /// Detect gestures from the current and previous contact state
    fn detect_gestures(&mut self) {
        let now = crate::time::clock::uptime_ms();

        // --- Single-finger gestures ---
        if self.active_count == 0 {
            // Check if a finger was just lifted
            for i in 0..MAX_CONTACTS {
                let prev = &self.prev_contacts[i];
                if prev.tracking_id >= 0 && !prev.is_palm {
                    let dx = prev.x - prev.start_x;
                    let dy = prev.y - prev.start_y;
                    let dist =
                        Self::isqrt(dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy)));
                    let hold_time = now.saturating_sub(prev.start_time);

                    if dist < SWIPE_MIN_DIST {
                        if hold_time >= LONG_PRESS_MS {
                            self.gesture_queue.push_back(Gesture::LongPress {
                                x: prev.x,
                                y: prev.y,
                            });
                        } else if now.saturating_sub(self.last_tap_time) < DOUBLE_TAP_MS
                            && (prev.x - self.last_tap_x).abs() < 30
                            && (prev.y - self.last_tap_y).abs() < 30
                        {
                            self.gesture_queue.push_back(Gesture::DoubleTap {
                                x: prev.x,
                                y: prev.y,
                            });
                            self.last_tap_time = 0; // Reset
                        } else {
                            self.gesture_queue.push_back(Gesture::Tap {
                                x: prev.x,
                                y: prev.y,
                            });
                            self.last_tap_time = now;
                            self.last_tap_x = prev.x;
                            self.last_tap_y = prev.y;
                        }
                    } else {
                        // Swipe direction — dominant axis
                        if dx.abs() > dy.abs() {
                            if dx > 0 {
                                self.gesture_queue.push_back(Gesture::SwipeRight {
                                    x: prev.start_x,
                                    y: prev.start_y,
                                    distance: dist,
                                });
                            } else {
                                self.gesture_queue.push_back(Gesture::SwipeLeft {
                                    x: prev.start_x,
                                    y: prev.start_y,
                                    distance: dist,
                                });
                            }
                        } else if dy > 0 {
                            self.gesture_queue.push_back(Gesture::SwipeDown {
                                x: prev.start_x,
                                y: prev.start_y,
                                distance: dist,
                            });
                        } else {
                            self.gesture_queue.push_back(Gesture::SwipeUp {
                                x: prev.start_x,
                                y: prev.start_y,
                                distance: dist,
                            });
                        }
                    }
                    break; // Only process first released contact
                }
            }
        }

        // --- Two-finger gestures (pinch / rotate) ---
        if self.active_count == 2 {
            let mut pts: Vec<(i32, i32)> = Vec::new();
            let mut prev_pts: Vec<(i32, i32)> = Vec::new();
            for i in 0..MAX_CONTACTS {
                if self.contacts[i].tracking_id >= 0 && !self.contacts[i].is_palm {
                    pts.push((self.contacts[i].x, self.contacts[i].y));
                }
                if self.prev_contacts[i].tracking_id >= 0 && !self.prev_contacts[i].is_palm {
                    prev_pts.push((self.prev_contacts[i].x, self.prev_contacts[i].y));
                }
            }

            if pts.len() == 2 && prev_pts.len() == 2 {
                let dx = pts[1].0 - pts[0].0;
                let dy = pts[1].1 - pts[0].1;
                let cur_dist =
                    Self::isqrt(dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy)));

                let pdx = prev_pts[1].0 - prev_pts[0].0;
                let pdy = prev_pts[1].1 - prev_pts[0].1;
                let prev_dist = Self::isqrt(
                    pdx.saturating_mul(pdx)
                        .saturating_add(pdy.saturating_mul(pdy)),
                );

                let center_x = (pts[0].0.saturating_add(pts[1].0)) / 2;
                let center_y = (pts[0].1.saturating_add(pts[1].1)) / 2;

                if prev_dist > 10 && cur_dist > 10 {
                    // Scale factor in Q16
                    let scale_q16 = (cur_dist << Q16_SHIFT) / prev_dist;
                    let delta = scale_q16.saturating_sub(Q16_ONE);

                    if delta > (Q16_ONE / 20) {
                        self.gesture_queue.push_back(Gesture::PinchOut {
                            center_x,
                            center_y,
                            scale_q16,
                        });
                    } else if delta < -(Q16_ONE / 20) {
                        self.gesture_queue.push_back(Gesture::PinchIn {
                            center_x,
                            center_y,
                            scale_q16,
                        });
                    }

                    // Rotation: cross product gives sine of angle (Q16 approx)
                    // cross = pdx*dy - pdy*dx, dot = pdx*dx + pdy*dy
                    let cross = pdx
                        .saturating_mul(dy)
                        .saturating_sub(pdy.saturating_mul(dx));
                    let dot = pdx
                        .saturating_mul(dx)
                        .saturating_add(pdy.saturating_mul(dy));
                    if dot.abs() > 10 {
                        let angle_q16 = (cross << Q16_SHIFT) / dot;
                        if angle_q16.abs() > (Q16_ONE / 30) {
                            self.gesture_queue.push_back(Gesture::Rotate {
                                center_x,
                                center_y,
                                angle_q16,
                            });
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static TOUCHSCREEN: Mutex<TouchscreenState> = Mutex::new(TouchscreenState::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the touchscreen driver
pub fn init() {
    let mut ts = TOUCHSCREEN.lock();
    if ts.init_controller() {
        ts.initialized = true;
        drop(ts);
        super::register("touchscreen", super::DeviceType::Other);
        serial_println!(
            "  Touchscreen: driver ready (multi-touch, {} contacts max)",
            MAX_CONTACTS
        );
    } else {
        serial_println!("  Touchscreen: no controller detected");
    }
}

/// Poll the touchscreen for new data (call periodically or from interrupt)
pub fn poll() {
    let mut ts = TOUCHSCREEN.lock();
    if !ts.initialized {
        return;
    }
    ts.read_raw_contacts();
    ts.detect_gestures();
}

/// Pop the next touch event from the queue
pub fn pop_event() -> Option<TouchEvent> {
    TOUCHSCREEN.lock().event_queue.pop_front()
}

/// Pop the next recognized gesture from the queue
pub fn pop_gesture() -> Option<Gesture> {
    TOUCHSCREEN.lock().gesture_queue.pop_front()
}

/// Get all currently active (non-palm) touch contacts
pub fn active_contacts() -> Vec<TouchContact> {
    let ts = TOUCHSCREEN.lock();
    ts.contacts
        .iter()
        .filter(|c| c.tracking_id >= 0 && !c.is_palm && c.pressure > MIN_PRESSURE)
        .copied()
        .collect()
}

/// Set screen dimensions (for coordinate clamping)
pub fn set_screen_size(w: i32, h: i32) {
    let mut ts = TOUCHSCREEN.lock();
    ts.screen_w = w;
    ts.screen_h = h;
}

/// Set calibration matrix directly
pub fn set_calibration(matrix: CalibrationMatrix) {
    TOUCHSCREEN.lock().calibration = matrix;
}

/// Run 3-point calibration from reference points
pub fn calibrate(points: &[CalibrationPoint; 3]) {
    let matrix = compute_calibration(points);
    TOUCHSCREEN.lock().calibration = matrix;
    serial_println!(
        "  Touchscreen: calibration updated (a={:#X} b={:#X})",
        matrix.a,
        matrix.b
    );
}

/// Set the pressure curve
pub fn set_pressure_curve(curve: PressureCurve) {
    TOUCHSCREEN.lock().pressure_curve = curve;
}

/// Enable or disable palm rejection
pub fn set_palm_rejection(enabled: bool) {
    TOUCHSCREEN.lock().palm_rejection = enabled;
}

/// Get the number of currently active contacts
pub fn active_count() -> usize {
    TOUCHSCREEN.lock().active_count
}
