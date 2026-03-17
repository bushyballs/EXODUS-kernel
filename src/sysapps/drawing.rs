/// Drawing / paint application for Genesis OS
///
/// Bitmap canvas drawing with multiple brush types, color palette,
/// geometric shape tools, layer support, undo history, and export.
/// All coordinates and dimensions use integer math; color blending
/// uses Q16 fixed-point for alpha compositing.
///
/// Inspired by: GIMP, MS Paint, Krita. All code is original.

use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers
// ---------------------------------------------------------------------------

/// Q16 constant: 1.0
const Q16_ONE: i32 = 65536;

/// Q16 multiply
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Q16 divide
fn q16_div(a: i32, b: i32) -> Option<i32> {
    if b == 0 { return None; }
    Some((((a as i64) << 16) / (b as i64)) as i32)
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum canvas width
const MAX_WIDTH: u32 = 4096;
/// Maximum canvas height
const MAX_HEIGHT: u32 = 4096;
/// Maximum layers
const MAX_LAYERS: usize = 32;
/// Maximum undo steps
const MAX_UNDO: usize = 100;
/// Maximum palette colors
const MAX_PALETTE: usize = 256;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Drawing tool type
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tool {
    Pencil,
    Brush,
    Eraser,
    Fill,
    Eyedropper,
    Line,
    Rectangle,
    Ellipse,
    RoundedRect,
    Triangle,
    Text,
    Select,
    Move,
    Spray,
}

/// Brush shape
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BrushShape {
    Round,
    Square,
    Diamond,
    Slash,
    Backslash,
}

/// Blend mode for layers
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BlendMode {
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    Difference,
}

/// Result codes for drawing operations
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DrawResult {
    Success,
    OutOfBounds,
    LayerNotFound,
    LimitReached,
    InvalidInput,
    NothingToUndo,
    NothingToRedo,
    IoError,
}

/// A 32-bit ARGB color
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub a: u8,
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// A point on the canvas
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// A rectangle region
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Brush settings
#[derive(Debug, Clone, Copy)]
pub struct BrushSettings {
    pub size: u32,
    pub shape: BrushShape,
    pub opacity_q16: i32,
    pub hardness_q16: i32,
    pub spacing_q16: i32,
}

/// A canvas layer
#[derive(Debug, Clone)]
pub struct Layer {
    pub id: u32,
    pub name_hash: u64,
    pub visible: bool,
    pub locked: bool,
    pub opacity_q16: i32,
    pub blend_mode: BlendMode,
    pub pixel_hashes: Vec<u64>,
    pub dirty_region: Option<Rect>,
}

/// A stroke record for undo
#[derive(Debug, Clone)]
struct StrokeRecord {
    layer_id: u32,
    affected_region: Rect,
    old_pixel_hashes: Vec<u64>,
    timestamp: u64,
}

/// Export format
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExportFormat {
    Bmp,
    Png,
    Raw,
}

/// Export result
#[derive(Debug, Clone)]
pub struct ExportResult {
    pub format: ExportFormat,
    pub data_hash: u64,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
}

/// Selection state
#[derive(Debug, Clone, Copy)]
pub struct SelectionState {
    pub active: bool,
    pub rect: Rect,
}

/// Persistent drawing state
struct DrawingState {
    width: u32,
    height: u32,
    layers: Vec<Layer>,
    active_layer: u32,
    next_layer_id: u32,
    current_tool: Tool,
    foreground: Color,
    background: Color,
    brush: BrushSettings,
    palette: Vec<Color>,
    undo_stack: Vec<StrokeRecord>,
    redo_stack: Vec<StrokeRecord>,
    selection: SelectionState,
    grid_visible: bool,
    grid_size: u32,
    zoom_q16: i32,
    pan_x: i32,
    pan_y: i32,
    is_modified: bool,
    file_hash: u64,
    timestamp_counter: u64,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static DRAWING: Mutex<Option<DrawingState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_color(a: u8, r: u8, g: u8, b: u8) -> Color {
    Color { a, r, g, b }
}

fn default_palette() -> Vec<Color> {
    vec![
        make_color(0xFF, 0x00, 0x00, 0x00), // Black
        make_color(0xFF, 0xFF, 0xFF, 0xFF), // White
        make_color(0xFF, 0xFF, 0x00, 0x00), // Red
        make_color(0xFF, 0x00, 0xFF, 0x00), // Green
        make_color(0xFF, 0x00, 0x00, 0xFF), // Blue
        make_color(0xFF, 0xFF, 0xFF, 0x00), // Yellow
        make_color(0xFF, 0xFF, 0x00, 0xFF), // Magenta
        make_color(0xFF, 0x00, 0xFF, 0xFF), // Cyan
        make_color(0xFF, 0x80, 0x80, 0x80), // Gray
        make_color(0xFF, 0xC0, 0xC0, 0xC0), // Light gray
        make_color(0xFF, 0x80, 0x00, 0x00), // Dark red
        make_color(0xFF, 0x00, 0x80, 0x00), // Dark green
        make_color(0xFF, 0x00, 0x00, 0x80), // Dark blue
        make_color(0xFF, 0x80, 0x80, 0x00), // Olive
        make_color(0xFF, 0x80, 0x00, 0x80), // Purple
        make_color(0xFF, 0x00, 0x80, 0x80), // Teal
        make_color(0xFF, 0xFF, 0xA5, 0x00), // Orange
        make_color(0xFF, 0xA5, 0x2A, 0x2A), // Brown
        make_color(0xFF, 0xFF, 0xC0, 0xCB), // Pink
        make_color(0xFF, 0xAD, 0xD8, 0xE6), // Light blue
    ]
}

fn default_brush() -> BrushSettings {
    BrushSettings {
        size: 3,
        shape: BrushShape::Round,
        opacity_q16: Q16_ONE,
        hardness_q16: Q16_ONE,
        spacing_q16: Q16_ONE / 4,
    }
}

fn create_layer(id: u32, name_hash: u64, pixel_count: usize) -> Layer {
    Layer {
        id,
        name_hash,
        visible: true,
        locked: false,
        opacity_q16: Q16_ONE,
        blend_mode: BlendMode::Normal,
        pixel_hashes: vec![0u64; pixel_count],
        dirty_region: None,
    }
}

fn default_state() -> DrawingState {
    let w = 640u32;
    let h = 480u32;
    let px = (w * h) as usize;
    let bg = create_layer(0, 0xBACE_0000_0001, px);

    DrawingState {
        width: w,
        height: h,
        layers: vec![bg],
        active_layer: 0,
        next_layer_id: 1,
        current_tool: Tool::Pencil,
        foreground: make_color(0xFF, 0x00, 0x00, 0x00),
        background: make_color(0xFF, 0xFF, 0xFF, 0xFF),
        brush: default_brush(),
        palette: default_palette(),
        undo_stack: Vec::new(),
        redo_stack: Vec::new(),
        selection: SelectionState { active: false, rect: Rect { x: 0, y: 0, w: 0, h: 0 } },
        grid_visible: false,
        grid_size: 16,
        zoom_q16: Q16_ONE,
        pan_x: 0,
        pan_y: 0,
        is_modified: false,
        file_hash: 0,
        timestamp_counter: 1_700_000_000,
    }
}

fn next_timestamp(state: &mut DrawingState) -> u64 {
    state.timestamp_counter += 1;
    state.timestamp_counter
}

fn pixel_index(width: u32, x: i32, y: i32) -> Option<usize> {
    if x < 0 || y < 0 || x >= width as i32 {
        return None;
    }
    Some((y as u32 * width + x as u32) as usize)
}

fn color_to_hash(c: Color) -> u64 {
    ((c.a as u64) << 24) | ((c.r as u64) << 16) | ((c.g as u64) << 8) | (c.b as u64)
}

/// Alpha-blend two colors using Q16 alpha
fn blend_colors(src: Color, dst: Color, alpha_q16: i32) -> Color {
    let inv = Q16_ONE - alpha_q16;
    Color {
        a: 0xFF,
        r: ((q16_mul(src.r as i32 * 256, alpha_q16) + q16_mul(dst.r as i32 * 256, inv)) >> 8) as u8,
        g: ((q16_mul(src.g as i32 * 256, alpha_q16) + q16_mul(dst.g as i32 * 256, inv)) >> 8) as u8,
        b: ((q16_mul(src.b as i32 * 256, alpha_q16) + q16_mul(dst.b as i32 * 256, inv)) >> 8) as u8,
    }
}

fn save_region_for_undo(state: &mut DrawingState, region: Rect) {
    let layer_id = state.active_layer;
    if let Some(layer) = state.layers.iter().find(|l| l.id == layer_id) {
        let old_hashes = layer.pixel_hashes.clone();
        let ts = next_timestamp(state);
        state.undo_stack.push(StrokeRecord {
            layer_id,
            affected_region: region,
            old_pixel_hashes: old_hashes,
            timestamp: ts,
        });
        if state.undo_stack.len() > MAX_UNDO {
            state.undo_stack.remove(0);
        }
        state.redo_stack.clear();
    }
}

// ---------------------------------------------------------------------------
// Public API -- Canvas management
// ---------------------------------------------------------------------------

/// Create a new canvas (replaces existing)
pub fn new_canvas(width: u32, height: u32) -> DrawResult {
    let mut guard = DRAWING.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return DrawResult::IoError };
    if width == 0 || height == 0 || width > MAX_WIDTH || height > MAX_HEIGHT {
        return DrawResult::InvalidInput;
    }

    let px = (width * height) as usize;
    let bg = create_layer(0, 0xBACE_0000_0001, px);
    state.width = width;
    state.height = height;
    state.layers = vec![bg];
    state.active_layer = 0;
    state.next_layer_id = 1;
    state.undo_stack.clear();
    state.redo_stack.clear();
    state.selection = SelectionState { active: false, rect: Rect { x: 0, y: 0, w: 0, h: 0 } };
    state.is_modified = false;
    state.zoom_q16 = Q16_ONE;
    state.pan_x = 0;
    state.pan_y = 0;
    DrawResult::Success
}

/// Get canvas dimensions
pub fn get_canvas_size() -> (u32, u32) {
    let guard = DRAWING.lock();
    match guard.as_ref() {
        Some(state) => (state.width, state.height),
        None => (0, 0),
    }
}

// ---------------------------------------------------------------------------
// Public API -- Drawing tools
// ---------------------------------------------------------------------------

/// Set the active tool
pub fn set_tool(tool: Tool) {
    let mut guard = DRAWING.lock();
    if let Some(state) = guard.as_mut() {
        state.current_tool = tool;
    }
}

/// Get the active tool
pub fn get_tool() -> Tool {
    let guard = DRAWING.lock();
    match guard.as_ref() {
        Some(state) => state.current_tool,
        None => Tool::Pencil,
    }
}

/// Set the foreground color
pub fn set_foreground(color: Color) {
    let mut guard = DRAWING.lock();
    if let Some(state) = guard.as_mut() {
        state.foreground = color;
    }
}

/// Set the background color
pub fn set_background(color: Color) {
    let mut guard = DRAWING.lock();
    if let Some(state) = guard.as_mut() {
        state.background = color;
    }
}

/// Swap foreground and background colors
pub fn swap_colors() {
    let mut guard = DRAWING.lock();
    if let Some(state) = guard.as_mut() {
        core::mem::swap(&mut state.foreground, &mut state.background);
    }
}

/// Set brush settings
pub fn set_brush(settings: BrushSettings) {
    let mut guard = DRAWING.lock();
    if let Some(state) = guard.as_mut() {
        state.brush = settings;
    }
}

/// Draw a single pixel at (x, y) on the active layer
pub fn draw_pixel(x: i32, y: i32) -> DrawResult {
    let mut guard = DRAWING.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return DrawResult::IoError };

    let idx = match pixel_index(state.width, x, y) {
        Some(i) if i < (state.width * state.height) as usize => i,
        _ => return DrawResult::OutOfBounds,
    };

    let layer = match state.layers.iter_mut().find(|l| l.id == state.active_layer) {
        Some(l) => l,
        None => return DrawResult::LayerNotFound,
    };
    if layer.locked { return DrawResult::InvalidInput; }

    if idx < layer.pixel_hashes.len() {
        let ch = color_to_hash(state.foreground);
        layer.pixel_hashes[idx] = ch;
        layer.dirty_region = Some(Rect { x, y, w: 1, h: 1 });
    }
    state.is_modified = true;
    DrawResult::Success
}

