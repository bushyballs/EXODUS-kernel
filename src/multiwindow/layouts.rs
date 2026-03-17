use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

/// Predefined window layout configurations
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Layout {
    /// Full screen - single window takes entire screen
    Fullscreen,
    /// Two windows side-by-side (50/50)
    SideBySide,
    /// Main window on left (70%), secondary on right (30%)
    MainSidebar,
    /// Three columns
    ThreeColumn,
    /// Grid of 4 windows (2x2)
    Grid2x2,
    /// Grid of 6 windows (2x3)
    Grid2x3,
    /// Grid of 9 windows (3x3)
    Grid3x3,
    /// Focus mode - one large window with others minimized
    Focus,
    /// Picture-in-picture over main window
    PipOverlay,
}

/// Window positioning information
#[derive(Clone, Copy, Debug)]
pub struct WindowRect {
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
}

impl WindowRect {
    pub fn new(x: i16, y: i16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// Screen configuration
pub struct ScreenConfig {
    pub width: u16,
    pub height: u16,
    pub taskbar_height: u16,
    pub margin: u16,
}

impl Default for ScreenConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            taskbar_height: 48,
            margin: 8,
        }
    }
}

/// Calculate window positions for a given layout
pub fn calculate_layout(
    layout: Layout,
    _window_count: usize,
    config: &ScreenConfig,
) -> Vec<WindowRect> {
    let usable_width = config.width - (2 * config.margin);
    let usable_height = config.height - config.taskbar_height - (2 * config.margin);
    let x_base = config.margin as i16;
    let y_base = config.margin as i16;

    match layout {
        Layout::Fullscreen => {
            vec![WindowRect::new(x_base, y_base, usable_width, usable_height)]
        }

        Layout::SideBySide => {
            let half_width = usable_width / 2 - config.margin / 2;
            vec![
                WindowRect::new(x_base, y_base, half_width, usable_height),
                WindowRect::new(
                    x_base + half_width as i16 + config.margin as i16,
                    y_base,
                    half_width,
                    usable_height,
                ),
            ]
        }

        Layout::MainSidebar => {
            let main_width = (usable_width as f32 * 0.7) as u16;
            let sidebar_width = usable_width - main_width - config.margin;
            vec![
                WindowRect::new(x_base, y_base, main_width, usable_height),
                WindowRect::new(
                    x_base + main_width as i16 + config.margin as i16,
                    y_base,
                    sidebar_width,
                    usable_height,
                ),
            ]
        }

        Layout::ThreeColumn => {
            let col_width = usable_width / 3 - config.margin * 2 / 3;
            vec![
                WindowRect::new(x_base, y_base, col_width, usable_height),
                WindowRect::new(
                    x_base + col_width as i16 + config.margin as i16,
                    y_base,
                    col_width,
                    usable_height,
                ),
                WindowRect::new(
                    x_base + (col_width * 2) as i16 + (config.margin * 2) as i16,
                    y_base,
                    col_width,
                    usable_height,
                ),
            ]
        }

        Layout::Grid2x2 => {
            let cell_width = usable_width / 2 - config.margin / 2;
            let cell_height = usable_height / 2 - config.margin / 2;
            vec![
                WindowRect::new(x_base, y_base, cell_width, cell_height),
                WindowRect::new(
                    x_base + cell_width as i16 + config.margin as i16,
                    y_base,
                    cell_width,
                    cell_height,
                ),
                WindowRect::new(
                    x_base,
                    y_base + cell_height as i16 + config.margin as i16,
                    cell_width,
                    cell_height,
                ),
                WindowRect::new(
                    x_base + cell_width as i16 + config.margin as i16,
                    y_base + cell_height as i16 + config.margin as i16,
                    cell_width,
                    cell_height,
                ),
            ]
        }

        Layout::Grid2x3 => {
            let cell_width = usable_width / 2 - config.margin / 2;
            let cell_height = usable_height / 3 - config.margin * 2 / 3;
            let mut rects = Vec::new();
            for row in 0..3 {
                for col in 0..2 {
                    rects.push(WindowRect::new(
                        x_base + (col as i16) * (cell_width as i16 + config.margin as i16),
                        y_base + (row as i16) * (cell_height as i16 + config.margin as i16),
                        cell_width,
                        cell_height,
                    ));
                }
            }
            rects
        }

        Layout::Grid3x3 => {
            let cell_width = usable_width / 3 - config.margin * 2 / 3;
            let cell_height = usable_height / 3 - config.margin * 2 / 3;
            let mut rects = Vec::new();
            for row in 0..3 {
                for col in 0..3 {
                    rects.push(WindowRect::new(
                        x_base + (col as i16) * (cell_width as i16 + config.margin as i16),
                        y_base + (row as i16) * (cell_height as i16 + config.margin as i16),
                        cell_width,
                        cell_height,
                    ));
                }
            }
            rects
        }

        Layout::Focus => {
            // Main window takes 80% of screen, centered
            let focus_width = (usable_width as f32 * 0.8) as u16;
            let focus_height = (usable_height as f32 * 0.8) as u16;
            let x_offset = (usable_width - focus_width) / 2;
            let y_offset = (usable_height - focus_height) / 2;
            vec![WindowRect::new(
                x_base + x_offset as i16,
                y_base + y_offset as i16,
                focus_width,
                focus_height,
            )]
        }

        Layout::PipOverlay => {
            // Main window fullscreen, PIP in corner
            let pip_width = 320;
            let pip_height = 180;
            vec![
                WindowRect::new(x_base, y_base, usable_width, usable_height),
                WindowRect::new(
                    x_base + usable_width as i16 - pip_width as i16 - config.margin as i16,
                    y_base + usable_height as i16 - pip_height as i16 - config.margin as i16,
                    pip_width,
                    pip_height,
                ),
            ]
        }
    }
}

