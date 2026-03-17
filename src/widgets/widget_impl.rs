use super::layout::{Point, Rect, Size};
use crate::serial_println;
use crate::sync::Mutex;
/// Concrete widget implementations for Genesis
///
/// Provides the standard set of UI widgets used by all Genesis applications:
///
///   - `Button`       — tappable labelled button with press state
///   - `Label`        — static or dynamic text display
///   - `TextInput`    — single-line text entry field with cursor
///   - `ProgressBar`  — horizontal fill bar (0-100%)
///   - `ScrollView`   — scrollable container with clip rect
///
/// All widgets implement the `Widget` trait defined in this module.
///
/// ## Coordinate system
///
/// Widget bounds are in screen pixels.  (0, 0) is the top-left corner of the
/// display.
///
/// ## Rendering
///
/// Widgets are rendered into a `FrameBuffer` trait object supplied by the
/// caller.  This keeps the widget module independent of the specific display
/// subsystem (bochs VGA, GC9A01 round LCD, etc.).
///
/// ## Input
///
/// Input events are delivered as `InputEvent` values.  Each widget handles
/// events relevant to its type and reports state changes via the `on_event`
/// return value.
///
/// All code is original — Hoags Inc. (c) 2026.

#[allow(dead_code)]
use alloc::vec::Vec;

// ============================================================================
// Color palette (RGB888 packed in u32: 0x00RRGGBB)
// ============================================================================

pub type Color = u32;

pub const COLOR_BACKGROUND: Color = 0x001A1A2E;
pub const COLOR_SURFACE: Color = 0x0016213E;
pub const COLOR_PRIMARY: Color = 0x00F59E0B; // amber accent
pub const COLOR_TEXT: Color = 0x00E0E0E0;
pub const COLOR_TEXT_DIM: Color = 0x00808080;
pub const COLOR_BORDER: Color = 0x00334155;
pub const COLOR_HOVER: Color = 0x001E3A5F;
pub const COLOR_PRESSED: Color = 0x00F59E0B;
pub const COLOR_DISABLED: Color = 0x00444444;
pub const COLOR_ERROR: Color = 0x00EF4444;
pub const COLOR_SUCCESS: Color = 0x0022C55E;
pub const COLOR_CURSOR: Color = 0x00F59E0B;

// ============================================================================
// Framebuffer abstraction
// ============================================================================

/// Minimal framebuffer drawing interface.
///
/// Widget renderers call these methods; the actual implementation is
/// provided by the display subsystem.
pub trait FrameBuffer {
    /// Fill a rectangle with a solid color.
    fn fill_rect(&mut self, rect: Rect, color: Color);

    /// Draw a rectangle outline.
    fn draw_rect(&mut self, rect: Rect, color: Color);

    /// Draw a single character at (x, y).  Returns the x advance.
    fn draw_char(&mut self, x: i32, y: i32, c: char, color: Color) -> i32;

    /// Draw a UTF-8 string at (x, y).  Returns the final x position.
    fn draw_str(&mut self, x: i32, y: i32, s: &str, color: Color) -> i32 {
        let mut cx = x;
        for ch in s.chars() {
            cx = self.draw_char(cx, y, ch, color);
        }
        cx
    }

    /// Set the clip rectangle.  Drawing outside the clip rect is silently
    /// discarded.  Pass `None` to disable clipping.
    fn set_clip(&mut self, clip: Option<Rect>);
}

// ============================================================================
// Input events
// ============================================================================

/// Input event delivered to a widget
#[derive(Clone, Copy, Debug)]
pub enum InputEvent {
    /// A pointer (touch / mouse) was pressed at screen coordinates
    PointerDown { x: i32, y: i32 },
    /// A pointer moved to screen coordinates (may be dragging)
    PointerMove { x: i32, y: i32 },
    /// A pointer was released
    PointerUp { x: i32, y: i32 },
    /// A key was pressed (ASCII keycode for now)
    KeyDown { ascii: u8 },
    /// A key was released
    KeyUp { ascii: u8 },
    /// Focus was given to this widget
    FocusGained,
    /// Focus was removed from this widget
    FocusLost,
}

