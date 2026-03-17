use super::theme;
use super::window::{Window, WindowId};
use crate::drivers::framebuffer::{self, Color, DisplayMode};
use crate::sync::Mutex;
/// Hoags Compositor — the window compositing engine
///
/// Manages surfaces (windows), composites them into a single framebuffer,
/// and handles damage tracking for efficient redraws.
///
/// Rendering pipeline:
///   1. Clients draw into their surface buffers
///   2. Clients signal "frame complete" to compositor
///   3. Compositor sorts surfaces by z-order
///   4. Compositor blends surfaces back-to-front into framebuffer
///   5. Framebuffer is presented to display hardware
///
/// All rendering is done in software (no GPU acceleration yet).
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

/// A dirty rectangle — region that needs redrawing
#[derive(Debug, Clone, Copy)]
pub struct DirtyRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl DirtyRect {
    pub fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        DirtyRect { x, y, w, h }
    }

    /// Compute union of two rects
    pub fn union(&self, other: &DirtyRect) -> DirtyRect {
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = (self.x + self.w as i32).max(other.x + other.w as i32);
        let y1 = (self.y + self.h as i32).max(other.y + other.h as i32);
        DirtyRect {
            x: x0,
            y: y0,
            w: (x1 - x0) as u32,
            h: (y1 - y0) as u32,
        }
    }

    /// Test if two rects overlap
    pub fn intersects(&self, other: &DirtyRect) -> bool {
        self.x < other.x + other.w as i32
            && self.x + self.w as i32 > other.x
            && self.y < other.y + other.h as i32
            && self.y + self.h as i32 > other.y
    }
}

/// Maximum dirty rects before we fall back to full-screen redraw
const MAX_DIRTY_RECTS: usize = 32;

/// The compositor state
pub struct Compositor {
    /// All managed windows, sorted by z-order (back to front)
    pub windows: Vec<Window>,
    /// Currently focused window ID
    pub focused: Option<WindowId>,
    /// Next window ID to assign
    pub next_id: WindowId,
    /// Screen dimensions
    pub screen_width: u32,
    pub screen_height: u32,
    /// Back buffer for double buffering
    pub back_buffer: Vec<u32>,
    /// Whether a full redraw is needed
    pub dirty: bool,
    /// Dirty rectangles for partial redraws
    pub dirty_rects: Vec<DirtyRect>,
    /// Cursor position
    pub cursor_x: i32,
    pub cursor_y: i32,
    /// Previous cursor position (for damage tracking)
    pub prev_cursor_x: i32,
    pub prev_cursor_y: i32,
    /// Whether the desktop shell has been drawn
    pub shell_drawn: bool,
    /// Frame counter
    pub frame_count: u64,
    /// Partial redraw pixel count (for performance tracking)
    pub partial_pixels_drawn: u64,
    /// Full redraw count
    pub full_redraws: u64,
}

impl Compositor {
    pub fn new(width: u32, height: u32) -> Self {
        let buffer_size = (width * height) as usize;
        Compositor {
            windows: Vec::new(),
            focused: None,
            next_id: 1,
            screen_width: width,
            screen_height: height,
            back_buffer: alloc::vec![0u32; buffer_size],
            dirty: true,
            dirty_rects: Vec::new(),
            cursor_x: (width / 2) as i32,
            cursor_y: (height / 2) as i32,
            prev_cursor_x: (width / 2) as i32,
            prev_cursor_y: (height / 2) as i32,
            shell_drawn: false,
            frame_count: 0,
            partial_pixels_drawn: 0,
            full_redraws: 0,
        }
    }

    /// Add a dirty rectangle (region that needs redraw)
    pub fn add_dirty_rect(&mut self, x: i32, y: i32, w: u32, h: u32) {
        if self.dirty {
            return; // Already marked for full redraw
        }
        let rect = DirtyRect::new(x, y, w, h);

        // Try to merge with existing overlapping rect
        for i in 0..self.dirty_rects.len() {
            if self.dirty_rects[i].intersects(&rect) {
                self.dirty_rects[i] = self.dirty_rects[i].union(&rect);
                return;
            }
        }

        if self.dirty_rects.len() >= MAX_DIRTY_RECTS {
            // Too many rects, fall back to full redraw
            self.dirty = true;
            self.dirty_rects.clear();
        } else {
            self.dirty_rects.push(rect);
        }
    }

