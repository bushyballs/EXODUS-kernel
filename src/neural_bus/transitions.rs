use super::*;
use crate::{serial_print, serial_println};
use alloc::vec::Vec;
use alloc::string::String;
use alloc::collections::BTreeMap;

/// Transition animation types for app switches
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TransitionType {
    SlideLeft,
    SlideRight,
    SlideUp,
    SlideDown,
    Fade,
    CrossFade,
    Zoom,
    ZoomOut,
    Morph,
    Ripple,
    Dissolve,
    Flip,
    Rotate,
    Elastic,
    Bounce,
    Custom(u32),
}

/// Single frame in a transition sequence
#[derive(Clone, Copy, Debug)]
pub struct TransitionFrame {
    /// Progress from 0 (start) to Q16_ONE (complete)
    pub progress: Q16,
    /// Opacity of source app (Q16_ONE = fully opaque)
    pub opacity_from: Q16,
    /// Opacity of dest app (Q16_ONE = fully opaque)
    pub opacity_to: Q16,
    /// Horizontal offset in pixels
    pub offset_x: i32,
    /// Vertical offset in pixels
    pub offset_y: i32,
    /// Scale factor (Q16_ONE = normal size)
    pub scale: Q16,
    /// Rotation in degrees * Q16
    pub rotation: Q16,
}

/// Easing curve for smooth animation interpolation
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EasingFunction {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    Spring,
    Bounce,
    Elastic,
    BackIn,
    BackOut,
    Anticipate,
}

/// Active transition state
#[derive(Clone, Debug)]
pub struct ActiveTransition {
    pub from_app: String,
    pub to_app: String,
    pub transition_type: TransitionType,
    pub easing: EasingFunction,
    pub start_time: u64,
    pub duration_ms: u32,
    pub current_frame: u32,
    pub total_frames: u32,
    pub frames: Vec<TransitionFrame>,
}

/// Core transition engine with learning and AI selection
pub struct TransitionEngine {
    active_transition: Option<ActiveTransition>,
    transition_history: BTreeMap<(String, String), TransitionType>,
    easing_preference: EasingFunction,
    speed_multiplier: Q16,
    pre_rendered_frames: Vec<TransitionFrame>,
    total_transitions: u64,
    avg_duration_ms: u32,
}

impl TransitionEngine {
    /// Initialize a new transition engine
    pub fn new() -> Self {
        TransitionEngine {
            active_transition: None,
            transition_history: BTreeMap::new(),
            easing_preference: EasingFunction::EaseInOut,
            speed_multiplier: Q16_ONE,
            pre_rendered_frames: Vec::new(),
            total_transitions: 0,
            avg_duration_ms: 300,
        }
    }

    /// Apply easing function to normalized progress (0.0 to 1.0)
    fn apply_easing(easing: EasingFunction, t: Q16) -> Q16 {
        // Clamp t to [0, Q16_ONE]
        let t = if t < Q16(0) { Q16(0) } else if t > Q16_ONE { Q16_ONE } else { t };
        let t_f = q16_to_f32(t); // normalize to [0.0, 1.0]

        let eased = match easing {
            EasingFunction::Linear => t_f,
            EasingFunction::EaseIn => t_f * t_f,
            EasingFunction::EaseOut => t_f * (2.0 - t_f),
            EasingFunction::EaseInOut => {
                if t_f < 0.5 {
                    2.0 * t_f * t_f
                } else {
                    -1.0 + (4.0 - 2.0 * t_f) * t_f
                }
            }
            EasingFunction::Spring => {
                let pi = 3.14159265;
                1.0 + (-6.0 * t_f).exp() * (4.0 * pi * t_f).cos()
            }
            EasingFunction::Bounce => {
                if t_f < 0.5 {
                    8.0 * t_f * t_f * t_f * t_f
                } else {
                    let f = t_f - 1.0;
                    1.0 + 8.0 * f * f * f * f
                }
            }
            EasingFunction::Elastic => {
                let pi = 3.14159265;
                if t_f == 0.0 || t_f == 1.0 {
                    t_f
                } else {
                    -(2.0_f32.powf(10.0 * (t_f - 1.0))) * ((t_f - 1.1) * 5.0 * pi).sin()
                }
            }
            EasingFunction::BackIn => {
                let c1 = 1.70158;
                let c3 = c1 + 1.0;
                c3 * t_f * t_f * t_f - c1 * t_f * t_f
            }
            EasingFunction::BackOut => {
                let c1 = 1.70158;
                let c3 = c1 + 1.0;
                1.0 + c3 * (t_f - 1.0).powi(3) + c1 * (t_f - 1.0).powi(2)
            }
            EasingFunction::Anticipate => {
                let c1 = 1.70158;
                let c2 = c1 * 1.525;
                (c2 + 1.0) * t_f * t_f * t_f - c2 * t_f * t_f
            }
        };

        f32_to_q16(eased.max(0.0).min(1.0))
    }

