/// Animation system for Genesis — frame-based animation engine
///
/// Provides: easing functions, keyframe animations, spring physics,
/// transition management, and frame scheduling.
///
/// Uses Q16 fixed-point math throughout (no floats).
///
/// Inspired by: Android Animator, CSS transitions. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Q16 fixed-point constant: 1.0
const Q16_ONE: i32 = 65536;
/// Q16 half: 0.5
const Q16_HALF: i32 = 32768;

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

/// Q16 absolute value
fn q16_abs(x: i32) -> i32 {
    if x < 0 {
        -x
    } else {
        x
    }
}

/// Easing function type
#[derive(Debug, Clone, Copy)]
pub enum Easing {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    CubicIn,
    CubicOut,
    CubicInOut,
    BounceOut,
    BackOut,
}

/// Apply easing function to t (Q16: 0..Q16_ONE), returns Q16 result
pub fn ease(easing: Easing, t: i32) -> i32 {
    let t = if t < 0 {
        0
    } else if t > Q16_ONE {
        Q16_ONE
    } else {
        t
    };
    match easing {
        Easing::Linear => t,
        Easing::EaseIn => {
            // t * t
            q16_mul(t, t)
        }
        Easing::EaseOut => {
            // t * (2 - t) = 2t - t^2
            let two = q16_from_int(2);
            q16_mul(t, two - t)
        }
        Easing::EaseInOut => {
            if t < Q16_HALF {
                // 2 * t * t
                let two = q16_from_int(2);
                q16_mul(two, q16_mul(t, t))
            } else {
                // -1 + (4 - 2*t) * t
                let neg_one = -Q16_ONE;
                let four = q16_from_int(4);
                let two = q16_from_int(2);
                neg_one + q16_mul(four - q16_mul(two, t), t)
            }
        }
        Easing::CubicIn => {
            // t * t * t
            q16_mul(q16_mul(t, t), t)
        }
        Easing::CubicOut => {
            // (t-1)^3 + 1
            let t1 = t - Q16_ONE;
            q16_mul(q16_mul(t1, t1), t1) + Q16_ONE
        }
        Easing::CubicInOut => {
            if t < Q16_HALF {
                // 4 * t * t * t
                let four = q16_from_int(4);
                q16_mul(four, q16_mul(q16_mul(t, t), t))
            } else {
                // 1 + 4 * (t-1)^3
                let four = q16_from_int(4);
                let t1 = t - Q16_ONE;
                Q16_ONE + q16_mul(four, q16_mul(q16_mul(t1, t1), t1))
            }
        }
        Easing::BounceOut => {
            // Using integer-scaled bounce constants
            // 1/2.75 in Q16 = 23831, 2/2.75 = 47662, 2.5/2.75 = 59578
            // 7.5625 in Q16 = 495616
            let k1 = 23831i32; // 1/2.75
            let k2 = 47662i32; // 2/2.75
            let k3 = 59578i32; // 2.5/2.75
            let bounce = 495616i32; // 7.5625
            let k_1_5 = 35747i32; // 1.5/2.75
            let k_2_25 = 53620i32; // 2.25/2.75
            let k_2_625 = 62599i32; // 2.625/2.75

            if t < k1 {
                q16_mul(bounce, q16_mul(t, t))
            } else if t < k2 {
                let t1 = t - k_1_5;
                q16_mul(bounce, q16_mul(t1, t1)) + 49152 // + 0.75
            } else if t < k3 {
                let t1 = t - k_2_25;
                q16_mul(bounce, q16_mul(t1, t1)) + 61440 // + 0.9375
            } else {
                let t1 = t - k_2_625;
                q16_mul(bounce, q16_mul(t1, t1)) + 64512 // + 0.984375
            }
        }
        Easing::BackOut => {
            // s = 1.70158 (Q16: 111477)
            // (t-1)^2 * ((s+1)*(t-1) + s) + 1
            let s: i32 = 111477; // 1.70158 in Q16
            let t1 = t - Q16_ONE;
            let sp1 = s + Q16_ONE; // s + 1
            let inner = q16_mul(sp1, t1) + s;
            q16_mul(q16_mul(t1, t1), inner) + Q16_ONE
        }
    }
}