/// Draw a line from (x0, y0) to (x1, y1) using Bresenham's algorithm
pub fn draw_line(x0: i32, y0: i32, x1: i32, y1: i32) -> DrawResult {
    let mut guard = DRAWING.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return DrawResult::IoError };

    let layer = match state.layers.iter_mut().find(|l| l.id == state.active_layer) {
        Some(l) => l,
        None => return DrawResult::LayerNotFound,
    };
    if layer.locked { return DrawResult::InvalidInput; }

    let ch = color_to_hash(state.foreground);
    let mut cx = x0;
    let mut cy = y0;
    let dx = if x1 > x0 { x1 - x0 } else { x0 - x1 };
    let dy = if y1 > y0 { -(y1 - y0) } else { y0 - y1 };
    let sx: i32 = if x0 < x1 { 1 } else { -1 };
    let sy: i32 = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;

    loop {
        if let Some(idx) = pixel_index(state.width, cx, cy) {
            if idx < layer.pixel_hashes.len() {
                layer.pixel_hashes[idx] = ch;
            }
        }
        if cx == x1 && cy == y1 { break; }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            cx += sx;
        }
        if e2 <= dx {
            err += dx;
            cy += sy;
        }
    }
    state.is_modified = true;
    DrawResult::Success
}

/// Draw a filled rectangle
pub fn draw_rect_filled(rect: Rect) -> DrawResult {
    let mut guard = DRAWING.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return DrawResult::IoError };

    let layer = match state.layers.iter_mut().find(|l| l.id == state.active_layer) {
        Some(l) => l,
        None => return DrawResult::LayerNotFound,
    };
    if layer.locked { return DrawResult::InvalidInput; }

    let ch = color_to_hash(state.foreground);
    for row in rect.y..(rect.y + rect.h as i32) {
        for col in rect.x..(rect.x + rect.w as i32) {
            if let Some(idx) = pixel_index(state.width, col, row) {
                if idx < layer.pixel_hashes.len() {
                    layer.pixel_hashes[idx] = ch;
                }
            }
        }
    }
    state.is_modified = true;
    DrawResult::Success
}

