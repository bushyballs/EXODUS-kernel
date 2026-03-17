// Emotion-Aware Computing Engine for Neural Bus
// Part of HoagsOS Genesis - bare-metal Rust kernel
// Detects and responds to user emotional states via behavioral signals

use super::*;
use crate::{serial_print, serial_println};
use alloc::vec::Vec;
use alloc::string::String;
use alloc::collections::BTreeMap;

// Fixed-point math constants
const Q16_ONE: i32 = 65536;
const Q16_HALF: i32 = 32768;

/// 10 core emotional states mapped to behavioral patterns
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmotionState {
    Calm,
    Focused,
    Stressed,
    Frustrated,
    Happy,
    Bored,
    Tired,
    Energized,
    Anxious,
    Neutral,
}

impl EmotionState {
    fn as_index(&self) -> usize {
        match self {
            EmotionState::Calm => 0,
            EmotionState::Focused => 1,
            EmotionState::Stressed => 2,
            EmotionState::Frustrated => 3,
            EmotionState::Happy => 4,
            EmotionState::Bored => 5,
            EmotionState::Tired => 6,
            EmotionState::Energized => 7,
            EmotionState::Anxious => 8,
            EmotionState::Neutral => 9,
        }
    }
}

/// 10-dimensional emotion vector using Q16 fixed-point math
#[derive(Clone, Copy, Debug)]
pub struct EmotionVector {
    // [Calm, Focused, Stressed, Frustrated, Happy, Bored, Tired, Energized, Anxious, Neutral]
    dimensions: [i32; 10],
}

impl EmotionVector {
    /// Create neutral emotion vector (all zero except neutral)
    pub fn neutral() -> Self {
        let mut dimensions = [0i32; 10];
        dimensions[9] = Q16_ONE; // neutral = 100%
        EmotionVector { dimensions }
    }

    /// Create emotion vector with single dominant state
    pub fn single(state: EmotionState) -> Self {
        let mut dimensions = [0i32; 10];
        dimensions[state.as_index()] = Q16_ONE;
        EmotionVector { dimensions }
    }

    /// Blend two emotion vectors with interpolation factor [0..Q16_ONE]
    pub fn blend(&self, other: &EmotionVector, factor: i32) -> Self {
        let mut result = [0i32; 10];
        let inv_factor = Q16_ONE - factor;

        for i in 0..10 {
            result[i] = ((self.dimensions[i] as i64 * inv_factor as i64)
                + (other.dimensions[i] as i64 * factor as i64))
                / Q16_ONE as i64;
            result[i] = result[i].max(0).min(Q16_ONE);
        }

        EmotionVector {
            dimensions: result,
        }
    }

    /// Get intensity of specific emotion [0..Q16_ONE]
    pub fn intensity(&self, state: EmotionState) -> i32 {
        self.dimensions[state.as_index()]
    }

    /// Get dominant emotion state
    pub fn dominant(&self) -> EmotionState {
        let mut max_intensity = 0i32;
        let mut dominant_idx = 9;

        for i in 0..10 {
            if self.dimensions[i] > max_intensity {
                max_intensity = self.dimensions[i];
                dominant_idx = i;
            }
        }

        match dominant_idx {
            0 => EmotionState::Calm,
            1 => EmotionState::Focused,
            2 => EmotionState::Stressed,
            3 => EmotionState::Frustrated,
            4 => EmotionState::Happy,
            5 => EmotionState::Bored,
            6 => EmotionState::Tired,
            7 => EmotionState::Energized,
            8 => EmotionState::Anxious,
            _ => EmotionState::Neutral,
        }
    }

    /// Normalize vector to sum to Q16_ONE
    pub fn normalize(&mut self) {
        let mut sum = 0i64;
        for i in 0..10 {
            sum += self.dimensions[i] as i64;
        }

        if sum > 0 {
            for i in 0..10 {
                self.dimensions[i] = ((self.dimensions[i] as i64 * Q16_ONE as i64) / sum) as i32;
            }
        }
    }
}

