/// AI-powered display for Genesis
///
/// Adaptive UI, smart layout, gesture prediction,
/// content-aware brightness, eye comfort, attention detection.
///
/// Uses Q16 fixed-point math throughout (no floats).
///
/// Inspired by: Android Adaptive Display, iOS True Tone. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Q16 fixed-point constant: 1.0
const Q16_ONE: i32 = 65536;

/// Q16 multiply
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Q16 divide (a / b)
fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << 16) / b as i64) as i32
}

/// Q16 from integer
fn q16_from_int(x: i32) -> i32 {
    x << 16
}

/// Display adaptation mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayAdaptation {
    ContentAware,   // Adjust based on content type
    AmbientLight,   // Adjust based on environment
    AttentionBased, // Dim when user looks away
    TimeOfDay,      // Blue light filter schedule
    BatterySaver,   // Reduce brightness/refresh for power
}

/// Content type detected by AI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    Text,
    Photo,
    Video,
    Game,
    Map,
    Drawing,
    Code,
    DarkContent,
    BrightContent,
}

/// Gesture prediction — probability is Q16 (0..Q16_ONE)
pub struct GesturePrediction {
    pub gesture_type: PredictedGesture,
    pub probability: i32,                    // Q16: 0..Q16_ONE
    pub target_region: (i32, i32, i32, i32), // x, y, w, h
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredictedGesture {
    Tap,
    DoubleTap,
    LongPress,
    SwipeUp,
    SwipeDown,
    SwipeLeft,
    SwipeRight,
    Pinch,
    Spread,
    Scroll,
}

/// Eye comfort settings — all intensity values Q16
pub struct EyeComfort {
    pub blue_light_filter: i32, // Q16: 0..Q16_ONE
    pub color_temperature: u32, // Kelvin (2700-6500)
    pub brightness: i32,        // Q16: 0..Q16_ONE
    pub auto_enabled: bool,
    pub schedule_start: u8, // Hour (0-23)
    pub schedule_end: u8,   // Hour (0-23)
}

/// Layout suggestion from AI
pub struct LayoutSuggestion {
    pub widget_id: u32,
    pub suggested_x: i32,
    pub suggested_y: i32,
    pub suggested_w: u32,
    pub suggested_h: u32,
    pub reason: String,
}

/// AI display engine — all intensity/probability values are Q16
pub struct AiDisplayEngine {
    pub enabled: bool,
    pub active_adaptations: Vec<DisplayAdaptation>,
    pub eye_comfort: EyeComfort,
    pub current_content: ContentType,
    pub gesture_history: Vec<(PredictedGesture, u64)>,
    pub touch_heatmap: [[u16; 20]; 30], // 20x30 grid of touch counts
    pub auto_brightness: bool,
    pub auto_refresh_rate: bool,
    pub current_brightness: i32, // Q16: 0..Q16_ONE
    pub current_refresh_hz: u32,
    pub attention_detected: bool,
    pub usage_minutes_today: u32,
    pub break_reminder_enabled: bool,
    pub break_interval_min: u32,
    pub last_break: u64,
}

impl AiDisplayEngine {
    const fn new() -> Self {
        AiDisplayEngine {
            enabled: true,
            active_adaptations: Vec::new(),
            eye_comfort: EyeComfort {
                blue_light_filter: 0,
                color_temperature: 6500,
                brightness: 45875, // ~0.7 in Q16
                auto_enabled: true,
                schedule_start: 21,
                schedule_end: 7,
            },
            current_content: ContentType::Text,
            gesture_history: Vec::new(),
            touch_heatmap: [[0u16; 20]; 30],
            auto_brightness: true,
            auto_refresh_rate: true,
            current_brightness: 45875, // ~0.7 in Q16
            current_refresh_hz: 60,
            attention_detected: true,
            usage_minutes_today: 0,
            break_reminder_enabled: true,
            break_interval_min: 30,
            last_break: 0,
        }
    }

    /// Detect content type from pixel data characteristics
    /// Parameters are Q16 values (0..Q16_ONE)
    pub fn detect_content(
        &mut self,
        avg_brightness: i32,
        color_variance: i32,
        motion: i32,
    ) -> ContentType {
        let half = Q16_ONE / 2; // 0.5
        let high = (Q16_ONE * 7) / 10; // 0.7
        let low = Q16_ONE / 5; // 0.2
        let very_high = (Q16_ONE * 9) / 10; // 0.9
        let very_low = Q16_ONE / 10; // 0.1
        let mid_high = (Q16_ONE * 6) / 10; // 0.6

        self.current_content = if motion > half {
            if color_variance > high {
                ContentType::Game
            } else {
                ContentType::Video
            }
        } else if avg_brightness < low {
            ContentType::DarkContent
        } else if avg_brightness > very_high {
            ContentType::BrightContent
        } else if color_variance < very_low {
            ContentType::Text
        } else if color_variance > mid_high {
            ContentType::Photo
        } else {
            ContentType::Text
        };
        self.current_content
    }

    /// Get optimal display settings for current content
    /// Returns (brightness_q16, refresh_hz, blue_light_filter_q16)
    pub fn optimize_display(&self) -> (i32, u32, i32) {
        let now = crate::time::clock::unix_time();
        let hour = ((now / 3600) % 24) as u8;

        let blue_filter = if self.eye_comfort.auto_enabled {
            if hour >= self.eye_comfort.schedule_start || hour < self.eye_comfort.schedule_end {
                Q16_ONE / 2 // Night mode: 0.5
            } else {
                0
            }
        } else {
            self.eye_comfort.blue_light_filter
        };

        let (brightness, refresh) = match self.current_content {
            ContentType::Video => ((Q16_ONE * 8) / 10, 60), // 0.8
            ContentType::Game => ((Q16_ONE * 9) / 10, 120), // 0.9
            ContentType::Text | ContentType::Code => ((Q16_ONE * 6) / 10, 60), // 0.6
            ContentType::Photo => ((Q16_ONE * 85) / 100, 60), // 0.85
            ContentType::DarkContent => ((Q16_ONE * 4) / 10, 60), // 0.4
            ContentType::BrightContent => (Q16_ONE / 2, 60), // 0.5
            _ => ((Q16_ONE * 7) / 10, 60),                  // 0.7
        };

        (brightness, refresh, blue_filter)
    }