/// Draw an ellipse outline (midpoint algorithm)
pub fn draw_ellipse(cx: i32, cy: i32, rx: u32, ry: u32) -> DrawResult {
    let mut guard = DRAWING.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return DrawResult::IoError };

    let layer = match state.layers.iter_mut().find(|l| l.id == state.active_layer) {
        Some(l) => l,
        None => return DrawResult::LayerNotFound,
    };
    if layer.locked { return DrawResult::InvalidInput; }

    let ch = color_to_hash(state.foreground);
    let a = rx as i64;
    let b = ry as i64;
    if a == 0 || b == 0 { return DrawResult::InvalidInput; }

    // Plot 4 symmetric points
    let mut plot = |px: i32, py: i32| {
        if let Some(idx) = pixel_index(state.width, px, py) {
            if idx < layer.pixel_hashes.len() {
                layer.pixel_hashes[idx] = ch;
            }
        }
    };

    let mut x: i64 = 0;
    let mut y: i64 = b;
    let a2 = a * a;
    let b2 = b * b;
    let mut d = b2 - a2 * b + a2 / 4;

    while b2 * x <= a2 * y {
        plot(cx + x as i32, cy + y as i32);
        plot(cx - x as i32, cy + y as i32);
        plot(cx + x as i32, cy - y as i32);
        plot(cx - x as i32, cy - y as i32);
        x += 1;
        if d < 0 {
            d += b2 * (2 * x + 1);
        } else {
            y -= 1;
            d += b2 * (2 * x + 1) - 2 * a2 * y;
        }
    }

    d = b2 * (x * 2 + 1) * (x * 2 + 1) / 4 + a2 * (y - 1) * (y - 1) - a2 * b2;
    while y >= 0 {
        plot(cx + x as i32, cy + y as i32);
        plot(cx - x as i32, cy + y as i32);
        plot(cx + x as i32, cy - y as i32);
        plot(cx - x as i32, cy - y as i32);
        y -= 1;
        if d > 0 {
            d -= 2 * a2 * y + a2;
        } else {
            x += 1;
            d += 2 * b2 * x - 2 * a2 * y + a2;
        }
    }

    state.is_modified = true;
    DrawResult::Success
}