    /// Generate animation frames for a transition
    pub fn generate_frames(
        &mut self,
        transition_type: TransitionType,
        easing: EasingFunction,
        num_frames: u32,
    ) -> Vec<TransitionFrame> {
        let mut frames = Vec::new();
        let frame_count = num_frames.max(2);

        for i in 0..=frame_count {
            let progress_raw = Q16::from(i as i32) * Q16_ONE / Q16::from(frame_count as i32);
            let progress = Self::apply_easing(easing, progress_raw);

            let frame = match transition_type {
                TransitionType::SlideLeft => TransitionFrame {
                    progress,
                    opacity_from: Q16_ONE,
                    opacity_to: Q16_ONE,
                    offset_x: (-320 * i as i32) / frame_count as i32,
                    offset_y: 0,
                    scale: Q16_ONE,
                    rotation: Q16(0),
                },
                TransitionType::SlideRight => TransitionFrame {
                    progress,
                    opacity_from: Q16_ONE,
                    opacity_to: Q16_ONE,
                    offset_x: (320 * i as i32) / frame_count as i32,
                    offset_y: 0,
                    scale: Q16_ONE,
                    rotation: Q16(0),
                },
                TransitionType::SlideUp => TransitionFrame {
                    progress,
                    opacity_from: Q16_ONE,
                    opacity_to: Q16_ONE,
                    offset_x: 0,
                    offset_y: (-240 * i as i32) / frame_count as i32,
                    scale: Q16_ONE,
                    rotation: Q16(0),
                },
                TransitionType::SlideDown => TransitionFrame {
                    progress,
                    opacity_from: Q16_ONE,
                    opacity_to: Q16_ONE,
                    offset_x: 0,
                    offset_y: (240 * i as i32) / frame_count as i32,
                    scale: Q16_ONE,
                    rotation: Q16(0),
                },
                TransitionType::Fade => TransitionFrame {
                    progress,
                    opacity_from: Q16_ONE - progress,
                    opacity_to: progress,
                    offset_x: 0,
                    offset_y: 0,
                    scale: Q16_ONE,
                    rotation: Q16(0),
                },
                TransitionType::CrossFade => TransitionFrame {
                    progress,
                    opacity_from: (Q16_ONE * (frame_count as i32 - i as i32)) / frame_count as i32,
                    opacity_to: progress,
                    offset_x: 0,
                    offset_y: 0,
                    scale: Q16_ONE,
                    rotation: Q16(0),
                },
                TransitionType::Zoom => TransitionFrame {
                    progress,
                    opacity_from: Q16_ONE - (progress >> 2),
                    opacity_to: progress,
                    offset_x: 0,
                    offset_y: 0,
                    scale: Q16_ONE + (progress >> 2),
                    rotation: Q16(0),
                },
                TransitionType::ZoomOut => TransitionFrame {
                    progress,
                    opacity_from: Q16_ONE - progress,
                    opacity_to: progress,
                    offset_x: 0,
                    offset_y: 0,
                    scale: Q16_ONE - (progress >> 3),
                    rotation: Q16(0),
                },
                TransitionType::Flip => TransitionFrame {
                    progress,
                    opacity_from: Q16_ONE - (progress >> 1),
                    opacity_to: progress,
                    offset_x: 0,
                    offset_y: 0,
                    scale: Q16_ONE,
                    rotation: (180 * Q16_ONE * i as i32) / frame_count as i32,
                },
                TransitionType::Rotate => TransitionFrame {
                    progress,
                    opacity_from: Q16_ONE,
                    opacity_to: Q16_ONE,
                    offset_x: 0,
                    offset_y: 0,
                    scale: Q16_ONE,
                    rotation: (360 * Q16_ONE * i as i32) / frame_count as i32,
                },
                TransitionType::Elastic => {
                    let elastic = Self::apply_easing(EasingFunction::Elastic, progress);
                    TransitionFrame {
                        progress,
                        opacity_from: Q16_ONE - elastic,
                        opacity_to: elastic,
                        offset_x: ((100 * (frame_count as i32 - i as i32)) / frame_count as i32),
                        offset_y: 0,
                        scale: Q16_ONE,
                        rotation: Q16(0),
                    }
                }
                TransitionType::Bounce => {
                    let bouncy = Self::apply_easing(EasingFunction::Bounce, progress);
                    TransitionFrame {
                        progress,
                        opacity_from: Q16_ONE - bouncy,
                        opacity_to: bouncy,
                        offset_x: 0,
                        offset_y: ((50 * (frame_count as i32 - i as i32)) / frame_count as i32),
                        scale: Q16_ONE,
                        rotation: Q16(0),
                    }
                }
                TransitionType::Dissolve => TransitionFrame {
                    progress,
                    opacity_from: Q16_ONE - (progress >> 1),
                    opacity_to: progress,
                    offset_x: if i % 2 == 0 { 2 } else { -2 },
                    offset_y: if i % 3 == 0 { 1 } else { -1 },
                    scale: Q16_ONE,
                    rotation: Q16(0),
                },
                TransitionType::Morph => TransitionFrame {
                    progress,
                    opacity_from: Q16_ONE - (progress >> 2),
                    opacity_to: progress,
                    offset_x: (160 * i as i32) / frame_count as i32,
                    offset_y: (120 * i as i32) / frame_count as i32,
                    scale: Q16_ONE + (progress >> 3),
                    rotation: (45 * Q16_ONE * i as i32) / frame_count as i32,
                },
                TransitionType::Ripple => TransitionFrame {
                    progress,
                    opacity_from: Q16_ONE - progress,
                    opacity_to: progress,
                    offset_x: ((10 * (i as i32)).wrapping_mul(i as i32)) / (frame_count as i32),
                    offset_y: 0,
                    scale: Q16_ONE + (progress >> 4),
                    rotation: Q16(0),
                },
                TransitionType::Custom(_id) => TransitionFrame {
                    progress,
                    opacity_from: Q16_ONE - progress,
                    opacity_to: progress,
                    offset_x: 0,
                    offset_y: 0,
                    scale: Q16_ONE,
                    rotation: Q16(0),
                },
            };

            frames.push(frame);
        }

        frames
    }

