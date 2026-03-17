/// Boot animation engine for Genesis
///
/// Provides: keyframe animation sequences, logo fade-in, progress bar,
/// spinner, and transition effects for the boot splash screen.
///
/// Uses Q16 fixed-point math throughout (no floats).
///
/// Inspired by: Android boot animation, Plymouth. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Q16 fixed-point constant: 1.0
const Q16_ONE: i32 = 65536;

/// Q16 multiply
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Q16 divide
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

/// Easing function type for keyframe interpolation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootEasing {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    Bounce,
}

/// Apply easing to a Q16 progress value (0..Q16_ONE)
fn apply_easing(easing: BootEasing, t: i32) -> i32 {
    let t = if t < 0 {
        0
    } else if t > Q16_ONE {
        Q16_ONE
    } else {
        t
    };
    match easing {
        BootEasing::Linear => t,
        BootEasing::EaseIn => q16_mul(t, t),
        BootEasing::EaseOut => {
            let inv = Q16_ONE - t;
            Q16_ONE - q16_mul(inv, inv)
        }
        BootEasing::EaseInOut => {
            let half = Q16_ONE / 2;
            if t < half {
                q16_mul(t, t) * 2
            } else {
                let inv = Q16_ONE - t;
                Q16_ONE - q16_mul(inv, inv) * 2
            }
        }
        BootEasing::Bounce => {
            // Simplified bounce: overshoot then settle
            let stretched = q16_mul(t, q16_from_int(3)) / 2;
            if stretched <= Q16_ONE {
                q16_mul(stretched, stretched)
            } else {
                let over = stretched - Q16_ONE;
                let bounce = Q16_ONE - q16_mul(over, over) / 4;
                if bounce < Q16_ONE {
                    bounce
                } else {
                    Q16_ONE
                }
            }
        }
    }
}

/// A single keyframe in an animation sequence
#[derive(Debug, Clone)]
pub struct Keyframe {
    pub time_ms: u64,
    pub alpha: i32,    // Q16: opacity (0 = transparent, Q16_ONE = opaque)
    pub x_offset: i32, // Q16: horizontal offset in pixels << 16
    pub y_offset: i32, // Q16: vertical offset in pixels << 16
    pub scale: i32,    // Q16: scale factor (Q16_ONE = 1x)
    pub easing: BootEasing,
}

/// Keyframe animation sequence
pub struct KeyframeAnimation {
    pub name: String,
    pub keyframes: Vec<Keyframe>,
    pub loop_count: i32, // -1 = infinite
    pub current_loop: i32,
    pub start_time: u64,
    pub finished: bool,
}

impl KeyframeAnimation {
    pub fn new(name: &str) -> Self {
        KeyframeAnimation {
            name: String::from(name),
            keyframes: Vec::new(),
            loop_count: 0,
            current_loop: 0,
            start_time: 0,
            finished: false,
        }
    }

    pub fn add_keyframe(&mut self, kf: Keyframe) {
        self.keyframes.push(kf);
    }

    /// Sample the animation at a given time, returning (alpha, x, y, scale) in Q16
    pub fn sample(&self, now_ms: u64) -> (i32, i32, i32, i32) {
        if self.keyframes.is_empty() || self.finished {
            return (Q16_ONE, 0, 0, Q16_ONE);
        }
        let elapsed = now_ms.saturating_sub(self.start_time);
        let total_dur = self.keyframes.last().map(|kf| kf.time_ms).unwrap_or(1);
        let local_time = if total_dur > 0 {
            elapsed % total_dur
        } else {
            0
        };

        // Find surrounding keyframes
        let mut prev_idx = 0;
        let mut next_idx = 0;
        for i in 0..self.keyframes.len() {
            if self.keyframes[i].time_ms <= local_time {
                prev_idx = i;
            }
            if self.keyframes[i].time_ms > local_time && next_idx <= prev_idx {
                next_idx = i;
                break;
            }
        }
        if next_idx <= prev_idx {
            next_idx = prev_idx;
        }

        let prev = &self.keyframes[prev_idx];
        let next = &self.keyframes[next_idx];

        if prev_idx == next_idx {
            return (prev.alpha, prev.x_offset, prev.y_offset, prev.scale);
        }

        let seg_dur = next.time_ms.saturating_sub(prev.time_ms);
        let seg_elapsed = local_time.saturating_sub(prev.time_ms);
        let t_raw = if seg_dur > 0 {
            ((seg_elapsed as i64 * Q16_ONE as i64) / seg_dur as i64) as i32
        } else {
            Q16_ONE
        };
        let t = apply_easing(next.easing, t_raw);

        let alpha = prev.alpha + q16_mul(next.alpha - prev.alpha, t);
        let x = prev.x_offset + q16_mul(next.x_offset - prev.x_offset, t);
        let y = prev.y_offset + q16_mul(next.y_offset - prev.y_offset, t);
        let scale = prev.scale + q16_mul(next.scale - prev.scale, t);

        (alpha, x, y, scale)
    }
}