/// Snap zones for window snapping
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SnapZone {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Center,
}

/// Get the window rect for a snap zone
pub fn get_snap_zone_rect(zone: SnapZone, config: &ScreenConfig) -> WindowRect {
    let usable_width = config.width - (2 * config.margin);
    let usable_height = config.height - config.taskbar_height - (2 * config.margin);
    let x_base = config.margin as i16;
    let y_base = config.margin as i16;
    let half_width = usable_width / 2;
    let half_height = usable_height / 2;

    match zone {
        SnapZone::Left => WindowRect::new(x_base, y_base, half_width, usable_height),
        SnapZone::Right => WindowRect::new(
            x_base + half_width as i16,
            y_base,
            half_width,
            usable_height,
        ),
        SnapZone::Top => WindowRect::new(x_base, y_base, usable_width, half_height),
        SnapZone::Bottom => WindowRect::new(
            x_base,
            y_base + half_height as i16,
            usable_width,
            half_height,
        ),
        SnapZone::TopLeft => WindowRect::new(x_base, y_base, half_width, half_height),
        SnapZone::TopRight => {
            WindowRect::new(x_base + half_width as i16, y_base, half_width, half_height)
        }
        SnapZone::BottomLeft => {
            WindowRect::new(x_base, y_base + half_height as i16, half_width, half_height)
        }
        SnapZone::BottomRight => WindowRect::new(
            x_base + half_width as i16,
            y_base + half_height as i16,
            half_width,
            half_height,
        ),
        SnapZone::Center => {
            let center_width = (usable_width as f32 * 0.8) as u16;
            let center_height = (usable_height as f32 * 0.8) as u16;
            let x_offset = (usable_width - center_width) / 2;
            let y_offset = (usable_height - center_height) / 2;
            WindowRect::new(
                x_base + x_offset as i16,
                y_base + y_offset as i16,
                center_width,
                center_height,
            )
        }
    }
}

static SCREEN_CONFIG: Mutex<ScreenConfig> = Mutex::new(ScreenConfig {
    width: 1920,
    height: 1080,
    taskbar_height: 48,
    margin: 8,
});

/// Set the screen configuration
pub fn set_screen_config(config: ScreenConfig) {
    let mut cfg = SCREEN_CONFIG.lock();
    *cfg = config;
}

/// Get the current screen configuration
pub fn get_screen_config() -> ScreenConfig {
    let cfg = SCREEN_CONFIG.lock();
    ScreenConfig {
        width: cfg.width,
        height: cfg.height,
        taskbar_height: cfg.taskbar_height,
        margin: cfg.margin,
    }
}