/// Animation state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimState {
    Pending,
    Running,
    Completed,
    Cancelled,
}

/// A single animation — values are Q16 fixed-point
pub struct Animation {
    pub id: u32,
    pub name: String,
    pub start_value: i32, // Q16
    pub end_value: i32,   // Q16
    pub duration_ms: u64,
    pub easing: Easing,
    pub state: AnimState,
    pub start_time: u64,
    pub current_value: i32, // Q16
    pub repeat_count: i32,  // -1 = infinite
    pub repeat_current: i32,
    pub auto_reverse: bool,
    pub reverse: bool,
}

impl Animation {
    /// Create an animation from Q16 values
    pub fn new(name: &str, from: i32, to: i32, duration_ms: u64) -> Self {
        Animation {
            id: 0,
            name: String::from(name),
            start_value: from,
            end_value: to,
            duration_ms,
            easing: Easing::EaseInOut,
            state: AnimState::Pending,
            start_time: 0,
            current_value: from,
            repeat_count: 0,
            repeat_current: 0,
            auto_reverse: false,
            reverse: false,
        }
    }

    /// Create an animation from integer pixel values (auto-converts to Q16)
    pub fn new_int(name: &str, from: i32, to: i32, duration_ms: u64) -> Self {
        Self::new(name, q16_from_int(from), q16_from_int(to), duration_ms)
    }

    pub fn update(&mut self, now_ms: u64) -> i32 {
        if self.state == AnimState::Completed || self.state == AnimState::Cancelled {
            return self.current_value;
        }
        if self.state == AnimState::Pending {
            self.start_time = now_ms;
            self.state = AnimState::Running;
        }
        let elapsed = now_ms.saturating_sub(self.start_time);
        // t as Q16: elapsed / duration
        let t = if self.duration_ms > 0 {
            let t_raw = ((elapsed as i64 * Q16_ONE as i64) / self.duration_ms as i64) as i32;
            if t_raw > Q16_ONE {
                Q16_ONE
            } else {
                t_raw
            }
        } else {
            Q16_ONE
        };
        let eased = ease(self.easing, if self.reverse { Q16_ONE - t } else { t });
        self.current_value = self.start_value + q16_mul(self.end_value - self.start_value, eased);
        if t >= Q16_ONE {
            if self.repeat_count == -1 || self.repeat_current < self.repeat_count {
                self.repeat_current = self.repeat_current.saturating_add(1);
                self.start_time = now_ms;
                if self.auto_reverse {
                    self.reverse = !self.reverse;
                }
            } else {
                self.state = AnimState::Completed;
            }
        }
        self.current_value
    }

    /// Get current value as integer (truncated from Q16)
    pub fn current_int(&self) -> i32 {
        self.current_value >> 16
    }
}

/// Spring physics animation — all values Q16
pub struct SpringAnimation {
    pub position: i32,  // Q16
    pub velocity: i32,  // Q16
    pub target: i32,    // Q16
    pub stiffness: i32, // Q16 (e.g., 170 * Q16_ONE)
    pub damping: i32,   // Q16 (e.g., 26 * Q16_ONE)
    pub mass: i32,      // Q16 (e.g., Q16_ONE)
    pub settled: bool,
}

impl SpringAnimation {
    /// Create spring animation from Q16 values
    pub fn new(initial: i32, target: i32) -> Self {
        SpringAnimation {
            position: initial,
            velocity: 0,
            target,
            stiffness: q16_from_int(170),
            damping: q16_from_int(26),
            mass: Q16_ONE,
            settled: false,
        }
    }

    /// Create spring animation from integer values
    pub fn new_int(initial: i32, target: i32) -> Self {
        Self::new(q16_from_int(initial), q16_from_int(target))
    }

    /// Step the spring by dt (Q16, e.g., Q16_ONE/60 for 60fps)
    pub fn step(&mut self, dt: i32) -> i32 {
        if self.settled {
            return self.position;
        }
        let displacement = self.position - self.target;
        let spring_force = -q16_mul(self.stiffness, displacement);
        let damping_force = -q16_mul(self.damping, self.velocity);
        let acceleration = q16_div(spring_force + damping_force, self.mass);
        self.velocity += q16_mul(acceleration, dt);
        self.position += q16_mul(self.velocity, dt);
        // Settle threshold: ~0.01 in Q16 = 655
        let threshold = 655;
        if q16_abs(displacement) < threshold && q16_abs(self.velocity) < threshold {
            self.position = self.target;
            self.velocity = 0;
            self.settled = true;
        }
        self.position
    }

