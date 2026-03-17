use super::font;
use crate::sync::Mutex;
/// Widget toolkit for Genesis — basic UI components
///
/// Provides: Button, TextLabel, TextInput, Scrollbar, Panel, Checkbox, Slider.
/// All widgets use integer coordinates and Q16 fixed-point where needed.
/// Renders into pixel buffers using the compositor's back buffer format.
///
/// Inspired by: Qt (signals/slots model), GTK (widget tree), iOS UIKit.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const DEFAULT_FONT_HEIGHT: u32 = 16;
const DEFAULT_FONT_WIDTH: u32 = 8;
const DEFAULT_PADDING: u32 = 6;

// ---------------------------------------------------------------------------
// Widget IDs and events
// ---------------------------------------------------------------------------

/// Unique widget identifier
pub type WidgetId = u32;

/// Events that widgets can generate
#[derive(Debug, Clone)]
pub enum WidgetEvent {
    /// Button was clicked
    ButtonClicked(WidgetId),
    /// Text input value changed
    TextChanged(WidgetId, String),
    /// Text input submitted (Enter key)
    TextSubmitted(WidgetId, String),
    /// Checkbox toggled
    CheckboxToggled(WidgetId, bool),
    /// Slider value changed (value is 0..max)
    SliderChanged(WidgetId, i32),
    /// Scrollbar position changed (value is 0..max)
    ScrollChanged(WidgetId, i32),
}

/// Mouse button state for hit testing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Input event that widgets consume
#[derive(Debug, Clone)]
pub enum WidgetInput {
    MouseMove(i32, i32),
    MouseDown(i32, i32, MouseButton),
    MouseUp(i32, i32, MouseButton),
    KeyPress(char),
    KeyBackspace,
    KeyEnter,
    KeyLeft,
    KeyRight,
    KeyHome,
    KeyEnd,
    KeyDelete,
}

// ---------------------------------------------------------------------------
// Widget state
// ---------------------------------------------------------------------------

/// Common widget state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetState {
    Normal,
    Hovered,
    Pressed,
    Focused,
    Disabled,
}

/// Rectangle bounds
#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Rect { x, y, w, h }
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w as i32 && py < self.y + self.h as i32
    }
}

// ---------------------------------------------------------------------------
// Color helpers
// ---------------------------------------------------------------------------

/// Widget theme colors
pub struct WidgetColors {
    pub bg_normal: u32,
    pub bg_hovered: u32,
    pub bg_pressed: u32,
    pub bg_disabled: u32,
    pub bg_focused: u32,
    pub text_normal: u32,
    pub text_disabled: u32,
    pub border_normal: u32,
    pub border_focused: u32,
    pub accent: u32,
    pub scrollbar_track: u32,
    pub scrollbar_thumb: u32,
    pub scrollbar_thumb_hover: u32,
}

impl WidgetColors {
    pub const fn dark_theme() -> Self {
        WidgetColors {
            bg_normal: 0xFF2A2A3C,
            bg_hovered: 0xFF353548,
            bg_pressed: 0xFF1E1E2E,
            bg_disabled: 0xFF1A1A24,
            bg_focused: 0xFF2E2E42,
            text_normal: 0xFFDDDDEE,
            text_disabled: 0xFF666688,
            border_normal: 0xFF444466,
            border_focused: 0xFF00C8DC,
            accent: 0xFF00C8DC,
            scrollbar_track: 0xFF1A1A24,
            scrollbar_thumb: 0xFF444466,
            scrollbar_thumb_hover: 0xFF5555AA,
        }
    }
}

static WIDGET_COLORS: WidgetColors = WidgetColors::dark_theme();

// ---------------------------------------------------------------------------
// Drawing helpers
// ---------------------------------------------------------------------------

fn fill_rect(buf: &mut [u32], buf_w: u32, x: i32, y: i32, w: u32, h: u32, color: u32) {
    for dy in 0..h {
        for dx in 0..w {
            let px = x + dx as i32;
            let py = y + dy as i32;
            if px >= 0 && py >= 0 {
                let idx = (py as u32 * buf_w + px as u32) as usize;
                if idx < buf.len() {
                    buf[idx] = color;
                }
            }
        }
    }
}