/// Flood fill from a point
pub fn flood_fill(x: i32, y: i32) -> DrawResult {
    let mut guard = DRAWING.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return DrawResult::IoError };

    let layer = match state.layers.iter_mut().find(|l| l.id == state.active_layer) {
        Some(l) => l,
        None => return DrawResult::LayerNotFound,
    };
    if layer.locked { return DrawResult::InvalidInput; }

    let start_idx = match pixel_index(state.width, x, y) {
        Some(i) if i < layer.pixel_hashes.len() => i,
        _ => return DrawResult::OutOfBounds,
    };

    let target = layer.pixel_hashes[start_idx];
    let fill = color_to_hash(state.foreground);
    if target == fill { return DrawResult::Success; }

    let w = state.width as i32;
    let h = state.height as i32;
    let mut stack: Vec<(i32, i32)> = vec![(x, y)];

    while let Some((px, py)) = stack.pop() {
        if px < 0 || py < 0 || px >= w || py >= h { continue; }
        let idx = (py * w + px) as usize;
        if idx >= layer.pixel_hashes.len() { continue; }
        if layer.pixel_hashes[idx] != target { continue; }
        layer.pixel_hashes[idx] = fill;
        stack.push((px + 1, py));
        stack.push((px - 1, py));
        stack.push((px, py + 1));
        stack.push((px, py - 1));
    }

    state.is_modified = true;
    DrawResult::Success
}