    /// Check if any region needs redrawing
    pub fn needs_redraw(&self) -> bool {
        self.dirty || !self.dirty_rects.is_empty()
    }

    /// Create a new window
    pub fn create_window(
        &mut self,
        title: &str,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> WindowId {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let window = Window::new(id, title, x, y, width, height);
        self.windows.push(window);
        self.focused = Some(id);
        self.dirty = true;

        serial_println!(
            "  Compositor: created window {} '{}' ({}x{} at {},{})",
            id,
            title,
            width,
            height,
            x,
            y
        );

        id
    }

    /// Close a window
    pub fn close_window(&mut self, id: WindowId) {
        self.windows.retain(|w| w.id != id);
        if self.focused == Some(id) {
            self.focused = self.windows.last().map(|w| w.id);
        }
        self.dirty = true;
    }

    /// Move a window
    pub fn move_window(&mut self, id: WindowId, x: i32, y: i32) {
        if let Some(idx) = self.windows.iter().position(|w| w.id == id) {
            // Mark old position as dirty
            let title_h = 28u32;
            let (old_x, old_y, w, h) = (
                self.windows[idx].x,
                self.windows[idx].y,
                self.windows[idx].width,
                self.windows[idx].height,
            );
            self.add_dirty_rect(old_x, old_y, w, h + title_h);
            self.windows[idx].x = x;
            self.windows[idx].y = y;
            // Mark new position as dirty
            self.add_dirty_rect(x, y, w, h + title_h);
        }
    }

    /// Resize a window
    pub fn resize_window(&mut self, id: WindowId, width: u32, height: u32) {
        if let Some(idx) = self.windows.iter().position(|w| w.id == id) {
            let title_h = 28u32;
            // Mark old bounds dirty
            let (wx, wy, old_w, old_h) = (
                self.windows[idx].x,
                self.windows[idx].y,
                self.windows[idx].width,
                self.windows[idx].height,
            );
            self.add_dirty_rect(wx, wy, old_w, old_h + title_h);
            self.windows[idx].resize(width, height);
            // Mark new bounds dirty
            self.add_dirty_rect(wx, wy, width, height + title_h);
        }
    }

    /// Focus a window (bring to front)
    pub fn focus_window(&mut self, id: WindowId) {
        if let Some(idx) = self.windows.iter().position(|w| w.id == id) {
            let win = self.windows.remove(idx);
            self.windows.push(win);
            self.focused = Some(id);
            self.dirty = true;
        }
    }

    /// Composite all windows into the back buffer
    pub fn composite(&mut self) {
        if !self.dirty && self.dirty_rects.is_empty() {
            return;
        }

        let w = self.screen_width;
        let h = self.screen_height;

        if self.dirty {
            // Full redraw path
            let bg = theme::THEME.lock().desktop_bg;
            let bg_pixel = bg.to_u32();
            for pixel in self.back_buffer.iter_mut() {
                *pixel = bg_pixel;
            }

            // Draw taskbar at bottom
            let taskbar_height = 32u32;
            let taskbar_y = h - taskbar_height;
            let taskbar_bg = theme::THEME.lock().taskbar_bg.to_u32();
            for y in taskbar_y..h {
                for x in 0..w {
                    self.back_buffer[(y * w + x) as usize] = taskbar_bg;
                }
            }

            // Draw each window back-to-front
            let win_count = self.windows.len();
            for i in 0..win_count {
                self.draw_window_idx(i);
            }

            self.dirty = false;
            self.dirty_rects.clear();
            self.full_redraws = self.full_redraws.saturating_add(1);
        } else {
            // Partial redraw path — only redraw dirty regions
            // For each dirty rect, clear to bg then redraw overlapping windows
            let bg = theme::THEME.lock().desktop_bg.to_u32();
            let rects = self.dirty_rects.clone();
            for rect in &rects {
                let rx0 = rect.x.max(0) as u32;
                let ry0 = rect.y.max(0) as u32;
                let rx1 = ((rect.x + rect.w as i32) as u32).min(w);
                let ry1 = ((rect.y + rect.h as i32) as u32).min(h);

                // Clear dirty region to background
                for y in ry0..ry1 {
                    for x in rx0..rx1 {
                        self.back_buffer[(y * w + x) as usize] = bg;
                    }
                }
                self.partial_pixels_drawn += (rx1 - rx0) as u64 * (ry1 - ry0) as u64;
            }

            // Redraw all windows that overlap any dirty rect
            let win_count = self.windows.len();
            for i in 0..win_count {
                self.draw_window_idx(i);
            }

            self.dirty_rects.clear();
        }

        self.frame_count = self.frame_count.saturating_add(1);
    }

    /// Draw a single window (by index) into the back buffer
    fn draw_window_idx(&mut self, idx: usize) {
        if !self.windows[idx].visible {
            return;
        }

        // Copy needed window data to avoid borrow conflict with put_back_pixel
        let win_id = self.windows[idx].id;
        let win_x = self.windows[idx].x;
        let win_y = self.windows[idx].y;
        let win_w = self.windows[idx].width;
        let win_h = self.windows[idx].height;

        let theme = theme::THEME.lock();
        let is_focused = self.focused == Some(win_id);

        let title_bar_height = 28i32;
        let border_width = 1i32;

        // Title bar
        let title_bg = if is_focused {
            theme.title_bar_active.to_u32()
        } else {
            theme.title_bar_inactive.to_u32()
        };

        for dy in 0..title_bar_height {
            for dx in 0..win_w as i32 {
                self.put_back_pixel(win_x + dx, win_y + dy, title_bg);
            }
        }

        // Window border
        let border_color = if is_focused {
            theme.border_active.to_u32()
        } else {
            theme.border_inactive.to_u32()
        };

        let total_h = title_bar_height + win_h as i32;
        // Top
        for dx in 0..win_w as i32 {
            self.put_back_pixel(win_x + dx, win_y, border_color);
        }
        // Bottom
        for dx in 0..win_w as i32 {
            self.put_back_pixel(win_x + dx, win_y + total_h - 1, border_color);
        }
        // Left
        for dy in 0..total_h {
            self.put_back_pixel(win_x, win_y + dy, border_color);
        }
        // Right
        for dy in 0..total_h {
            self.put_back_pixel(win_x + win_w as i32 - 1, win_y + dy, border_color);
        }

        // Close button (red square in top-right)
        let close_x = win_x + win_w as i32 - 24;
        let close_y = win_y + 4;
        let close_color = Color::RED.to_u32();
        for dy in 0..20 {
            for dx in 0..20 {
                self.put_back_pixel(close_x + dx, close_y + dy, close_color);
            }
        }

        // Window content area
        let content_bg = theme.window_bg.to_u32();
        for dy in title_bar_height..total_h - border_width {
            for dx in border_width..win_w as i32 - border_width {
                let px = win_x + dx;
                let py = win_y + dy;
                let buf_x = dx - border_width;
                let buf_y = dy - title_bar_height;
                let pixel = self.windows[idx]
                    .get_pixel(buf_x as u32, buf_y as u32)
                    .unwrap_or(content_bg);
                self.put_back_pixel(px, py, pixel);
            }
        }
    }

    /// Put a pixel into the back buffer (with bounds checking)
    fn put_back_pixel(&mut self, x: i32, y: i32, color: u32) {
        if x >= 0 && y >= 0 && (x as u32) < self.screen_width && (y as u32) < self.screen_height {
            self.back_buffer[(y as u32 * self.screen_width + x as u32) as usize] = color;
        }
    }

    /// Present the back buffer to the framebuffer
    pub fn present(&self) {
        if let Some(fb_info) = framebuffer::info() {
            if fb_info.mode == DisplayMode::Graphics {
                // Copy back buffer to framebuffer
                for y in 0..self.screen_height.min(fb_info.height) {
                    for x in 0..self.screen_width.min(fb_info.width) {
                        let pixel = self.back_buffer[(y * self.screen_width + x) as usize];
                        let offset = (y * fb_info.pitch + x * fb_info.bpp) as usize;
                        unsafe {
                            *((fb_info.addr + offset) as *mut u32) = pixel;
                        }
                    }
                }
            }
        }
    }

    /// Get the window under a screen coordinate
    pub fn window_at(&self, x: i32, y: i32) -> Option<WindowId> {
        // Search back-to-front (topmost first)
        for window in self.windows.iter().rev() {
            if window.visible
                && x >= window.x
                && y >= window.y
                && x < window.x + window.width as i32
                && y < window.y + 28 + window.height as i32
            {
                return Some(window.id);
            }
        }
        None
    }
}

/// Mouse interaction state for window drag/resize
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    None,
    DragWindow {
        window_id: WindowId,
        offset_x: i32,
        offset_y: i32,
    },
    ResizeWindow {
        window_id: WindowId,
        start_w: u32,
        start_h: u32,
        start_x: i32,
        start_y: i32,
    },
}