/// Behavioral stress indicators
#[derive(Clone, Debug)]
struct StressIndicators {
    typing_speed_samples: Vec<i32>, // chars per second in Q16
    error_count: u32,               // backspace/delete count
    app_switch_count: u32,          // rapid app switches
    last_typing_time: u64,
    sample_window: usize,
}

impl StressIndicators {
    fn new() -> Self {
        StressIndicators {
            typing_speed_samples: Vec::new(),
            error_count: 0,
            app_switch_count: 0,
            last_typing_time: 0,
            sample_window: 20,
        }
    }

    fn record_typing(&mut self, speed: i32) {
        if self.typing_speed_samples.len() >= self.sample_window {
            self.typing_speed_samples.remove(0);
        }
        self.typing_speed_samples.push(speed);
    }

    fn record_error(&mut self) {
        self.error_count = self.error_count.saturating_add(1);
    }

    fn record_app_switch(&mut self) {
        self.app_switch_count = self.app_switch_count.saturating_add(1);
    }

    /// Calculate typing speed variance (high variance = stressed)
    fn typing_variance(&self) -> i32 {
        if self.typing_speed_samples.len() < 2 {
            return 0;
        }

        let mean: i64 = self.typing_speed_samples.iter().map(|&x| x as i64).sum::<i64>()
            / self.typing_speed_samples.len() as i64;

        let variance: i64 = self.typing_speed_samples
            .iter()
            .map(|&x| {
                let diff = x as i64 - mean;
                diff * diff
            })
            .sum::<i64>()
            / self.typing_speed_samples.len() as i64;

        (variance.min(Q16_ONE as i64 * Q16_ONE as i64) as i32).max(0)
    }

    /// Normalized error rate [0..Q16_ONE]
    fn error_rate(&self) -> i32 {
        let typing_events = self.typing_speed_samples.len() as u32 + self.error_count;
        if typing_events == 0 {
            return 0;
        }
        ((self.error_count as i64 * Q16_ONE as i64) / typing_events as i64) as i32
    }

    /// Normalized app switch frequency [0..Q16_ONE]
    fn switch_frequency(&self) -> i32 {
        (self.app_switch_count as i64 * Q16_ONE as i64 / 100i64) as i32
            | 0
            .max(self.app_switch_count as i32)
            .min(Q16_ONE)
    }
}

/// Frustration detection from repeated failed actions
#[derive(Clone, Debug)]
struct FrustrationDetector {
    same_app_close_time: u64,
    same_app_name: Option<String>,
    failed_searches: u32,
    rapid_clicks_area: Option<(u32, u32)>, // (x, y)
    rapid_click_count: u32,
}

impl FrustrationDetector {
    fn new() -> Self {
        FrustrationDetector {
            same_app_close_time: 0,
            same_app_name: None,
            failed_searches: 0,
            rapid_clicks_area: None,
            rapid_click_count: 0,
        }
    }

    fn record_app_close(&mut self, app_name: String, current_time: u64) {
        self.same_app_close_time = current_time;
        self.same_app_name = Some(app_name);
    }

    fn record_app_reopen(&mut self, app_name: &str, current_time: u64) -> bool {
        // Check if same app reopened within 30 seconds
        if let Some(ref closed_app) = self.same_app_name {
            if closed_app == app_name && current_time.saturating_sub(self.same_app_close_time) < 30 {
                self.same_app_name = None;
                return true; // Frustration detected
            }
        }
        false
    }

    fn record_failed_search(&mut self) {
        self.failed_searches = self.failed_searches.saturating_add(1);
    }

    fn record_rapid_click(&mut self, x: u32, y: u32) {
        match self.rapid_clicks_area {
            Some((px, py)) if px == x && py == y => {
                self.rapid_click_count = self.rapid_click_count.saturating_add(1);
            }
            _ => {
                self.rapid_clicks_area = Some((x, y));
                self.rapid_click_count = 1;
            }
        }
    }