/// Boot animation phase
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootPhase {
    LogoFadeIn,
    ProgressBar,
    Spinner,
    TransitionOut,
    Complete,
}

/// Spinner style
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpinnerStyle {
    Dots,
    Ring,
    Pulse,
    Bar,
}

/// Progress bar state
pub struct ProgressBar {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub progress: i32,     // Q16: 0..Q16_ONE
    pub bar_color: u32,    // ARGB packed
    pub bg_color: u32,     // ARGB packed
    pub border_color: u32, // ARGB packed
    pub show_text: bool,
}

impl ProgressBar {
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        ProgressBar {
            x,
            y,
            width,
            height,
            progress: 0,
            bar_color: 0xFF00C8DC,    // Hoags cyan
            bg_color: 0xFF191923,     // Dark bg
            border_color: 0xFF00B4C8, // Accent border
            show_text: true,
        }
    }

    pub fn set_progress(&mut self, p: i32) {
        self.progress = if p < 0 {
            0
        } else if p > Q16_ONE {
            Q16_ONE
        } else {
            p
        };
    }

    /// Return the filled width in pixels
    pub fn filled_width(&self) -> u32 {
        let w_q16 = q16_from_int(self.width as i32);
        let filled = q16_mul(w_q16, self.progress);
        (filled >> 16) as u32
    }

    /// Return progress as percentage (0..100)
    pub fn percentage(&self) -> u32 {
        let pct = q16_mul(self.progress, q16_from_int(100));
        (pct >> 16) as u32
    }
}

/// Spinner animation state
pub struct Spinner {
    pub cx: u32,
    pub cy: u32,
    pub radius: u32,
    pub style: SpinnerStyle,
    pub color: u32,
    pub tick: u32,
    pub num_dots: u32,
}

impl Spinner {
    pub fn new(cx: u32, cy: u32, radius: u32, style: SpinnerStyle) -> Self {
        Spinner {
            cx,
            cy,
            radius,
            style,
            color: 0xFF00C8DC,
            tick: 0,
            num_dots: 8,
        }
    }

    pub fn advance(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    /// Get the active dot index (for Dots style)
    pub fn active_dot(&self) -> u32 {
        self.tick % self.num_dots
    }

    /// Get pulse intensity (for Pulse style) as Q16
    pub fn pulse_intensity(&self) -> i32 {
        // Triangle wave: 0 -> Q16_ONE -> 0 over 60 ticks
        let phase = self.tick % 60;
        if phase < 30 {
            (phase as i32 * Q16_ONE) / 30
        } else {
            ((60 - phase) as i32 * Q16_ONE) / 30
        }
    }
}

/// Transition effect between boot phases
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionEffect {
    FadeToBlack,
    FadeToWhite,
    SlideUp,
    SlideDown,
    CrossFade,
    Dissolve,
}

/// Transition state
pub struct Transition {
    pub effect: TransitionEffect,
    pub duration_ms: u64,
    pub start_time: u64,
    pub active: bool,
}

impl Transition {
    pub fn new(effect: TransitionEffect, duration_ms: u64) -> Self {
        Transition {
            effect,
            duration_ms,
            start_time: 0,
            active: false,
        }
    }

    pub fn start(&mut self, now_ms: u64) {
        self.start_time = now_ms;
        self.active = true;
    }