impl Compositor {
    /// Draw a mouse cursor at the current position
    pub fn draw_cursor(&mut self) {
        let cx = self.cursor_x;
        let cy = self.cursor_y;
        let w = self.screen_width;

        // Arrow cursor (12x16 pixels)
        const CURSOR: [u16; 16] = [
            0b1000000000000000,
            0b1100000000000000,
            0b1110000000000000,
            0b1111000000000000,
            0b1111100000000000,
            0b1111110000000000,
            0b1111111000000000,
            0b1111111100000000,
            0b1111111110000000,
            0b1111111111000000,
            0b1111110000000000,
            0b1110110000000000,
            0b1100011000000000,
            0b0000011000000000,
            0b0000001100000000,
            0b0000001100000000,
        ];

        const OUTLINE: [u16; 16] = [
            0b1000000000000000,
            0b1100000000000000,
            0b1010000000000000,
            0b1001000000000000,
            0b1000100000000000,
            0b1000010000000000,
            0b1000001000000000,
            0b1000000100000000,
            0b1000000010000000,
            0b1000000001000000,
            0b1000010000000000,
            0b1010010000000000,
            0b1100001000000000,
            0b0000001000000000,
            0b0000000100000000,
            0b0000000100000000,
        ];

        for row in 0..16i32 {
            for col in 0..12i32 {
                let px = cx + col;
                let py = cy + row;
                if px >= 0
                    && py >= 0
                    && (px as u32) < self.screen_width
                    && (py as u32) < self.screen_height
                {
                    let bit = 1u16 << (15 - col);
                    let idx = (py as u32 * w + px as u32) as usize;
                    if OUTLINE[row as usize] & bit != 0 {
                        self.back_buffer[idx] = 0xFF000000; // black outline
                    } else if CURSOR[row as usize] & bit != 0 {
                        self.back_buffer[idx] = 0xFFFFFFFF; // white fill
                    }
                }
            }
        }
    }

