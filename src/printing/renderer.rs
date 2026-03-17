/// Print renderer for Genesis
///
/// PDF rendering, page layout, text rendering,
/// image scaling, print preview generation.
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy)]
pub struct PageLayout {
    pub width_pt: u32, // points (1/72 inch)
    pub height_pt: u32,
    pub margin_top: u32,
    pub margin_bottom: u32,
    pub margin_left: u32,
    pub margin_right: u32,
    pub orientation: Orientation,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Orientation {
    Portrait,
    Landscape,
}

struct RenderEngine {
    default_layout: PageLayout,
    dpi: u16,
    pages_rendered: u32,
}

static RENDERER: Mutex<Option<RenderEngine>> = Mutex::new(None);

impl RenderEngine {
    fn new() -> Self {
        RenderEngine {
            default_layout: PageLayout {
                width_pt: 612,  // 8.5 inches
                height_pt: 792, // 11 inches (Letter)
                margin_top: 72, // 1 inch margins
                margin_bottom: 72,
                margin_left: 72,
                margin_right: 72,
                orientation: Orientation::Portrait,
            },
            dpi: 300,
            pages_rendered: 0,
        }
    }

    fn content_width(&self) -> u32 {
        self.default_layout.width_pt
            - self.default_layout.margin_left
            - self.default_layout.margin_right
    }

    fn content_height(&self) -> u32 {
        self.default_layout.height_pt
            - self.default_layout.margin_top
            - self.default_layout.margin_bottom
    }

    fn pixels_per_point(&self) -> u32 {
        self.dpi as u32 / 72
    }
}

pub fn init() {
    let mut r = RENDERER.lock();
    *r = Some(RenderEngine::new());
    serial_println!("    Printing: PDF renderer ready");
}
