/// AR/VR renderer for Genesis
///
/// 3D rendering, object placement, occlusion,
/// lighting estimation, passthrough video.
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum RenderMode {
    Ar,           // camera passthrough + overlays
    Vr,           // fully rendered
    MixedReality, // blended
}

struct ArRenderer {
    mode: RenderMode,
    fov_deg: u16,
    render_width: u32,
    render_height: u32,
    fps: u16,
    objects_rendered: u32,
    occlusion_enabled: bool,
    lighting_estimation: bool,
    passthrough_alpha: u8, // 0-255
}

static AR_RENDERER: Mutex<Option<ArRenderer>> = Mutex::new(None);

impl ArRenderer {
    fn new() -> Self {
        ArRenderer {
            mode: RenderMode::Ar,
            fov_deg: 110,
            render_width: 1920,
            render_height: 1080,
            fps: 60,
            objects_rendered: 0,
            occlusion_enabled: true,
            lighting_estimation: true,
            passthrough_alpha: 255,
        }
    }

    fn set_mode(&mut self, mode: RenderMode) {
        self.mode = mode;
        match mode {
            RenderMode::Ar => {
                self.passthrough_alpha = 255;
            }
            RenderMode::Vr => {
                self.passthrough_alpha = 0;
            }
            RenderMode::MixedReality => {
                self.passthrough_alpha = 128;
            }
        }
    }
}

pub fn init() {
    let mut r = AR_RENDERER.lock();
    *r = Some(ArRenderer::new());
    serial_println!("    AR: 3D renderer (AR/VR/MR modes) ready");
}