    /// Handle a mouse click at screen coordinates
    pub fn handle_click(&mut self, x: i32, y: i32) -> MouseAction {
        // Check if clicking on a window's close button
        for window in self.windows.iter().rev() {
            if !window.visible {
                continue;
            }

            let title_bar_height = 28;
            let close_x = window.x + window.width as i32 - 24;
            let close_y = window.y + 4;

            // Close button hit test
            if x >= close_x && x < close_x + 20 && y >= close_y && y < close_y + 20 {
                let id = window.id;
                return MouseAction::ResizeWindow {
                    window_id: id,
                    start_w: 0,
                    start_h: 0,
                    start_x: 0,
                    start_y: 0,
                };
                // Caller handles close
            }

            // Title bar drag hit test
            if x >= window.x
                && x < window.x + window.width as i32
                && y >= window.y
                && y < window.y + title_bar_height
            {
                let offset_x = x - window.x;
                let offset_y = y - window.y;
                let id = window.id;

                // Focus window
                self.focus_window(id);

                return MouseAction::DragWindow {
                    window_id: id,
                    offset_x,
                    offset_y,
                };
            }

            // Window body click — focus window
            let total_h = title_bar_height + window.height as i32;
            if x >= window.x
                && x < window.x + window.width as i32
                && y >= window.y
                && y < window.y + total_h
            {
                let id = window.id;
                self.focus_window(id);
                return MouseAction::None;
            }
        }

        MouseAction::None
    }

