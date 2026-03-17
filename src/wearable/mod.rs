pub mod ble_sync;
pub mod companion;
pub mod display_round;
pub mod sensors;
pub mod watch_apps;
pub mod watch_complication;
/// Wearable framework for Genesis
///
/// Watch face, complications, fitness companion,
/// health sync, notification mirroring, watch apps.
///
/// Subsystems:
///   - watch_face:      customisable faces, complications, ambient mode
///   - companion:       phone-sync state machine (pairing, battery, health)
///   - watch_apps:      on-device mini app runner
///   - watch_complication: complication data providers
///   - sensors:         heart-rate, step counter, gyroscope, GPS
///   - display_round:   GC9A01 round-LCD 240×240 RGB565 driver + primitives
///   - ble_sync:        BLE phone bridge (notifications, health upload, OTA)
///
/// Original implementation for Hoags OS.
pub mod watch_face;

use crate::{serial_print, serial_println};

pub fn init() {
    watch_face::init();
    companion::init();
    watch_apps::init();
    sensors::init();
    display_round::init();
    ble_sync::init();
    serial_println!(
        "  Wearable framework initialized (watch face, companion, apps, sensors, display, BLE)"
    );
}
