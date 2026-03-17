/// Widget layout engine for Genesis
///
/// Implements three layout containers that position child widgets
/// automatically:
///
///   - `VBox`  — stack children vertically (top to bottom)
///   - `HBox`  — arrange children horizontally (left to right)
///   - `Grid`  — fixed-column grid, children fill left-to-right, top-to-bottom
///
/// All coordinates are in pixels.  The layout engine performs a two-pass
/// algorithm:
///   1. **Measure** — ask each child for its preferred size.
///   2. **Arrange** — assign each child a concrete (x, y, width, height) rect.
///
/// This module is `no_std`-compatible and allocation-free for the layout
/// containers themselves (children are referenced by index into an external
/// slice).
///
/// All code is original — Hoags Inc. (c) 2026.

#[allow(dead_code)]
// ============================================================================
// Geometry types
// ============================================================================

/// A point in 2D space
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Point { x, y }
    }
    pub const ZERO: Point = Point { x: 0, y: 0 };
}

/// A size (width × height)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub const fn new(width: u32, height: u32) -> Self {
        Size { width, height }
    }
    pub const ZERO: Size = Size {
        width: 0,
        height: 0,
    };
}

/// An axis-aligned rectangle
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(&self) -> i32 {
        self.x + self.width as i32
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.height as i32
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    pub fn intersects(&self, other: Rect) -> bool {
        self.x < other.right()
            && self.right() > other.x
            && self.y < other.bottom()
            && self.bottom() > other.y
    }

    /// Inset the rectangle on all sides by `amount` pixels.
    pub fn inset(&self, amount: i32) -> Rect {
        let amount_u = amount.unsigned_abs();
        if amount >= 0 {
            Rect {
                x: self.x + amount,
                y: self.y + amount,
                width: self.width.saturating_sub(amount_u * 2),
                height: self.height.saturating_sub(amount_u * 2),
            }
        } else {
            Rect {
                x: self.x + amount,
                y: self.y + amount,
                width: self.width + amount_u * 2,
                height: self.height + amount_u * 2,
            }
        }
    }
}

// ============================================================================
// Layout constraints
// ============================================================================

/// How a widget fills available space along one axis
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fill {
    /// Use exactly the preferred size
    Fixed,
    /// Expand to fill all available space
    Expand,
    /// Expand but no larger than `max_pixels`
    MaxPixels(u32),
}

impl Default for Fill {
    fn default() -> Self {
        Fill::Fixed
    }
}

/// Per-widget layout constraints passed to the layout engine
#[derive(Clone, Copy, Debug, Default)]
pub struct LayoutConstraints {
    /// Preferred size (may be ignored if Fill::Expand)
    pub preferred: Size,
    /// How to fill horizontal space
    pub fill_h: Fill,
    /// How to fill vertical space
    pub fill_v: Fill,
    /// Outer margin (space outside the widget border)
    pub margin: u32,
}

impl LayoutConstraints {
    pub fn fixed(w: u32, h: u32) -> Self {
        LayoutConstraints {
            preferred: Size::new(w, h),
            fill_h: Fill::Fixed,
            fill_v: Fill::Fixed,
            margin: 4,
        }
    }

    pub fn fill_h(h: u32) -> Self {
        LayoutConstraints {
            preferred: Size::new(0, h),
            fill_h: Fill::Expand,
            fill_v: Fill::Fixed,
            margin: 4,
        }
    }

    pub fn fill_both() -> Self {
        LayoutConstraints {
            preferred: Size::ZERO,
            fill_h: Fill::Expand,
            fill_v: Fill::Expand,
            margin: 4,
        }
    }
}

// ============================================================================
// Layout result
// ============================================================================

/// Maximum children a layout container can hold
pub const MAX_LAYOUT_CHILDREN: usize = 64;

/// Computed rectangle for each child
#[derive(Clone, Copy, Debug, Default)]
pub struct ChildRect {
    /// Child index (matches the input slice)
    pub index: usize,
    /// Final computed bounds
    pub bounds: Rect,
}