/// Outcome returned from `Widget::on_event`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventResult {
    /// Event was handled; no further propagation
    Consumed,
    /// Widget ignored the event; pass it to the next widget
    Ignored,
    /// Widget requests to become the focused widget
    RequestFocus,
}

// ============================================================================
// Widget trait
// ============================================================================

/// Common interface for all widgets.
pub trait Widget {
    /// Return the preferred (minimum) size of this widget.
    fn preferred_size(&self) -> Size;

    /// Return the current bounding rectangle.
    fn bounds(&self) -> Rect;

    /// Set the bounding rectangle (called by the layout engine).
    fn set_bounds(&mut self, bounds: Rect);

    /// Return `true` if the widget is visible.
    fn is_visible(&self) -> bool;

    /// Show or hide the widget.
    fn set_visible(&mut self, visible: bool);

    /// Return `true` if the widget is enabled (accepts input).
    fn is_enabled(&self) -> bool;

    /// Enable or disable the widget.
    fn set_enabled(&mut self, enabled: bool);

    /// Render the widget into the given framebuffer.
    fn render(&self, fb: &mut dyn FrameBuffer);

    /// Deliver an input event.  Returns how the event was handled.
    fn on_event(&mut self, event: InputEvent) -> EventResult;
}

// ============================================================================
// Button
// ============================================================================

/// Button state
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ButtonState {
    Normal,
    Hovered,
    Pressed,
    Disabled,
}

/// Tappable button widget
pub struct Button {
    bounds: Rect,
    label: [u8; 64],
    label_len: usize,
    state: ButtonState,
    visible: bool,
    /// Optional callback index (application registers callbacks in a table)
    pub action_id: u32,
    /// Set to `true` for one frame when the button is clicked
    pub clicked: bool,
}

impl Button {
    pub fn new(label: &str, x: i32, y: i32, w: u32, h: u32) -> Self {
        let mut lbl = [0u8; 64];
        let llen = label.len().min(63);
        lbl[..llen].copy_from_slice(&label.as_bytes()[..llen]);
        Button {
            bounds: Rect::new(x, y, w, h),
            label: lbl,
            label_len: llen,
            state: ButtonState::Normal,
            visible: true,
            action_id: 0,
            clicked: false,
        }
    }

    pub fn label_str(&self) -> &str {
        core::str::from_utf8(&self.label[..self.label_len]).unwrap_or("")
    }

    pub fn set_label(&mut self, label: &str) {
        let llen = label.len().min(63);
        self.label[..llen].copy_from_slice(&label.as_bytes()[..llen]);
        self.label_len = llen;
    }
}

impl Widget for Button {
    fn preferred_size(&self) -> Size {
        Size::new(120, 36)
    }

    fn bounds(&self) -> Rect {
        self.bounds
    }

    fn set_bounds(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }

    fn is_visible(&self) -> bool {
        self.visible
    }

    fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    fn is_enabled(&self) -> bool {
        self.state != ButtonState::Disabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.state = if enabled {
            ButtonState::Normal
        } else {
            ButtonState::Disabled
        };
    }

    fn render(&self, fb: &mut dyn FrameBuffer) {
        if !self.visible {
            return;
        }
        let bg = match self.state {
            ButtonState::Pressed => COLOR_PRESSED,
            ButtonState::Hovered => COLOR_HOVER,
            ButtonState::Disabled => COLOR_DISABLED,
            ButtonState::Normal => COLOR_SURFACE,
        };
        let text_color = if self.state == ButtonState::Disabled {
            COLOR_TEXT_DIM
        } else {
            COLOR_TEXT
        };

        fb.fill_rect(self.bounds, bg);
        fb.draw_rect(self.bounds, COLOR_BORDER);

        // Centre the label (approximate — 6 px per character)
        let text_w = (self.label_len as i32) * 6;
        let cx = self.bounds.x + (self.bounds.width as i32 - text_w) / 2;
        let cy = self.bounds.y + (self.bounds.height as i32 - 8) / 2;
        fb.draw_str(cx, cy, self.label_str(), text_color);
    }