    /// Get frustration level [0..Q16_ONE]
    fn frustration_level(&self) -> i32 {
        let mut level = 0i64;

        // Weight rapid clicks (5+ in same spot = high frustration)
        if self.rapid_click_count > 5 {
            level += ((self.rapid_click_count as i64 - 5) * Q16_ONE as i64 / 10) as i64;
        }

        // Weight repeated failed searches
        level += (self.failed_searches.min(5) as i64 * Q16_ONE as i64 / 5) as i64;

        (level.min(Q16_ONE as i64) as i32).max(0)
    }
}

/// Focus tracking from sustained single-app usage
#[derive(Clone, Debug)]
struct FocusTracker {
    current_app: Option<String>,
    app_start_time: u64,
    touch_gesture_count: u32,
    sustained_threshold: u64, // 5 minutes = 300 seconds
}

impl FocusTracker {
    fn new() -> Self {
        FocusTracker {
            current_app: None,
            app_start_time: 0,
            touch_gesture_count: 0,
            sustained_threshold: 300,
        }
    }

    fn set_current_app(&mut self, app: String, current_time: u64) {
        if let Some(ref prev) = self.current_app {
            if prev != &app {
                self.touch_gesture_count = 0; // Reset on app switch
            }
        }
        self.current_app = Some(app);
        self.app_start_time = current_time;
    }

    fn record_gesture(&mut self) {
        self.touch_gesture_count = self.touch_gesture_count.saturating_add(1);
    }

    /// Get focus level [0..Q16_ONE]
    fn focus_level(&self, current_time: u64) -> i32 {
        if self.current_app.is_none() {
            return 0;
        }

        let sustained_time = current_time.saturating_sub(self.app_start_time);
        if sustained_time >= self.sustained_threshold {
            // Award points for sustained usage, penalize for high gesture count
            let sustained_score =
                (sustained_time as i64 * Q16_ONE as i64 / (self.sustained_threshold as i64 * 2));
            let gesture_penalty = (self.touch_gesture_count as i64 * Q16_ONE as i64 / 50);
            ((sustained_score - gesture_penalty).max(0).min(Q16_ONE as i64) as i32).max(0)
        } else {
            0
        }
    }
}

/// Energy estimator based on time of day and break patterns
#[derive(Clone, Debug)]
struct EnergyEstimator {
    break_times: Vec<u64>,
    typing_vigor: Vec<i32>, // Key press force/speed
    last_break: u64,
}

impl EnergyEstimator {
    fn new() -> Self {
        EnergyEstimator {
            break_times: Vec::new(),
            typing_vigor: Vec::new(),
            last_break: 0,
        }
    }

    fn record_break(&mut self, current_time: u64) {
        self.break_times.push(current_time);
        self.last_break = current_time;
        if self.break_times.len() > 10 {
            self.break_times.remove(0);
        }
    }

    fn record_typing_vigor(&mut self, vigor: i32) {
        self.typing_vigor.push(vigor);
        if self.typing_vigor.len() > 20 {
            self.typing_vigor.remove(0);
        }
    }

    /// Estimate energy level [0..Q16_ONE] based on breaks and vigor
    fn energy_level(&self, current_time: u64) -> i32 {
        let mut energy = Q16_HALF; // Start at 50%

        // Penalize time since last break (every 30 minutes, lose 25%)
        let time_since_break = current_time.saturating_sub(self.last_break);
        if time_since_break > 1800 {
            energy = (energy as i64 - (Q16_ONE as i64 / 4)).max(0) as i32;
        }

        // Factor in average typing vigor
        if !self.typing_vigor.is_empty() {
            let avg_vigor: i32 =
                self.typing_vigor.iter().sum::<i32>() / self.typing_vigor.len() as i32;
            energy = ((energy as i64 + avg_vigor as i64) / 2).min(Q16_ONE as i64) as i32;
        }

        energy
    }
}

/// Recommended adaptive responses based on detected emotion
#[derive(Clone, Debug)]
pub struct EmotionResponse {
    pub recommended_brightness: i32,     // Q16, [0..Q16_ONE]
    pub notification_level: i32,         // Q16, [0..Q16_ONE] (0=silent, 1=max)
    pub ui_complexity: i32,              // Q16, [0..Q16_ONE] (0=minimal, 1=full)
    pub suggested_action: String,
    pub dnd_enabled: bool,
}

