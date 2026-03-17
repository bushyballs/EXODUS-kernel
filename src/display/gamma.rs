use crate::sync::Mutex;
/// Gamma correction and color calibration
///
/// Part of the AIOS display layer. Manages gamma ramps for each
/// color channel, brightness adjustment, and night shift (blue light
/// reduction). Supports custom ICC-style color temperature correction.
use alloc::vec::Vec;

/// Gamma ramp (256 entries per channel)
pub struct GammaRamp {
    pub red: Vec<u16>,
    pub green: Vec<u16>,
    pub blue: Vec<u16>,
}

impl GammaRamp {
    /// Create a linear (identity) gamma ramp
    fn linear() -> Self {
        let mut red = Vec::with_capacity(256);
        let mut green = Vec::with_capacity(256);
        let mut blue = Vec::with_capacity(256);
        for i in 0..256u16 {
            let val = i * 257; // maps 0..255 to 0..65535
            red.push(val);
            green.push(val);
            blue.push(val);
        }
        Self { red, green, blue }
    }

    /// Apply a power-law gamma curve with the given exponent (fixed-point 8.8)
    fn apply_gamma(&mut self, gamma_fp: u16) {
        for i in 0..256 {
            let normalized = i as u32 * 256; // 0..65536 range
                                             // Compute pow(normalized/65536, gamma) * 65535
                                             // Using integer approximation: repeated squaring
            let result = gamma_power(normalized, gamma_fp);
            self.red[i] = result as u16;
            self.green[i] = result as u16;
            self.blue[i] = result as u16;
        }
    }

    /// Scale a channel by a factor (0..256 = 0.0..1.0)
    fn scale_channel(ramp: &mut Vec<u16>, factor_256: u16) {
        for entry in ramp.iter_mut() {
            *entry = ((*entry as u32 * factor_256 as u32) / 256) as u16;
        }
    }

    /// Apply brightness scaling to all channels
    fn apply_brightness(&mut self, brightness_256: u16) {
        Self::scale_channel(&mut self.red, brightness_256);
        Self::scale_channel(&mut self.green, brightness_256);
        Self::scale_channel(&mut self.blue, brightness_256);
    }

    /// Apply color temperature adjustment (reduce blue for warm, reduce red for cool)
    fn apply_temperature(&mut self, temp_kelvin: u32) {
        // Calculate RGB scaling factors based on color temperature
        // Using simplified Planckian locus approximation
        let (r_scale, g_scale, b_scale) = temperature_to_rgb_scale(temp_kelvin);
        Self::scale_channel(&mut self.red, r_scale);
        Self::scale_channel(&mut self.green, g_scale);
        Self::scale_channel(&mut self.blue, b_scale);
    }

    /// Lookup a gamma-corrected value for a given input (0..255)
    pub fn lookup(&self, r: u8, g: u8, b: u8) -> (u16, u16, u16) {
        (
            self.red[r as usize],
            self.green[g as usize],
            self.blue[b as usize],
        )
    }
}

/// Approximate pow(x/65536, gamma) * 65535 using integer math.
/// gamma_fp is in 8.8 fixed-point (256 = gamma 1.0).
fn gamma_power(x: u32, gamma_fp: u16) -> u32 {
    if x == 0 {
        return 0;
    }
    if gamma_fp == 256 {
        // gamma 1.0 = linear
        return (x * 65535) / 65536;
    }

    // Use log-based approach with fixed-point:
    // result = exp(gamma * ln(x/65536)) * 65535
    // Approximate with linear interpolation for simplicity
    let normalized = x as u64; // 0..65536
    let gamma_f = gamma_fp as u64; // 8.8 fixed-point

    // For gamma < 1.0 (brightening): bias toward higher values
    // For gamma > 1.0 (darkening): bias toward lower values
    if gamma_fp < 256 {
        // gamma < 1: sqrt-like curve
        // Interpolate between linear and sqrt
        let linear = (normalized * 65535) / 65536;
        let sqrt_val = integer_sqrt(normalized * 65536) * 255 / 256;
        let blend = 256 - gamma_f; // how much to blend toward sqrt
        let result = (linear * gamma_f + sqrt_val * blend) / 256;
        result.min(65535) as u32
    } else {
        // gamma > 1: square-like curve
        let linear = (normalized * 65535) / 65536;
        let square_val = (normalized * normalized * 65535) / (65536 * 65536);
        let blend = gamma_f - 256; // how much to blend toward square
        let blend = blend.min(256);
        let result = (linear * (256 - blend) + square_val * blend) / 256;
        result.min(65535) as u32
    }
}