    /// Get current position as integer
    pub fn position_int(&self) -> i32 {
        self.position >> 16
    }
}

/// Animation manager
pub struct AnimationManager {
    animations: Vec<Animation>,
    next_id: u32,
}

impl AnimationManager {
    const fn new() -> Self {
        AnimationManager {
            animations: Vec::new(),
            next_id: 1,
        }
    }

    pub fn start(&mut self, mut anim: Animation) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        anim.id = id;
        self.animations.push(anim);
        id
    }

    pub fn cancel(&mut self, id: u32) {
        if let Some(a) = self.animations.iter_mut().find(|a| a.id == id) {
            a.state = AnimState::Cancelled;
        }
    }

    pub fn update_all(&mut self, now_ms: u64) {
        for anim in &mut self.animations {
            if anim.state == AnimState::Running || anim.state == AnimState::Pending {
                anim.update(now_ms);
            }
        }
        self.animations
            .retain(|a| a.state != AnimState::Completed && a.state != AnimState::Cancelled);
    }

    pub fn get_value(&self, id: u32) -> Option<i32> {
        self.animations
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.current_value)
    }

    /// Get value as integer pixel value
    pub fn get_value_int(&self, id: u32) -> Option<i32> {
        self.animations
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.current_value >> 16)
    }

    pub fn active_count(&self) -> usize {
        self.animations.len()
    }
}

static ANIM_MGR: Mutex<AnimationManager> = Mutex::new(AnimationManager::new());

pub fn start(anim: Animation) -> u32 {
    ANIM_MGR.lock().start(anim)
}
pub fn cancel(id: u32) {
    ANIM_MGR.lock().cancel(id);
}
pub fn update_all(now_ms: u64) {
    ANIM_MGR.lock().update_all(now_ms);
}
pub fn get_value(id: u32) -> Option<i32> {
    ANIM_MGR.lock().get_value(id)
}
pub fn get_value_int(id: u32) -> Option<i32> {
    ANIM_MGR.lock().get_value_int(id)
}

// ---------------------------------------------------------------------------
// Standalone easing functions (Q16 fixed-point, integer-only)
//
// All functions map a time fraction expressed as (t / max_t) in the range
// [0, max_t] to an output in [0, Q16_ONE].  max_t == 0 is handled safely.
//
// These are thin convenience wrappers around the `ease()` function above,
// using the u32-based API requested by callers that prefer explicit t/max_t.
// ---------------------------------------------------------------------------

/// Linear easing: output = t / max_t  (Q16, 0..Q16_ONE).
pub fn ease_linear(t: u32, max_t: u32) -> u32 {
    if max_t == 0 {
        return Q16_ONE as u32;
    }
    let t_q16 = ((t as i64 * Q16_ONE as i64) / max_t as i64).min(Q16_ONE as i64) as i32;
    ease(Easing::Linear, t_q16) as u32
}

/// Quadratic ease-in: output = (t/max_t)²  (Q16, 0..Q16_ONE).
pub fn ease_in_quad(t: u32, max_t: u32) -> u32 {
    if max_t == 0 {
        return Q16_ONE as u32;
    }
    let t_q16 = ((t as i64 * Q16_ONE as i64) / max_t as i64).min(Q16_ONE as i64) as i32;
    ease(Easing::EaseIn, t_q16) as u32
}

/// Quadratic ease-out: output = 1 − (1 − t/max_t)²  (Q16, 0..Q16_ONE).
pub fn ease_out_quad(t: u32, max_t: u32) -> u32 {
    if max_t == 0 {
        return Q16_ONE as u32;
    }
    let t_q16 = ((t as i64 * Q16_ONE as i64) / max_t as i64).min(Q16_ONE as i64) as i32;
    ease(Easing::EaseOut, t_q16) as u32
}