    /// Predict next gesture based on touch history
    pub fn predict_gesture(&self) -> Option<GesturePrediction> {
        if self.gesture_history.len() < 3 {
            return None;
        }

        // Simple frequency-based prediction
        let mut gesture_counts = [0u32; 10];
        for (gesture, _) in self.gesture_history.iter().rev().take(20) {
            let idx = *gesture as usize;
            if idx < 10 {
                gesture_counts[idx] += 1;
            }
        }

        let max_idx = gesture_counts
            .iter()
            .enumerate()
            .max_by_key(|(_, &count)| count)
            .map(|(idx, _)| idx)
            .unwrap_or(0);

        let total: u32 = gesture_counts.iter().sum();
        if total == 0 {
            return None;
        }

        // prob as Q16 = count / total * Q16_ONE
        let prob = ((gesture_counts[max_idx] as i64 * Q16_ONE as i64) / total as i64) as i32;
        // Threshold: 0.3 in Q16 = 19661
        if prob < 19661 {
            return None;
        }

        let gesture = match max_idx {
            0 => PredictedGesture::Tap,
            1 => PredictedGesture::DoubleTap,
            2 => PredictedGesture::LongPress,
            3 => PredictedGesture::SwipeUp,
            4 => PredictedGesture::SwipeDown,
            5 => PredictedGesture::SwipeLeft,
            6 => PredictedGesture::SwipeRight,
            7 => PredictedGesture::Pinch,
            8 => PredictedGesture::Spread,
            _ => PredictedGesture::Scroll,
        };

        Some(GesturePrediction {
            gesture_type: gesture,
            probability: prob,
            target_region: (0, 0, 0, 0),
        })
    }

    /// Record a touch event for heatmap and gesture learning
    pub fn record_touch(&mut self, x: i32, y: i32, gesture: PredictedGesture) {
        let gx = (x / 50).min(19).max(0) as usize;
        let gy = (y / 50).min(29).max(0) as usize;
        self.touch_heatmap[gy][gx] = self.touch_heatmap[gy][gx].saturating_add(1);

        let now = crate::time::clock::unix_time();
        self.gesture_history.push((gesture, now));
        if self.gesture_history.len() > 200 {
            self.gesture_history.remove(0);
        }
    }

    /// Check if break reminder should fire
    pub fn check_break_reminder(&mut self) -> bool {
        if !self.break_reminder_enabled {
            return false;
        }
        let now = crate::time::clock::unix_time();
        let elapsed_min = (now - self.last_break) / 60;
        if elapsed_min >= self.break_interval_min as u64 {
            self.last_break = now;
            true
        } else {
            false
        }
    }

    /// Get hottest touch regions for UI optimization
    pub fn get_touch_hotspots(&self) -> Vec<(usize, usize, u16)> {
        let mut hotspots = Vec::new();
        for y in 0..30 {
            for x in 0..20 {
                if self.touch_heatmap[y][x] > 10 {
                    hotspots.push((x, y, self.touch_heatmap[y][x]));
                }
            }
        }
        hotspots.sort_by(|a, b| b.2.cmp(&a.2));
        hotspots.truncate(10);
        hotspots
    }

    /// Get layout suggestions based on usage patterns
    pub fn suggest_layouts(&self) -> Vec<LayoutSuggestion> {
        let mut suggestions = Vec::new();

        // Find the hottest touch zone and suggest placing key controls there
        let hotspots = self.get_touch_hotspots();
        if let Some(&(hx, hy, _count)) = hotspots.first() {
            suggestions.push(LayoutSuggestion {
                widget_id: 0,
                suggested_x: (hx * 50) as i32,
                suggested_y: (hy * 50) as i32,
                suggested_w: 100,
                suggested_h: 40,
                reason: String::from("high-frequency touch zone"),
            });
        }

        suggestions
    }
}

static AI_DISPLAY: Mutex<AiDisplayEngine> = Mutex::new(AiDisplayEngine::new());

pub fn init() {
    crate::serial_println!(
        "    [ai-display] AI display intelligence initialized (adaptive, eye comfort, gestures)"
    );
}

/// Detect content type — parameters are Q16 (0..Q16_ONE)
pub fn detect_content(brightness: i32, variance: i32, motion: i32) -> ContentType {
    AI_DISPLAY
        .lock()
        .detect_content(brightness, variance, motion)
}

/// Optimize display settings — returns (brightness_q16, refresh_hz, blue_filter_q16)
pub fn optimize_display() -> (i32, u32, i32) {
    AI_DISPLAY.lock().optimize_display()
}

pub fn record_touch(x: i32, y: i32, gesture: PredictedGesture) {
    AI_DISPLAY.lock().record_touch(x, y, gesture);
}

pub fn check_break() -> bool {
    AI_DISPLAY.lock().check_break_reminder()
}

pub fn predict_gesture() -> Option<GesturePrediction> {
    AI_DISPLAY.lock().predict_gesture()
}

pub fn suggest_layouts() -> Vec<LayoutSuggestion> {
    AI_DISPLAY.lock().suggest_layouts()
}