/// Integer square root
fn integer_sqrt(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// Convert color temperature (Kelvin) to RGB scaling factors (0..256).
/// Based on Tanner Helland's approximation.
fn temperature_to_rgb_scale(temp_k: u32) -> (u16, u16, u16) {
    let temp = if temp_k < 1000 {
        1000
    } else if temp_k > 12000 {
        12000
    } else {
        temp_k
    };

    let r_scale: u16;
    let g_scale: u16;
    let b_scale: u16;

    // Red channel
    if temp <= 6600 {
        r_scale = 256; // full red at warm temps
    } else {
        // Decrease red as temperature rises above 6600K
        let t = temp - 6000;
        let r = 256u32.saturating_sub(t / 40);
        r_scale = r.min(256) as u16;
    }

    // Green channel
    if temp <= 6600 {
        // Increase green as temperature rises to 6600K
        let g = 80 + (temp * 176) / 6600;
        g_scale = g.min(256) as u16;
    } else {
        let t = temp - 6000;
        let g = 256u32.saturating_sub(t / 50);
        g_scale = g.min(256) as u16;
    }

    // Blue channel
    if temp <= 2000 {
        b_scale = 64; // very warm: minimal blue
    } else if temp <= 6600 {
        // Increase blue as temp rises
        let b = 64 + ((temp - 2000) * 192) / 4600;
        b_scale = b.min(256) as u16;
    } else {
        b_scale = 256; // full blue at cool temps
    }

    (r_scale, g_scale, b_scale)
}

/// Night shift mode state
struct NightShiftState {
    enabled: bool,
    temperature_k: u32,
    transition_progress: u32, // 0..256, for smooth fade-in
}

/// Manages display gamma and color calibration
pub struct GammaController {
    pub ramp: GammaRamp,
    pub brightness: f32,
    pub night_shift: bool,
    gamma_value_fp: u16,
    night_state: NightShiftState,
    dirty: bool,
    contrast: f32,
}

impl GammaController {
    pub fn new() -> Self {
        crate::serial_println!("[gamma] controller created with default ramp");
        Self {
            ramp: GammaRamp::linear(),
            brightness: 1.0,
            night_shift: false,
            gamma_value_fp: 256, // 1.0 in 8.8 fixed-point
            night_state: NightShiftState {
                enabled: false,
                temperature_k: 6500,
                transition_progress: 0,
            },
            dirty: true,
            contrast: 1.0,
        }
    }

    pub fn set_brightness(&mut self, level: f32) {
        let clamped = if level < 0.0 {
            0.0
        } else if level > 1.0 {
            1.0
        } else {
            level
        };
        self.brightness = clamped;
        self.dirty = true;
        self.rebuild_ramp();
        crate::serial_println!("[gamma] brightness set to {}", (clamped * 100.0) as u32);
    }

    pub fn set_night_shift(&mut self, enabled: bool, temperature: u32) {
        let temp_clamped = if temperature < 2700 {
            2700
        } else if temperature > 6500 {
            6500
        } else {
            temperature
        };
        self.night_shift = enabled;
        self.night_state.enabled = enabled;
        self.night_state.temperature_k = temp_clamped;
        if !enabled {
            self.night_state.transition_progress = 0;
        }
        self.dirty = true;
        self.rebuild_ramp();
        crate::serial_println!(
            "[gamma] night shift: enabled={}, temp={}K",
            enabled,
            temp_clamped
        );
    }

    /// Set the display gamma value (1.0 = linear, 2.2 = standard)
    pub fn set_gamma(&mut self, gamma: f32) {
        let clamped = if gamma < 0.5 {
            0.5
        } else if gamma > 3.0 {
            3.0
        } else {
            gamma
        };
        self.gamma_value_fp = (clamped * 256.0) as u16;
        self.dirty = true;
        self.rebuild_ramp();
        crate::serial_println!("[gamma] gamma set to {}", (clamped * 100.0) as u32);
    }

    /// Set contrast level (0.5..2.0)
    pub fn set_contrast(&mut self, contrast: f32) {
        let clamped = if contrast < 0.5 {
            0.5
        } else if contrast > 2.0 {
            2.0
        } else {
            contrast
        };
        self.contrast = clamped;
        self.dirty = true;
        self.rebuild_ramp();
    }

    /// Advance night shift transition (call periodically)
    pub fn tick_transition(&mut self) {
        if self.night_state.enabled && self.night_state.transition_progress < 256 {
            self.night_state.transition_progress =
                self.night_state.transition_progress.saturating_add(4);
            if self.night_state.transition_progress > 256 {
                self.night_state.transition_progress = 256;
            }
            self.dirty = true;
            self.rebuild_ramp();
        }
    }

    /// Rebuild the gamma ramp from current settings
    fn rebuild_ramp(&mut self) {
        if !self.dirty {
            return;
        }
        // Start from linear ramp
        self.ramp = GammaRamp::linear();

        // Apply gamma curve
        if self.gamma_value_fp != 256 {
            self.ramp.apply_gamma(self.gamma_value_fp);
        }

        // Apply contrast adjustment
        if (self.contrast - 1.0).abs() > 0.01 {
            let factor = (self.contrast * 256.0) as u16;
            for i in 0..256 {
                let centered_r = self.ramp.red[i] as i32 - 32768;
                let adjusted_r = ((centered_r as i64 * factor as i64) / 256) as i32 + 32768;
                self.ramp.red[i] = adjusted_r.max(0).min(65535) as u16;

                let centered_g = self.ramp.green[i] as i32 - 32768;
                let adjusted_g = ((centered_g as i64 * factor as i64) / 256) as i32 + 32768;
                self.ramp.green[i] = adjusted_g.max(0).min(65535) as u16;

                let centered_b = self.ramp.blue[i] as i32 - 32768;
                let adjusted_b = ((centered_b as i64 * factor as i64) / 256) as i32 + 32768;
                self.ramp.blue[i] = adjusted_b.max(0).min(65535) as u16;
            }
        }

        // Apply brightness scaling
        let brightness_256 = (self.brightness * 256.0) as u16;
        if brightness_256 != 256 {
            self.ramp.apply_brightness(brightness_256);
        }

        // Apply night shift color temperature
        if self.night_state.enabled && self.night_state.transition_progress > 0 {
            // Blend between 6500K (neutral) and target temperature
            let progress = self.night_state.transition_progress;
            let target = self.night_state.temperature_k;
            let blended_temp = 6500 - ((6500 - target) * progress) / 256;
            self.ramp.apply_temperature(blended_temp);
        }

        self.dirty = false;
    }

    /// Check if the ramp needs updating
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Get a reference to the current gamma ramp
    pub fn current_ramp(&self) -> &GammaRamp {
        &self.ramp
    }
}

impl GammaRamp {
    // Impl needed for abs()
}

static GAMMA: Mutex<Option<GammaController>> = Mutex::new(None);

pub fn init() {
    let controller = GammaController::new();
    let mut g = GAMMA.lock();
    *g = Some(controller);
    crate::serial_println!("[gamma] subsystem initialized");
}

/// Set brightness from external code
pub fn set_brightness(level: f32) {
    let mut g = GAMMA.lock();
    if let Some(ref mut ctrl) = *g {
        ctrl.set_brightness(level);
    }
}

/// Enable/disable night shift
pub fn set_night_shift(enabled: bool, temperature: u32) {
    let mut g = GAMMA.lock();
    if let Some(ref mut ctrl) = *g {
        ctrl.set_night_shift(enabled, temperature);
    }
}

/// Helper: abs for f32 without std
trait AbsF32 {
    fn abs(self) -> f32;
}
impl AbsF32 for f32 {
    fn abs(self) -> f32 {
        if self < 0.0 {
            -self
        } else {
            self
        }
    }
}
