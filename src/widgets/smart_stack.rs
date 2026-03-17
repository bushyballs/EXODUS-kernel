use crate::sync::Mutex;
/// Smart widget stack for Genesis
///
/// AI-powered widget rotation, context-aware ordering,
/// time-based suggestions, usage learning.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

struct WidgetScore {
    widget_id: u32,
    time_score: u32,      // relevance to current time
    context_score: u32,   // relevance to current activity
    frequency_score: u32, // how often user views this
    recency_score: u32,   // how recently used
}

struct SmartStack {
    widgets: Vec<u32>,
    scores: Vec<WidgetScore>,
    current_top: Option<u32>,
    rotations: u32,
}

static SMART_STACK: Mutex<Option<SmartStack>> = Mutex::new(None);

impl SmartStack {
    fn new() -> Self {
        SmartStack {
            widgets: Vec::new(),
            scores: Vec::new(),
            current_top: None,
            rotations: 0,
        }
    }

    fn add_widget(&mut self, widget_id: u32) {
        self.widgets.push(widget_id);
        self.scores.push(WidgetScore {
            widget_id,
            time_score: 50,
            context_score: 50,
            frequency_score: 50,
            recency_score: 50,
        });
    }

    fn get_top_widget(&mut self, hour: u8) -> Option<u32> {
        if self.widgets.is_empty() {
            return None;
        }

        // Update time scores
        for score in self.scores.iter_mut() {
            // Morning: fitness, weather
            // Workday: calendar, notes
            // Evening: music, entertainment
            score.time_score = match hour {
                6..=8 => 70,   // morning widgets
                9..=16 => 60,  // work widgets
                17..=22 => 50, // evening widgets
                _ => 30,       // night
            };
        }

        // Find highest total score
        let best = self
            .scores
            .iter()
            .max_by_key(|s| s.time_score + s.context_score + s.frequency_score + s.recency_score)
            .map(|s| s.widget_id);

        if best != self.current_top {
            self.rotations = self.rotations.saturating_add(1);
            self.current_top = best;
        }
        best
    }

    fn record_interaction(&mut self, widget_id: u32) {
        if let Some(score) = self.scores.iter_mut().find(|s| s.widget_id == widget_id) {
            score.frequency_score = (score.frequency_score + 5).min(100);
            score.recency_score = 100;
        }
        // Decay other recency scores
        for score in self.scores.iter_mut() {
            if score.widget_id != widget_id {
                score.recency_score = score.recency_score.saturating_sub(5);
            }
        }
    }
}

pub fn init() {
    let mut s = SMART_STACK.lock();
    *s = Some(SmartStack::new());
    serial_println!("    Widgets: smart stack (AI rotation) ready");
}

// ── Smart stack rendering ─────────────────────────────────────────────────────

const FB_STRIDE: u32 = 1920;

const BG: u32 = 0xFF12122A;
const BORDER: u32 = 0xFF2D2D4E;
const ACTIVE: u32 = 0xFFF59E0B;
const INACTIVE: u32 = 0xFF3D3D5E;
const DOT_GAP: u32 = 6;

/// Render the smart-stack tray onto the kernel framebuffer.
///
/// Draws a dark background with a row of indicator dots along the bottom edge.
/// The dot at index `active_slot` is highlighted in amber; the others are dim.
/// This mirrors the Android 12 widget stack "dot navigation" UX pattern.
///
/// # Arguments
/// * `fb`          — ARGB framebuffer slice for the full display
/// * `x`, `y`      — top-left corner (pixels)
/// * `w`, `h`      — width and height (pixels)
/// * `active_slot` — index of the currently surfaced widget (0-based)
/// * `total_slots` — total number of widgets in the stack
pub fn render(fb: &mut [u32], x: u32, y: u32, w: u32, h: u32, active_slot: u32, total_slots: u32) {
    if w == 0 || h == 0 {
        return;
    }

    // Background + border
    for row in 0..h {
        let py = y.saturating_add(row);
        for col in 0..w {
            let px = x.saturating_add(col);
            let idx = py.saturating_mul(FB_STRIDE).saturating_add(px) as usize;
            if idx < fb.len() {
                fb[idx] = if row == 0 || row == h - 1 || col == 0 || col == w - 1 {
                    BORDER
                } else {
                    BG
                };
            }
        }
    }

    // Dot row at the bottom of the stack frame
    if total_slots == 0 || h < 8 {
        return;
    }
    let dots_y = y.saturating_add(h).saturating_sub(5);
    let total_dot_width = total_slots.saturating_mul(DOT_GAP).saturating_sub(1);
    let start_x = x.saturating_add(w / 2).saturating_sub(total_dot_width / 2);

    for slot in 0..total_slots {
        let dot_x = start_x.saturating_add(slot.saturating_mul(DOT_GAP));
        let color = if slot == active_slot {
            ACTIVE
        } else {
            INACTIVE
        };
        // Draw 3×3 dot
        for dr in 0..3u32 {
            for dc in 0..3u32 {
                let px = dot_x.saturating_add(dc);
                let py = dots_y.saturating_add(dr);
                let idx = py.saturating_mul(FB_STRIDE).saturating_add(px) as usize;
                if idx < fb.len() {
                    fb[idx] = color;
                }
            }
        }
    }
}
