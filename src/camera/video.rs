/// Video recording for Genesis
///
/// 4K/8K recording, stabilization, slow-motion,
/// time-lapse, video codec selection.
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum VideoResolution {
    Hd720p,
    FullHd1080p,
    Uhd4K,
    Uhd8K,
}

#[derive(Clone, Copy, PartialEq)]
pub enum VideoCodec {
    H264,
    H265,
    Av1,
    Vp9,
}

#[derive(Clone, Copy, PartialEq)]
pub enum StabilizationMode {
    Off,
    Ois,
    Eis,
    Hybrid,
}

struct VideoRecorder {
    recording: bool,
    resolution: VideoResolution,
    fps: u16,
    codec: VideoCodec,
    stabilization: StabilizationMode,
    bitrate_kbps: u32,
    duration_secs: u64,
    file_size_bytes: u64,
    audio_enabled: bool,
    hdr_video: bool,
}

static VIDEO: Mutex<Option<VideoRecorder>> = Mutex::new(None);

impl VideoRecorder {
    fn new() -> Self {
        VideoRecorder {
            recording: false,
            resolution: VideoResolution::Uhd4K,
            fps: 30,
            codec: VideoCodec::H265,
            stabilization: StabilizationMode::Hybrid,
            bitrate_kbps: 40000,
            duration_secs: 0,
            file_size_bytes: 0,
            audio_enabled: true,
            hdr_video: false,
        }
    }

    fn start(&mut self) {
        self.recording = true;
        self.duration_secs = 0;
        self.file_size_bytes = 0;
    }

    fn stop(&mut self) -> u64 {
        self.recording = false;
        self.file_size_bytes
    }

    fn estimated_file_size_mb(&self, duration_secs: u64) -> u64 {
        // bitrate_kbps * duration / 8 / 1024 = size in MB
        self.bitrate_kbps as u64 * duration_secs / 8 / 1024
    }

    fn max_resolution_for_fps(&self, fps: u16) -> VideoResolution {
        match fps {
            0..=30 => VideoResolution::Uhd8K,
            31..=60 => VideoResolution::Uhd4K,
            61..=120 => VideoResolution::FullHd1080p,
            _ => VideoResolution::Hd720p,
        }
    }
}

pub fn init() {
    let mut v = VIDEO.lock();
    *v = Some(VideoRecorder::new());
    serial_println!("    Camera: video recording (4K/8K, H.265, stabilization) ready");
}