    fn on_event(&mut self, event: InputEvent) -> EventResult {
        if self.state == ButtonState::Disabled {
            return EventResult::Ignored;
        }
        self.clicked = false;

        match event {
            InputEvent::PointerDown { x, y } => {
                if self.bounds.contains(Point::new(x, y)) {
                    self.state = ButtonState::Pressed;
                    return EventResult::Consumed;
                }
            }
            InputEvent::PointerUp { x, y } => {
                if self.state == ButtonState::Pressed {
                    self.state = if self.bounds.contains(Point::new(x, y)) {
                        self.clicked = true;
                        ButtonState::Normal
                    } else {
                        ButtonState::Normal
                    };
                    return EventResult::Consumed;
                }
            }
            InputEvent::PointerMove { x, y } => {
                if self.bounds.contains(Point::new(x, y)) {
                    if self.state == ButtonState::Normal {
                        self.state = ButtonState::Hovered;
                    }
                    return EventResult::Consumed;
                } else if self.state == ButtonState::Hovered {
                    self.state = ButtonState::Normal;
                }
            }
            _ => {}
        }
        EventResult::Ignored
    }
}

// ============================================================================
// Label
// ============================================================================

/// Static or dynamic text label
pub struct Label {
    bounds: Rect,
    text: [u8; 256],
    text_len: usize,
    color: Color,
    visible: bool,
}

impl Label {
    pub fn new(text: &str, x: i32, y: i32, w: u32, h: u32) -> Self {
        let mut t = [0u8; 256];
        let tlen = text.len().min(255);
        t[..tlen].copy_from_slice(&text.as_bytes()[..tlen]);
        Label {
            bounds: Rect::new(x, y, w, h),
            text: t,
            text_len: tlen,
            color: COLOR_TEXT,
            visible: true,
        }
    }

    pub fn set_text(&mut self, text: &str) {
        let tlen = text.len().min(255);
        self.text[..tlen].copy_from_slice(&text.as_bytes()[..tlen]);
        self.text_len = tlen;
    }

    pub fn set_color(&mut self, color: Color) {
        self.color = color;
    }

    pub fn text_str(&self) -> &str {
        core::str::from_utf8(&self.text[..self.text_len]).unwrap_or("")
    }
}

impl Widget for Label {
    fn preferred_size(&self) -> Size {
        Size::new((self.text_len as u32) * 6 + 8, 20)
    }

    fn bounds(&self) -> Rect {
        self.bounds
    }

    fn set_bounds(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }

    fn is_visible(&self) -> bool {
        self.visible
    }

    fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    fn is_enabled(&self) -> bool {
        true
    }

    fn set_enabled(&mut self, _enabled: bool) {}

    fn render(&self, fb: &mut dyn FrameBuffer) {
        if !self.visible {
            return;
        }
        fb.draw_str(
            self.bounds.x + 4,
            self.bounds.y + 4,
            self.text_str(),
            self.color,
        );
    }

    fn on_event(&mut self, _event: InputEvent) -> EventResult {
        EventResult::Ignored // Labels don't accept input
    }
}

// ============================================================================
// TextInput
// ============================================================================

/// Single-line text input field
pub struct TextInput {
    bounds: Rect,
    buffer: [u8; 256],
    buf_len: usize,
    /// Cursor position in bytes (0 = before first char)
    cursor: usize,
    focused: bool,
    visible: bool,
    enabled: bool,
    /// Placeholder text shown when empty
    placeholder: [u8; 64],
    placeholder_len: usize,
}

impl TextInput {
    pub fn new(placeholder: &str, x: i32, y: i32, w: u32, h: u32) -> Self {
        let mut ph = [0u8; 64];
        let phlen = placeholder.len().min(63);
        ph[..phlen].copy_from_slice(&placeholder.as_bytes()[..phlen]);
        TextInput {
            bounds: Rect::new(x, y, w, h),
            buffer: [0u8; 256],
            buf_len: 0,
            cursor: 0,
            focused: false,
            visible: true,
            enabled: true,
            placeholder: ph,
            placeholder_len: phlen,
        }
    }