    /// Get transition progress as Q16 (0..Q16_ONE)
    pub fn progress(&self, now_ms: u64) -> i32 {
        if !self.active {
            return 0;
        }
        let elapsed = now_ms.saturating_sub(self.start_time);
        if self.duration_ms == 0 {
            return Q16_ONE;
        }
        let t = ((elapsed as i64 * Q16_ONE as i64) / self.duration_ms as i64) as i32;
        if t > Q16_ONE {
            Q16_ONE
        } else {
            t
        }
    }

    /// Check if transition is complete
    pub fn is_done(&self, now_ms: u64) -> bool {
        if !self.active {
            return true;
        }
        now_ms.saturating_sub(self.start_time) >= self.duration_ms
    }

    /// Get the alpha overlay for fade effects (Q16)
    pub fn fade_alpha(&self, now_ms: u64) -> i32 {
        let p = self.progress(now_ms);
        match self.effect {
            TransitionEffect::FadeToBlack | TransitionEffect::FadeToWhite => p,
            TransitionEffect::CrossFade => p,
            _ => 0,
        }
    }

    /// Get the vertical offset for slide effects (in pixels, Q16)
    pub fn slide_offset(&self, now_ms: u64, screen_height: u32) -> i32 {
        let p = self.progress(now_ms);
        let h_q16 = q16_from_int(screen_height as i32);
        match self.effect {
            TransitionEffect::SlideUp => q16_mul(h_q16, Q16_ONE - p),
            TransitionEffect::SlideDown => q16_mul(h_q16, p) - h_q16,
            _ => 0,
        }
    }
}

/// The boot animation engine
pub struct BootAnimEngine {
    pub phase: BootPhase,
    pub logo_anim: KeyframeAnimation,
    pub progress_bar: ProgressBar,
    pub spinner: Spinner,
    pub transition: Transition,
    pub start_time: u64,
    pub boot_messages: Vec<String>,
    pub max_messages: usize,
    pub splash_color: u32,
    pub logo_width: u32,
    pub logo_height: u32,
}

impl BootAnimEngine {
    const fn new() -> Self {
        BootAnimEngine {
            phase: BootPhase::LogoFadeIn,
            logo_anim: KeyframeAnimation {
                name: String::new(),
                keyframes: Vec::new(),
                loop_count: 0,
                current_loop: 0,
                start_time: 0,
                finished: false,
            },
            progress_bar: ProgressBar {
                x: 0,
                y: 0,
                width: 400,
                height: 8,
                progress: 0,
                bar_color: 0xFF00C8DC,
                bg_color: 0xFF191923,
                border_color: 0xFF00B4C8,
                show_text: true,
            },
            spinner: Spinner {
                cx: 0,
                cy: 0,
                radius: 16,
                style: SpinnerStyle::Dots,
                color: 0xFF00C8DC,
                tick: 0,
                num_dots: 8,
            },
            transition: Transition {
                effect: TransitionEffect::FadeToBlack,
                duration_ms: 500,
                start_time: 0,
                active: false,
            },
            start_time: 0,
            boot_messages: Vec::new(),
            max_messages: 12,
            splash_color: 0xFF12121C,
            logo_width: 200,
            logo_height: 60,
        }
    }

    /// Begin the boot animation sequence
    pub fn begin(&mut self, now_ms: u64, screen_w: u32, screen_h: u32) {
        self.start_time = now_ms;
        self.phase = BootPhase::LogoFadeIn;

        // Center the progress bar
        self.progress_bar.x = (screen_w - self.progress_bar.width) / 2;
        self.progress_bar.y = screen_h / 2 + 60;

        // Center the spinner
        self.spinner.cx = screen_w / 2;
        self.spinner.cy = screen_h / 2 + 100;

        // Set up logo fade-in animation
        self.logo_anim = KeyframeAnimation::new("logo_fadein");
        self.logo_anim.start_time = now_ms;
        self.logo_anim.add_keyframe(Keyframe {
            time_ms: 0,
            alpha: 0,
            x_offset: 0,
            y_offset: q16_from_int(20),
            scale: q16_from_int(1) / 2, // start at 0.5x
            easing: BootEasing::Linear,
        });
        self.logo_anim.add_keyframe(Keyframe {
            time_ms: 1500,
            alpha: Q16_ONE,
            x_offset: 0,
            y_offset: 0,
            scale: Q16_ONE,
            easing: BootEasing::EaseOut,
        });
    }