    /// Select an appropriate transition based on context
    fn select_transition(
        &self,
        from_app: &str,
        to_app: &str,
        _time_of_day_hour: u32,
        _is_focused_mode: bool,
        _is_gaming_mode: bool,
    ) -> TransitionType {
        let key = (String::from(from_app), String::from(to_app));

        if let Some(learned) = self.transition_history.get(&key) {
            return *learned;
        }

        // AI heuristic: infer app relationship
        let is_related = (from_app.contains("term") && to_app.contains("edit"))
            || (from_app.contains("brain") && to_app.contains("mem"))
            || (from_app.contains("chat") && to_app.contains("note"));

        match (is_related, self.speed_multiplier > Q16_ONE) {
            (true, true) => TransitionType::Fade,
            (true, false) => TransitionType::SlideLeft,
            (false, true) => TransitionType::Zoom,
            (false, false) => TransitionType::CrossFade,
        }
    }

    /// Start a new transition
    pub fn start_transition(
        &mut self,
        from_app: &str,
        to_app: &str,
        time_of_day_hour: u32,
        is_focused_mode: bool,
        is_gaming_mode: bool,
        current_time_ms: u64,
    ) {
        let transition_type = self.select_transition(
            from_app,
            to_app,
            time_of_day_hour,
            is_focused_mode,
            is_gaming_mode,
        );

        let easing = if is_focused_mode {
            EasingFunction::Linear
        } else {
            self.easing_preference
        };

        let base_duration = if is_gaming_mode { 200 } else { 300 };
        let duration_ms = ((Q16::from(base_duration as i32) * self.speed_multiplier) >> 16).0 as u32;
        let num_frames = (duration_ms / 16).max(4);

        let frames = self.generate_frames(transition_type, easing, num_frames);

        self.active_transition = Some(ActiveTransition {
            from_app: String::from(from_app),
            to_app: String::from(to_app),
            transition_type,
            easing,
            start_time: current_time_ms,
            duration_ms,
            current_frame: 0,
            total_frames: num_frames,
            frames,
        });

        self.total_transitions = self.total_transitions.saturating_add(1);
    }

    /// Advance transition by one tick (call from cortex tick)
    pub fn tick(&mut self, current_time_ms: u64) -> Option<TransitionFrame> {
        if let Some(ref mut active) = self.active_transition {
            let elapsed = current_time_ms.saturating_sub(active.start_time);

            if elapsed >= active.duration_ms as u64 {
                // Transition complete
                let frame = active.frames.last().copied().unwrap_or(TransitionFrame {
                    progress: Q16_ONE,
                    opacity_from: Q16(0),
                    opacity_to: Q16_ONE,
                    offset_x: 0,
                    offset_y: 0,
                    scale: Q16_ONE,
                    rotation: Q16(0),
                });

                self.active_transition = None;
                return Some(frame);
            }

            let frame_index = (elapsed * active.total_frames as u64 / active.duration_ms as u64)
                .min(active.total_frames as u64 - 1) as usize;
            active.current_frame = frame_index as u32;

            active.frames.get(frame_index).copied()
        } else {
            None
        }
    }