    pub fn text(&self) -> &str {
        core::str::from_utf8(&self.buffer[..self.buf_len]).unwrap_or("")
    }

    pub fn clear(&mut self) {
        self.buf_len = 0;
        self.cursor = 0;
    }

    fn insert_char(&mut self, c: u8) {
        if self.buf_len >= 255 {
            return;
        }
        // Shift right
        let src = self.cursor;
        for i in (src..self.buf_len).rev() {
            self.buffer[i + 1] = self.buffer[i];
        }
        self.buffer[src] = c;
        self.buf_len += 1;
        self.cursor += 1;
    }

    fn delete_before_cursor(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let src = self.cursor;
        for i in src..self.buf_len {
            self.buffer[i - 1] = self.buffer[i];
        }
        self.buf_len -= 1;
        self.cursor -= 1;
    }
}

impl Widget for TextInput {
    fn preferred_size(&self) -> Size {
        Size::new(200, 32)
    }

    fn bounds(&self) -> Rect {
        self.bounds
    }

    fn set_bounds(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }

    fn is_visible(&self) -> bool {
        self.visible
    }

    fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    fn render(&self, fb: &mut dyn FrameBuffer) {
        if !self.visible {
            return;
        }
        let bg = if self.focused {
            COLOR_SURFACE
        } else {
            COLOR_BACKGROUND
        };
        let border = if self.focused {
            COLOR_PRIMARY
        } else {
            COLOR_BORDER
        };

        fb.fill_rect(self.bounds, bg);
        fb.draw_rect(self.bounds, border);

        let text_x = self.bounds.x + 6;
        let text_y = self.bounds.y + (self.bounds.height as i32 - 8) / 2;

        if self.buf_len == 0 && !self.focused {
            // Draw placeholder
            let ph = core::str::from_utf8(&self.placeholder[..self.placeholder_len]).unwrap_or("");
            fb.draw_str(text_x, text_y, ph, COLOR_TEXT_DIM);
        } else {
            let text = core::str::from_utf8(&self.buffer[..self.buf_len]).unwrap_or("");
            let end_x = fb.draw_str(text_x, text_y, text, COLOR_TEXT);

            // Draw cursor if focused
            if self.focused {
                // Approximate cursor x by counting chars before cursor
                let pre = core::str::from_utf8(&self.buffer[..self.cursor]).unwrap_or("");
                let cursor_x = text_x + (pre.len() as i32) * 6;
                fb.fill_rect(Rect::new(cursor_x, text_y, 2, 10), COLOR_CURSOR);
                let _ = end_x;
            }
        }
    }

    fn on_event(&mut self, event: InputEvent) -> EventResult {
        if !self.enabled {
            return EventResult::Ignored;
        }
        match event {
            InputEvent::PointerDown { x, y } => {
                if self.bounds.contains(Point::new(x, y)) {
                    return EventResult::RequestFocus;
                }
            }
            InputEvent::FocusGained => {
                self.focused = true;
                return EventResult::Consumed;
            }
            InputEvent::FocusLost => {
                self.focused = false;
                return EventResult::Consumed;
            }
            InputEvent::KeyDown { ascii } => {
                if !self.focused {
                    return EventResult::Ignored;
                }
                match ascii {
                    8 | 127 => {
                        // Backspace / DEL
                        self.delete_before_cursor();
                    }
                    0x1B => {
                        // Escape — lose focus
                        self.focused = false;
                    }
                    32..=126 => {
                        // Printable ASCII
                        self.insert_char(ascii);
                    }
                    _ => {}
                }
                return EventResult::Consumed;
            }
            _ => {}
        }
        EventResult::Ignored
    }
}

// ============================================================================
// ProgressBar
// ============================================================================

/// Horizontal progress bar (0-100%)
pub struct ProgressBar {
    bounds: Rect,
    /// Progress value 0-100
    pub value: u8,
    fill_color: Color,
    visible: bool,
    /// Whether to show a percentage label inside the bar
    pub show_label: bool,
}