fn draw_border(buf: &mut [u32], buf_w: u32, x: i32, y: i32, w: u32, h: u32, color: u32) {
    // Top
    for dx in 0..w {
        let idx = (y as u32 * buf_w + (x as u32 + dx)) as usize;
        if y >= 0 && idx < buf.len() {
            buf[idx] = color;
        }
    }
    // Bottom
    let by = y + h as i32 - 1;
    for dx in 0..w {
        let idx = (by as u32 * buf_w + (x as u32 + dx)) as usize;
        if by >= 0 && idx < buf.len() {
            buf[idx] = color;
        }
    }
    // Left
    for dy in 0..h {
        let idx = ((y as u32 + dy) * buf_w + x as u32) as usize;
        if x >= 0 && idx < buf.len() {
            buf[idx] = color;
        }
    }
    // Right
    let rx = x + w as i32 - 1;
    for dy in 0..h {
        let idx = ((y as u32 + dy) * buf_w + rx as u32) as usize;
        if rx >= 0 && idx < buf.len() {
            buf[idx] = color;
        }
    }
}

// ---------------------------------------------------------------------------
// Button widget
// ---------------------------------------------------------------------------

pub struct Button {
    pub id: WidgetId,
    pub bounds: Rect,
    pub label: String,
    pub state: WidgetState,
    pub visible: bool,
}

impl Button {
    pub fn new(id: WidgetId, x: i32, y: i32, w: u32, h: u32, label: &str) -> Self {
        Button {
            id,
            bounds: Rect::new(x, y, w, h),
            label: String::from(label),
            state: WidgetState::Normal,
            visible: true,
        }
    }

    /// Auto-size button to fit label text + padding
    pub fn auto_sized(id: WidgetId, x: i32, y: i32, label: &str) -> Self {
        let text_w = label.len() as u32 * DEFAULT_FONT_WIDTH;
        let w = text_w + DEFAULT_PADDING * 2;
        let h = DEFAULT_FONT_HEIGHT + DEFAULT_PADDING * 2;
        Self::new(id, x, y, w, h, label)
    }

    pub fn draw(&self, buf: &mut [u32], buf_w: u32) {
        if !self.visible {
            return;
        }
        let colors = &WIDGET_COLORS;
        let bg = match self.state {
            WidgetState::Normal => colors.bg_normal,
            WidgetState::Hovered => colors.bg_hovered,
            WidgetState::Pressed => colors.bg_pressed,
            WidgetState::Disabled => colors.bg_disabled,
            WidgetState::Focused => colors.bg_focused,
        };
        let text_color = if self.state == WidgetState::Disabled {
            colors.text_disabled
        } else {
            colors.text_normal
        };
        let border = if self.state == WidgetState::Focused || self.state == WidgetState::Pressed {
            colors.border_focused
        } else {
            colors.border_normal
        };

        fill_rect(
            buf,
            buf_w,
            self.bounds.x,
            self.bounds.y,
            self.bounds.w,
            self.bounds.h,
            bg,
        );
        draw_border(
            buf,
            buf_w,
            self.bounds.x,
            self.bounds.y,
            self.bounds.w,
            self.bounds.h,
            border,
        );

        // Center the label text
        let text_w = self.label.len() as u32 * DEFAULT_FONT_WIDTH;
        let tx = self.bounds.x + ((self.bounds.w as i32 - text_w as i32) / 2);
        let ty = self.bounds.y + ((self.bounds.h as i32 - DEFAULT_FONT_HEIGHT as i32) / 2);
        font::draw_string(buf, buf_w, tx as u32, ty as u32, &self.label, text_color);
    }

