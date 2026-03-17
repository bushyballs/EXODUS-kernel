use crate::sync::Mutex;
/// Widget host for Genesis
///
/// Manages widget instances on home screen,
/// handles sizing, updates, interaction.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum WidgetSize {
    Small,      // 2x2
    Medium,     // 4x2
    Large,      // 4x4
    ExtraLarge, // 4x6
}

struct WidgetInstance {
    id: u32,
    provider_id: u32,
    size: WidgetSize,
    position_x: u16,
    position_y: u16,
    page: u8,
    update_interval_secs: u32,
    last_update: u64,
    interactive: bool,
    visible: bool,
}

struct WidgetHost {
    instances: Vec<WidgetInstance>,
    next_id: u32,
    grid_cols: u8,
    grid_rows: u8,
    pages: u8,
}

static WIDGET_HOST: Mutex<Option<WidgetHost>> = Mutex::new(None);

impl WidgetHost {
    fn new() -> Self {
        WidgetHost {
            instances: Vec::new(),
            next_id: 1,
            grid_cols: 4,
            grid_rows: 6,
            pages: 5,
        }
    }

    fn add_widget(&mut self, provider_id: u32, size: WidgetSize, page: u8, x: u16, y: u16) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.instances.push(WidgetInstance {
            id,
            provider_id,
            size,
            position_x: x,
            position_y: y,
            page,
            update_interval_secs: 1800,
            last_update: 0,
            interactive: true,
            visible: true,
        });
        id
    }

    fn remove_widget(&mut self, widget_id: u32) {
        self.instances.retain(|w| w.id != widget_id);
    }

    fn needs_update(&self, current_time: u64) -> Vec<u32> {
        self.instances
            .iter()
            .filter(|w| {
                w.visible && (current_time - w.last_update) >= w.update_interval_secs as u64
            })
            .map(|w| w.id)
            .collect()
    }

    fn mark_updated(&mut self, widget_id: u32, timestamp: u64) {
        if let Some(w) = self.instances.iter_mut().find(|w| w.id == widget_id) {
            w.last_update = timestamp;
        }
    }
}

pub fn init() {
    let mut h = WIDGET_HOST.lock();
    *h = Some(WidgetHost::new());
    serial_println!("    Widgets: host manager ready");
}

// ── Widget rendering ──────────────────────────────────────────────────────────

/// Framebuffer stride (pixels per row).  Must match the boot-time resolution.
const FB_STRIDE: u32 = 1920;

/// Background fill colour for host-managed widget slots (dark slate).
const WIDGET_BG: u32 = 0xFF1A1A2E;
/// Border colour for widget boundaries.
const WIDGET_BORDER: u32 = 0xFF2D2D4E;
/// Text placeholder colour (amber, matching the shell aesthetic).
const WIDGET_FG: u32 = 0xFFF59E0B;

/// Render one widget slot onto the kernel framebuffer.
///
/// Draws a filled background rectangle with a 1-pixel border, then writes a
/// minimal "W" glyph at the top-left corner as a visual placeholder until a
/// real widget provider supplies its own pixels.
///
/// # Arguments
/// * `fb`  — mutable slice of ARGB pixel words covering the whole screen
/// * `x`   — left edge of the widget area (pixels)
/// * `y`   — top edge of the widget area (pixels)
/// * `w`   — width of the widget area (pixels)
/// * `h`   — height of the widget area (pixels)
pub fn render(fb: &mut [u32], x: u32, y: u32, w: u32, h: u32) {
    if w == 0 || h == 0 {
        return;
    }

    // Fill background
    for row in 0..h {
        let py = y.saturating_add(row);
        for col in 0..w {
            let px = x.saturating_add(col);
            let idx = py.saturating_mul(FB_STRIDE).saturating_add(px) as usize;
            if idx < fb.len() {
                fb[idx] = if row == 0 || row == h - 1 || col == 0 || col == w - 1 {
                    WIDGET_BORDER
                } else {
                    WIDGET_BG
                };
            }
        }
    }

    // Draw a 3×5 "W" glyph as a visual ID placeholder
    let glyph: [u8; 5] = [0b10001, 0b10001, 0b10101, 0b11011, 0b10001];
    let gx = x.saturating_add(2);
    let gy = y.saturating_add(2);
    for (row, &bits) in glyph.iter().enumerate() {
        for col in 0..5u32 {
            if bits & (1 << (4 - col)) != 0 {
                let px = gx.saturating_add(col);
                let py = gy.saturating_add(row as u32);
                let idx = py.saturating_mul(FB_STRIDE).saturating_add(px) as usize;
                if idx < fb.len() {
                    fb[idx] = WIDGET_FG;
                }
            }
        }
    }
}
