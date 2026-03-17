use crate::sync::Mutex;
use alloc::vec;
/// Screenshot capture and annotation
///
/// Part of the Genesis System UI. Captures the screen
/// contents and optionally opens an annotation editor.
use alloc::vec::Vec;

/// Captured screenshot data
pub struct Screenshot {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub timestamp: u64,
}

impl Screenshot {
    /// Total size in bytes of the pixel buffer.
    pub fn byte_size(&self) -> usize {
        self.pixels.len()
    }

    /// Check if this screenshot has valid dimensions.
    pub fn is_valid(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.pixels.len() == (self.width as usize * self.height as usize * 4)
    }
}

/// Default screen dimensions (used when no framebuffer info is available)
const DEFAULT_WIDTH: u32 = 1920;
const DEFAULT_HEIGHT: u32 = 1080;
/// Bytes per pixel (RGBA)
const BPP: u32 = 4;

pub struct ScreenshotService {
    pub save_path_prefix: u64,
    capture_count: u64,
    screen_width: u32,
    screen_height: u32,
}

impl ScreenshotService {
    pub fn new() -> Self {
        ScreenshotService {
            save_path_prefix: 0,
            capture_count: 0,
            screen_width: DEFAULT_WIDTH,
            screen_height: DEFAULT_HEIGHT,
        }
    }

    /// Set the screen resolution for captures.
    pub fn set_resolution(&mut self, width: u32, height: u32) {
        self.screen_width = width;
        self.screen_height = height;
        crate::serial_println!("  [screenshot] resolution set to {}x{}", width, height);
    }

    /// Capture the full screen.
    ///
    /// In a real system this would read the framebuffer. Here we generate
    /// a placeholder pixel buffer representing the capture.
    pub fn capture_full(&self) -> Screenshot {
        let w = self.screen_width;
        let h = self.screen_height;
        let pixel_count = (w as usize) * (h as usize) * (BPP as usize);

        // Allocate a zeroed pixel buffer (represents a black capture placeholder)
        let pixels = vec![0u8; pixel_count];

        crate::serial_println!(
            "  [screenshot] captured full screen {}x{} ({} bytes)",
            w,
            h,
            pixel_count
        );

        Screenshot {
            width: w,
            height: h,
            pixels,
            timestamp: self.capture_count,
        }
    }

    /// Capture a rectangular region of the screen.
    ///
    /// Clamps the region to screen bounds.
    pub fn capture_region(&self, x: u32, y: u32, w: u32, h: u32) -> Screenshot {
        // Clamp to screen bounds
        let x = x.min(self.screen_width);
        let y = y.min(self.screen_height);
        let w = w.min(self.screen_width - x);
        let h = h.min(self.screen_height - y);

        if w == 0 || h == 0 {
            crate::serial_println!("  [screenshot] region capture: empty region");
            return Screenshot {
                width: 0,
                height: 0,
                pixels: Vec::new(),
                timestamp: self.capture_count,
            };
        }

        let pixel_count = (w as usize) * (h as usize) * (BPP as usize);
        let pixels = vec![0u8; pixel_count];

        crate::serial_println!(
            "  [screenshot] captured region ({},{}) {}x{} ({} bytes)",
            x,
            y,
            w,
            h,
            pixel_count
        );

        Screenshot {
            width: w,
            height: h,
            pixels,
            timestamp: self.capture_count,
        }
    }

    /// Number of captures taken since init.
    pub fn capture_count(&self) -> u64 {
        self.capture_count
    }
}

static SCREENSHOT_SVC: Mutex<Option<ScreenshotService>> = Mutex::new(None);

pub fn init() {
    *SCREENSHOT_SVC.lock() = Some(ScreenshotService::new());
    crate::serial_println!("  [screenshot] Screenshot service initialized");
}

/// Capture the full screen globally.
pub fn capture_full() -> Option<Screenshot> {
    match SCREENSHOT_SVC.lock().as_ref() {
        Some(svc) => Some(svc.capture_full()),
        None => None,
    }
}

/// Capture a screen region globally.
pub fn capture_region(x: u32, y: u32, w: u32, h: u32) -> Option<Screenshot> {
    match SCREENSHOT_SVC.lock().as_ref() {
        Some(svc) => Some(svc.capture_region(x, y, w, h)),
        None => None,
    }
}
