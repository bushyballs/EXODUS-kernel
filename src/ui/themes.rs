/// Theme engine for Genesis — system-wide theming
///
/// Provides Material-Design-inspired theming with dynamic color,
/// dark/light mode, custom accents, and per-app overrides.
///
/// Inspired by: Material You, iOS appearance. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Color (ARGB)
#[derive(Debug, Clone, Copy)]
pub struct ThemeColor {
    pub a: u8,
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl ThemeColor {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        ThemeColor { a: 255, r, g, b }
    }

    pub fn to_u32(&self) -> u32 {
        ((self.a as u32) << 24) | ((self.r as u32) << 16) | ((self.g as u32) << 8) | self.b as u32
    }

    /// Lighten by percentage (0-100)
    pub fn lighten(&self, pct: u8) -> Self {
        let factor = pct.min(100) as u16;
        ThemeColor {
            a: self.a,
            r: (self.r as u16 + (255 - self.r as u16) * factor / 100) as u8,
            g: (self.g as u16 + (255 - self.g as u16) * factor / 100) as u8,
            b: (self.b as u16 + (255 - self.b as u16) * factor / 100) as u8,
        }
    }

    /// Darken by percentage (0-100)
    pub fn darken(&self, pct: u8) -> Self {
        let factor = (100 - pct.min(100)) as u16;
        ThemeColor {
            a: self.a,
            r: (self.r as u16 * factor / 100) as u8,
            g: (self.g as u16 * factor / 100) as u8,
            b: (self.b as u16 * factor / 100) as u8,
        }
    }
}

/// Color scheme
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorScheme {
    Light,
    Dark,
    System, // follow system setting
}

/// Theme definition
pub struct Theme {
    pub name: String,
    pub scheme: ColorScheme,
    /// Primary accent color
    pub primary: ThemeColor,
    pub primary_variant: ThemeColor,
    pub secondary: ThemeColor,
    /// Backgrounds
    pub background: ThemeColor,
    pub surface: ThemeColor,
    pub surface_variant: ThemeColor,
    /// Text colors
    pub on_primary: ThemeColor,
    pub on_background: ThemeColor,
    pub on_surface: ThemeColor,
    /// Status
    pub error: ThemeColor,
    pub success: ThemeColor,
    pub warning: ThemeColor,
    /// Elevation colors (surface + shadow)
    pub elevation_1: ThemeColor,
    pub elevation_2: ThemeColor,
    pub elevation_3: ThemeColor,
    /// Navigation bar color
    pub nav_bar: ThemeColor,
    /// Status bar color
    pub status_bar: ThemeColor,
    /// Corner radius (dp)
    pub corner_radius: u8,
}

impl Theme {
    /// Default dark theme
    pub fn dark() -> Self {
        Theme {
            name: String::from("Genesis Dark"),
            scheme: ColorScheme::Dark,
            primary: ThemeColor::new(100, 150, 255),
            primary_variant: ThemeColor::new(60, 100, 220),
            secondary: ThemeColor::new(150, 100, 255),
            background: ThemeColor::new(18, 18, 18),
            surface: ThemeColor::new(30, 30, 30),
            surface_variant: ThemeColor::new(45, 45, 45),
            on_primary: ThemeColor::new(255, 255, 255),
            on_background: ThemeColor::new(230, 230, 230),
            on_surface: ThemeColor::new(200, 200, 200),
            error: ThemeColor::new(255, 80, 80),
            success: ThemeColor::new(80, 200, 80),
            warning: ThemeColor::new(255, 180, 50),
            elevation_1: ThemeColor::new(35, 35, 35),
            elevation_2: ThemeColor::new(40, 40, 40),
            elevation_3: ThemeColor::new(48, 48, 48),
            nav_bar: ThemeColor::new(20, 20, 20),
            status_bar: ThemeColor::new(0, 0, 0),
            corner_radius: 12,
        }
    }