impl EmotionResponse {
    fn from_emotion(emotion: &EmotionVector) -> Self {
        let dominant = emotion.dominant();

        match dominant {
            EmotionState::Calm => EmotionResponse {
                recommended_brightness: (Q16_ONE as i64 * 80 / 100) as i32,
                notification_level: (Q16_ONE as i64 * 50 / 100) as i32,
                ui_complexity: (Q16_ONE as i64 * 75 / 100) as i32,
                suggested_action: "Maintain current pace".into(),
                dnd_enabled: false,
            },
            EmotionState::Stressed => EmotionResponse {
                recommended_brightness: (Q16_ONE as i64 * 60 / 100) as i32,
                notification_level: (Q16_ONE as i64 * 20 / 100) as i32,
                ui_complexity: (Q16_ONE as i64 * 30 / 100) as i32,
                suggested_action: "Simplify UI. Take a 5-minute break.".into(),
                dnd_enabled: true,
            },
            EmotionState::Frustrated => EmotionResponse {
                recommended_brightness: (Q16_ONE as i64 * 70 / 100) as i32,
                notification_level: (Q16_ONE as i64 * 30 / 100) as i32,
                ui_complexity: (Q16_ONE as i64 * 50 / 100) as i32,
                suggested_action: "Enlarge buttons. Offer help tooltip.".into(),
                dnd_enabled: true,
            },
            EmotionState::Focused => EmotionResponse {
                recommended_brightness: (Q16_ONE as i64 * 75 / 100) as i32,
                notification_level: (Q16_ONE as i64 * 10 / 100) as i32,
                ui_complexity: (Q16_ONE as i64 * 60 / 100) as i32,
                suggested_action: "Minimize distractions. Full DND.".into(),
                dnd_enabled: true,
            },
            EmotionState::Happy => EmotionResponse {
                recommended_brightness: (Q16_ONE as i64 * 100 / 100) as i32,
                notification_level: (Q16_ONE as i64 * 80 / 100) as i32,
                ui_complexity: (Q16_ONE as i64 * 100 / 100) as i32,
                suggested_action: "Enable vibrant colors and animations.".into(),
                dnd_enabled: false,
            },
            EmotionState::Energized => EmotionResponse {
                recommended_brightness: (Q16_ONE as i64 * 95 / 100) as i32,
                notification_level: (Q16_ONE as i64 * 70 / 100) as i32,
                ui_complexity: (Q16_ONE as i64 * 90 / 100) as i32,
                suggested_action: "Full feature set. Encourage productivity.".into(),
                dnd_enabled: false,
            },
            EmotionState::Tired => EmotionResponse {
                recommended_brightness: (Q16_ONE as i64 * 85 / 100) as i32,
                notification_level: (Q16_ONE as i64 * 40 / 100) as i32,
                ui_complexity: (Q16_ONE as i64 * 40 / 100) as i32,
                suggested_action: "Increase font. Suggest rest.".into(),
                dnd_enabled: true,
            },
            EmotionState::Bored => EmotionResponse {
                recommended_brightness: (Q16_ONE as i64 * 90 / 100) as i32,
                notification_level: (Q16_ONE as i64 * 60 / 100) as i32,
                ui_complexity: (Q16_ONE as i64 * 80 / 100) as i32,
                suggested_action: "Suggest new features or switch tasks.".into(),
                dnd_enabled: false,
            },
            EmotionState::Anxious => EmotionResponse {
                recommended_brightness: (Q16_ONE as i64 * 65 / 100) as i32,
                notification_level: (Q16_ONE as i64 * 25 / 100) as i32,
                ui_complexity: (Q16_ONE as i64 * 45 / 100) as i32,
                suggested_action: "Simplify choices. Offer reassurance.".into(),
                dnd_enabled: true,
            },
            EmotionState::Neutral => EmotionResponse {
                recommended_brightness: (Q16_ONE as i64 * 75 / 100) as i32,
                notification_level: (Q16_ONE as i64 * 50 / 100) as i32,
                ui_complexity: (Q16_ONE as i64 * 70 / 100) as i32,
                suggested_action: "Standard operation.".into(),
                dnd_enabled: false,
            },
        }
    }
}