// ---------------------------------------------------------------------------
// Public API -- Layers
// ---------------------------------------------------------------------------

/// Add a new layer
pub fn add_layer(name_hash: u64) -> Result<u32, DrawResult> {
    let mut guard = DRAWING.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return Err(DrawResult::IoError) };
    if state.layers.len() >= MAX_LAYERS { return Err(DrawResult::LimitReached); }

    let id = state.next_layer_id;
    state.next_layer_id += 1;
    let px = (state.width * state.height) as usize;
    state.layers.push(create_layer(id, name_hash, px));
    state.active_layer = id;
    Ok(id)
}

/// Remove a layer
pub fn remove_layer(layer_id: u32) -> DrawResult {
    let mut guard = DRAWING.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return DrawResult::IoError };
    if state.layers.len() <= 1 { return DrawResult::InvalidInput; }

    let before = state.layers.len();
    state.layers.retain(|l| l.id != layer_id);
    if state.layers.len() == before { return DrawResult::LayerNotFound; }

    if state.active_layer == layer_id {
        state.active_layer = state.layers[0].id;
    }
    DrawResult::Success
}

/// Set the active layer
pub fn set_active_layer(layer_id: u32) -> DrawResult {
    let mut guard = DRAWING.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return DrawResult::IoError };
    if !state.layers.iter().any(|l| l.id == layer_id) { return DrawResult::LayerNotFound; }
    state.active_layer = layer_id;
    DrawResult::Success
}

/// Toggle layer visibility
pub fn toggle_layer_visibility(layer_id: u32) -> DrawResult {
    let mut guard = DRAWING.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return DrawResult::IoError };
    if let Some(layer) = state.layers.iter_mut().find(|l| l.id == layer_id) {
        layer.visible = !layer.visible;
        DrawResult::Success
    } else {
        DrawResult::LayerNotFound
    }
}

/// Set layer opacity (Q16)
pub fn set_layer_opacity(layer_id: u32, opacity_q16: i32) -> DrawResult {
    let mut guard = DRAWING.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return DrawResult::IoError };
    if let Some(layer) = state.layers.iter_mut().find(|l| l.id == layer_id) {
        layer.opacity_q16 = opacity_q16;
        DrawResult::Success
    } else {
        DrawResult::LayerNotFound
    }
}

/// Set layer blend mode
pub fn set_layer_blend(layer_id: u32, mode: BlendMode) -> DrawResult {
    let mut guard = DRAWING.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return DrawResult::IoError };
    if let Some(layer) = state.layers.iter_mut().find(|l| l.id == layer_id) {
        layer.blend_mode = mode;
        DrawResult::Success
    } else {
        DrawResult::LayerNotFound
    }
}

