use crate::sync::Mutex;
/// Layout and paint engine for Genesis browser
///
/// Implements a basic CSS box model layout engine. Computes
/// positions and dimensions using Q16 fixed-point arithmetic.
/// Supports block, inline, and none display modes. Produces
/// a list of paint commands for the display subsystem.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

static RENDERER: Mutex<Option<RendererState>> = Mutex::new(None);

/// Q16 fixed-point: 1 << 16
const Q16_ONE: i32 = 65536;

/// Q16 multiply: (a * b) >> 16
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Display type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Display {
    Block,
    Inline,
    None,
}

/// Edge sizes (margin, padding, border) in Q16 pixels
#[derive(Debug, Clone, Copy)]
pub struct EdgeSizes {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

impl EdgeSizes {
    pub fn zero() -> Self {
        EdgeSizes {
            top: 0,
            right: 0,
            bottom: 0,
            left: 0,
        }
    }

    pub fn uniform(val: i32) -> Self {
        EdgeSizes {
            top: val,
            right: val,
            bottom: val,
            left: val,
        }
    }

    pub fn horizontal(&self) -> i32 {
        self.left + self.right
    }

    pub fn vertical(&self) -> i32 {
        self.top + self.bottom
    }
}

/// Dimensions of a layout box
#[derive(Debug, Clone, Copy)]
pub struct Dimensions {
    pub x: i32, // Q16 position
    pub y: i32,
    pub width: i32,  // Q16 content width
    pub height: i32, // Q16 content height
}

impl Dimensions {
    pub fn zero() -> Self {
        Dimensions {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        }
    }
}

/// Style inputs for layout
#[derive(Debug, Clone)]
pub struct BoxStyle {
    pub display: Display,
    pub width: i32,  // Q16 (0 = auto)
    pub height: i32, // Q16 (0 = auto)
    pub margin: EdgeSizes,
    pub padding: EdgeSizes,
    pub border: EdgeSizes,
    pub color: u32, // 0xAARRGGBB
    pub background: u32,
    pub font_size: i32, // Q16 px
    pub tag_hash: u64,
    pub node_id: u32,
}

impl BoxStyle {
    pub fn default_block() -> Self {
        BoxStyle {
            display: Display::Block,
            width: 0,
            height: 0,
            margin: EdgeSizes::zero(),
            padding: EdgeSizes::zero(),
            border: EdgeSizes::zero(),
            color: 0xFF000000,
            background: 0x00000000, // transparent
            font_size: 16 * Q16_ONE,
            tag_hash: 0,
            node_id: 0,
        }
    }
}

/// A layout box in the render tree
#[derive(Debug, Clone)]
pub struct LayoutBox {
    pub dimensions: Dimensions,
    pub margin: EdgeSizes,
    pub padding: EdgeSizes,
    pub border: EdgeSizes,
    pub display: Display,
    pub style: BoxStyle,
    pub children: Vec<LayoutBox>,
}

impl LayoutBox {
    pub fn new(style: BoxStyle) -> Self {
        LayoutBox {
            dimensions: Dimensions::zero(),
            margin: style.margin,
            padding: style.padding,
            border: style.border,
            display: style.display,
            style,
            children: Vec::new(),
        }
    }

    /// Total width including margin + border + padding + content
    pub fn total_width(&self) -> i32 {
        self.margin.horizontal()
            + self.border.horizontal()
            + self.padding.horizontal()
            + self.dimensions.width
    }

    /// Total height including margin + border + padding + content
    pub fn total_height(&self) -> i32 {
        self.margin.vertical()
            + self.border.vertical()
            + self.padding.vertical()
            + self.dimensions.height
    }

    /// The border-box rectangle (content + padding + border)
    pub fn border_box_x(&self) -> i32 {
        self.dimensions.x - self.padding.left - self.border.left
    }

    pub fn border_box_y(&self) -> i32 {
        self.dimensions.y - self.padding.top - self.border.top
    }

