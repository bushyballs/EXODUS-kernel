/// Sensor framework for Genesis
///
/// Unified sensor subsystem: registration, polling, fusion, and HAL.

pub mod framework;
pub mod accelerometer;
pub mod gyroscope;
pub mod magnetometer;
pub mod barometer;
pub mod light;
pub mod proximity;
pub mod temperature;
pub mod humidity;
pub mod gps;
pub mod fusion;
pub mod hal;

use crate::{serial_print, serial_println};

pub fn init() {
    framework::init();
    hal::init();
    accelerometer::init();
    gyroscope::init();
    magnetometer::init();
    barometer::init();
    light::init();
    proximity::init();
    temperature::init();
    humidity::init();
    gps::init();
    fusion::init();
    serial_println!("  Sensor framework initialized");
}
