pub mod ai_camera;
/// Camera framework for Genesis
///
/// Camera HAL, multi-camera, HDR, night mode,
/// video recording, computational photography,
/// barcode/QR scanning, AR camera, AI scene detection.
///
/// Original implementation for Hoags OS.
pub mod camera_hal;
pub mod processing;
pub mod scanner;
pub mod video;

use crate::{serial_print, serial_println};

pub fn init() {
    camera_hal::init();
    processing::init();
    video::init();
    scanner::init();
    ai_camera::init();
    serial_println!("  Camera framework initialized (HAL, HDR, video, QR, AI scene)");
}