    /// Default light theme
    pub fn light() -> Self {
        Theme {
            name: String::from("Genesis Light"),
            scheme: ColorScheme::Light,
            primary: ThemeColor::new(50, 100, 220),
            primary_variant: ThemeColor::new(30, 70, 180),
            secondary: ThemeColor::new(100, 60, 200),
            background: ThemeColor::new(250, 250, 250),
            surface: ThemeColor::new(255, 255, 255),
            surface_variant: ThemeColor::new(240, 240, 240),
            on_primary: ThemeColor::new(255, 255, 255),
            on_background: ThemeColor::new(20, 20, 20),
            on_surface: ThemeColor::new(40, 40, 40),
            error: ThemeColor::new(200, 50, 50),
            success: ThemeColor::new(50, 160, 50),
            warning: ThemeColor::new(220, 150, 30),
            elevation_1: ThemeColor::new(245, 245, 245),
            elevation_2: ThemeColor::new(238, 238, 238),
            elevation_3: ThemeColor::new(230, 230, 230),
            nav_bar: ThemeColor::new(255, 255, 255),
            status_bar: ThemeColor::new(240, 240, 240),
            corner_radius: 12,
        }
    }

    /// Generate theme from a seed color (Material You style)
    pub fn from_seed(seed: ThemeColor, scheme: ColorScheme) -> Self {
        let mut theme = if scheme == ColorScheme::Dark {
            Self::dark()
        } else {
            Self::light()
        };
        theme.primary = seed;
        theme.primary_variant = seed.darken(20);
        // Generate complementary secondary
        theme.secondary = ThemeColor::new(
            seed.b.wrapping_add(50),
            seed.r.wrapping_add(50),
            seed.g.wrapping_add(50),
        );
        theme
    }
}

/// Theme engine
pub struct ThemeEngine {
    pub current: Theme,
    pub available: Vec<Theme>,
    /// Per-app theme overrides
    pub app_overrides: BTreeMap<String, usize>,
}

impl ThemeEngine {
    const fn new() -> Self {
        ThemeEngine {
            current: Theme {
                name: String::new(),
                scheme: ColorScheme::Dark,
                primary: ThemeColor::new(100, 150, 255),
                primary_variant: ThemeColor::new(60, 100, 220),
                secondary: ThemeColor::new(150, 100, 255),
                background: ThemeColor::new(18, 18, 18),
                surface: ThemeColor::new(30, 30, 30),
                surface_variant: ThemeColor::new(45, 45, 45),
                on_primary: ThemeColor::new(255, 255, 255),
                on_background: ThemeColor::new(230, 230, 230),
                on_surface: ThemeColor::new(200, 200, 200),
                error: ThemeColor::new(255, 80, 80),
                success: ThemeColor::new(80, 200, 80),
                warning: ThemeColor::new(255, 180, 50),
                elevation_1: ThemeColor::new(35, 35, 35),
                elevation_2: ThemeColor::new(40, 40, 40),
                elevation_3: ThemeColor::new(48, 48, 48),
                nav_bar: ThemeColor::new(20, 20, 20),
                status_bar: ThemeColor::new(0, 0, 0),
                corner_radius: 12,
            },
            available: Vec::new(),
            app_overrides: BTreeMap::new(),
        }
    }

    fn setup(&mut self) {
        self.current = Theme::dark();
        self.available.push(Theme::dark());
        self.available.push(Theme::light());
    }

    /// Switch to dark/light
    pub fn set_scheme(&mut self, scheme: ColorScheme) {
        self.current = match scheme {
            ColorScheme::Dark => Theme::dark(),
            ColorScheme::Light => Theme::light(),
            ColorScheme::System => Theme::dark(), // default to dark
        };
    }

    /// Set accent color
    pub fn set_accent(&mut self, color: ThemeColor) {
        self.current.primary = color;
        self.current.primary_variant = color.darken(20);
    }
}

static THEME_ENGINE: Mutex<ThemeEngine> = Mutex::new(ThemeEngine::new());

pub fn init() {
    THEME_ENGINE.lock().setup();
    crate::serial_println!("  [themes] Theme engine initialized (dark mode)");
}

pub fn set_dark_mode(dark: bool) {
    THEME_ENGINE.lock().set_scheme(if dark {
        ColorScheme::Dark
    } else {
        ColorScheme::Light
    });
}

pub fn primary_color() -> u32 {
    THEME_ENGINE.lock().current.primary.to_u32()
}

pub fn background_color() -> u32 {
    THEME_ENGINE.lock().current.background.to_u32()
}
