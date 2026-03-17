use crate::sync::Mutex;
/// Adaptive UI — AI-driven malleable graphics engine for Genesis
///
/// Per-app learned layouts, touch heatmaps, handedness detection,
/// color adaptation (time + content), morph commands, animation speed,
/// content density, gesture sensitivity — all learned per-user.
///
/// All Q16 fixed-point. No floats. Pure integer learning.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use super::{
    q16_div, q16_from_int, q16_mul, NeuralSignal, SignalKind, SignalPayload, Q16, Q16_HALF,
    Q16_ONE, Q16_TENTH, Q16_ZERO,
};

// ── Constants ───────────────────────────────────────────────────────

const HEATMAP_ROWS: usize = 16;
const HEATMAP_COLS: usize = 10;
const MAX_LAYOUTS: usize = 64;
const MAX_WIDGETS: usize = 32;
const HEATMAP_DECAY: Q16 = 64225; // 0.98
const COLOR_LERP_RATE: Q16 = 3277; // 0.05
const DENSITY_LERP_RATE: Q16 = 1638; // 0.025
const HANDEDNESS_THRESHOLD: Q16 = 9830; // 0.15

// ── Types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handedness {
    Left,
    Right,
    Ambidextrous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EasingFn {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    Spring,
    Bounce,
}

/// A command to morph the UI
#[derive(Clone)]
pub struct MorphCommand {
    pub widget_id: u32,
    pub target_x: i32,
    pub target_y: i32,
    pub target_w: u32,
    pub target_h: u32,
    pub easing: EasingFn,
    pub duration_ms: u32,
    pub opacity: Q16,
}

/// Per-widget placement learned from user behavior
pub struct WidgetPlacement {
    pub widget_id: u32,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub touch_count: u32,
    pub visible: bool,
}

/// Per-app adaptive layout
pub struct AdaptiveLayout {
    pub app_name: String,
    pub widgets: Vec<WidgetPlacement>,
    pub total_interactions: u32,
    pub confidence: Q16,
    pub last_used: u64,
    pub custom_spacing: Q16,
    pub toolbar_on_left: bool,
}

/// AI-adjusted color profile
#[derive(Clone)]
pub struct ColorProfile {
    pub brightness: Q16,
    pub contrast: Q16,
    pub saturation: Q16,
    pub warmth: Q16,
    pub blue_light_filter: Q16,
    pub auto_dark_mode: bool,
    pub content_aware: bool,
    pub accent_r: u8,
    pub accent_g: u8,
    pub accent_b: u8,
}

impl ColorProfile {
    pub fn default_profile() -> Self {
        ColorProfile {
            brightness: Q16_ONE * 7 / 10,
            contrast: Q16_HALF,
            saturation: Q16_HALF,
            warmth: Q16_HALF,
            blue_light_filter: Q16_ZERO,
            auto_dark_mode: true,
            content_aware: true,
            accent_r: 66,
            accent_g: 133,
            accent_b: 244,
        }
    }
}

// ── Adaptive UI Engine ──────────────────────────────────────────────

pub struct AdaptiveUiEngine {
    pub enabled: bool,
    pub layouts: BTreeMap<String, AdaptiveLayout>,
    pub usage_heatmap: [[Q16; HEATMAP_COLS]; HEATMAP_ROWS],
    pub color_profile: ColorProfile,
    pub user_handedness: Handedness,
    pub animation_speed: Q16,
    pub font_scale: Q16,
    pub touch_targets: Q16,
    pub content_density: Q16,
    pub scroll_momentum: Q16,
    pub gesture_sensitivity: Q16,
    pub morphs_applied: u64,
    pub tick_counter: u64,
    pub current_hour: u64,
    pub app_time_spent: BTreeMap<String, u32>,
}

impl AdaptiveUiEngine {
    pub const fn new() -> Self {
        AdaptiveUiEngine {
            enabled: true,
            layouts: BTreeMap::new(),
            usage_heatmap: [[Q16_ZERO; HEATMAP_COLS]; HEATMAP_ROWS],
            color_profile: ColorProfile {
                brightness: 45875,
                contrast: Q16_HALF,
                saturation: Q16_HALF,
                warmth: Q16_HALF,
                blue_light_filter: Q16_ZERO,
                auto_dark_mode: true,
                content_aware: true,
                accent_r: 66,
                accent_g: 133,
                accent_b: 244,
            },
            user_handedness: Handedness::Right,
            animation_speed: Q16_ONE,
            font_scale: Q16_ONE,
            touch_targets: Q16_ONE,
            content_density: Q16_HALF,
            scroll_momentum: Q16_HALF,
            gesture_sensitivity: Q16_HALF,
            morphs_applied: 0,
            tick_counter: 0,
            current_hour: 12,
            app_time_spent: BTreeMap::new(),
        }
    }