    pub fn border_box_width(&self) -> i32 {
        self.dimensions.width + self.padding.horizontal() + self.border.horizontal()
    }

    pub fn border_box_height(&self) -> i32 {
        self.dimensions.height + self.padding.vertical() + self.border.vertical()
    }
}

/// Paint command for the display subsystem
#[derive(Debug, Clone)]
pub enum PaintCommand {
    FillRect {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        color: u32,
    },
    DrawBorder {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        width: i32,
        color: u32,
    },
    DrawText {
        x: i32,
        y: i32,
        text_hash: u64,
        color: u32,
        size: i32,
    },
}

/// Renderer persistent state
struct RendererState {
    viewport_width: i32,  // Q16
    viewport_height: i32, // Q16
    frames_rendered: u64,
    paint_commands: Vec<PaintCommand>,
}

/// Set the viewport dimensions (Q16 pixels)
pub fn set_viewport(width: i32, height: i32) {
    let mut guard = RENDERER.lock();
    if let Some(ref mut state) = *guard {
        state.viewport_width = width;
        state.viewport_height = height;
    }
}

/// Build a layout tree from a list of BoxStyles (flat list with hierarchy implied by order)
pub fn layout_tree(styles: &[BoxStyle], container_width: i32) -> Vec<LayoutBox> {
    let mut result = Vec::new();
    for style in styles {
        if style.display == Display::None {
            continue;
        }
        let layout = compute_layout(style, container_width);
        result.push(layout);
    }
    // Position blocks vertically
    resolve_dimensions(&mut result, 0, 0, container_width);
    result
}

/// Compute layout for a single box within a container
pub fn compute_layout(style: &BoxStyle, container_width: i32) -> LayoutBox {
    let mut layout = LayoutBox::new(style.clone());

    // Resolve width
    if style.width > 0 {
        layout.dimensions.width = style.width;
    } else if style.display == Display::Block {
        // Block-level: fill available width minus margins/padding/border
        let used =
            layout.margin.horizontal() + layout.border.horizontal() + layout.padding.horizontal();
        layout.dimensions.width = container_width - used;
        if layout.dimensions.width < 0 {
            layout.dimensions.width = 0;
        }
    }

    // Resolve height (auto if not specified)
    if style.height > 0 {
        layout.dimensions.height = style.height;
    }
    // If height is auto (0), it will be determined by children in resolve_dimensions

    layout
}

/// Position all layout boxes, resolving auto heights.
/// Lays out blocks vertically and inlines horizontally.
pub fn resolve_dimensions(
    boxes: &mut Vec<LayoutBox>,
    start_x: i32,
    start_y: i32,
    container_width: i32,
) {
    let mut cursor_y = start_y;
    let mut inline_x = start_x;
    let mut inline_row_height: i32 = 0;

    for layout_box in boxes.iter_mut() {
        match layout_box.display {
            Display::Block => {
                // Flush any inline row
                if inline_x > start_x {
                    cursor_y += inline_row_height;
                    inline_x = start_x;
                    inline_row_height = 0;
                }

                layout_box.dimensions.x = start_x
                    + layout_box.margin.left
                    + layout_box.border.left
                    + layout_box.padding.left;
                layout_box.dimensions.y = cursor_y
                    + layout_box.margin.top
                    + layout_box.border.top
                    + layout_box.padding.top;

                // Layout children recursively
                if !layout_box.children.is_empty() {
                    let child_x = layout_box.dimensions.x;
                    let child_y = layout_box.dimensions.y;
                    let child_w = layout_box.dimensions.width;
                    resolve_dimensions(&mut layout_box.children, child_x, child_y, child_w);

                    // Auto height: sum of children
                    if layout_box.dimensions.height == 0 {
                        let mut max_y = child_y;
                        for child in &layout_box.children {
                            let child_bottom = child.dimensions.y + child.total_height();
                            if child_bottom > max_y {
                                max_y = child_bottom;
                            }
                        }
                        layout_box.dimensions.height = max_y - child_y;
                    }
                }

                // If still zero height and no children, use font size as minimum
                if layout_box.dimensions.height == 0 {
                    layout_box.dimensions.height = layout_box.style.font_size;
                }

                cursor_y += layout_box.total_height();
            }
            Display::Inline => {
                let box_width = if layout_box.dimensions.width > 0 {
                    layout_box.total_width()
                } else {
                    // Estimate: use font_size * 4 as default inline width
                    q16_mul(layout_box.style.font_size, 4 * Q16_ONE)
                };

                // Wrap to next line if needed
                if inline_x + box_width > start_x + container_width && inline_x > start_x {
                    cursor_y += inline_row_height;
                    inline_x = start_x;
                    inline_row_height = 0;
                }

                layout_box.dimensions.x = inline_x
                    + layout_box.margin.left
                    + layout_box.border.left
                    + layout_box.padding.left;
                layout_box.dimensions.y = cursor_y
                    + layout_box.margin.top
                    + layout_box.border.top
                    + layout_box.padding.top;

                if layout_box.dimensions.height == 0 {
                    layout_box.dimensions.height = layout_box.style.font_size;
                }
                if layout_box.dimensions.width == 0 {
                    layout_box.dimensions.width = q16_mul(layout_box.style.font_size, 4 * Q16_ONE);
                }

                inline_x += layout_box.total_width();
                let th = layout_box.total_height();
                if th > inline_row_height {
                    inline_row_height = th;
                }
            }
            Display::None => {
                // Skip entirely
            }
        }
    }
}

/// Generate paint commands from a layout tree
pub fn paint(boxes: &[LayoutBox]) -> Vec<PaintCommand> {
    let mut commands = Vec::new();
    for layout_box in boxes {
        if layout_box.display == Display::None {
            continue;
        }
        paint_box(layout_box, &mut commands);
    }

    let mut guard = RENDERER.lock();
    if let Some(ref mut state) = *guard {
        state.frames_rendered = state.frames_rendered.saturating_add(1);
        state.paint_commands = commands.clone();
    }
    commands
}

/// Paint a single layout box and its children
fn paint_box(layout_box: &LayoutBox, commands: &mut Vec<PaintCommand>) {
    // Draw background
    let bg = layout_box.style.background;
    if bg & 0xFF000000 != 0 {
        commands.push(PaintCommand::FillRect {
            x: layout_box.border_box_x(),
            y: layout_box.border_box_y(),
            w: layout_box.border_box_width(),
            h: layout_box.border_box_height(),
            color: bg,
        });
    }

    // Draw border if any
    let bw = layout_box.border.top;
    if bw > 0 {
        commands.push(PaintCommand::DrawBorder {
            x: layout_box.border_box_x(),
            y: layout_box.border_box_y(),
            w: layout_box.border_box_width(),
            h: layout_box.border_box_height(),
            width: bw,
            color: layout_box.style.color,
        });
    }

    // Draw text placeholder (real text rendering would use a font rasterizer)
    if layout_box.style.tag_hash != 0 {
        commands.push(PaintCommand::DrawText {
            x: layout_box.dimensions.x,
            y: layout_box.dimensions.y,
            text_hash: layout_box.style.tag_hash,
            color: layout_box.style.color,
            size: layout_box.style.font_size,
        });
    }

    // Paint children
    for child in &layout_box.children {
        paint_box(child, commands);
    }
}

/// Get the number of frames rendered
pub fn frames_rendered() -> u64 {
    let guard = RENDERER.lock();
    match guard.as_ref() {
        Some(state) => state.frames_rendered,
        None => 0,
    }
}

pub fn init() {
    let mut guard = RENDERER.lock();
    *guard = Some(RendererState {
        viewport_width: 1024 * Q16_ONE,
        viewport_height: 768 * Q16_ONE,
        frames_rendered: 0,
        paint_commands: Vec::new(),
    });
    serial_println!("    browser::renderer initialized");
}
