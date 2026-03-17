pub mod ai_home;
pub mod automation;
/// Smart home / IoT framework for Genesis
///
/// Device discovery, Matter/Thread/Zigbee/Z-Wave,
/// home automation, scenes, routines, device groups,
/// and AI-powered home intelligence.
///
/// Original implementation for Hoags OS.
pub mod device_manager;
pub mod protocols;
pub mod scenes;

use crate::{serial_print, serial_println};

pub fn init() {
    device_manager::init();
    protocols::init();
    automation::init();
    scenes::init();
    ai_home::init();
    serial_println!("  Smart home initialized (Matter, Thread, Zigbee, AI automation)");
}