    // ── Touch Processing ────────────────────────────────────────────

    pub fn process_touch(&mut self, x: i32, y: i32, app: &str) {
        if !self.enabled {
            return;
        }
        let row = ((y / 100).max(0) as usize).min(HEATMAP_ROWS - 1);
        let col = ((x / 100).max(0) as usize).min(HEATMAP_COLS - 1);
        self.usage_heatmap[row][col] = self.usage_heatmap[row][col].saturating_add(Q16_ONE);

        let app_key = String::from(app);
        if let Some(layout) = self.layouts.get_mut(&app_key) {
            layout.total_interactions = layout.total_interactions.saturating_add(1);
            layout.last_used = self.tick_counter;
            // Strengthen confidence on interaction
            let gap = Q16_ONE - layout.confidence;
            layout.confidence += q16_mul(gap, Q16_TENTH / 10);
            // Track per-widget touches
            for widget in layout.widgets.iter_mut() {
                if x >= widget.x
                    && x < widget.x + widget.w as i32
                    && y >= widget.y
                    && y < widget.y + widget.h as i32
                {
                    widget.touch_count = widget.touch_count.saturating_add(1);
                }
            }
        }
        // Track app time
        let t = self.app_time_spent.entry(app_key).or_insert(0);
        *t = t.saturating_add(1);
    }

    // ── Layout Learning ─────────────────────────────────────────────

    pub fn learn_layout(&mut self, app: &str, interactions: &[(u32, u32)]) {
        if !self.enabled {
            return;
        }
        let app_key = String::from(app);

        if !self.layouts.contains_key(&app_key) {
            self.layouts.insert(
                app_key.clone(),
                AdaptiveLayout {
                    app_name: app_key.clone(),
                    widgets: Vec::new(),
                    total_interactions: 0,
                    confidence: Q16_TENTH,
                    last_used: self.tick_counter,
                    custom_spacing: q16_from_int(16),
                    toolbar_on_left: matches!(self.user_handedness, Handedness::Left),
                },
            );
        }

        if let Some(layout) = self.layouts.get_mut(&app_key) {
            for &(wx, wy) in interactions {
                let mut found = false;
                for widget in layout.widgets.iter_mut() {
                    if widget.x == wx as i32 && widget.y == wy as i32 {
                        widget.touch_count = widget.touch_count.saturating_add(1);
                        found = true;
                        break;
                    }
                }
                if !found && layout.widgets.len() < MAX_WIDGETS {
                    layout.widgets.push(WidgetPlacement {
                        widget_id: layout.widgets.len() as u32,
                        x: wx as i32,
                        y: wy as i32,
                        w: 80,
                        h: 40,
                        touch_count: 1,
                        visible: true,
                    });
                }
            }
            if self.layouts.len() > MAX_LAYOUTS {
                // Evict least-used layout
                let weakest = self
                    .layouts
                    .iter()
                    .min_by_key(|(_, l)| l.confidence)
                    .map(|(k, _)| k.clone());
                if let Some(key) = weakest {
                    self.layouts.remove(&key);
                }
            }
        }
    }

    // ── Morph Generation ────────────────────────────────────────────

    pub fn morph_for_app(&mut self, app: &str) -> Vec<MorphCommand> {
        let mut commands = Vec::new();
        let app_key = String::from(app);

        if let Some(layout) = self.layouts.get(&app_key) {
            for widget in &layout.widgets {
                if !widget.visible {
                    continue;
                }
                commands.push(MorphCommand {
                    widget_id: widget.widget_id,
                    target_x: widget.x,
                    target_y: widget.y,
                    target_w: widget.w,
                    target_h: widget.h,
                    easing: EasingFn::EaseInOut,
                    duration_ms: q16_mul(200 * Q16_ONE / 1000, self.animation_speed) as u32 * 1000
                        / Q16_ONE as u32,
                    opacity: Q16_ONE,
                });
            }
        }
        self.morphs_applied += commands.len() as u64;
        commands
    }

    // ── Auto-Hide Unused Widgets ────────────────────────────────────

    pub fn auto_hide_unused(&mut self, app: &str) {
        if !self.enabled {
            return;
        }
        let app_key = String::from(app);
        if let Some(layout) = self.layouts.get_mut(&app_key) {
            let total = layout.total_interactions.max(1);
            for widget in layout.widgets.iter_mut() {
                // Hide widgets with < 2% interaction share
                let ratio = q16_div(
                    q16_from_int(widget.touch_count as i32),
                    q16_from_int(total as i32),
                );
                if ratio < Q16_ONE / 50 && widget.touch_count < 2 {
                    widget.visible = false;
                }
            }
        }
    }

    // ── Auto-Enlarge Frequent Widgets ───────────────────────────────

