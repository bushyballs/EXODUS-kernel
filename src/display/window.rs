/// Window management for the Hoags Display Server
///
/// Each window has a pixel buffer, position, size, title,
/// and various state flags.
use alloc::string::String;
use alloc::vec::Vec;

/// Unique window identifier
pub type WindowId = u32;

/// Window state flags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    Normal,
    Minimized,
    Maximized,
    Fullscreen,
}

/// A window managed by the compositor
pub struct Window {
    pub id: WindowId,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub state: WindowState,
    pub visible: bool,
    pub resizable: bool,
    pub closeable: bool,
    /// Pixel buffer (ARGB format, width * height pixels)
    pub buffer: Vec<u32>,
    /// Process that owns this window
    pub owner_pid: u32,
}

impl Window {
    /// Create a new window
    pub fn new(id: WindowId, title: &str, x: i32, y: i32, width: u32, height: u32) -> Self {
        let buffer_size = (width * height) as usize;
        Window {
            id,
            title: String::from(title),
            x,
            y,
            width,
            height,
            state: WindowState::Normal,
            visible: true,
            resizable: true,
            closeable: true,
            buffer: alloc::vec![0xFFFFFFu32; buffer_size], // white background
            owner_pid: 0,
        }
    }

    /// Get a pixel from the window buffer
    pub fn get_pixel(&self, x: u32, y: u32) -> Option<u32> {
        if x < self.width && y < self.height {
            Some(self.buffer[(y * self.width + x) as usize])
        } else {
            None
        }
    }

    /// Set a pixel in the window buffer
    pub fn set_pixel(&mut self, x: u32, y: u32, color: u32) {
        if x < self.width && y < self.height {
            self.buffer[(y * self.width + x) as usize] = color;
        }
    }

    /// Fill the entire window with a color
    pub fn fill(&mut self, color: u32) {
        for pixel in self.buffer.iter_mut() {
            *pixel = color;
        }
    }

    /// Fill a rectangle within the window
    pub fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, color: u32) {
        for dy in 0..h {
            for dx in 0..w {
                self.set_pixel(x + dx, y + dy, color);
            }
        }
    }

    /// Draw a horizontal line
    pub fn hline(&mut self, x: u32, y: u32, len: u32, color: u32) {
        for dx in 0..len {
            self.set_pixel(x + dx, y, color);
        }
    }

    /// Draw a vertical line
    pub fn vline(&mut self, x: u32, y: u32, len: u32, color: u32) {
        for dy in 0..len {
            self.set_pixel(x, y + dy, color);
        }
    }

    /// Draw a 1px border rectangle (outline only)
    pub fn draw_rect(&mut self, x: u32, y: u32, w: u32, h: u32, color: u32) {
        self.hline(x, y, w, color);
        self.hline(x, y + h - 1, w, color);
        self.vline(x, y, h, color);
        self.vline(x + w - 1, y, h, color);
    }

    /// Resize the window (reallocates buffer)
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.buffer = alloc::vec![0xFFFFFFu32; (width * height) as usize];
    }

    /// Maximize to fill the screen
    pub fn maximize(&mut self, screen_w: u32, screen_h: u32) {
        self.x = 0;
        self.y = 0;
        self.resize(screen_w, screen_h - 32); // leave room for taskbar
        self.state = WindowState::Maximized;
    }

    /// Minimize (hide)
    pub fn minimize(&mut self) {
        self.state = WindowState::Minimized;
        self.visible = false;
    }

    /// Restore from minimized/maximized
    pub fn restore(&mut self) {
        self.state = WindowState::Normal;
        self.visible = true;
    }
}