/// Cubic ease-in-out  (Q16, 0..Q16_ONE).
pub fn ease_in_out_cubic(t: u32, max_t: u32) -> u32 {
    if max_t == 0 {
        return Q16_ONE as u32;
    }
    let t_q16 = ((t as i64 * Q16_ONE as i64) / max_t as i64).min(Q16_ONE as i64) as i32;
    ease(Easing::CubicInOut, t_q16) as u32
}

// ---------------------------------------------------------------------------
// Spring animation — single Verlet step (integer-only)
//
// Simulates a damped spring that pulls `*pos` toward `target`.
//
// Parameters (all in Q16 fixed-point units):
//   pos       — current position (updated in place)
//   vel       — current velocity (updated in place)
//   target    — desired resting position
//   stiffness — spring constant k  (e.g. 170 * Q16_ONE)
//   damping   — damping coefficient (e.g.  26 * Q16_ONE)
//
// One call = one simulation tick.  Callers should pass a fixed dt such as
// Q16_ONE / 60  (≈ one 60 fps frame).  The spring auto-settles when both
// displacement and velocity fall below the threshold ~0.01 (Q16: 655).
// ---------------------------------------------------------------------------

/// Advance the spring simulation by one tick.
///
/// Uses a fixed dt of Q16_ONE/60 (one frame at 60 fps) internally.
/// Modify the implementation if a variable dt is needed.
pub fn spring_step(pos: &mut i32, vel: &mut i32, target: i32, stiffness: i32, damping: i32) {
    // dt = 1/60 in Q16
    let dt = Q16_ONE / 60;
    let displacement = *pos - target;
    let spring_force = -q16_mul(stiffness, displacement);
    let damping_force = -q16_mul(damping, *vel);
    let acceleration = spring_force + damping_force; // mass == 1 so a = F
    *vel = vel.saturating_add(q16_mul(acceleration, dt));
    *pos = pos.saturating_add(q16_mul(*vel, dt));

    // Settle when both displacement and velocity are tiny (< ~0.01 in Q16).
    let threshold = 655i32;
    let disp_now = (*pos - target).abs();
    if disp_now < threshold && vel.abs() < threshold {
        *pos = target;
        *vel = 0;
    }
}

// ---------------------------------------------------------------------------
// Color interpolation
//
// Interpolates two packed RGBA colors independently per channel.
// `t_q16` is in [0, Q16_ONE] where 0 = fully `from` and Q16_ONE = fully `to`.
// ---------------------------------------------------------------------------

/// Linear interpolation between two RGBA colours (packed as 0xAARRGGBB).
///
/// `t_q16` is a Q16 fraction in [0, Q16_ONE].  Values outside this range are
/// clamped.  Each of the four byte channels is interpolated independently.
pub fn lerp_color(from: u32, to: u32, t_q16: u32) -> u32 {
    // Clamp t to [0, Q16_ONE].
    let t = t_q16.min(Q16_ONE as u32);
    let inv_t = (Q16_ONE as u32).saturating_sub(t);

    // Extract channels from `from` (little-endian byte order assumed).
    let fa = (from >> 24) & 0xFF;
    let fr = (from >> 16) & 0xFF;
    let fg = (from >> 8) & 0xFF;
    let fb = from & 0xFF;

    // Extract channels from `to`.
    let ta = (to >> 24) & 0xFF;
    let tr = (to >> 16) & 0xFF;
    let tg = (to >> 8) & 0xFF;
    let tb = to & 0xFF;

    // Interpolate each channel.
    // (from_ch * inv_t + to_ch * t) / Q16_ONE — use u64 to avoid overflow.
    let q = Q16_ONE as u64;
    let oa = ((fa as u64 * inv_t as u64 + ta as u64 * t as u64) / q) as u32;
    let or_ = ((fr as u64 * inv_t as u64 + tr as u64 * t as u64) / q) as u32;
    let og = ((fg as u64 * inv_t as u64 + tg as u64 * t as u64) / q) as u32;
    let ob = ((fb as u64 * inv_t as u64 + tb as u64 * t as u64) / q) as u32;

    (oa << 24) | (or_ << 16) | (og << 8) | ob
}
