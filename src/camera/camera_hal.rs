use crate::sync::Mutex;
/// Camera HAL for Genesis
///
/// Sensor abstraction, resolution/FPS config,
/// flash control, autofocus, exposure, white balance.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum CameraFacing {
    Back,
    Front,
    External,
}

#[derive(Clone, Copy, PartialEq)]
pub enum AutofocusMode {
    Off,
    Auto,
    Continuous,
    Macro,
    Infinity,
}

#[derive(Clone, Copy, PartialEq)]
pub enum FlashMode {
    Off,
    On,
    Auto,
    Torch,
}

struct CameraSensor {
    id: u32,
    facing: CameraFacing,
    megapixels: u32, // x10 (e.g., 480 = 48MP)
    max_width: u32,
    max_height: u32,
    max_fps: u16,
    has_ois: bool, // optical image stabilization
    has_autofocus: bool,
    has_flash: bool,
    focal_length_mm: u16, // x10
    aperture: u16,        // x10 (e.g., 18 = f/1.8)
}

struct CameraState {
    active_sensor: Option<u32>,
    width: u32,
    height: u32,
    fps: u16,
    af_mode: AutofocusMode,
    flash: FlashMode,
    zoom: u16,         // x10 (10 = 1.0x)
    exposure_comp: i8, // -3 to +3
    iso: u32,
    shutter_speed_us: u32,
    wb_temp_k: u16,
    streaming: bool,
}

struct CameraHal {
    sensors: Vec<CameraSensor>,
    state: CameraState,
    captures: u32,
}

static CAMERA_HAL: Mutex<Option<CameraHal>> = Mutex::new(None);

impl CameraHal {
    fn new() -> Self {
        CameraHal {
            sensors: Vec::new(),
            state: CameraState {
                active_sensor: None,
                width: 4032,
                height: 3024,
                fps: 30,
                af_mode: AutofocusMode::Continuous,
                flash: FlashMode::Auto,
                zoom: 10,
                exposure_comp: 0,
                iso: 100,
                shutter_speed_us: 10000,
                wb_temp_k: 5500,
                streaming: false,
            },
            captures: 0,
        }
    }

    fn register_sensor(
        &mut self,
        facing: CameraFacing,
        mp: u32,
        w: u32,
        h: u32,
        fps: u16,
        ois: bool,
        af: bool,
        flash: bool,
        fl: u16,
        ap: u16,
    ) -> u32 {
        let id = self.sensors.len() as u32;
        self.sensors.push(CameraSensor {
            id,
            facing,
            megapixels: mp,
            max_width: w,
            max_height: h,
            max_fps: fps,
            has_ois: ois,
            has_autofocus: af,
            has_flash: flash,
            focal_length_mm: fl,
            aperture: ap,
        });
        id
    }

    fn open(&mut self, sensor_id: u32) -> bool {
        if self.sensors.iter().any(|s| s.id == sensor_id) {
            self.state.active_sensor = Some(sensor_id);
            true
        } else {
            false
        }
    }

    fn capture(&mut self) -> Option<u32> {
        if self.state.active_sensor.is_some() {
            self.captures = self.captures.saturating_add(1);
            Some(self.captures)
        } else {
            None
        }
    }

    fn start_preview(&mut self) {
        self.state.streaming = true;
    }

    fn stop_preview(&mut self) {
        self.state.streaming = false;
    }

    fn set_zoom(&mut self, zoom_x10: u16) {
        self.state.zoom = zoom_x10.max(10).min(100); // 1x to 10x
    }
}

pub fn init() {
    let mut hal = CAMERA_HAL.lock();
    let mut h = CameraHal::new();
    // Register default sensors
    h.register_sensor(
        CameraFacing::Back,
        480,
        4032,
        3024,
        60,
        true,
        true,
        true,
        260,
        18,
    );
    h.register_sensor(
        CameraFacing::Front,
        120,
        2316,
        1736,
        30,
        false,
        true,
        false,
        220,
        22,
    );
    *hal = Some(h);
    serial_println!("    Camera: HAL (back 48MP, front 12MP) ready");
}