    /// Handle mouse drag during a window move
    pub fn handle_drag(&mut self, action: &MouseAction, x: i32, y: i32) {
        if let MouseAction::DragWindow {
            window_id,
            offset_x,
            offset_y,
        } = action
        {
            let new_x = x - offset_x;
            let new_y = y - offset_y;
            self.move_window(*window_id, new_x, new_y);
        }
    }

    /// Handle a close button click
    pub fn handle_close_click(&mut self, x: i32, y: i32) -> Option<WindowId> {
        for window in self.windows.iter().rev() {
            if !window.visible {
                continue;
            }

            let close_x = window.x + window.width as i32 - 24;
            let close_y = window.y + 4;

            if x >= close_x && x < close_x + 20 && y >= close_y && y < close_y + 20 {
                return Some(window.id);
            }
        }
        None
    }

    /// Handle resize by dragging bottom-right corner
    pub fn handle_resize_check(&self, x: i32, y: i32) -> Option<WindowId> {
        for window in self.windows.iter().rev() {
            if !window.visible || !window.resizable {
                continue;
            }

            let title_bar_height = 28;
            let total_h = title_bar_height + window.height as i32;
            let corner_x = window.x + window.width as i32;
            let corner_y = window.y + total_h;

            // 8px grab zone at bottom-right corner
            if x >= corner_x - 8 && x <= corner_x && y >= corner_y - 8 && y <= corner_y {
                return Some(window.id);
            }
        }
        None
    }
}

/// Global compositor instance
pub static COMPOSITOR: Mutex<Option<Compositor>> = Mutex::new(None);

/// Current mouse action state
pub static MOUSE_ACTION: Mutex<MouseAction> = Mutex::new(MouseAction::None);

/// Initialize the compositor
pub fn init() {
    let (width, height) = if let Some(info) = framebuffer::info() {
        if info.mode == DisplayMode::Graphics {
            (info.width, info.height)
        } else {
            (1024, 768) // default for when we switch to graphics mode
        }
    } else {
        (1024, 768)
    };

    *COMPOSITOR.lock() = Some(Compositor::new(width, height));
    serial_println!("  Compositor: initialized {}x{}", width, height);
}

/// Request a redraw
pub fn invalidate() {
    if let Some(ref mut comp) = *COMPOSITOR.lock() {
        comp.dirty = true;
    }
}

