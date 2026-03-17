pub mod ai_media;
pub mod camera;
pub mod media_player;
pub mod screen_capture;
/// Media framework for Genesis — video, camera, screen recording
///
/// Provides: video codec pipeline, camera HAL, screen capture,
/// and media playback/recording APIs.
///
/// Inspired by: Android MediaCodec, GStreamer, FFmpeg. All code is original.
pub mod video;

use crate::{serial_print, serial_println};

pub fn init() {
    video::init();
    camera::init();
    screen_capture::init();
    media_player::init();
    ai_media::init();
    serial_println!("  Media framework initialized (AI tagging, face grouping, audio classify)");
}