// ============================================================================
// VBox — vertical stack layout
// ============================================================================

/// Vertical box layout.
///
/// Stacks children top-to-bottom within `bounds`.  Children with
/// `Fill::Expand` on the vertical axis share the remaining vertical space
/// equally.
///
/// Returns a `[ChildRect; N]`-style result via `out` slice.
/// Returns the number of entries written.
pub fn vbox_layout(
    bounds: Rect,
    constraints: &[LayoutConstraints],
    out: &mut [ChildRect],
    spacing: u32,
) -> usize {
    let n = constraints.len().min(out.len());
    if n == 0 {
        return 0;
    }

    // Pass 1: measure fixed children, count expanders
    let mut fixed_height: u32 = 0;
    let mut expand_count: u32 = 0;

    for c in constraints[..n].iter() {
        let m2 = c.margin * 2;
        match c.fill_v {
            Fill::Expand => {
                expand_count += 1;
                fixed_height = fixed_height.saturating_add(m2);
            }
            Fill::MaxPixels(max) => {
                fixed_height = fixed_height
                    .saturating_add(c.preferred.height.min(max))
                    .saturating_add(m2);
            }
            Fill::Fixed => {
                fixed_height = fixed_height
                    .saturating_add(c.preferred.height)
                    .saturating_add(m2);
            }
        }
    }

    // Account for spacing between children
    let total_spacing = spacing.saturating_mul((n as u32).saturating_sub(1));
    fixed_height = fixed_height.saturating_add(total_spacing);

    let available = bounds.height.saturating_sub(fixed_height);
    let per_expand = if expand_count > 0 {
        available / expand_count
    } else {
        0
    };

    // Pass 2: assign rects
    let mut cursor_y = bounds.y;
    let content_x = bounds.x;
    let content_w = bounds.width;

    for (i, c) in constraints[..n].iter().enumerate() {
        let child_h = match c.fill_v {
            Fill::Expand => per_expand.saturating_sub(c.margin * 2),
            Fill::MaxPixels(max) => c.preferred.height.min(max),
            Fill::Fixed => c.preferred.height,
        };

        let child_w = match c.fill_h {
            Fill::Expand => content_w.saturating_sub(c.margin * 2),
            Fill::MaxPixels(max) => c
                .preferred
                .width
                .min(max)
                .min(content_w.saturating_sub(c.margin * 2)),
            Fill::Fixed => c
                .preferred
                .width
                .min(content_w.saturating_sub(c.margin * 2)),
        };

        let child_x = content_x + c.margin as i32;
        let child_y = cursor_y + c.margin as i32;

        out[i] = ChildRect {
            index: i,
            bounds: Rect::new(child_x, child_y, child_w, child_h),
        };

        cursor_y += (c.margin * 2 + child_h) as i32 + spacing as i32;
    }

    n
}

// ============================================================================
// HBox — horizontal stack layout
// ============================================================================