    /// Update the boot animation state
    pub fn update(&mut self, now_ms: u64) {
        let elapsed = now_ms.saturating_sub(self.start_time);

        match self.phase {
            BootPhase::LogoFadeIn => {
                if elapsed > 2000 {
                    self.phase = BootPhase::ProgressBar;
                }
            }
            BootPhase::ProgressBar => {
                self.spinner.advance();
                if self.progress_bar.progress >= Q16_ONE {
                    self.transition = Transition::new(TransitionEffect::FadeToBlack, 500);
                    self.transition.start(now_ms);
                    self.phase = BootPhase::TransitionOut;
                }
            }
            BootPhase::Spinner => {
                self.spinner.advance();
            }
            BootPhase::TransitionOut => {
                if self.transition.is_done(now_ms) {
                    self.phase = BootPhase::Complete;
                }
            }
            BootPhase::Complete => {}
        }
    }

    /// Set boot progress (0..Q16_ONE)
    pub fn set_progress(&mut self, progress: i32) {
        self.progress_bar.set_progress(progress);
    }

    /// Add a boot status message
    pub fn add_message(&mut self, msg: &str) {
        self.boot_messages.push(String::from(msg));
        if self.boot_messages.len() > self.max_messages {
            self.boot_messages.remove(0);
        }
    }

    /// Get current logo opacity (Q16)
    pub fn logo_alpha(&self, now_ms: u64) -> i32 {
        let (alpha, _, _, _) = self.logo_anim.sample(now_ms);
        alpha
    }

    /// Get current logo vertical offset (Q16, in pixels << 16)
    pub fn logo_y_offset(&self, now_ms: u64) -> i32 {
        let (_, _, y, _) = self.logo_anim.sample(now_ms);
        y
    }

    /// Check if boot animation is complete
    pub fn is_complete(&self) -> bool {
        self.phase == BootPhase::Complete
    }