    pub fn auto_enlarge_frequent(&mut self, app: &str) {
        if !self.enabled {
            return;
        }
        let app_key = String::from(app);
        if let Some(layout) = self.layouts.get_mut(&app_key) {
            let total = layout.total_interactions.max(1);
            for widget in layout.widgets.iter_mut() {
                let ratio = q16_div(
                    q16_from_int(widget.touch_count as i32),
                    q16_from_int(total as i32),
                );
                if ratio > Q16_ONE / 5 {
                    // > 20% of touches
                    widget.w = (widget.w * 5 / 4).min(300);
                    widget.h = (widget.h * 5 / 4).min(200);
                }
            }
        }
    }

    // ── Color Adaptation ────────────────────────────────────────────

    pub fn adjust_colors_for_time(&mut self) {
        if !self.enabled {
            return;
        }
        let hour = self.current_hour;
        let is_night = hour >= 22 || hour < 6;
        let is_dawn_dusk = (hour >= 6 && hour < 8) || (hour >= 18 && hour < 22);

        let (target_bright, target_warm, target_blue) = if is_night {
            (Q16_ONE * 3 / 10, Q16_ONE * 8 / 10, Q16_ONE * 9 / 10)
        } else if is_dawn_dusk {
            (Q16_ONE * 6 / 10, Q16_ONE * 7 / 10, Q16_HALF)
        } else {
            (Q16_ONE * 9 / 10, Q16_HALF, Q16_ZERO)
        };

        self.color_profile.brightness = self.lerp(
            self.color_profile.brightness,
            target_bright,
            COLOR_LERP_RATE,
        );
        self.color_profile.warmth =
            self.lerp(self.color_profile.warmth, target_warm, COLOR_LERP_RATE);
        self.color_profile.blue_light_filter = self.lerp(
            self.color_profile.blue_light_filter,
            target_blue,
            COLOR_LERP_RATE,
        );
    }

    pub fn adjust_colors_for_content(&mut self, content_type: u8) {
        if !self.enabled || !self.color_profile.content_aware {
            return;
        }
        let (target_contrast, target_sat) = match content_type {
            0 => (Q16_ONE * 9 / 10, Q16_ONE * 3 / 10), // Text
            1 => (Q16_HALF, Q16_HALF),                 // Photo
            2 => (Q16_ONE * 6 / 10, Q16_ONE * 7 / 10), // Video
            _ => (Q16_HALF, Q16_HALF),                 // Mixed
        };
        self.color_profile.contrast = self.lerp(
            self.color_profile.contrast,
            target_contrast,
            COLOR_LERP_RATE,
        );
        self.color_profile.saturation =
            self.lerp(self.color_profile.saturation, target_sat, COLOR_LERP_RATE);
    }

    // ── Handedness Detection ────────────────────────────────────────

    pub fn detect_handedness(&mut self) {
        if !self.enabled {
            return;
        }
        let mid = HEATMAP_COLS / 2;
        let mut left: i64 = 0;
        let mut right: i64 = 0;
        for row in 0..HEATMAP_ROWS {
            for col in 0..HEATMAP_COLS {
                let heat = self.usage_heatmap[row][col] as i64;
                if col < mid {
                    left += heat;
                } else {
                    right += heat;
                }
            }
        }
        let total = left + right;
        if total == 0 {
            return;
        }
        let right_ratio = q16_div(
            q16_from_int((right >> 4) as i32),
            q16_from_int((total >> 4) as i32),
        );
        let high = Q16_HALF + HANDEDNESS_THRESHOLD;
        let low = Q16_HALF - HANDEDNESS_THRESHOLD;
        self.user_handedness = if right_ratio > high {
            Handedness::Right
        } else if right_ratio < low {
            Handedness::Left
        } else {
            Handedness::Ambidextrous
        };
    }

    // ── Accent Color Generation ─────────────────────────────────────

    pub fn generate_accent_color(&self, app: &str) -> (u8, u8, u8) {
        let mut hash: u32 = 5381;
        for byte in app.bytes() {
            hash = hash.wrapping_mul(33).wrapping_add(byte as u32);
        }
        let hue = (hash % 360) as i32;
        let sat = 180 + ((hash >> 8) % 76) as i32;
        let val = 180 + ((hash >> 16) % 76) as i32;
        let hi = (hue / 60) % 6;
        let f = ((hue % 60) * 255) / 60;
        let p = (val * (255 - sat)) / 255;
        let q = (val * (255 - (sat * f) / 255)) / 255;
        let t = (val * (255 - (sat * (255 - f)) / 255)) / 255;
        let (r, g, b) = match hi {
            0 => (val, t, p),
            1 => (q, val, p),
            2 => (p, val, t),
            3 => (p, q, val),
            4 => (t, p, val),
            _ => (val, p, q),
        };
        (
            r.clamp(0, 255) as u8,
            g.clamp(0, 255) as u8,
            b.clamp(0, 255) as u8,
        )
    }