/// Horizontal box layout.
///
/// Arranges children left-to-right within `bounds`.
pub fn hbox_layout(
    bounds: Rect,
    constraints: &[LayoutConstraints],
    out: &mut [ChildRect],
    spacing: u32,
) -> usize {
    let n = constraints.len().min(out.len());
    if n == 0 {
        return 0;
    }

    // Pass 1: measure
    let mut fixed_width: u32 = 0;
    let mut expand_count: u32 = 0;

    for c in constraints[..n].iter() {
        let m2 = c.margin * 2;
        match c.fill_h {
            Fill::Expand => {
                expand_count += 1;
                fixed_width = fixed_width.saturating_add(m2);
            }
            Fill::MaxPixels(max) => {
                fixed_width = fixed_width
                    .saturating_add(c.preferred.width.min(max))
                    .saturating_add(m2);
            }
            Fill::Fixed => {
                fixed_width = fixed_width
                    .saturating_add(c.preferred.width)
                    .saturating_add(m2);
            }
        }
    }

    let total_spacing = spacing.saturating_mul((n as u32).saturating_sub(1));
    fixed_width = fixed_width.saturating_add(total_spacing);

    let available = bounds.width.saturating_sub(fixed_width);
    let per_expand = if expand_count > 0 {
        available / expand_count
    } else {
        0
    };

    // Pass 2: assign rects
    let mut cursor_x = bounds.x;
    let content_y = bounds.y;
    let content_h = bounds.height;

    for (i, c) in constraints[..n].iter().enumerate() {
        let child_w = match c.fill_h {
            Fill::Expand => per_expand.saturating_sub(c.margin * 2),
            Fill::MaxPixels(max) => c.preferred.width.min(max),
            Fill::Fixed => c.preferred.width,
        };

        let child_h = match c.fill_v {
            Fill::Expand => content_h.saturating_sub(c.margin * 2),
            Fill::MaxPixels(max) => c
                .preferred
                .height
                .min(max)
                .min(content_h.saturating_sub(c.margin * 2)),
            Fill::Fixed => c
                .preferred
                .height
                .min(content_h.saturating_sub(c.margin * 2)),
        };

        let child_x = cursor_x + c.margin as i32;
        let child_y = content_y + c.margin as i32;

        out[i] = ChildRect {
            index: i,
            bounds: Rect::new(child_x, child_y, child_w, child_h),
        };

        cursor_x += (c.margin * 2 + child_w) as i32 + spacing as i32;
    }

    n
}

// ============================================================================
// Grid layout
// ============================================================================

/// Grid layout.
///
/// Arranges children in a fixed-column grid.  All cells in the same row have
/// the same height (the tallest preferred height in that row).  Column widths
/// are equal, computed as `(bounds.width - spacing*(cols-1)) / cols`.
///
/// `cols` — number of columns (>= 1)
/// `spacing` — gap between cells in both axes
pub fn grid_layout(
    bounds: Rect,
    constraints: &[LayoutConstraints],
    out: &mut [ChildRect],
    cols: u32,
    spacing: u32,
) -> usize {
    let n = constraints.len().min(out.len());
    if n == 0 || cols == 0 {
        return 0;
    }

    // Column width (all equal)
    let total_hspacing = spacing.saturating_mul(cols.saturating_sub(1));
    let col_width = bounds.width.saturating_sub(total_hspacing) / cols;

    // Compute row heights: max preferred height in each row
    let rows = (n as u32 + cols - 1) / cols;
    let mut row_heights = [0u32; 64]; // max 64 rows
    let rows_capped = rows.min(64) as usize;

    for (i, c) in constraints[..n].iter().enumerate() {
        let row = (i as u32 / cols) as usize;
        if row < rows_capped {
            row_heights[row] = row_heights[row].max(c.preferred.height + c.margin * 2);
        }
    }

    // Assign rects
    for (i, c) in constraints[..n].iter().enumerate() {
        let col = (i as u32 % cols) as i32;
        let row = (i as u32 / cols) as usize;
        if row >= rows_capped {
            break;
        }

        // Compute y by summing preceding row heights
        let mut cell_y = bounds.y;
        for r in 0..row {
            cell_y += row_heights[r] as i32 + spacing as i32;
        }

        let cell_x = bounds.x + col * (col_width as i32 + spacing as i32);

        out[i] = ChildRect {
            index: i,
            bounds: Rect::new(
                cell_x + c.margin as i32,
                cell_y + c.margin as i32,
                col_width.saturating_sub(c.margin * 2),
                row_heights[row].saturating_sub(c.margin * 2),
            ),
        };
    }

    n
}

// ============================================================================
// Module init
// ============================================================================

pub fn init() {
    // Layout engine is stateless — nothing to initialise at runtime.
    crate::serial_println!("    Widgets/layout: layout engine ready (VBox, HBox, Grid)");
}
