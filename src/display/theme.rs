use crate::drivers::framebuffer::Color;
use crate::sync::Mutex;
/// Hoags OS theme system
///
/// Defines the visual appearance of the desktop: colors, spacing,
/// window decorations. Inspired by modern flat design but with
/// the Hoags Inc brand identity.
use crate::{serial_print, serial_println};

/// Desktop theme — all visual parameters in one place
pub struct Theme {
    // Desktop
    pub desktop_bg: Color,

    // Taskbar
    pub taskbar_bg: Color,
    pub taskbar_text: Color,
    pub taskbar_height: u32,

    // Window title bar
    pub title_bar_active: Color,
    pub title_bar_inactive: Color,
    pub title_text: Color,
    pub title_bar_height: u32,

    // Window
    pub window_bg: Color,
    pub window_text: Color,
    pub border_active: Color,
    pub border_inactive: Color,

    // Accent colors
    pub accent: Color,
    pub accent_hover: Color,

    // Font
    pub font_size: u32,
}

impl Theme {
    /// Default Hoags OS dark theme
    pub fn hoags_dark() -> Self {
        Theme {
            desktop_bg: Color::rgb(18, 18, 28), // deep dark blue-black

            taskbar_bg: Color::rgb(25, 25, 35), // slightly lighter than bg
            taskbar_text: Color::rgb(200, 200, 210),
            taskbar_height: 32,

            title_bar_active: Color::rgb(0, 140, 160), // Hoags teal/cyan
            title_bar_inactive: Color::rgb(40, 40, 50),
            title_text: Color::rgb(255, 255, 255),
            title_bar_height: 28,

            window_bg: Color::rgb(30, 30, 40),
            window_text: Color::rgb(220, 220, 230),
            border_active: Color::rgb(0, 180, 200),
            border_inactive: Color::rgb(50, 50, 60),

            accent: Color::rgb(0, 200, 220),        // Hoags cyan
            accent_hover: Color::rgb(255, 100, 50), // Hoags orange

            font_size: 16,
        }
    }

    /// Light theme alternative
    pub fn hoags_light() -> Self {
        Theme {
            desktop_bg: Color::rgb(230, 235, 240),

            taskbar_bg: Color::rgb(245, 245, 250),
            taskbar_text: Color::rgb(40, 40, 50),
            taskbar_height: 32,

            title_bar_active: Color::rgb(0, 140, 160),
            title_bar_inactive: Color::rgb(180, 180, 190),
            title_text: Color::rgb(255, 255, 255),
            title_bar_height: 28,

            window_bg: Color::rgb(255, 255, 255),
            window_text: Color::rgb(30, 30, 40),
            border_active: Color::rgb(0, 160, 180),
            border_inactive: Color::rgb(200, 200, 210),

            accent: Color::rgb(0, 160, 180),
            accent_hover: Color::rgb(220, 80, 40),

            font_size: 16,
        }
    }
}

/// Global theme instance
pub static THEME: Mutex<Theme> = Mutex::new(Theme {
    desktop_bg: Color::rgb(18, 18, 28),
    taskbar_bg: Color::rgb(25, 25, 35),
    taskbar_text: Color::rgb(200, 200, 210),
    taskbar_height: 32,
    title_bar_active: Color::rgb(0, 140, 160),
    title_bar_inactive: Color::rgb(40, 40, 50),
    title_text: Color::rgb(255, 255, 255),
    title_bar_height: 28,
    window_bg: Color::rgb(30, 30, 40),
    window_text: Color::rgb(220, 220, 230),
    border_active: Color::rgb(0, 180, 200),
    border_inactive: Color::rgb(50, 50, 60),
    accent: Color::rgb(0, 200, 220),
    accent_hover: Color::rgb(255, 100, 50),
    font_size: 16,
});

/// Initialize theme system
pub fn init() {
    // Default theme is already set via const initialization
    serial_println!("  Theme: Hoags Dark loaded");
}

/// Switch to light theme
pub fn set_light() {
    *THEME.lock() = Theme::hoags_light();
}

/// Switch to dark theme
pub fn set_dark() {
    *THEME.lock() = Theme::hoags_dark();
}
