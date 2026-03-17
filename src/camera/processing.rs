/// Computational photography for Genesis
///
/// HDR, night mode, portrait mode (depth),
/// panorama, super resolution, noise reduction.
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum PhotoMode {
    Auto,
    Hdr,
    Night,
    Portrait,
    Panorama,
    ProManual,
    SuperResolution,
    Macro,
    TimeLapse,
    SlowMotion,
}

struct ProcessingPipeline {
    active_mode: PhotoMode,
    hdr_brackets: u8,       // number of exposures
    night_exposure_ms: u32, // total exposure time
    portrait_blur_radius: u8,
    noise_reduction_level: u8, // 0-10
    sharpening_level: u8,      // 0-10
    ai_scene_enhance: bool,
    raw_capture: bool,
    processing: bool,
}

static PROCESSING: Mutex<Option<ProcessingPipeline>> = Mutex::new(None);

impl ProcessingPipeline {
    fn new() -> Self {
        ProcessingPipeline {
            active_mode: PhotoMode::Auto,
            hdr_brackets: 3,
            night_exposure_ms: 3000,
            portrait_blur_radius: 8,
            noise_reduction_level: 5,
            sharpening_level: 5,
            ai_scene_enhance: true,
            raw_capture: false,
            processing: false,
        }
    }

    fn set_mode(&mut self, mode: PhotoMode) {
        self.active_mode = mode;
        match mode {
            PhotoMode::Hdr => {
                self.hdr_brackets = 3;
            }
            PhotoMode::Night => {
                self.night_exposure_ms = 3000;
                self.noise_reduction_level = 8;
            }
            PhotoMode::Portrait => {
                self.portrait_blur_radius = 8;
            }
            PhotoMode::SuperResolution => {
                self.sharpening_level = 8;
            }
            _ => {}
        }
    }

    fn should_use_hdr(&self, scene_dynamic_range: u32) -> bool {
        scene_dynamic_range > 10 // stops of dynamic range
    }

    fn should_use_night_mode(&self, ambient_lux: u32) -> bool {
        ambient_lux < 10
    }
}

pub fn init() {
    let mut p = PROCESSING.lock();
    *p = Some(ProcessingPipeline::new());
    serial_println!("    Camera: computational photography (HDR, night, portrait) ready");
}