impl ProgressBar {
    pub fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        ProgressBar {
            bounds: Rect::new(x, y, w, h),
            value: 0,
            fill_color: COLOR_PRIMARY,
            visible: true,
            show_label: false,
        }
    }

    pub fn set_value(&mut self, value: u8) {
        self.value = value.min(100);
    }

    pub fn set_color(&mut self, color: Color) {
        self.fill_color = color;
    }
}

impl Widget for ProgressBar {
    fn preferred_size(&self) -> Size {
        Size::new(200, 16)
    }

    fn bounds(&self) -> Rect {
        self.bounds
    }

    fn set_bounds(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }

    fn is_visible(&self) -> bool {
        self.visible
    }

    fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    fn is_enabled(&self) -> bool {
        true
    }

    fn set_enabled(&mut self, _enabled: bool) {}

    fn render(&self, fb: &mut dyn FrameBuffer) {
        if !self.visible {
            return;
        }
        // Background track
        fb.fill_rect(self.bounds, COLOR_SURFACE);
        fb.draw_rect(self.bounds, COLOR_BORDER);

        // Fill bar
        let fill_w = (self.bounds.width as u32 * self.value as u32 / 100).saturating_sub(2); // 1px padding each side
        if fill_w > 0 {
            let fill_rect = Rect::new(
                self.bounds.x + 1,
                self.bounds.y + 1,
                fill_w,
                self.bounds.height.saturating_sub(2),
            );
            fb.fill_rect(fill_rect, self.fill_color);
        }

        // Percentage label
        if self.show_label && self.bounds.height >= 12 {
            let pct_str: [u8; 4] = [
                b'0' + (self.value / 100) % 10,
                b'0' + (self.value / 10) % 10,
                b'0' + self.value % 10,
                b'%',
            ];
            let s = core::str::from_utf8(&pct_str).unwrap_or("  0%");
            let lx = self.bounds.x + (self.bounds.width as i32 - 24) / 2;
            let ly = self.bounds.y + (self.bounds.height as i32 - 8) / 2;
            fb.draw_str(lx, ly, s, COLOR_TEXT);
        }
    }

    fn on_event(&mut self, _event: InputEvent) -> EventResult {
        EventResult::Ignored
    }
}

// ============================================================================
// ScrollView
// ============================================================================

/// Scrollable container widget.
///
/// Clips rendering of its child content to `bounds` and allows vertical
/// scrolling via `scroll_y`.  Horizontal scrolling is not implemented.
pub struct ScrollView {
    bounds: Rect,
    /// Current vertical scroll offset in pixels (>= 0)
    pub scroll_y: i32,
    /// Total height of the content in pixels
    pub content_height: u32,
    visible: bool,
    /// Whether a scrollbar should be drawn on the right edge
    pub show_scrollbar: bool,
    /// Whether a pointer drag is in progress
    drag_active: bool,
    drag_start_y: i32,
    drag_start_scroll: i32,
}

