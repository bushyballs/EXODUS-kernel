pub mod ai_location;
pub mod geofencing;
pub mod indoor;
/// Location services for Genesis
///
/// GPS/network provider, geofencing,
/// indoor positioning, AI predictions.
pub mod provider;

use crate::{serial_print, serial_println};

pub fn init() {
    serial_println!("[LOCATION] Initializing location subsystem...");

    provider::init();
    geofencing::init();
    indoor::init();
    ai_location::init();

    serial_println!("[LOCATION] Location subsystem initialized");
}