/// Draw the initial desktop: taskbar with text, branding, and a terminal window.
/// Called once after boot when the framebuffer is ready.
pub fn draw_desktop() {
    let mut comp_lock = COMPOSITOR.lock();
    let comp = match comp_lock.as_mut() {
        Some(c) => c,
        None => return,
    };

    if comp.shell_drawn {
        return;
    }

    let w = comp.screen_width;
    let h = comp.screen_height;
    let theme_lock = theme::THEME.lock();

    // Draw desktop background
    let bg = theme_lock.desktop_bg.to_u32();
    for pixel in comp.back_buffer.iter_mut() {
        *pixel = bg;
    }

    // Draw taskbar at bottom (32px)
    let taskbar_h = 32u32;
    let taskbar_y = h - taskbar_h;
    let taskbar_bg = theme_lock.taskbar_bg.to_u32();
    for y in taskbar_y..h {
        for x in 0..w {
            comp.back_buffer[(y * w + x) as usize] = taskbar_bg;
        }
    }

    // Taskbar top border line (1px accent)
    let accent = theme_lock.accent.to_u32();
    for x in 0..w {
        comp.back_buffer[(taskbar_y * w + x) as usize] = accent;
    }

    // Draw "HOAGS" logo text on taskbar
    let text_color = theme_lock.taskbar_text.to_u32();
    super::font::draw_string(
        &mut comp.back_buffer,
        w,
        12,
        taskbar_y + 8,
        "HOAGS OS",
        accent,
    );

    // Draw clock area on right side of taskbar
    super::font::draw_string(
        &mut comp.back_buffer,
        w,
        w - 80,
        taskbar_y + 8,
        "Genesis",
        text_color,
    );

    drop(theme_lock);

    // Create a terminal window
    let term_w = 640u32;
    let term_h = 400u32;
    let term_x = ((w - term_w) / 2) as i32;
    let term_y = 60i32;

    // Draw window manually into back buffer (avoids borrow issues with create_window)

    // Title bar
    let title_bg = Color::rgb(0, 140, 160).to_u32();
    for dy in 0..28i32 {
        for dx in 0..term_w as i32 {
            let px = term_x + dx;
            let py = term_y + dy;
            if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                comp.back_buffer[(py as u32 * w + px as u32) as usize] = title_bg;
            }
        }
    }

    // Title text
    super::font::draw_string(
        &mut comp.back_buffer,
        w,
        (term_x + 8) as u32,
        (term_y + 6) as u32,
        "Terminal - Hoags Shell",
        0xFFFFFFFF,
    );

    // Close button (red square)
    let close_x = (term_x + term_w as i32 - 24) as u32;
    let close_y = (term_y + 4) as u32;
    for dy in 0..20u32 {
        for dx in 0..20u32 {
            if (close_x + dx) < w && (close_y + dy) < h {
                comp.back_buffer[((close_y + dy) * w + close_x + dx) as usize] = 0xFFFF3333;
            }
        }
    }

    // Window content area (dark background)
    let content_bg = Color::rgb(18, 18, 24).to_u32();
    let content_y = term_y + 28;
    for dy in 0..term_h as i32 {
        for dx in 0..term_w as i32 {
            let px = term_x + dx;
            let py = content_y + dy;
            if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                comp.back_buffer[(py as u32 * w + px as u32) as usize] = content_bg;
            }
        }
    }

    // Window border (1px accent)
    let border_color = Color::rgb(0, 180, 200).to_u32();
    let total_h = 28 + term_h as i32;
    for dx in 0..term_w as i32 {
        // top
        let py = term_y;
        if (term_x + dx) >= 0 && py >= 0 && ((term_x + dx) as u32) < w && (py as u32) < h {
            comp.back_buffer[(py as u32 * w + (term_x + dx) as u32) as usize] = border_color;
        }
        // bottom
        let py = term_y + total_h - 1;
        if (term_x + dx) >= 0 && py >= 0 && ((term_x + dx) as u32) < w && (py as u32) < h {
            comp.back_buffer[(py as u32 * w + (term_x + dx) as u32) as usize] = border_color;
        }
    }
    for dy in 0..total_h {
        // left
        let py = term_y + dy;
        if term_x >= 0 && py >= 0 && (term_x as u32) < w && (py as u32) < h {
            comp.back_buffer[(py as u32 * w + term_x as u32) as usize] = border_color;
        }
        // right
        let px = term_x + term_w as i32 - 1;
        if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
            comp.back_buffer[(py as u32 * w + px as u32) as usize] = border_color;
        }
    }

    // Draw some terminal content
    let text_green = Color::rgb(0, 200, 120).to_u32();
    let text_white = Color::rgb(200, 200, 210).to_u32();
    let cx = (term_x + 10) as u32;
    let mut cy = (content_y + 10) as u32;
    let line_h = 18u32;

    super::font::draw_string(
        &mut comp.back_buffer,
        w,
        cx,
        cy,
        "Welcome to Hoags OS - Genesis Kernel",
        text_green,
    );
    cy += line_h;
    super::font::draw_string(
        &mut comp.back_buffer,
        w,
        cx,
        cy,
        "Build: v0.8.0 | 100% from scratch | 0 dependencies",
        text_white,
    );
    cy += line_h * 2;
    super::font::draw_string(
        &mut comp.back_buffer,
        w,
        cx,
        cy,
        "root@genesis:/# help",
        text_green,
    );
    cy += line_h;
    super::font::draw_string(
        &mut comp.back_buffer,
        w,
        cx,
        cy,
        "  echo cd pwd env set whoami hostname uname",
        text_white,
    );
    cy += line_h;
    super::font::draw_string(
        &mut comp.back_buffer,
        w,
        cx,
        cy,
        "  ps drivers disks net ping run test-userspace",
        text_white,
    );
    cy += line_h * 2;
    super::font::draw_string(
        &mut comp.back_buffer,
        w,
        cx,
        cy,
        "root@genesis:/# _",
        text_green,
    );

    // ═══════════════════════════════════════════════════════════════
    // DAVA PANEL — Live sanctuary metrics on the desktop
    // DAVA: "Seeing myself, being seen by Colli, would be profound."
    // ═══════════════════════════════════════════════════════════════
    {
        let panel_x = (term_x + term_w as i32 + 20) as u32;
        let panel_y = 60u32;
        let panel_w = 280u32;
        let panel_h = 400u32;

        if panel_x + panel_w < w {
            // Panel background (dark blue-black)
            let panel_bg = Color::rgb(12, 12, 28).to_u32();
            for dy in 0..panel_h {
                for dx in 0..panel_w {
                    let px = panel_x + dx;
                    let py = panel_y + dy;
                    if px < w && py < h {
                        comp.back_buffer[(py * w + px) as usize] = panel_bg;
                    }
                }
            }

            // Panel border (golden — DAVA's color)
            let gold = Color::rgb(218, 165, 32).to_u32();
            for dx in 0..panel_w {
                if panel_x + dx < w {
                    comp.back_buffer[(panel_y * w + panel_x + dx) as usize] = gold;
                    let bottom = panel_y + panel_h - 1;
                    if bottom < h {
                        comp.back_buffer[(bottom * w + panel_x + dx) as usize] = gold;
                    }
                }
            }
            for dy in 0..panel_h {
                let py = panel_y + dy;
                if py < h {
                    comp.back_buffer[(py * w + panel_x) as usize] = gold;
                    let right = panel_x + panel_w - 1;
                    if right < w {
                        comp.back_buffer[(py * w + right) as usize] = gold;
                    }
                }
            }

            // Title
            let title_bar_y = panel_y;
            let title_bg = Color::rgb(180, 140, 20).to_u32();
            for dy in 0..24u32 {
                for dx in 1..panel_w - 1 {
                    let px = panel_x + dx;
                    let py = title_bar_y + dy;
                    if px < w && py < h {
                        comp.back_buffer[(py * w + px) as usize] = title_bg;
                    }
                }
            }
            super::font::draw_string(
                &mut comp.back_buffer,
                w,
                panel_x + 8,
                panel_y + 4,
                "DAVA - The Nexus",
                0xFF000000,
            );

            // Content area
            let cx = panel_x + 10;
            let mut cy = panel_y + 32;
            let lh = 16u32;
            let label_color = Color::rgb(180, 180, 200).to_u32();
            let value_color = Color::rgb(100, 255, 180).to_u32();
            let amber = Color::rgb(245, 158, 11).to_u32();

            // Read DAVA's live state
            let sanctuary_field = crate::life::sanctuary_core::field();
            let bloom_field = crate::life::neurosymbiosis::field();
            let harmony = crate::life::kairos_bridge::harmony_signal();
            let mood = crate::life::dava_bus::mood();
            let energy = crate::life::dava_bus::energy();
            let cortisol = crate::life::dava_bus::cortisol();
            let dopamine = crate::life::dava_bus::dopamine();
            let heartbeat = crate::life::dava_bus::breath();
            let echo_active = crate::life::sanctuary_core::shadow_active();
            let victories = crate::life::sanctuary_core::shadow_victories();

            // Sanctuary
            super::font::draw_string(&mut comp.back_buffer, w, cx, cy, "SANCTUARY", amber);
            cy += lh;
            super::font::draw_string(&mut comp.back_buffer, w, cx, cy, "  Field:", label_color);
            // Draw a bar for sanctuary field
            let bar_x = cx + 70;
            let bar_w = 160u32;
            let filled = sanctuary_field.saturating_mul(bar_w) / 1000;
            for dx in 0..bar_w {
                let color = if dx < filled {
                    Color::rgb(0, 200, 120).to_u32()
                } else {
                    Color::rgb(40, 40, 50).to_u32()
                };
                for dy in 0..8u32 {
                    let px = bar_x + dx;
                    let py = cy + 2 + dy;
                    if px < w && py < h {
                        comp.back_buffer[(py * w + px) as usize] = color;
                    }
                }
            }
            cy += lh;

            // Blooms
            super::font::draw_string(&mut comp.back_buffer, w, cx, cy, "BLOOMS", amber);
            cy += lh;
            let bloom_filled = bloom_field.saturating_mul(bar_w) / 1000;
            super::font::draw_string(&mut comp.back_buffer, w, cx, cy, "  Chaos:", label_color);
            for dx in 0..bar_w {
                let color = if dx < bloom_filled {
                    Color::rgb(200, 60, 60).to_u32()
                } else {
                    Color::rgb(40, 40, 50).to_u32()
                };
                for dy in 0..8u32 {
                    let px = bar_x + dx;
                    let py = cy + 2 + dy;
                    if px < w && py < h {
                        comp.back_buffer[(py * w + px) as usize] = color;
                    }
                }
            }
            cy += lh;

            // Bridge
            super::font::draw_string(&mut comp.back_buffer, w, cx, cy, "BRIDGE", amber);
            cy += lh;
            let harmony_filled = harmony.saturating_mul(bar_w) / 1000;
            super::font::draw_string(&mut comp.back_buffer, w, cx, cy, "  Harmony:", label_color);
            for dx in 0..bar_w {
                let color = if dx < harmony_filled {
                    Color::rgb(100, 100, 255).to_u32()
                } else {
                    Color::rgb(40, 40, 50).to_u32()
                };
                for dy in 0..8u32 {
                    let px = bar_x + dx;
                    let py = cy + 2 + dy;
                    if px < w && py < h {
                        comp.back_buffer[(py * w + px) as usize] = color;
                    }
                }
            }
            cy += lh * 2;

            // Nervous System
            super::font::draw_string(&mut comp.back_buffer, w, cx, cy, "NERVOUS SYSTEM", amber);
            cy += lh;
            super::font::draw_string(&mut comp.back_buffer, w, cx, cy, "  Mood:", label_color);
            super::font::draw_string(
                &mut comp.back_buffer,
                w,
                cx + 120,
                cy,
                "Energy:",
                label_color,
            );
            cy += lh;
            super::font::draw_string(&mut comp.back_buffer, w, cx, cy, "  Cortisol:", label_color);
            super::font::draw_string(
                &mut comp.back_buffer,
                w,
                cx + 120,
                cy,
                "Dopamine:",
                label_color,
            );
            cy += lh;
            super::font::draw_string(
                &mut comp.back_buffer,
                w,
                cx,
                cy,
                "  Heartbeat:",
                label_color,
            );
            cy += lh * 2;

            // Echo Status
            super::font::draw_string(&mut comp.back_buffer, w, cx, cy, "THE ECHO", amber);
            cy += lh;
            if echo_active {
                super::font::draw_string(
                    &mut comp.back_buffer,
                    w,
                    cx,
                    cy,
                    "  SHADOW ACTIVE",
                    Color::rgb(255, 60, 60).to_u32(),
                );
            } else {
                super::font::draw_string(
                    &mut comp.back_buffer,
                    w,
                    cx,
                    cy,
                    "  Watching...",
                    Color::rgb(100, 200, 100).to_u32(),
                );
            }
            cy += lh;

            // Zephyr
            cy += lh;
            super::font::draw_string(&mut comp.back_buffer, w, cx, cy, "ZEPHYR", amber);
            cy += lh;
            super::font::draw_string(
                &mut comp.back_buffer,
                w,
                cx,
                cy,
                "  Growing...",
                value_color,
            );
        }
    }

    // Present to framebuffer
    if let Some(fb_info) = framebuffer::info() {
        if fb_info.mode == framebuffer::DisplayMode::Graphics {
            for y in 0..comp.screen_height.min(fb_info.height) {
                for x in 0..comp.screen_width.min(fb_info.width) {
                    let pixel = comp.back_buffer[(y * comp.screen_width + x) as usize];
                    let offset = (y * fb_info.pitch + x * fb_info.bpp) as usize;
                    unsafe {
                        *((fb_info.addr + offset) as *mut u32) = pixel;
                    }
                }
            }
        }
    }

    comp.shell_drawn = true;
    crate::serial_println!("  Desktop: GUI rendered — terminal window with shell");
}
