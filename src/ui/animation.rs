use crate::sync::Mutex;
/// UI animation engine
///
/// Part of the AIOS UI layer.
use alloc::vec::Vec;

/// Approximate e^x using Taylor series (no_std compatible).
fn exp_f32(x: f32) -> f32 {
    // For large negative values, result is near zero
    if x < -20.0 {
        return 0.0;
    }
    // Taylor series: e^x = 1 + x + x^2/2! + x^3/3! + ...
    let mut sum = 1.0f32;
    let mut term = 1.0f32;
    for i in 1..30 {
        term *= x / (i as f32);
        sum += term;
        if term.abs() < 1e-7 {
            break;
        }
    }
    if sum < 0.0 {
        0.0
    } else {
        sum
    }
}

/// Easing function type
#[derive(Debug, Clone, Copy)]
pub enum Easing {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    Spring,
}

/// Apply an easing function to a normalized time t (0.0 to 1.0).
fn apply_easing(easing: Easing, t: f32) -> f32 {
    match easing {
        Easing::Linear => t,
        Easing::EaseIn => t * t,
        Easing::EaseOut => t * (2.0 - t),
        Easing::EaseInOut => {
            if t < 0.5 {
                2.0 * t * t
            } else {
                -1.0 + (4.0 - 2.0 * t) * t
            }
        }
        Easing::Spring => {
            // Damped spring approximation
            let decay = exp_f32(-5.0 * t);
            1.0 - decay * (1.0 - t)
        }
    }
}

/// A running animation instance
pub struct Animation {
    pub from: f32,
    pub to: f32,
    pub duration_ms: u32,
    pub elapsed_ms: u32,
    pub easing: Easing,
}

impl Animation {
    /// Get the current interpolated value.
    pub fn current_value(&self) -> f32 {
        if self.duration_ms == 0 {
            return self.to;
        }
        let t = (self.elapsed_ms as f32 / self.duration_ms as f32).min(1.0);
        let eased = apply_easing(self.easing, t);
        self.from + (self.to - self.from) * eased
    }

    /// Check if the animation has completed.
    pub fn is_done(&self) -> bool {
        self.elapsed_ms >= self.duration_ms
    }
}

/// Manages all active UI animations
pub struct AnimationEngine {
    pub animations: Vec<Animation>,
}

impl AnimationEngine {
    pub fn new() -> Self {
        AnimationEngine {
            animations: Vec::new(),
        }
    }

    /// Start an animation and return its index.
    pub fn start(&mut self, anim: Animation) -> usize {
        let idx = self.animations.len();
        self.animations.push(anim);
        idx
    }

    /// Advance all animations by dt_ms milliseconds.
    /// Removes completed animations.
    pub fn tick(&mut self, dt_ms: u32) {
        for anim in self.animations.iter_mut() {
            anim.elapsed_ms = anim.elapsed_ms.saturating_add(dt_ms);
        }
        // Remove completed animations
        self.animations.retain(|a| !a.is_done());
    }

    /// Number of active animations.
    pub fn active_count(&self) -> usize {
        self.animations.len()
    }

    /// Get the current value of an animation by index.
    pub fn value(&self, idx: usize) -> Option<f32> {
        self.animations.get(idx).map(|a| a.current_value())
    }
}

static ANIMATION_ENGINE: Mutex<Option<AnimationEngine>> = Mutex::new(None);

pub fn init() {
    *ANIMATION_ENGINE.lock() = Some(AnimationEngine::new());
    crate::serial_println!("  [animation] Animation engine initialized");
}

/// Start a global animation.
pub fn start(from: f32, to: f32, duration_ms: u32, easing: Easing) -> usize {
    match ANIMATION_ENGINE.lock().as_mut() {
        Some(engine) => engine.start(Animation {
            from,
            to,
            duration_ms,
            elapsed_ms: 0,
            easing,
        }),
        None => 0,
    }
}

/// Tick all global animations.
pub fn tick(dt_ms: u32) {
    if let Some(ref mut engine) = *ANIMATION_ENGINE.lock() {
        engine.tick(dt_ms);
    }
}
