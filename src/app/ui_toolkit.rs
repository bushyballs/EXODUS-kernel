use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

/// Color (ARGB8888)
#[derive(Debug, Clone, Copy)]
pub struct Color {
    pub a: u8,
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const WHITE: Color = Color {
        a: 255,
        r: 255,
        g: 255,
        b: 255,
    };
    pub const BLACK: Color = Color {
        a: 255,
        r: 0,
        g: 0,
        b: 0,
    };
    pub const RED: Color = Color {
        a: 255,
        r: 255,
        g: 0,
        b: 0,
    };
    pub const GREEN: Color = Color {
        a: 255,
        r: 0,
        g: 200,
        b: 0,
    };
    pub const BLUE: Color = Color {
        a: 255,
        r: 50,
        g: 100,
        b: 255,
    };
    pub const TRANSPARENT: Color = Color {
        a: 0,
        r: 0,
        g: 0,
        b: 0,
    };

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Color { a, r, g, b }
    }

    pub fn to_u32(&self) -> u32 {
        ((self.a as u32) << 24) | ((self.r as u32) << 16) | ((self.g as u32) << 8) | self.b as u32
    }
}

/// Edge insets (padding/margin)
#[derive(Debug, Clone, Copy, Default)]
pub struct EdgeInsets {
    pub top: u16,
    pub right: u16,
    pub bottom: u16,
    pub left: u16,
}

impl EdgeInsets {
    pub const fn all(v: u16) -> Self {
        EdgeInsets {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }
    pub const fn symmetric(h: u16, v: u16) -> Self {
        EdgeInsets {
            top: v,
            right: h,
            bottom: v,
            left: h,
        }
    }
}

/// Layout direction
#[derive(Debug, Clone, Copy)]
pub enum Axis {
    Horizontal,
    Vertical,
}

/// Alignment
#[derive(Debug, Clone, Copy)]
pub enum Alignment {
    Start,
    Center,
    End,
    SpaceBetween,
    SpaceAround,
}

/// Text alignment
#[derive(Debug, Clone, Copy)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

/// Font weight
#[derive(Debug, Clone, Copy)]
pub enum FontWeight {
    Light,
    Regular,
    Medium,
    Bold,
}

/// Border style
#[derive(Debug, Clone, Copy)]
pub struct Border {
    pub width: u8,
    pub color: Color,
    pub radius: u16,
}

/// Widget — the base building block
pub enum Widget {
    /// Text label
    Text {
        content: String,
        size: u16,
        color: Color,
        weight: FontWeight,
        align: TextAlign,
    },
    /// Container (box with style)
    Container {
        width: Option<u16>,
        height: Option<u16>,
        color: Color,
        border: Option<Border>,
        padding: EdgeInsets,
        child: Option<Box<Widget>>,
    },
    /// Flex layout (row or column)
    Flex {
        axis: Axis,
        main_align: Alignment,
        cross_align: Alignment,
        spacing: u16,
        children: Vec<Widget>,
    },
    /// Button
    Button {
        label: String,
        color: Color,
        text_color: Color,
        on_press: u32, // event ID
        disabled: bool,
    },
    /// Text input field
    TextInput {
        placeholder: String,
        value: String,
        on_change: u32, // event ID
        password: bool,
    },
    /// Image
    Image {
        data: Vec<u8>,
        width: u16,
        height: u16,
        fit: ImageFit,
    },
    /// Checkbox/Switch
    Toggle {
        checked: bool,
        label: String,
        on_toggle: u32,
    },
    /// Slider
    Slider {
        value: f32,
        min: f32,
        max: f32,
        on_change: u32,
    },
    /// Progress bar
    Progress {
        value: f32, // 0.0 to 1.0
        color: Color,
    },
    /// Scrollable container
    Scroll {
        axis: Axis,
        child: Box<Widget>,
        offset: i32,
    },
    /// List (efficient scrolling)
    List {
        items: Vec<Widget>,
        item_height: u16,
        scroll_offset: i32,
    },
    /// Spacer (flexible space)
    Spacer { flex: u8 },
    /// Icon
    Icon {
        name: String,
        size: u16,
        color: Color,
    },
    /// Divider line
    Divider { color: Color, thickness: u8 },
}

/// Image fit mode
#[derive(Debug, Clone, Copy)]
pub enum ImageFit {
    Contain,
    Cover,
    Fill,
    None,
}

/// Computed layout rectangle
#[derive(Debug, Clone, Copy, Default)]
pub struct LayoutRect {
    pub x: i32,
    pub y: i32,
    pub width: u16,
    pub height: u16,
}

/// Layout a widget tree into positioned rectangles
pub fn layout(widget: &Widget, bounds: LayoutRect) -> Vec<LayoutRect> {
    let mut rects = Vec::new();

    match widget {
        Widget::Container {
            width,
            height,
            child,
            padding,
            ..
        } => {
            let w = width.unwrap_or(bounds.width);
            let h = height.unwrap_or(bounds.height);
            rects.push(LayoutRect {
                x: bounds.x,
                y: bounds.y,
                width: w,
                height: h,
            });

            if let Some(child) = child {
                let inner = LayoutRect {
                    x: bounds.x + padding.left as i32,
                    y: bounds.y + padding.top as i32,
                    width: w.saturating_sub(padding.left + padding.right),
                    height: h.saturating_sub(padding.top + padding.bottom),
                };
                rects.extend(layout(child, inner));
            }
        }
        Widget::Flex {
            axis,
            children,
            spacing,
            ..
        } => {
            let mut offset = 0i32;
            for child in children {
                let child_bounds = match axis {
                    Axis::Horizontal => LayoutRect {
                        x: bounds.x + offset,
                        y: bounds.y,
                        width: bounds.width / children.len().max(1) as u16,
                        height: bounds.height,
                    },
                    Axis::Vertical => LayoutRect {
                        x: bounds.x,
                        y: bounds.y + offset,
                        width: bounds.width,
                        height: bounds.height / children.len().max(1) as u16,
                    },
                };
                rects.extend(layout(child, child_bounds));
                offset += match axis {
                    Axis::Horizontal => child_bounds.width as i32 + *spacing as i32,
                    Axis::Vertical => child_bounds.height as i32 + *spacing as i32,
                };
            }
        }
        Widget::Text { .. } | Widget::Button { .. } | Widget::TextInput { .. } => {
            rects.push(bounds);
        }
        _ => {
            rects.push(bounds);
        }
    }
    rects
}

/// Helper functions for building widget trees
pub fn text(content: &str) -> Widget {
    Widget::Text {
        content: String::from(content),
        size: 14,
        color: Color::WHITE,
        weight: FontWeight::Regular,
        align: TextAlign::Left,
    }
}

pub fn button(label: &str, event_id: u32) -> Widget {
    Widget::Button {
        label: String::from(label),
        color: Color::BLUE,
        text_color: Color::WHITE,
        on_press: event_id,
        disabled: false,
    }
}

pub fn column(children: Vec<Widget>) -> Widget {
    Widget::Flex {
        axis: Axis::Vertical,
        main_align: Alignment::Start,
        cross_align: Alignment::Start,
        spacing: 8,
        children,
    }
}

pub fn row(children: Vec<Widget>) -> Widget {
    Widget::Flex {
        axis: Axis::Horizontal,
        main_align: Alignment::Start,
        cross_align: Alignment::Center,
        spacing: 8,
        children,
    }
}

pub fn init() {
    // UI toolkit is stateless — nothing to initialize
}
