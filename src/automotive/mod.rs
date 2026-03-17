pub mod driving_mode;
pub mod obd;
/// Automotive / Android Auto framework for Genesis
///
/// Car projection, vehicle HAL, OBD-II diagnostics,
/// navigation integration, hands-free, driving mode,
/// ADAS integration, and satellite communication.
///
/// Original implementation for Hoags OS.
pub mod projection;
pub mod satellite;
pub mod vehicle_hal;

use crate::{serial_print, serial_println};

pub fn init() {
    projection::init();
    vehicle_hal::init();
    obd::init();
    driving_mode::init();
    satellite::init();
    serial_println!("  Automotive initialized (car projection, OBD-II, satellite comm)");
}