/// Get layer count
pub fn layer_count() -> usize {
    let guard = DRAWING.lock();
    match guard.as_ref() {
        Some(state) => state.layers.len(),
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Public API -- Undo / Redo
// ---------------------------------------------------------------------------

/// Undo the last stroke
pub fn undo() -> DrawResult {
    let mut guard = DRAWING.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return DrawResult::IoError };

    let record = match state.undo_stack.pop() {
        Some(r) => r,
        None => return DrawResult::NothingToUndo,
    };

    if let Some(layer) = state.layers.iter_mut().find(|l| l.id == record.layer_id) {
        let current = layer.pixel_hashes.clone();
        layer.pixel_hashes = record.old_pixel_hashes.clone();
        let ts = next_timestamp(state);
        state.redo_stack.push(StrokeRecord {
            layer_id: record.layer_id,
            affected_region: record.affected_region,
            old_pixel_hashes: current,
            timestamp: ts,
        });
    }
    state.is_modified = true;
    DrawResult::Success
}

/// Redo the last undone stroke
pub fn redo() -> DrawResult {
    let mut guard = DRAWING.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return DrawResult::IoError };

    let record = match state.redo_stack.pop() {
        Some(r) => r,
        None => return DrawResult::NothingToRedo,
    };

    if let Some(layer) = state.layers.iter_mut().find(|l| l.id == record.layer_id) {
        let current = layer.pixel_hashes.clone();
        layer.pixel_hashes = record.old_pixel_hashes.clone();
        let ts = next_timestamp(state);
        state.undo_stack.push(StrokeRecord {
            layer_id: record.layer_id,
            affected_region: record.affected_region,
            old_pixel_hashes: current,
            timestamp: ts,
        });
    }
    state.is_modified = true;
    DrawResult::Success
}

// ---------------------------------------------------------------------------
// Public API -- Palette and view
// ---------------------------------------------------------------------------

/// Add a color to the palette
pub fn add_palette_color(color: Color) -> DrawResult {
    let mut guard = DRAWING.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return DrawResult::IoError };
    if state.palette.len() >= MAX_PALETTE { return DrawResult::LimitReached; }
    state.palette.push(color);
    DrawResult::Success
}

/// Get the palette
pub fn get_palette() -> Vec<Color> {
    let guard = DRAWING.lock();
    match guard.as_ref() {
        Some(state) => state.palette.clone(),
        None => Vec::new(),
    }
}

/// Set zoom level (Q16)
pub fn set_zoom(zoom_q16: i32) {
    let mut guard = DRAWING.lock();
    if let Some(state) = guard.as_mut() {
        state.zoom_q16 = zoom_q16;
    }
}

/// Pan the view
pub fn pan(dx: i32, dy: i32) {
    let mut guard = DRAWING.lock();
    if let Some(state) = guard.as_mut() {
        state.pan_x += dx;
        state.pan_y += dy;
    }
}

/// Toggle grid visibility
pub fn toggle_grid() -> bool {
    let mut guard = DRAWING.lock();
    match guard.as_mut() {
        Some(state) => {
            state.grid_visible = !state.grid_visible;
            state.grid_visible
        }
        None => false,
    }
}

/// Export the canvas as a flattened image
pub fn export(format: ExportFormat) -> Result<ExportResult, DrawResult> {
    let guard = DRAWING.lock();
    let state = match guard.as_ref() { Some(s) => s, None => return Err(DrawResult::IoError) };

    let mut combined_hash: u64 = 0;
    for layer in state.layers.iter().filter(|l| l.visible) {
        for &ph in layer.pixel_hashes.iter() {
            combined_hash = combined_hash.wrapping_add(ph);
        }
    }

    let bpp: u64 = match format {
        ExportFormat::Bmp => 4,
        ExportFormat::Png => 3,
        ExportFormat::Raw => 4,
    };
    let size = (state.width as u64) * (state.height as u64) * bpp;

    Ok(ExportResult {
        format,
        data_hash: combined_hash,
        width: state.width,
        height: state.height,
        size_bytes: size,
    })
}

/// Check if canvas is modified
pub fn is_modified() -> bool {
    let guard = DRAWING.lock();
    match guard.as_ref() {
        Some(state) => state.is_modified,
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the drawing application subsystem
pub fn init() {
    let mut guard = DRAWING.lock();
    *guard = Some(default_state());
    serial_println!("    Drawing app ready (640x480 canvas)");
}