    /// Get current frame without advancing
    pub fn current_frame(&self) -> Option<TransitionFrame> {
        self.active_transition
            .as_ref()
            .and_then(|active| active.frames.get(active.current_frame as usize).copied())
    }

    /// Pre-render frames for predicted app switch
    pub fn prerender(&mut self, from_app: &str, to_app: &str, num_frames: u32) {
        let transition_type = self.select_transition(from_app, to_app, 12, false, false);
        self.pre_rendered_frames = self.generate_frames(transition_type, self.easing_preference, num_frames);
    }

    /// Record user preference (learning)
    pub fn record_user_interaction(&mut self, skipped_early: bool) {
        if let Some(ref active) = self.active_transition {
            if skipped_early {
                // User skipped early — maybe learn to use faster transition next time
                if self.speed_multiplier < Q16_ONE * 2 {
                    self.speed_multiplier = (self.speed_multiplier * 11) / 10;
                }
            } else {
                // User watched full transition — they like it
                let key = (active.from_app.clone(), active.to_app.clone());
                self.transition_history.insert(key, active.transition_type);
            }
        }
    }

    /// Get statistics
    pub fn stats(&self) -> (u64, u32) {
        (self.total_transitions, self.avg_duration_ms)
    }

    /// Set easing preference
    pub fn set_easing_preference(&mut self, easing: EasingFunction) {
        self.easing_preference = easing;
    }

    /// Set speed multiplier (Q16 value, Q16_ONE = 1x)
    pub fn set_speed_multiplier(&mut self, speed: Q16) {
        self.speed_multiplier = speed.max(Q16::from(1)).min(Q16::from(4));
    }

    /// Check if transition is active
    pub fn is_active(&self) -> bool {
        self.active_transition.is_some()
    }
}

/// Global transition engine (static instance)
static TRANSITION_ENGINE: Mutex<Option<TransitionEngine>> = Mutex::new(None);

/// Initialize global transition engine
pub fn init() {
    let mut engine = TRANSITION_ENGINE.lock();
    *engine = Some(TransitionEngine::new());
}

/// Start a transition
pub fn start_transition(
    from_app: &str,
    to_app: &str,
    time_of_day_hour: u32,
    is_focused_mode: bool,
    is_gaming_mode: bool,
    current_time_ms: u64,
) {
    if let Some(ref mut engine) = *TRANSITION_ENGINE.lock() {
        engine.start_transition(
            from_app,
            to_app,
            time_of_day_hour,
            is_focused_mode,
            is_gaming_mode,
            current_time_ms,
        );
    }
}

/// Tick the transition engine
pub fn tick(current_time_ms: u64) -> Option<TransitionFrame> {
    TRANSITION_ENGINE.lock().as_mut().and_then(|e| e.tick(current_time_ms))
}

/// Get current frame
pub fn current_frame() -> Option<TransitionFrame> {
    TRANSITION_ENGINE.lock().as_ref().and_then(|e| e.current_frame())
}

/// Prerender for predicted switch
pub fn prerender(from_app: &str, to_app: &str, num_frames: u32) {
    if let Some(ref mut engine) = *TRANSITION_ENGINE.lock() {
        engine.prerender(from_app, to_app, num_frames);
    }
}

/// Record user interaction for learning
pub fn record_user_interaction(skipped_early: bool) {
    if let Some(ref mut engine) = *TRANSITION_ENGINE.lock() {
        engine.record_user_interaction(skipped_early);
    }
}

/// Get statistics
pub fn stats() -> (u64, u32) {
    TRANSITION_ENGINE
        .lock()
        .as_ref()
        .map(|e| e.stats())
        .unwrap_or((0, 0))
}

/// Set easing preference
pub fn set_easing_preference(easing: EasingFunction) {
    if let Some(ref mut engine) = *TRANSITION_ENGINE.lock() {
        engine.set_easing_preference(easing);
    }
}

/// Set speed multiplier
pub fn set_speed_multiplier(speed: Q16) {
    if let Some(ref mut engine) = *TRANSITION_ENGINE.lock() {
        engine.set_speed_multiplier(speed);
    }
}

/// Check if transition is active
pub fn is_active() -> bool {
    TRANSITION_ENGINE
        .lock()
        .as_ref()
        .map(|e| e.is_active())
        .unwrap_or(false)
}
