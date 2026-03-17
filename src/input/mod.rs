pub mod accelerometer;
pub mod compass;
pub mod evdev;
pub mod gamepad;
pub mod gestures;
pub mod gyroscope;
/// Input framework for Genesis
///
/// Stylus, gamepad, touch gestures.
pub mod stylus;
pub mod touchpad;

use crate::{serial_print, serial_println};

pub fn init() {
    stylus::init();
    gamepad::init();
    gestures::init();
    evdev::init();
    touchpad::init();
    accelerometer::init();
    gyroscope::init();
    compass::init();
    serial_println!("  Input framework initialized (stylus, gamepad, gestures, evdev, touchpad, accel, gyro, compass)");
}
