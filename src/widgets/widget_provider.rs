use crate::sync::Mutex;
/// Widget provider for Genesis
///
/// System widgets: clock, weather, calendar,
/// battery, music, fitness, quick settings.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum SystemWidget {
    Clock,
    Weather,
    Calendar,
    Battery,
    MusicPlayer,
    Fitness,
    QuickSettings,
    Notes,
    Contacts,
    Search,
}

struct WidgetProvider {
    id: u32,
    widget_type: SystemWidget,
    name: [u8; 24],
    name_len: usize,
    min_width: u8,
    min_height: u8,
    resizable: bool,
    refresh_rate_secs: u32,
}

struct ProviderRegistry {
    providers: Vec<WidgetProvider>,
    next_id: u32,
}

static PROVIDERS: Mutex<Option<ProviderRegistry>> = Mutex::new(None);

impl ProviderRegistry {
    fn new() -> Self {
        let mut reg = ProviderRegistry {
            providers: Vec::new(),
            next_id: 1,
        };
        reg.register_system_widgets();
        reg
    }

    fn register_system_widgets(&mut self) {
        let widgets = [
            (SystemWidget::Clock, b"Clock" as &[u8], 2, 2, 60),
            (SystemWidget::Weather, b"Weather", 2, 2, 1800),
            (SystemWidget::Calendar, b"Calendar", 4, 2, 3600),
            (SystemWidget::Battery, b"Battery", 2, 1, 300),
            (SystemWidget::MusicPlayer, b"Music", 4, 2, 1),
            (SystemWidget::Fitness, b"Fitness", 2, 2, 600),
            (SystemWidget::QuickSettings, b"Quick Settings", 4, 2, 30),
            (SystemWidget::Notes, b"Notes", 2, 2, 0),
            (SystemWidget::Search, b"Search", 4, 1, 0),
        ];
        for (wtype, name, w, h, refresh) in widgets {
            let id = self.next_id;
            self.next_id = self.next_id.saturating_add(1);
            let mut n = [0u8; 24];
            let nlen = name.len().min(24);
            n[..nlen].copy_from_slice(&name[..nlen]);
            self.providers.push(WidgetProvider {
                id,
                widget_type: wtype,
                name: n,
                name_len: nlen,
                min_width: w,
                min_height: h,
                resizable: true,
                refresh_rate_secs: refresh,
            });
        }
    }
}

pub fn init() {
    let mut p = PROVIDERS.lock();
    *p = Some(ProviderRegistry::new());
    serial_println!("    Widgets: system providers registered");
}

// ── Widget provider rendering ─────────────────────────────────────────────────

const FB_STRIDE: u32 = 1920;

/// Colour palette (ARGB, matching the shell amber-on-dark aesthetic)
const BG_DARK: u32 = 0xFF0F0F1A;
const AMBER: u32 = 0xFFF59E0B;
const BLUE_HINT: u32 = 0xFF3B82F6;
const BORDER: u32 = 0xFF2D2D4E;

/// Render a system widget provider preview onto the kernel framebuffer.
///
/// Each `SystemWidget` variant paints a distinctive colour pattern so that
/// widget placement is visually distinguishable during kernel UI development.
/// Real data injection (clock digits, weather icons, etc.) is wired in once
/// the compositor's data-source pipeline is complete.
///
/// # Arguments
/// * `fb`          — ARGB framebuffer slice for the full display
/// * `x`, `y`      — top-left corner of the widget's grid cell (pixels)
/// * `w`, `h`      — width and height of the grid cell (pixels)
/// * `widget_type` — which system widget to draw
pub fn render(fb: &mut [u32], x: u32, y: u32, w: u32, h: u32, widget_type: SystemWidget) {
    if w == 0 || h == 0 {
        return;
    }

    // Choose an accent colour per widget type so they are distinguishable at a glance
    let accent = match widget_type {
        SystemWidget::Clock => AMBER,
        SystemWidget::Weather => BLUE_HINT,
        SystemWidget::Calendar => 0xFF10B981,      // emerald
        SystemWidget::Battery => 0xFF22D3EE,       // cyan
        SystemWidget::MusicPlayer => 0xFFA855F7,   // purple
        SystemWidget::Fitness => 0xFFEF4444,       // red
        SystemWidget::QuickSettings => 0xFF94A3B8, // slate
        SystemWidget::Notes => 0xFFFBBF24,         // yellow
        SystemWidget::Contacts => 0xFF60A5FA,      // sky blue
        SystemWidget::Search => 0xFF34D399,        // teal
    };

    for row in 0..h {
        let py = y.saturating_add(row);
        for col in 0..w {
            let px = x.saturating_add(col);
            let idx = py.saturating_mul(FB_STRIDE).saturating_add(px) as usize;
            if idx < fb.len() {
                fb[idx] = if row == 0 || row == h - 1 || col == 0 || col == w - 1 {
                    BORDER
                } else if row < 3 {
                    // Accent header strip
                    accent
                } else {
                    BG_DARK
                };
            }
        }
    }
}
