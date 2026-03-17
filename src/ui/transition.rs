/// Screen transition effects
///
/// Part of the AIOS UI layer.
use crate::sync::Mutex;

/// Transition effect types
#[derive(Debug, Clone, Copy)]
pub enum TransitionEffect {
    Fade,
    SlideLeft,
    SlideRight,
    SlideUp,
    SlideDown,
    ZoomIn,
    ZoomOut,
}

/// Manages screen-to-screen transitions
pub struct TransitionManager {
    pub active: bool,
    pub effect: TransitionEffect,
    pub progress: f32,
    pub duration_ms: u32,
    elapsed_ms: u32,
}

impl TransitionManager {
    pub fn new() -> Self {
        TransitionManager {
            active: false,
            effect: TransitionEffect::Fade,
            progress: 0.0,
            duration_ms: 300,
            elapsed_ms: 0,
        }
    }

    /// Start a screen transition with the given effect and duration.
    pub fn start(&mut self, effect: TransitionEffect, duration_ms: u32) {
        self.effect = effect;
        self.duration_ms = if duration_ms == 0 { 1 } else { duration_ms };
        self.elapsed_ms = 0;
        self.progress = 0.0;
        self.active = true;

        crate::serial_println!("  [transition] start {:?} ({}ms)", effect, duration_ms);
    }

    /// Advance the transition by dt_ms milliseconds.
    ///
    /// Returns true if the transition is still active, false if it completed.
    pub fn tick(&mut self, dt_ms: u32) -> bool {
        if !self.active {
            return false;
        }

        self.elapsed_ms = self.elapsed_ms.saturating_add(dt_ms);
        let t = (self.elapsed_ms as f32 / self.duration_ms as f32).min(1.0);

        // Apply easing (ease-in-out quadratic)
        self.progress = if t < 0.5 {
            2.0 * t * t
        } else {
            -1.0 + (4.0 - 2.0 * t) * t
        };

        if self.elapsed_ms >= self.duration_ms {
            self.progress = 1.0;
            self.active = false;
            crate::serial_println!("  [transition] completed {:?}", self.effect);
            return false;
        }

        true
    }

    /// Get the current transition value for rendering.
    ///
    /// Returns a value between 0.0 (start) and 1.0 (end) after easing.
    pub fn value(&self) -> f32 {
        self.progress
    }

    /// Get the pixel offset for slide transitions.
    ///
    /// For slide effects, returns the signed offset in pixels
    /// based on a given dimension (width for horizontal, height for vertical).
    pub fn slide_offset(&self, dimension: i32) -> i32 {
        let inv = 1.0 - self.progress;
        match self.effect {
            TransitionEffect::SlideLeft => (inv * dimension as f32) as i32,
            TransitionEffect::SlideRight => -(inv * dimension as f32) as i32,
            TransitionEffect::SlideUp => (inv * dimension as f32) as i32,
            TransitionEffect::SlideDown => -(inv * dimension as f32) as i32,
            _ => 0,
        }
    }

    /// Get the opacity for fade transitions (0.0 = transparent, 1.0 = opaque).
    pub fn fade_alpha(&self) -> f32 {
        match self.effect {
            TransitionEffect::Fade => self.progress,
            _ => 1.0,
        }
    }

    /// Get the scale factor for zoom transitions (0.0 = invisible, 1.0 = full size).
    pub fn zoom_scale(&self) -> f32 {
        match self.effect {
            TransitionEffect::ZoomIn => self.progress,
            TransitionEffect::ZoomOut => 1.0 - self.progress,
            _ => 1.0,
        }
    }

    /// Cancel the current transition immediately.
    pub fn cancel(&mut self) {
        if self.active {
            crate::serial_println!("  [transition] cancelled {:?}", self.effect);
            self.active = false;
            self.progress = 0.0;
        }
    }

    /// Check if a transition is currently running.
    pub fn is_active(&self) -> bool {
        self.active
    }
}

static TRANSITION_MGR: Mutex<Option<TransitionManager>> = Mutex::new(None);

pub fn init() {
    *TRANSITION_MGR.lock() = Some(TransitionManager::new());
    crate::serial_println!("  [transition] Transition manager initialized");
}

/// Start a global screen transition.
pub fn start(effect: TransitionEffect, duration_ms: u32) {
    if let Some(ref mut mgr) = *TRANSITION_MGR.lock() {
        mgr.start(effect, duration_ms);
    }
}

/// Tick the global transition.
pub fn tick(dt_ms: u32) -> bool {
    match TRANSITION_MGR.lock().as_mut() {
        Some(mgr) => mgr.tick(dt_ms),
        None => false,
    }
}