    // ── Tick ────────────────────────────────────────────────────────

    pub fn tick(&mut self) {
        if !self.enabled {
            return;
        }
        self.tick_counter = self.tick_counter.saturating_add(1);

        if self.tick_counter & 63 == 0 {
            self.decay_heatmap();
        }
        if self.tick_counter & 511 == 0 {
            self.detect_handedness();
        }
        if self.tick_counter & 255 == 0 {
            self.adjust_colors_for_time();
        }
        if self.tick_counter & 4095 == 0 {
            self.prune_stale_layouts();
        }
    }

    // ── Helpers ─────────────────────────────────────────────────────

    fn decay_heatmap(&mut self) {
        for row in 0..HEATMAP_ROWS {
            for col in 0..HEATMAP_COLS {
                self.usage_heatmap[row][col] = q16_mul(self.usage_heatmap[row][col], HEATMAP_DECAY);
                if self.usage_heatmap[row][col] < 32 {
                    self.usage_heatmap[row][col] = Q16_ZERO;
                }
            }
        }
    }

    fn prune_stale_layouts(&mut self) {
        let threshold = self.tick_counter.saturating_sub(100_000);
        let stale: Vec<String> = self
            .layouts
            .iter()
            .filter(|(_, l)| l.last_used < threshold && l.confidence < Q16_HALF)
            .map(|(k, _)| k.clone())
            .collect();
        for key in stale {
            self.layouts.remove(&key);
        }
    }

    fn lerp(&self, from: Q16, to: Q16, rate: Q16) -> Q16 {
        from + q16_mul(to - from, rate)
    }

    pub fn set_current_hour(&mut self, hour: u64) {
        self.current_hour = hour % 24;
    }

    pub fn process_signal(&mut self, signal: &NeuralSignal) {
        if !self.enabled {
            return;
        }
        match signal.kind {
            SignalKind::BatteryDrain => {
                if let SignalPayload::Integer(level) = signal.payload {
                    if level < 20 {
                        self.animation_speed = q16_from_int(3); // fast = skip
                        self.color_profile.brightness = Q16_TENTH * 2;
                    } else if level < 50 {
                        self.animation_speed = q16_from_int(2);
                    }
                }
            }
            SignalKind::ThermalEvent => {
                if let SignalPayload::Integer(temp) = signal.payload {
                    if temp > 80 {
                        self.animation_speed = q16_from_int(4);
                    }
                }
            }
            _ => {}
        }
    }
}

// ── Global Instance ─────────────────────────────────────────────────

pub static ADAPTIVE_UI: Mutex<AdaptiveUiEngine> = Mutex::new(AdaptiveUiEngine::new());

// ── Public API ──────────────────────────────────────────────────────

pub fn init() {
    let mut engine = ADAPTIVE_UI.lock();
    engine.enabled = true;
    engine.color_profile = ColorProfile::default_profile();
    engine.animation_speed = Q16_ONE;
    engine.font_scale = Q16_ONE;
    engine.touch_targets = Q16_ONE;
    engine.content_density = Q16_HALF;
    serial_println!("    [adaptive_ui] Malleable UI engine initialized");
}

pub fn touch(x: i32, y: i32, app: &str) {
    ADAPTIVE_UI.lock().process_touch(x, y, app);
}

pub fn morph(app: &str) -> Vec<MorphCommand> {
    let mut engine = ADAPTIVE_UI.lock();
    engine.auto_hide_unused(app);
    engine.auto_enlarge_frequent(app);
    let (r, g, b) = engine.generate_accent_color(app);
    engine.color_profile.accent_r = r;
    engine.color_profile.accent_g = g;
    engine.color_profile.accent_b = b;
    engine.morph_for_app(app)
}

pub fn color_profile() -> ColorProfile {
    ADAPTIVE_UI.lock().color_profile.clone()
}
pub fn tick() {
    ADAPTIVE_UI.lock().tick();
}
pub fn stats() -> (u64, Q16, Q16) {
    let e = ADAPTIVE_UI.lock();
    (e.morphs_applied, e.animation_speed, e.content_density)
}
pub fn learn(app: &str, interactions: &[(u32, u32)]) {
    ADAPTIVE_UI.lock().learn_layout(app, interactions);
}
pub fn adjust_for_content(content_type: u8) {
    ADAPTIVE_UI.lock().adjust_colors_for_content(content_type);
}
pub fn set_hour(hour: u64) {
    ADAPTIVE_UI.lock().set_current_hour(hour);
}
pub fn handedness() -> Handedness {
    ADAPTIVE_UI.lock().user_handedness
}
pub fn process_signal(signal: &NeuralSignal) {
    ADAPTIVE_UI.lock().process_signal(signal);
}
