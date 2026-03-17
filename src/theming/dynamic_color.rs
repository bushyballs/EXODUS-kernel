use crate::sync::Mutex;
/// Material You-style dynamic color for Genesis
///
/// Extract palette from wallpaper, generate light/dark schemes,
/// harmonize custom colors.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum ThemeMode {
    Light,
    Dark,
    System,
}

#[derive(Clone, Copy)]
pub struct ColorScheme {
    pub primary: u32,
    pub on_primary: u32,
    pub primary_container: u32,
    pub secondary: u32,
    pub on_secondary: u32,
    pub tertiary: u32,
    pub background: u32,
    pub surface: u32,
    pub on_surface: u32,
    pub error: u32,
    pub outline: u32,
}

struct DynamicColorEngine {
    current_scheme: ColorScheme,
    mode: ThemeMode,
    source_color: u32,
    custom_schemes: Vec<ColorScheme>,
}

static DYN_COLOR: Mutex<Option<DynamicColorEngine>> = Mutex::new(None);

impl DynamicColorEngine {
    fn new() -> Self {
        // Default Material-style blue scheme
        let default_scheme = ColorScheme {
            primary: 0xFF_1976D2,
            on_primary: 0xFF_FFFFFF,
            primary_container: 0xFF_BBDEFB,
            secondary: 0xFF_455A64,
            on_secondary: 0xFF_FFFFFF,
            tertiary: 0xFF_7B1FA2,
            background: 0xFF_FAFAFA,
            surface: 0xFF_FFFFFF,
            on_surface: 0xFF_212121,
            error: 0xFF_D32F2F,
            outline: 0xFF_BDBDBD,
        };
        DynamicColorEngine {
            current_scheme: default_scheme,
            mode: ThemeMode::System,
            source_color: 0xFF_1976D2,
            custom_schemes: Vec::new(),
        }
    }

    fn generate_scheme(&mut self, seed: u32, dark: bool) {
        // Extract RGB components
        let _r = ((seed >> 16) & 0xFF) as u8;
        let _g = ((seed >> 8) & 0xFF) as u8;
        let _b = (seed & 0xFF) as u8;

        if dark {
            self.current_scheme = ColorScheme {
                primary: seed,
                on_primary: 0xFF_000000,
                primary_container: Self::darken(seed, 40),
                secondary: Self::shift_hue(seed, 30),
                on_secondary: 0xFF_000000,
                tertiary: Self::shift_hue(seed, 60),
                background: 0xFF_121212,
                surface: 0xFF_1E1E1E,
                on_surface: 0xFF_E0E0E0,
                error: 0xFF_CF6679,
                outline: 0xFF_424242,
            };
        } else {
            self.current_scheme = ColorScheme {
                primary: seed,
                on_primary: 0xFF_FFFFFF,
                primary_container: Self::lighten(seed, 40),
                secondary: Self::shift_hue(seed, 30),
                on_secondary: 0xFF_FFFFFF,
                tertiary: Self::shift_hue(seed, 60),
                background: 0xFF_FAFAFA,
                surface: 0xFF_FFFFFF,
                on_surface: 0xFF_212121,
                error: 0xFF_D32F2F,
                outline: 0xFF_BDBDBD,
            };
        }
        self.source_color = seed;
    }

    fn darken(color: u32, amount: u8) -> u32 {
        let r = ((color >> 16) & 0xFF).saturating_sub(amount as u32);
        let g = ((color >> 8) & 0xFF).saturating_sub(amount as u32);
        let b = (color & 0xFF).saturating_sub(amount as u32);
        0xFF_000000 | (r << 16) | (g << 8) | b
    }

    fn lighten(color: u32, amount: u8) -> u32 {
        let r = (((color >> 16) & 0xFF) + amount as u32).min(255);
        let g = (((color >> 8) & 0xFF) + amount as u32).min(255);
        let b = ((color & 0xFF) + amount as u32).min(255);
        0xFF_000000 | (r << 16) | (g << 8) | b
    }

    fn shift_hue(color: u32, shift: u8) -> u32 {
        let r = ((color >> 16) & 0xFF).wrapping_add(shift as u32) & 0xFF;
        let g = ((color >> 8) & 0xFF) & 0xFF;
        let b = (color & 0xFF).wrapping_add((shift / 2) as u32) & 0xFF;
        0xFF_000000 | (r << 16) | (g << 8) | b
    }

    fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::Light,
            ThemeMode::System => ThemeMode::Dark,
        };
        let dark = self.mode == ThemeMode::Dark;
        self.generate_scheme(self.source_color, dark);
    }

    fn get_current(&self) -> &ColorScheme {
        &self.current_scheme
    }
}

pub fn init() {
    let mut dc = DYN_COLOR.lock();
    *dc = Some(DynamicColorEngine::new());
    serial_println!("    Dynamic color engine ready");
}