    pub fn handle_input(&mut self, input: &WidgetInput) -> Option<WidgetEvent> {
        if !self.visible || self.state == WidgetState::Disabled {
            return None;
        }
        match input {
            WidgetInput::MouseMove(x, y) => {
                if self.bounds.contains(*x, *y) {
                    if self.state == WidgetState::Normal {
                        self.state = WidgetState::Hovered;
                    }
                } else {
                    if self.state == WidgetState::Hovered {
                        self.state = WidgetState::Normal;
                    }
                }
                None
            }
            WidgetInput::MouseDown(x, y, MouseButton::Left) => {
                if self.bounds.contains(*x, *y) {
                    self.state = WidgetState::Pressed;
                }
                None
            }
            WidgetInput::MouseUp(x, y, MouseButton::Left) => {
                if self.state == WidgetState::Pressed && self.bounds.contains(*x, *y) {
                    self.state = WidgetState::Hovered;
                    return Some(WidgetEvent::ButtonClicked(self.id));
                }
                if self.state == WidgetState::Pressed {
                    self.state = WidgetState::Normal;
                }
                None
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// TextLabel widget (read-only text display)
// ---------------------------------------------------------------------------

pub struct TextLabel {
    pub id: WidgetId,
    pub bounds: Rect,
    pub text: String,
    pub color: u32,
    pub visible: bool,
}

impl TextLabel {
    pub fn new(id: WidgetId, x: i32, y: i32, text: &str) -> Self {
        let w = text.len() as u32 * DEFAULT_FONT_WIDTH;
        let h = DEFAULT_FONT_HEIGHT;
        TextLabel {
            id,
            bounds: Rect::new(x, y, w, h),
            text: String::from(text),
            color: WIDGET_COLORS.text_normal,
            visible: true,
        }
    }

    pub fn set_text(&mut self, text: &str) {
        self.text = String::from(text);
        self.bounds.w = text.len() as u32 * DEFAULT_FONT_WIDTH;
    }

    pub fn draw(&self, buf: &mut [u32], buf_w: u32) {
        if !self.visible {
            return;
        }
        font::draw_string(
            buf,
            buf_w,
            self.bounds.x as u32,
            self.bounds.y as u32,
            &self.text,
            self.color,
        );
    }
}

// ---------------------------------------------------------------------------
// TextInput widget (editable single-line text)
// ---------------------------------------------------------------------------

pub struct TextInput {
    pub id: WidgetId,
    pub bounds: Rect,
    pub text: String,
    pub cursor_pos: usize,
    pub scroll_offset: usize,
    pub placeholder: String,
    pub state: WidgetState,
    pub max_len: usize,
    pub visible: bool,
}

impl TextInput {
    pub fn new(id: WidgetId, x: i32, y: i32, w: u32, placeholder: &str) -> Self {
        let h = DEFAULT_FONT_HEIGHT + DEFAULT_PADDING * 2;
        TextInput {
            id,
            bounds: Rect::new(x, y, w, h),
            text: String::new(),
            cursor_pos: 0,
            scroll_offset: 0,
            placeholder: String::from(placeholder),
            state: WidgetState::Normal,
            max_len: 256,
            visible: true,
        }
    }

    /// Visible character capacity
    fn visible_chars(&self) -> usize {
        ((self.bounds.w - DEFAULT_PADDING * 2) / DEFAULT_FONT_WIDTH) as usize
    }

    /// Ensure cursor is visible by adjusting scroll offset
    fn ensure_cursor_visible(&mut self) {
        let visible = self.visible_chars();
        if self.cursor_pos < self.scroll_offset {
            self.scroll_offset = self.cursor_pos;
        } else if self.cursor_pos > self.scroll_offset + visible {
            self.scroll_offset = self.cursor_pos - visible;
        }
    }

    pub fn draw(&self, buf: &mut [u32], buf_w: u32) {
        if !self.visible {
            return;
        }
        let colors = &WIDGET_COLORS;
        let bg = match self.state {
            WidgetState::Focused => colors.bg_focused,
            WidgetState::Disabled => colors.bg_disabled,
            _ => colors.bg_normal,
        };
        let border = if self.state == WidgetState::Focused {
            colors.border_focused
        } else {
            colors.border_normal
        };

        fill_rect(
            buf,
            buf_w,
            self.bounds.x,
            self.bounds.y,
            self.bounds.w,
            self.bounds.h,
            bg,
        );
        draw_border(
            buf,
            buf_w,
            self.bounds.x,
            self.bounds.y,
            self.bounds.w,
            self.bounds.h,
            border,
        );

        let tx = self.bounds.x + DEFAULT_PADDING as i32;
        let ty = self.bounds.y + DEFAULT_PADDING as i32;
        let visible = self.visible_chars();

        if self.text.is_empty() {
            // Draw placeholder
            let end = visible.min(self.placeholder.len());
            let display: String = self.placeholder.chars().take(end).collect();
            font::draw_string(
                buf,
                buf_w,
                tx as u32,
                ty as u32,
                &display,
                colors.text_disabled,
            );
        } else {
            // Draw visible portion of text
            let end = (self.scroll_offset + visible).min(self.text.len());
            let display: String = self
                .text
                .chars()
                .skip(self.scroll_offset)
                .take(end - self.scroll_offset)
                .collect();
            font::draw_string(
                buf,
                buf_w,
                tx as u32,
                ty as u32,
                &display,
                colors.text_normal,
            );
        }

        // Draw cursor if focused
        if self.state == WidgetState::Focused {
            let cursor_screen = (self.cursor_pos - self.scroll_offset) as u32;
            let cx = tx + (cursor_screen * DEFAULT_FONT_WIDTH) as i32;
            fill_rect(buf, buf_w, cx, ty, 1, DEFAULT_FONT_HEIGHT, colors.accent);
        }
    }

    pub fn handle_input(&mut self, input: &WidgetInput) -> Option<WidgetEvent> {
        if !self.visible || self.state == WidgetState::Disabled {
            return None;
        }
        match input {
            WidgetInput::MouseDown(x, y, MouseButton::Left) => {
                if self.bounds.contains(*x, *y) {
                    self.state = WidgetState::Focused;
                    // Position cursor at click location
                    let rel_x = (*x - self.bounds.x - DEFAULT_PADDING as i32).max(0) as u32;
                    let char_pos = self.scroll_offset + (rel_x / DEFAULT_FONT_WIDTH) as usize;
                    self.cursor_pos = char_pos.min(self.text.len());
                } else if self.state == WidgetState::Focused {
                    self.state = WidgetState::Normal;
                }
                None
            }
            WidgetInput::KeyPress(ch) => {
                if self.state != WidgetState::Focused {
                    return None;
                }
                if self.text.len() < self.max_len {
                    if self.cursor_pos >= self.text.len() {
                        self.text.push(*ch);
                    } else {
                        let mut new_text = String::new();
                        for (i, c) in self.text.chars().enumerate() {
                            if i == self.cursor_pos {
                                new_text.push(*ch);
                            }
                            new_text.push(c);
                        }
                        self.text = new_text;
                    }
                    self.cursor_pos += 1;
                    self.ensure_cursor_visible();
                    return Some(WidgetEvent::TextChanged(self.id, self.text.clone()));
                }
                None
            }
            WidgetInput::KeyBackspace => {
                if self.state != WidgetState::Focused {
                    return None;
                }
                if self.cursor_pos > 0 && !self.text.is_empty() {
                    let mut new_text = String::new();
                    for (i, c) in self.text.chars().enumerate() {
                        if i != self.cursor_pos - 1 {
                            new_text.push(c);
                        }
                    }
                    self.text = new_text;
                    self.cursor_pos -= 1;
                    self.ensure_cursor_visible();
                    return Some(WidgetEvent::TextChanged(self.id, self.text.clone()));
                }
                None
            }
            WidgetInput::KeyDelete => {
                if self.state != WidgetState::Focused {
                    return None;
                }
                if self.cursor_pos < self.text.len() {
                    let mut new_text = String::new();
                    for (i, c) in self.text.chars().enumerate() {
                        if i != self.cursor_pos {
                            new_text.push(c);
                        }
                    }
                    self.text = new_text;
                    return Some(WidgetEvent::TextChanged(self.id, self.text.clone()));
                }
                None
            }
            WidgetInput::KeyEnter => {
                if self.state != WidgetState::Focused {
                    return None;
                }
                Some(WidgetEvent::TextSubmitted(self.id, self.text.clone()))
            }
            WidgetInput::KeyLeft => {
                if self.state != WidgetState::Focused {
                    return None;
                }
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                }
                self.ensure_cursor_visible();
                None
            }
            WidgetInput::KeyRight => {
                if self.state != WidgetState::Focused {
                    return None;
                }
                if self.cursor_pos < self.text.len() {
                    self.cursor_pos += 1;
                }
                self.ensure_cursor_visible();
                None
            }
            WidgetInput::KeyHome => {
                if self.state != WidgetState::Focused {
                    return None;
                }
                self.cursor_pos = 0;
                self.ensure_cursor_visible();
                None
            }
            WidgetInput::KeyEnd => {
                if self.state != WidgetState::Focused {
                    return None;
                }
                self.cursor_pos = self.text.len();
                self.ensure_cursor_visible();
                None
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Scrollbar widget (vertical)
// ---------------------------------------------------------------------------

pub struct Scrollbar {
    pub id: WidgetId,
    pub bounds: Rect,
    pub total_content: u32, // Total content height in pixels
    pub visible_area: u32,  // Visible area height
    pub scroll_pos: i32,    // Current scroll position (0..max)
    pub state: WidgetState,
    pub dragging: bool,
    pub drag_start_y: i32,
    pub drag_start_pos: i32,
    pub visible: bool,
}

impl Scrollbar {
    pub fn new(
        id: WidgetId,
        x: i32,
        y: i32,
        height: u32,
        total_content: u32,
        visible_area: u32,
    ) -> Self {
        Scrollbar {
            id,
            bounds: Rect::new(x, y, 14, height),
            total_content,
            visible_area,
            scroll_pos: 0,
            state: WidgetState::Normal,
            dragging: false,
            drag_start_y: 0,
            drag_start_pos: 0,
            visible: true,
        }
    }

    fn max_scroll(&self) -> i32 {
        if self.total_content <= self.visible_area {
            return 0;
        }
        (self.total_content - self.visible_area) as i32
    }

    fn thumb_height(&self) -> u32 {
        if self.total_content == 0 {
            return self.bounds.h;
        }
        let h =
            (self.visible_area as u64 * self.bounds.h as u64 / self.total_content as u64) as u32;
        h.max(20).min(self.bounds.h)
    }

    fn thumb_y(&self) -> i32 {
        let max = self.max_scroll();
        if max == 0 {
            return self.bounds.y;
        }
        let track = self.bounds.h - self.thumb_height();
        self.bounds.y + ((self.scroll_pos as u64 * track as u64 / max as u64) as i32)
    }

    fn thumb_rect(&self) -> Rect {
        Rect::new(
            self.bounds.x,
            self.thumb_y(),
            self.bounds.w,
            self.thumb_height(),
        )
    }

    pub fn draw(&self, buf: &mut [u32], buf_w: u32) {
        if !self.visible {
            return;
        }
        let colors = &WIDGET_COLORS;

        // Track
        fill_rect(
            buf,
            buf_w,
            self.bounds.x,
            self.bounds.y,
            self.bounds.w,
            self.bounds.h,
            colors.scrollbar_track,
        );

        // Thumb
        let thumb = self.thumb_rect();
        let thumb_color = if self.dragging || self.state == WidgetState::Hovered {
            colors.scrollbar_thumb_hover
        } else {
            colors.scrollbar_thumb
        };
        fill_rect(buf, buf_w, thumb.x, thumb.y, thumb.w, thumb.h, thumb_color);
    }

    pub fn handle_input(&mut self, input: &WidgetInput) -> Option<WidgetEvent> {
        if !self.visible || self.state == WidgetState::Disabled {
            return None;
        }
        match input {
            WidgetInput::MouseMove(x, y) => {
                if self.dragging {
                    let max = self.max_scroll();
                    let track = self.bounds.h - self.thumb_height();
                    if track > 0 {
                        let delta_y = *y - self.drag_start_y;
                        let delta_scroll = ((delta_y as i64 * max as i64) / track as i64) as i32;
                        self.scroll_pos = (self.drag_start_pos + delta_scroll).max(0).min(max);
                        return Some(WidgetEvent::ScrollChanged(self.id, self.scroll_pos));
                    }
                } else if self.bounds.contains(*x, *y) {
                    self.state = WidgetState::Hovered;
                } else {
                    self.state = WidgetState::Normal;
                }
                None
            }
            WidgetInput::MouseDown(x, y, MouseButton::Left) => {
                let thumb = self.thumb_rect();
                if thumb.contains(*x, *y) {
                    self.dragging = true;
                    self.drag_start_y = *y;
                    self.drag_start_pos = self.scroll_pos;
                } else if self.bounds.contains(*x, *y) {
                    // Click on track: page up/down
                    let thumb_center = self.thumb_y() + self.thumb_height() as i32 / 2;
                    if *y < thumb_center {
                        self.scroll_pos = (self.scroll_pos - self.visible_area as i32).max(0);
                    } else {
                        self.scroll_pos =
                            (self.scroll_pos + self.visible_area as i32).min(self.max_scroll());
                    }
                    return Some(WidgetEvent::ScrollChanged(self.id, self.scroll_pos));
                }
                None
            }
            WidgetInput::MouseUp(_, _, MouseButton::Left) => {
                self.dragging = false;
                None
            }
            _ => None,
        }
    }

    pub fn set_scroll(&mut self, pos: i32) {
        self.scroll_pos = pos.max(0).min(self.max_scroll());
    }

    pub fn update_content(&mut self, total: u32, visible: u32) {
        self.total_content = total;
        self.visible_area = visible;
        self.scroll_pos = self.scroll_pos.min(self.max_scroll());
    }
}

// ---------------------------------------------------------------------------
// Checkbox widget
// ---------------------------------------------------------------------------

pub struct Checkbox {
    pub id: WidgetId,
    pub bounds: Rect,
    pub label: String,
    pub checked: bool,
    pub state: WidgetState,
    pub visible: bool,
}

impl Checkbox {
    pub fn new(id: WidgetId, x: i32, y: i32, label: &str) -> Self {
        let box_size = DEFAULT_FONT_HEIGHT;
        let text_w = label.len() as u32 * DEFAULT_FONT_WIDTH;
        let w = box_size + DEFAULT_PADDING + text_w;
        let h = DEFAULT_FONT_HEIGHT + DEFAULT_PADDING;
        Checkbox {
            id,
            bounds: Rect::new(x, y, w, h),
            label: String::from(label),
            checked: false,
            state: WidgetState::Normal,
            visible: true,
        }
    }

    pub fn draw(&self, buf: &mut [u32], buf_w: u32) {
        if !self.visible {
            return;
        }
        let colors = &WIDGET_COLORS;
        let box_size = DEFAULT_FONT_HEIGHT;
        let bx = self.bounds.x;
        let by = self.bounds.y;

        // Checkbox box
        let bg = if self.checked {
            colors.accent
        } else {
            colors.bg_normal
        };
        fill_rect(buf, buf_w, bx, by, box_size, box_size, bg);
        draw_border(buf, buf_w, bx, by, box_size, box_size, colors.border_normal);

        // Checkmark (simple X shape when checked)
        if self.checked {
            let cx = bx + (box_size / 2) as i32;
            let cy = by + (box_size / 2) as i32;
            let s = (box_size / 4) as i32;
            // Draw two diagonal lines for checkmark
            for i in -s..=s {
                let idx1 = ((cy + i) as u32 * buf_w + (cx + i) as u32) as usize;
                let idx2 = ((cy + i) as u32 * buf_w + (cx - i) as u32) as usize;
                if idx1 < buf.len() {
                    buf[idx1] = 0xFFFFFFFF;
                }
                if idx2 < buf.len() {
                    buf[idx2] = 0xFFFFFFFF;
                }
            }
        }

        // Label
        let tx = bx + box_size as i32 + DEFAULT_PADDING as i32;
        font::draw_string(
            buf,
            buf_w,
            tx as u32,
            by as u32,
            &self.label,
            colors.text_normal,
        );
    }

    pub fn handle_input(&mut self, input: &WidgetInput) -> Option<WidgetEvent> {
        if !self.visible || self.state == WidgetState::Disabled {
            return None;
        }
        match input {
            WidgetInput::MouseDown(x, y, MouseButton::Left) => {
                if self.bounds.contains(*x, *y) {
                    self.checked = !self.checked;
                    return Some(WidgetEvent::CheckboxToggled(self.id, self.checked));
                }
                None
            }
            WidgetInput::MouseMove(x, y) => {
                if self.bounds.contains(*x, *y) {
                    self.state = WidgetState::Hovered;
                } else {
                    self.state = WidgetState::Normal;
                }
                None
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Slider widget (horizontal)
// ---------------------------------------------------------------------------

pub struct Slider {
    pub id: WidgetId,
    pub bounds: Rect,
    pub min_val: i32,
    pub max_val: i32,
    pub value: i32,
    pub state: WidgetState,
    pub dragging: bool,
    pub visible: bool,
}

impl Slider {
    pub fn new(id: WidgetId, x: i32, y: i32, w: u32, min_val: i32, max_val: i32) -> Self {
        Slider {
            id,
            bounds: Rect::new(x, y, w, 20),
            min_val,
            max_val,
            value: min_val,
            state: WidgetState::Normal,
            dragging: false,
            visible: true,
        }
    }

    fn thumb_x(&self) -> i32 {
        let range = self.max_val - self.min_val;
        if range == 0 {
            return self.bounds.x;
        }
        let track_w = self.bounds.w as i32 - 12; // thumb width = 12
        self.bounds.x + ((self.value - self.min_val) as i64 * track_w as i64 / range as i64) as i32
    }

    fn value_from_x(&self, x: i32) -> i32 {
        let track_w = self.bounds.w as i32 - 12;
        if track_w <= 0 {
            return self.min_val;
        }
        let rel = (x - self.bounds.x).max(0).min(track_w);
        let range = self.max_val - self.min_val;
        self.min_val + (rel as i64 * range as i64 / track_w as i64) as i32
    }

    pub fn draw(&self, buf: &mut [u32], buf_w: u32) {
        if !self.visible {
            return;
        }
        let colors = &WIDGET_COLORS;

        // Track (centered vertically)
        let track_y = self.bounds.y + (self.bounds.h as i32 / 2) - 2;
        fill_rect(
            buf,
            buf_w,
            self.bounds.x,
            track_y,
            self.bounds.w,
            4,
            colors.scrollbar_track,
        );

        // Filled portion (left of thumb)
        let thumb_x = self.thumb_x();
        let filled_w = (thumb_x - self.bounds.x) as u32;
        if filled_w > 0 {
            fill_rect(
                buf,
                buf_w,
                self.bounds.x,
                track_y,
                filled_w,
                4,
                colors.accent,
            );
        }

        // Thumb
        let thumb_color = if self.dragging || self.state == WidgetState::Hovered {
            colors.scrollbar_thumb_hover
        } else {
            colors.scrollbar_thumb
        };
        fill_rect(
            buf,
            buf_w,
            thumb_x,
            self.bounds.y + 2,
            12,
            self.bounds.h - 4,
            thumb_color,
        );
        draw_border(
            buf,
            buf_w,
            thumb_x,
            self.bounds.y + 2,
            12,
            self.bounds.h - 4,
            colors.border_normal,
        );
    }

    pub fn handle_input(&mut self, input: &WidgetInput) -> Option<WidgetEvent> {
        if !self.visible || self.state == WidgetState::Disabled {
            return None;
        }
        match input {
            WidgetInput::MouseDown(x, y, MouseButton::Left) => {
                if self.bounds.contains(*x, *y) {
                    self.dragging = true;
                    self.value = self.value_from_x(*x);
                    return Some(WidgetEvent::SliderChanged(self.id, self.value));
                }
                None
            }
            WidgetInput::MouseMove(x, y) => {
                if self.dragging {
                    self.value = self.value_from_x(*x);
                    return Some(WidgetEvent::SliderChanged(self.id, self.value));
                }
                if self.bounds.contains(*x, *y) {
                    self.state = WidgetState::Hovered;
                } else {
                    self.state = WidgetState::Normal;
                }
                None
            }
            WidgetInput::MouseUp(_, _, MouseButton::Left) => {
                self.dragging = false;
                None
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Panel widget (container for grouping widgets)
// ---------------------------------------------------------------------------

pub struct Panel {
    pub id: WidgetId,
    pub bounds: Rect,
    pub title: String,
    pub bg_color: u32,
    pub show_border: bool,
    pub visible: bool,
}

impl Panel {
    pub fn new(id: WidgetId, x: i32, y: i32, w: u32, h: u32, title: &str) -> Self {
        Panel {
            id,
            bounds: Rect::new(x, y, w, h),
            title: String::from(title),
            bg_color: WIDGET_COLORS.bg_normal,
            show_border: true,
            visible: true,
        }
    }

    pub fn draw(&self, buf: &mut [u32], buf_w: u32) {
        if !self.visible {
            return;
        }
        let colors = &WIDGET_COLORS;

        fill_rect(
            buf,
            buf_w,
            self.bounds.x,
            self.bounds.y,
            self.bounds.w,
            self.bounds.h,
            self.bg_color,
        );
        if self.show_border {
            draw_border(
                buf,
                buf_w,
                self.bounds.x,
                self.bounds.y,
                self.bounds.w,
                self.bounds.h,
                colors.border_normal,
            );
        }
        if !self.title.is_empty() {
            // Title bar
            fill_rect(
                buf,
                buf_w,
                self.bounds.x,
                self.bounds.y,
                self.bounds.w,
                DEFAULT_FONT_HEIGHT + 4,
                colors.bg_pressed,
            );
            font::draw_string(
                buf,
                buf_w,
                (self.bounds.x + 4) as u32,
                (self.bounds.y + 2) as u32,
                &self.title,
                colors.accent,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Widget manager — tracks all widgets and routes events
// ---------------------------------------------------------------------------

pub struct WidgetManager {
    pub buttons: Vec<Button>,
    pub labels: Vec<TextLabel>,
    pub text_inputs: Vec<TextInput>,
    pub scrollbars: Vec<Scrollbar>,
    pub checkboxes: Vec<Checkbox>,
    pub sliders: Vec<Slider>,
    pub panels: Vec<Panel>,
    pub event_queue: Vec<WidgetEvent>,
    next_id: WidgetId,
}

impl WidgetManager {
    pub const fn new() -> Self {
        WidgetManager {
            buttons: Vec::new(),
            labels: Vec::new(),
            text_inputs: Vec::new(),
            scrollbars: Vec::new(),
            checkboxes: Vec::new(),
            sliders: Vec::new(),
            panels: Vec::new(),
            event_queue: Vec::new(),
            next_id: 1,
        }
    }

    pub fn alloc_id(&mut self) -> WidgetId {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    pub fn add_button(&mut self, x: i32, y: i32, w: u32, h: u32, label: &str) -> WidgetId {
        let id = self.alloc_id();
        self.buttons.push(Button::new(id, x, y, w, h, label));
        id
    }

    pub fn add_label(&mut self, x: i32, y: i32, text: &str) -> WidgetId {
        let id = self.alloc_id();
        self.labels.push(TextLabel::new(id, x, y, text));
        id
    }

    pub fn add_text_input(&mut self, x: i32, y: i32, w: u32, placeholder: &str) -> WidgetId {
        let id = self.alloc_id();
        self.text_inputs
            .push(TextInput::new(id, x, y, w, placeholder));
        id
    }

    pub fn add_scrollbar(&mut self, x: i32, y: i32, h: u32, total: u32, visible: u32) -> WidgetId {
        let id = self.alloc_id();
        self.scrollbars
            .push(Scrollbar::new(id, x, y, h, total, visible));
        id
    }

    pub fn add_checkbox(&mut self, x: i32, y: i32, label: &str) -> WidgetId {
        let id = self.alloc_id();
        self.checkboxes.push(Checkbox::new(id, x, y, label));
        id
    }

    pub fn add_slider(&mut self, x: i32, y: i32, w: u32, min: i32, max: i32) -> WidgetId {
        let id = self.alloc_id();
        self.sliders.push(Slider::new(id, x, y, w, min, max));
        id
    }

    pub fn add_panel(&mut self, x: i32, y: i32, w: u32, h: u32, title: &str) -> WidgetId {
        let id = self.alloc_id();
        self.panels.push(Panel::new(id, x, y, w, h, title));
        id
    }

    /// Route input to all widgets, collecting events
    pub fn handle_input(&mut self, input: &WidgetInput) {
        for btn in &mut self.buttons {
            if let Some(ev) = btn.handle_input(input) {
                self.event_queue.push(ev);
            }
        }
        for ti in &mut self.text_inputs {
            if let Some(ev) = ti.handle_input(input) {
                self.event_queue.push(ev);
            }
        }
        for sb in &mut self.scrollbars {
            if let Some(ev) = sb.handle_input(input) {
                self.event_queue.push(ev);
            }
        }
        for cb in &mut self.checkboxes {
            if let Some(ev) = cb.handle_input(input) {
                self.event_queue.push(ev);
            }
        }
        for sl in &mut self.sliders {
            if let Some(ev) = sl.handle_input(input) {
                self.event_queue.push(ev);
            }
        }
    }

    /// Draw all widgets (panels first as background, then others)
    pub fn draw_all(&self, buf: &mut [u32], buf_w: u32) {
        for panel in &self.panels {
            panel.draw(buf, buf_w);
        }
        for label in &self.labels {
            label.draw(buf, buf_w);
        }
        for btn in &self.buttons {
            btn.draw(buf, buf_w);
        }
        for ti in &self.text_inputs {
            ti.draw(buf, buf_w);
        }
        for sb in &self.scrollbars {
            sb.draw(buf, buf_w);
        }
        for cb in &self.checkboxes {
            cb.draw(buf, buf_w);
        }
        for sl in &self.sliders {
            sl.draw(buf, buf_w);
        }
    }

    /// Drain the event queue
    pub fn poll_events(&mut self) -> Vec<WidgetEvent> {
        let events = self.event_queue.clone();
        self.event_queue.clear();
        events
    }
}

static WIDGET_MGR: Mutex<WidgetManager> = Mutex::new(WidgetManager::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("    [widget] Widget toolkit initialized (button, label, input, scrollbar, checkbox, slider, panel)");
}

pub fn add_button(x: i32, y: i32, w: u32, h: u32, label: &str) -> WidgetId {
    WIDGET_MGR.lock().add_button(x, y, w, h, label)
}

pub fn add_label(x: i32, y: i32, text: &str) -> WidgetId {
    WIDGET_MGR.lock().add_label(x, y, text)
}

pub fn add_text_input(x: i32, y: i32, w: u32, placeholder: &str) -> WidgetId {
    WIDGET_MGR.lock().add_text_input(x, y, w, placeholder)
}

pub fn add_scrollbar(x: i32, y: i32, h: u32, total: u32, visible: u32) -> WidgetId {
    WIDGET_MGR.lock().add_scrollbar(x, y, h, total, visible)
}

pub fn handle_input(input: &WidgetInput) {
    WIDGET_MGR.lock().handle_input(input);
}

pub fn draw_all(buf: &mut [u32], buf_w: u32) {
    WIDGET_MGR.lock().draw_all(buf, buf_w);
}

pub fn poll_events() -> Vec<WidgetEvent> {
    WIDGET_MGR.lock().poll_events()
}