/// Main Emotion-Aware Computing Engine
pub struct EmotionEngine {
    current_emotion: EmotionVector,
    emotion_history: Vec<EmotionVector>,
    stress_indicators: StressIndicators,
    frustration_detector: FrustrationDetector,
    focus_tracker: FocusTracker,
    energy_estimator: EnergyEstimator,
    max_history: usize,
}

impl EmotionEngine {
    /// Initialize emotion engine
    pub fn new() -> Self {
        serial_println!("[EMOTION] Initializing Emotion Engine");
        EmotionEngine {
            current_emotion: EmotionVector::neutral(),
            emotion_history: Vec::new(),
            stress_indicators: StressIndicators::new(),
            frustration_detector: FrustrationDetector::new(),
            focus_tracker: FocusTracker::new(),
            energy_estimator: EnergyEstimator::new(),
            max_history: 100,
        }
    }

    /// Process behavioral signals and update emotion state
    pub fn tick(&mut self, current_time: u64) {
        // Calculate stress level
        let typing_var = self.stress_indicators.typing_variance();
        let error_rate = self.stress_indicators.error_rate();
        let switch_freq = self.stress_indicators.switch_frequency();

        let stress_level = ((typing_var as i64
            + error_rate as i64
            + switch_freq as i64 * 2)
            / 4)
            .min(Q16_ONE as i64) as i32;

        // Calculate frustration level
        let frustration_level = self.frustration_detector.frustration_level();

        // Calculate focus level
        let focus_level = self.focus_tracker.focus_level(current_time);

        // Calculate energy level
        let energy_level = self.energy_estimator.energy_level(current_time);

        // Build new emotion vector
        let mut new_emotion = EmotionVector::neutral();
        new_emotion.dimensions[2] = stress_level; // Stressed
        new_emotion.dimensions[3] = frustration_level; // Frustrated
        new_emotion.dimensions[1] = focus_level; // Focused
        new_emotion.dimensions[7] = energy_level; // Energized

        // Blend with current emotion (smooth transitions)
        self.current_emotion = self.current_emotion.blend(&new_emotion, Q16_ONE / 4);
        self.current_emotion.normalize();

        // Record history
        if self.emotion_history.len() >= self.max_history {
            self.emotion_history.remove(0);
        }
        self.emotion_history.push(self.current_emotion);

        serial_println!(
            "[EMOTION] Dominant: {:?}, Stress: {}, Focus: {}",
            self.current_emotion.dominant(),
            stress_level,
            focus_level
        );
    }

    /// Record typing event
    pub fn record_typing(&mut self, speed: i32) {
        self.stress_indicators.record_typing(speed);
    }

    /// Record typing error
    pub fn record_error(&mut self) {
        self.stress_indicators.record_error();
    }

    /// Record app switch
    pub fn record_app_switch(&mut self, app_name: String, current_time: u64) {
        self.stress_indicators.record_app_switch();
        self.focus_tracker.set_current_app(app_name, current_time);
    }

    /// Record app close
    pub fn record_app_close(&mut self, app_name: String, current_time: u64) {
        self.frustration_detector
            .record_app_close(app_name, current_time);
    }

    /// Record app reopen (detects frustration pattern)
    pub fn record_app_reopen(&mut self, app_name: &str, current_time: u64) {
        if self
            .frustration_detector
            .record_app_reopen(app_name, current_time)
        {
            serial_println!("[EMOTION] Frustration: App reopen detected");
        }
    }

    /// Record failed search
    pub fn record_failed_search(&mut self) {
        self.frustration_detector.record_failed_search();
    }

    /// Record rapid click (potential frustration indicator)
    pub fn record_rapid_click(&mut self, x: u32, y: u32) {
        self.frustration_detector.record_rapid_click(x, y);
    }

    /// Record touch/gesture
    pub fn record_gesture(&mut self) {
        self.focus_tracker.record_gesture();
    }

    /// Record break taken
    pub fn record_break(&mut self, current_time: u64) {
        self.energy_estimator.record_break(current_time);
        serial_println!("[EMOTION] Break recorded");
    }