    /// Render the current state to the framebuffer
    pub fn render(&self, now_ms: u64) {
        use crate::drivers::framebuffer::{self, Color};

        let info = match framebuffer::info() {
            Some(i) if i.mode == framebuffer::DisplayMode::Graphics => i,
            _ => return,
        };

        // 1. Clear background
        framebuffer::fill_rect(
            0,
            0,
            info.width,
            info.height,
            Color::from_u32(self.splash_color),
        );

        // 2. Render Logo (Stylized Hoags 'H' with pulse glow)
        let alpha = self.logo_alpha(now_ms);
        let y_off = self.logo_y_offset(now_ms) >> 16;
        let logo_x = (info.width - 64) / 2;
        let logo_y = (info.height / 2 - 80) as i32 + y_off;

        if alpha > 0 {
            let base_color = Color::HOAGS_CYAN;
            let a_val = (q16_mul(alpha, q16_from_int(255)) >> 16) as u8;

            // Add pulse intensity based on uptime
            let pulse = ((now_ms / 20) % 100) as i32; // 0..100
            let pulse_alpha = if pulse < 50 {
                pulse * 2
            } else {
                200 - pulse * 2
            };
            let glow_a = (pulse_alpha as u32 * a_val as u32 / 255) as u8;

            // Draw glow/shadow (subtle)
            if glow_a > 20 {
                let glow_color = Color::rgba(0, 150, 180, glow_a / 2);
                framebuffer::fill_rect(logo_x - 4, logo_y as u32 - 4, 16 + 8, 64 + 8, glow_color);
                framebuffer::fill_rect(
                    logo_x + 48 - 4,
                    logo_y as u32 - 4,
                    16 + 8,
                    64 + 8,
                    glow_color,
                );
            }

            let logo_color = Color::rgba(base_color.r, base_color.g, base_color.b, a_val);
            // Draw a stylized 'H'
            framebuffer::fill_rect(logo_x, logo_y as u32, 16, 64, logo_color);
            framebuffer::fill_rect(logo_x + 48, logo_y as u32, 16, 64, logo_color);
            framebuffer::fill_rect(logo_x + 16, logo_y as u32 + 24, 32, 16, logo_color);

            // Highlight shine
            let shine_y = (now_ms / 10) % 200;
            if shine_y < 64 {
                let shine_color = Color::rgba(255, 255, 255, 40);
                framebuffer::fill_rect(logo_x, logo_y as u32 + shine_y as u32, 64, 2, shine_color);
            }
        }

        // 3. Render Progress Bar
        if self.phase == BootPhase::ProgressBar || self.phase == BootPhase::TransitionOut {
            let pb = &self.progress_bar;
            // Background
            framebuffer::fill_rect(
                pb.x,
                pb.y,
                pb.width,
                pb.height,
                Color::from_u32(pb.bg_color),
            );
            // Fill
            let filled = pb.filled_width();
            if filled > 0 {
                framebuffer::fill_rect(
                    pb.x,
                    pb.y,
                    filled,
                    pb.height,
                    Color::from_u32(pb.bar_color),
                );
            }
            // Border
            framebuffer::draw_rect(
                pb.x - 1,
                pb.y - 1,
                pb.width + 2,
                pb.height + 2,
                Color::from_u32(pb.border_color),
            );
        }

        // 4. Render Spinner
        if self.phase == BootPhase::ProgressBar || self.phase == BootPhase::Spinner {
            let s = &self.spinner;
            let dot_radius = 3i32;
            let active = s.active_dot();
            let color = Color::from_u32(s.color);

            for i in 0..s.num_dots {
                let (dx, dy) = match i {
                    0 => (0, -1),
                    1 => (1, -1),
                    2 => (1, 0),
                    3 => (1, 1),
                    4 => (0, 1),
                    5 => (-1, 1),
                    6 => (-1, 0),
                    7 => (-1, -1),
                    _ => (0, 0),
                };
                let px = s.cx as i32 + dx * s.radius as i32;
                let py = s.cy as i32 + dy * s.radius as i32;

                if i == active {
                    framebuffer::fill_circle(px, py, dot_radius + 1, color);
                } else {
                    framebuffer::fill_circle(
                        px,
                        py,
                        dot_radius,
                        Color::rgba(color.r, color.g, color.b, 64),
                    );
                }
            }
        }

        // 5. Render Boot Messages
        let msg_x = (info.width - 400) / 2;
        let mut msg_y = info.height - 150;
        let text_color = Color::rgb(100, 100, 120);

        for msg in &self.boot_messages {
            framebuffer::draw_string_transparent(msg_x, msg_y, msg, text_color);
            msg_y += 14;
        }

        // 6. Handle Transition Overlay
        if self.phase == BootPhase::TransitionOut {
            let t_alpha = self.transition.fade_alpha(now_ms);
            if t_alpha > 0 {
                let overlay =
                    Color::rgba(0, 0, 0, (q16_mul(t_alpha, q16_from_int(255)) >> 16) as u8);
                framebuffer::fill_rect(0, 0, info.width, info.height, overlay);
            }
        }
    }
}

static BOOT_ANIM: Mutex<BootAnimEngine> = Mutex::new(BootAnimEngine::new());

/// Initialize the boot animation engine
pub fn init() {
    serial_println!("    [boot-anim] Boot animation engine initialized (keyframe, progress, spinner, transitions)");
}

/// Begin boot animation
pub fn begin(now_ms: u64, screen_w: u32, screen_h: u32) {
    BOOT_ANIM.lock().begin(now_ms, screen_w, screen_h);
}

/// Update animation state
pub fn update(now_ms: u64) {
    BOOT_ANIM.lock().update(now_ms);
}

/// Set boot progress (0..Q16_ONE = 65536)
pub fn set_progress(progress: i32) {
    BOOT_ANIM.lock().set_progress(progress);
}

/// Add boot status message
pub fn add_message(msg: &str) {
    BOOT_ANIM.lock().add_message(msg);
}

/// Check if boot animation is complete
pub fn is_complete() -> bool {
    BOOT_ANIM.lock().is_complete()
}

/// Force the animation into the Complete state immediately (headless/QEMU boot).
pub fn force_complete() {
    BOOT_ANIM.lock().phase = BootPhase::Complete;
}

/// Get current phase
pub fn current_phase() -> BootPhase {
    BOOT_ANIM.lock().phase
}

/// Render the current boot animation frame
pub fn render_frame(now_ms: u64) {
    BOOT_ANIM.lock().render(now_ms);
    crate::drivers::framebuffer::flip();
}
