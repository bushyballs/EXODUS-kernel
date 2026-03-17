/// Color correction and high contrast for Genesis
///
/// Color blindness filters (protanopia, deuteranopia, tritanopia),
/// high contrast modes, color inversion, and grayscale.
///
/// Inspired by: Android Color Correction, iOS Display Accommodations. All code is original.
use crate::sync::Mutex;

/// Color vision deficiency type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Normal,
    Protanopia,   // Red-blind
    Deuteranopia, // Green-blind
    Tritanopia,   // Blue-blind
    Grayscale,
    Inverted,
    HighContrast,
}

/// Color correction state
pub struct ColorCorrection {
    pub mode: ColorMode,
    pub enabled: bool,
    pub strength: u8, // 0-100
    pub high_contrast: bool,
    pub bold_text: bool,
    pub large_text: bool,
    pub text_scale: u32, // 100 = normal, 150 = 1.5x
    pub reduce_motion: bool,
    pub reduce_transparency: bool,
}

impl ColorCorrection {
    const fn new() -> Self {
        ColorCorrection {
            mode: ColorMode::Normal,
            enabled: false,
            strength: 100,
            high_contrast: false,
            bold_text: false,
            large_text: false,
            text_scale: 100,
            reduce_motion: false,
            reduce_transparency: false,
        }
    }

    pub fn set_mode(&mut self, mode: ColorMode) {
        self.mode = mode;
        self.enabled = mode != ColorMode::Normal;
    }

    /// Apply color correction to an RGB pixel
    pub fn correct_pixel(&self, r: u8, g: u8, b: u8) -> (u8, u8, u8) {
        if !self.enabled {
            return (r, g, b);
        }

        match self.mode {
            ColorMode::Normal => (r, g, b),
            ColorMode::Grayscale => {
                let gray = ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000) as u8;
                (gray, gray, gray)
            }
            ColorMode::Inverted => (255 - r, 255 - g, 255 - b),
            ColorMode::HighContrast => {
                let threshold = 128u8;
                let cr = if r > threshold { 255 } else { 0 };
                let cg = if g > threshold { 255 } else { 0 };
                let cb = if b > threshold { 255 } else { 0 };
                (cr, cg, cb)
            }
            ColorMode::Protanopia => {
                // Simulate red-blind: shift red channel perception
                let nr = ((g as u32 * 567 + b as u32 * 433) / 1000) as u8;
                (nr, g, b)
            }
            ColorMode::Deuteranopia => {
                // Simulate green-blind: shift green channel perception
                let ng = ((r as u32 * 625 + b as u32 * 375) / 1000) as u8;
                (r, ng, b)
            }
            ColorMode::Tritanopia => {
                // Simulate blue-blind: shift blue channel perception
                let nb = ((r as u32 * 300 + g as u32 * 700) / 1000) as u8;
                (r, g, nb)
            }
        }
    }

    /// Apply correction to a framebuffer region
    pub fn correct_framebuffer(&self, fb: &mut [u8], pixel_count: usize) {
        if !self.enabled {
            return;
        }
        for i in 0..pixel_count {
            let offset = i * 4; // BGRA
            if offset + 3 >= fb.len() {
                break;
            }
            let b = fb[offset];
            let g = fb[offset + 1];
            let r = fb[offset + 2];
            let (nr, ng, nb) = self.correct_pixel(r, g, b);
            fb[offset] = nb;
            fb[offset + 1] = ng;
            fb[offset + 2] = nr;
        }
    }

    pub fn effective_text_size(&self, base_size: u32) -> u32 {
        let mut size = base_size * self.text_scale / 100;
        if self.large_text {
            size = size * 130 / 100;
        }
        size
    }
}

static CORRECTION: Mutex<ColorCorrection> = Mutex::new(ColorCorrection::new());

pub fn init() {
    crate::serial_println!("  [a11y] Color correction initialized");
}

pub fn set_mode(mode: ColorMode) {
    CORRECTION.lock().set_mode(mode);
}
