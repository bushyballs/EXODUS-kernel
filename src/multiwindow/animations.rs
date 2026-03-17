use crate::{serial_print, serial_println};

/// Animation easing functions
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EasingFunction {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    Bounce,
    Elastic,
}

/// Window animation types
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AnimationType {
    Move,
    Resize,
    Fade,
    Minimize,
    Maximize,
    Restore,
}

/// Animation state
#[derive(Clone, Copy, Debug)]
pub struct Animation {
    pub window_id: u32,
    pub anim_type: AnimationType,
    pub easing: EasingFunction,
    pub duration_ms: u32,
    pub elapsed_ms: u32,
    pub start_x: i16,
    pub start_y: i16,
    pub start_width: u16,
    pub start_height: u16,
    pub end_x: i16,
    pub end_y: i16,
    pub end_width: u16,
    pub end_height: u16,
    pub start_opacity: u8,
    pub end_opacity: u8,
}

impl Animation {
    /// Create a new movement animation
    pub fn new_move(
        window_id: u32,
        from_x: i16,
        from_y: i16,
        to_x: i16,
        to_y: i16,
        duration_ms: u32,
    ) -> Self {
        Self {
            window_id,
            anim_type: AnimationType::Move,
            easing: EasingFunction::EaseOut,
            duration_ms,
            elapsed_ms: 0,
            start_x: from_x,
            start_y: from_y,
            start_width: 0,
            start_height: 0,
            end_x: to_x,
            end_y: to_y,
            end_width: 0,
            end_height: 0,
            start_opacity: 255,
            end_opacity: 255,
        }
    }

    /// Create a new resize animation
    pub fn new_resize(
        window_id: u32,
        from_width: u16,
        from_height: u16,
        to_width: u16,
        to_height: u16,
        duration_ms: u32,
    ) -> Self {
        Self {
            window_id,
            anim_type: AnimationType::Resize,
            easing: EasingFunction::EaseOut,
            duration_ms,
            elapsed_ms: 0,
            start_x: 0,
            start_y: 0,
            start_width: from_width,
            start_height: from_height,
            end_x: 0,
            end_y: 0,
            end_width: to_width,
            end_height: to_height,
            start_opacity: 255,
            end_opacity: 255,
        }
    }

    /// Create a new fade animation
    pub fn new_fade(window_id: u32, from_opacity: u8, to_opacity: u8, duration_ms: u32) -> Self {
        Self {
            window_id,
            anim_type: AnimationType::Fade,
            easing: EasingFunction::Linear,
            duration_ms,
            elapsed_ms: 0,
            start_x: 0,
            start_y: 0,
            start_width: 0,
            start_height: 0,
            end_x: 0,
            end_y: 0,
            end_width: 0,
            end_height: 0,
            start_opacity: from_opacity,
            end_opacity: to_opacity,
        }
    }

    /// Update animation with delta time
    pub fn update(&mut self, delta_ms: u32) -> bool {
        self.elapsed_ms += delta_ms;
        self.elapsed_ms >= self.duration_ms
    }

    /// Get current interpolated value (0.0 to 1.0)
    pub fn get_progress(&self) -> f32 {
        let t = (self.elapsed_ms as f32) / (self.duration_ms as f32);
        let t = t.min(1.0);

        match self.easing {
            EasingFunction::Linear => t,
            EasingFunction::EaseIn => t * t,
            EasingFunction::EaseOut => t * (2.0 - t),
            EasingFunction::EaseInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    -1.0 + (4.0 - 2.0 * t) * t
                }
            }
            EasingFunction::Bounce => {
                // Simplified bounce
                let t = 1.0 - t;
                1.0 - (t * t * 8.0) % 1.0
            }
            EasingFunction::Elastic => {
                // Polynomial approximation of elastic easing (no libm needed)
                if t == 0.0 || t == 1.0 {
                    t
                } else {
                    let t2 = t - 1.0;
                    // Approximates 2^(-10*t) using rational function
                    let decay = 1.0 / (1.0 + t2 * t2 * 100.0);
                    // Oscillation approximation (replaces sin wave)
                    let osc = t2 * 20.93;
                    // Simple cubic approximation for sine-like oscillation
                    let osc_approx = osc - (osc * osc * osc) / 6.0;
                    1.0 - decay * osc_approx
                }
            }
        }
    }

    /// Get current X position
    pub fn get_x(&self) -> i16 {
        let progress = self.get_progress();
        self.start_x + ((self.end_x - self.start_x) as f32 * progress) as i16
    }

    /// Get current Y position
    pub fn get_y(&self) -> i16 {
        let progress = self.get_progress();
        self.start_y + ((self.end_y - self.start_y) as f32 * progress) as i16
    }

    /// Get current width
    pub fn get_width(&self) -> u16 {
        let progress = self.get_progress();
        (self.start_width as f32
            + (self.end_width as i32 - self.start_width as i32) as f32 * progress) as u16
    }

    /// Get current height
    pub fn get_height(&self) -> u16 {
        let progress = self.get_progress();
        (self.start_height as f32
            + (self.end_height as i32 - self.start_height as i32) as f32 * progress) as u16
    }

    /// Get current opacity
    pub fn get_opacity(&self) -> u8 {
        let progress = self.get_progress();
        (self.start_opacity as f32
            + (self.end_opacity as i32 - self.start_opacity as i32) as f32 * progress) as u8
    }
}

/// Predefined animation presets
pub struct AnimationPresets;

impl AnimationPresets {
    /// Default window open animation (fade in + scale up)
    pub const WINDOW_OPEN_DURATION: u32 = 200;

    /// Default window close animation (fade out + scale down)
    pub const WINDOW_CLOSE_DURATION: u32 = 150;

    /// Default minimize animation (slide down to taskbar)
    pub const MINIMIZE_DURATION: u32 = 250;

    /// Default maximize animation (expand to fullscreen)
    pub const MAXIMIZE_DURATION: u32 = 200;

    /// Default window move animation (smooth drag)
    pub const MOVE_DURATION: u32 = 100;

    /// Default window resize animation
    pub const RESIZE_DURATION: u32 = 150;

    /// Snap animation (quick snap to zone)
    pub const SNAP_DURATION: u32 = 180;
}

/// Simple sin approximation (no std lib available)
trait FloatExt {
    fn sin(self) -> Self;
}

impl FloatExt for f32 {
    fn sin(self) -> f32 {
        // Taylor series approximation for sin
        let x = self % (2.0 * 3.14159);
        let x2 = x * x;
        x - (x * x2 / 6.0) + (x * x2 * x2 / 120.0) - (x * x2 * x2 * x2 / 5040.0)
    }
}