impl ScrollView {
    pub fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        ScrollView {
            bounds: Rect::new(x, y, w, h),
            scroll_y: 0,
            content_height: 0,
            visible: true,
            show_scrollbar: true,
            drag_active: false,
            drag_start_y: 0,
            drag_start_scroll: 0,
        }
    }

    /// Clamp scroll_y to valid range.
    pub fn clamp_scroll(&mut self) {
        let max_scroll = (self.content_height as i32 - self.bounds.height as i32).max(0);
        self.scroll_y = self.scroll_y.clamp(0, max_scroll);
    }

    /// Scroll by `delta` pixels (positive = scroll down / content moves up).
    pub fn scroll_by(&mut self, delta: i32) {
        self.scroll_y += delta;
        self.clamp_scroll();
    }

    /// Scroll to ensure that a rect at `content_y` with height `h` is visible.
    pub fn ensure_visible(&mut self, content_y: i32, h: u32) {
        let view_h = self.bounds.height as i32;
        if content_y < self.scroll_y {
            self.scroll_y = content_y;
        } else if content_y + h as i32 > self.scroll_y + view_h {
            self.scroll_y = content_y + h as i32 - view_h;
        }
        self.clamp_scroll();
    }

    /// Apply the scroll offset transform and set the clip rect on the
    /// framebuffer, then call `render_fn` to draw content.
    ///
    /// `render_fn` receives the vertical offset to subtract from content y.
    pub fn render_with<F>(&self, fb: &mut dyn FrameBuffer, render_fn: F)
    where
        F: FnOnce(&mut dyn FrameBuffer, i32),
    {
        if !self.visible {
            return;
        }
        fb.set_clip(Some(self.bounds));
        render_fn(fb, self.scroll_y);
        fb.set_clip(None);

        if self.show_scrollbar && self.content_height > self.bounds.height {
            self.draw_scrollbar(fb);
        }
    }

    fn draw_scrollbar(&self, fb: &mut dyn FrameBuffer) {
        let track_w = 4u32;
        let track_x = self.bounds.x + self.bounds.width as i32 - track_w as i32;
        let track_h = self.bounds.height;

        // Track background
        fb.fill_rect(
            Rect::new(track_x, self.bounds.y, track_w, track_h),
            COLOR_SURFACE,
        );

        // Thumb
        let thumb_h = ((track_h as u64 * track_h as u64) / self.content_height as u64)
            .min(track_h as u64) as u32;
        let max_scroll = (self.content_height as i32 - self.bounds.height as i32).max(1);
        let thumb_y = self.bounds.y
            + (self.scroll_y as i64 * (track_h as i64 - thumb_h as i64) / max_scroll as i64) as i32;
        fb.fill_rect(
            Rect::new(track_x, thumb_y, track_w, thumb_h.max(8)),
            COLOR_PRIMARY,
        );
    }
}

impl Widget for ScrollView {
    fn preferred_size(&self) -> Size {
        Size::new(self.bounds.width, self.bounds.height)
    }

    fn bounds(&self) -> Rect {
        self.bounds
    }

    fn set_bounds(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }

    fn is_visible(&self) -> bool {
        self.visible
    }

    fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    fn is_enabled(&self) -> bool {
        true
    }

    fn set_enabled(&mut self, _enabled: bool) {}

    fn render(&self, fb: &mut dyn FrameBuffer) {
        if !self.visible {
            return;
        }
        // ScrollView without content — just draw a background
        fb.fill_rect(self.bounds, COLOR_BACKGROUND);
        if self.show_scrollbar {
            self.draw_scrollbar(fb);
        }
    }

    fn on_event(&mut self, event: InputEvent) -> EventResult {
        match event {
            InputEvent::PointerDown { x, y } => {
                if self.bounds.contains(Point::new(x, y)) {
                    self.drag_active = true;
                    self.drag_start_y = y;
                    self.drag_start_scroll = self.scroll_y;
                    return EventResult::Consumed;
                }
            }
            InputEvent::PointerMove { x: _, y } => {
                if self.drag_active {
                    let delta = self.drag_start_y - y;
                    self.scroll_y = self.drag_start_scroll + delta;
                    self.clamp_scroll();
                    return EventResult::Consumed;
                }
            }
            InputEvent::PointerUp { .. } => {
                if self.drag_active {
                    self.drag_active = false;
                    return EventResult::Consumed;
                }
            }
            InputEvent::KeyDown { ascii } => {
                match ascii {
                    // Page Up
                    0x21 => {
                        self.scroll_by(-(self.bounds.height as i32));
                        return EventResult::Consumed;
                    }
                    // Page Down
                    0x22 => {
                        self.scroll_by(self.bounds.height as i32);
                        return EventResult::Consumed;
                    }
                    // Home
                    0x24 => {
                        self.scroll_y = 0;
                        return EventResult::Consumed;
                    }
                    // End
                    0x23 => {
                        self.scroll_y =
                            (self.content_height as i32 - self.bounds.height as i32).max(0);
                        return EventResult::Consumed;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        EventResult::Ignored
    }
}

// ============================================================================
// Module init
// ============================================================================

pub fn init() {
    serial_println!("    Widgets/impl: Button, Label, TextInput, ProgressBar, ScrollView ready");
}