    /// Get current emotion vector
    pub fn current_emotion(&self) -> &EmotionVector {
        &self.current_emotion
    }

    /// Get dominant emotion state
    pub fn dominant_state(&self) -> EmotionState {
        self.current_emotion.dominant()
    }

    /// Get adaptive response for current emotion
    pub fn respond(&self) -> EmotionResponse {
        EmotionResponse::from_emotion(&self.current_emotion)
    }

    /// Get emotion history (recent states)
    pub fn history(&self) -> &[EmotionVector] {
        &self.emotion_history
    }
}

/// Global emotion engine instance
static mut EMOTION_ENGINE: Option<Mutex<EmotionEngine>> = None;

/// Initialize global emotion engine
pub fn init() {
    unsafe {
        EMOTION_ENGINE = Some(Mutex::new(EmotionEngine::new()));
    }
    serial_println!("[EMOTION] Global engine initialized");
}

/// Tick the global emotion engine
pub fn tick(current_time: u64) {
    if let Some(ref engine) = unsafe { &EMOTION_ENGINE } {
        if let Ok(mut e) = engine.lock() {
            e.tick(current_time);
        }
    }
}

/// Get current dominant emotion state
pub fn current_emotion() -> EmotionState {
    if let Some(ref engine) = unsafe { &EMOTION_ENGINE } {
        if let Ok(e) = engine.lock() {
            return e.dominant_state();
        }
    }
    EmotionState::Neutral
}

/// Get adaptive response for current emotion
pub fn respond() -> EmotionResponse {
    if let Some(ref engine) = unsafe { &EMOTION_ENGINE } {
        if let Ok(e) = engine.lock() {
            return e.respond();
        }
    }
    EmotionResponse::from_emotion(&EmotionVector::neutral())
}

/// Record typing event
pub fn record_typing(speed: i32) {
    if let Some(ref engine) = unsafe { &EMOTION_ENGINE } {
        if let Ok(mut e) = engine.lock() {
            e.record_typing(speed);
        }
    }
}

/// Record typing error
pub fn record_error() {
    if let Some(ref engine) = unsafe { &EMOTION_ENGINE } {
        if let Ok(mut e) = engine.lock() {
            e.record_error();
        }
    }
}

/// Record app switch
pub fn record_app_switch(app_name: String, current_time: u64) {
    if let Some(ref engine) = unsafe { &EMOTION_ENGINE } {
        if let Ok(mut e) = engine.lock() {
            e.record_app_switch(app_name, current_time);
        }
    }
}

/// Record app close
pub fn record_app_close(app_name: String, current_time: u64) {
    if let Some(ref engine) = unsafe { &EMOTION_ENGINE } {
        if let Ok(mut e) = engine.lock() {
            e.record_app_close(app_name, current_time);
        }
    }
}

/// Record app reopen
pub fn record_app_reopen(app_name: &str, current_time: u64) {
    if let Some(ref engine) = unsafe { &EMOTION_ENGINE } {
        if let Ok(mut e) = engine.lock() {
            e.record_app_reopen(app_name, current_time);
        }
    }
}

/// Record failed search
pub fn record_failed_search() {
    if let Some(ref engine) = unsafe { &EMOTION_ENGINE } {
        if let Ok(mut e) = engine.lock() {
            e.record_failed_search();
        }
    }
}

/// Record rapid click
pub fn record_rapid_click(x: u32, y: u32) {
    if let Some(ref engine) = unsafe { &EMOTION_ENGINE } {
        if let Ok(mut e) = engine.lock() {
            e.record_rapid_click(x, y);
        }
    }
}

/// Record gesture
pub fn record_gesture() {
    if let Some(ref engine) = unsafe { &EMOTION_ENGINE } {
        if let Ok(mut e) = engine.lock() {
            e.record_gesture();
        }
    }
}

/// Record break
pub fn record_break(current_time: u64) {
    if let Some(ref engine) = unsafe { &EMOTION_ENGINE } {
        if let Ok(mut e) = engine.lock() {
            e.record_break(current_time);
        }
    }
}
