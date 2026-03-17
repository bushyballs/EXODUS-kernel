/// Color management for Genesis
///
/// Provides: ICC color profiles, gamut mapping, night light / blue filter,
/// HDR tone mapping, color temperature adjustment, and per-display profiles.
///
/// Uses Q16 fixed-point math throughout (no floats).
///
/// Inspired by: colord (Linux), ICC specification, macOS ColorSync.
/// All code is original.
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

/// Q16 from integer
fn q16_from_int(x: i32) -> i32 {
    x << 16
}

/// Color space identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    SRGB,
    AdobeRGB,
    DciP3,
    Rec709,
    Rec2020,
    ProPhotoRGB,
    LinearRGB,
    Custom,
}

/// ICC profile class
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileClass {
    Input,
    Display,
    Output,
    DeviceLink,
    Abstract,
    NamedColor,
}

/// ICC rendering intent
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderingIntent {
    Perceptual,
    RelativeColorimetric,
    Saturation,
    AbsoluteColorimetric,
}

/// 3x3 color matrix in Q16 fixed-point
#[derive(Debug, Clone, Copy)]
pub struct ColorMatrix {
    pub m: [i32; 9], // Row-major: [r0c0, r0c1, r0c2, r1c0, r1c1, r1c2, r2c0, r2c1, r2c2]
}

impl ColorMatrix {
    /// Identity matrix
    pub const fn identity() -> Self {
        ColorMatrix {
            m: [Q16_ONE, 0, 0, 0, Q16_ONE, 0, 0, 0, Q16_ONE],
        }
    }

    /// Transform an RGB triplet (each channel Q16, 0..Q16_ONE)
    pub fn transform(&self, r: i32, g: i32, b: i32) -> (i32, i32, i32) {
        let out_r = q16_mul(self.m[0], r) + q16_mul(self.m[1], g) + q16_mul(self.m[2], b);
        let out_g = q16_mul(self.m[3], r) + q16_mul(self.m[4], g) + q16_mul(self.m[5], b);
        let out_b = q16_mul(self.m[6], r) + q16_mul(self.m[7], g) + q16_mul(self.m[8], b);
        (out_r, out_g, out_b)
    }

    /// Multiply two matrices: self * other
    pub fn multiply(&self, other: &ColorMatrix) -> ColorMatrix {
        let mut result = [0i32; 9];
        for row in 0..3 {
            for col in 0..3 {
                let mut sum: i64 = 0;
                for k in 0..3 {
                    sum += (self.m[row * 3 + k] as i64) * (other.m[k * 3 + col] as i64);
                }
                result[row * 3 + col] = (sum >> 16) as i32;
            }
        }
        ColorMatrix { m: result }
    }
}

/// A 1D lookup table (tone response curve) with Q16 values
pub struct ToneCurve {
    pub entries: Vec<i32>, // Q16 values, evenly spaced from 0..Q16_ONE input
}

impl ToneCurve {
    /// Linear (identity) tone curve
    pub fn linear(size: usize) -> Self {
        let mut entries = Vec::with_capacity(size);
        for i in 0..size {
            let t = ((i as i64 * Q16_ONE as i64) / (size - 1).max(1) as i64) as i32;
            entries.push(t);
        }
        ToneCurve { entries }
    }

    /// Gamma curve approximation using integer math
    /// gamma_q16: gamma value in Q16 (e.g., 2.2 = 144179)
    pub fn gamma(size: usize, gamma_q16: i32) -> Self {
        // Approximate gamma with a piecewise linear curve
        let mut entries = Vec::with_capacity(size);
        for i in 0..size {
            let t = ((i as i64 * Q16_ONE as i64) / (size - 1).max(1) as i64) as i32;
            // Simple power approximation: x^gamma ~ x for gamma near 1
            // For gamma 2.2: use quadratic approximation
            let result = if gamma_q16 > Q16_ONE + Q16_ONE / 2 {
                // gamma > 1.5: approximate as t^2 blended toward t
                let t2 = q16_mul(t, t);
                let blend = gamma_q16 - Q16_ONE;
                let blend_clamped = if blend > Q16_ONE { Q16_ONE } else { blend };
                t + q16_mul(t2 - t, blend_clamped)
            } else if gamma_q16 < Q16_ONE / 2 {
                // gamma < 0.5: approximate as sqrt(t) blended toward t
                // sqrt approximation: start at t, iterate
                let mut guess = t;
                if t > 0 {
                    for _ in 0..4 {
                        let _g2 = q16_mul(guess, guess);
                        if guess > 0 {
                            guess =
                                (guess + ((t as i64 * Q16_ONE as i64 / guess as i64) as i32)) / 2;
                        }
                    }
                }
                let inv_blend = Q16_ONE - gamma_q16;
                let inv_clamped = if inv_blend > Q16_ONE {
                    Q16_ONE
                } else {
                    inv_blend
                };
                t + q16_mul(guess - t, inv_clamped)
            } else {
                t
            };
            entries.push(if result < 0 {
                0
            } else if result > Q16_ONE {
                Q16_ONE
            } else {
                result
            });
        }
        ToneCurve { entries }
    }

    /// Sample the curve at a Q16 input value
    pub fn sample(&self, input: i32) -> i32 {
        if self.entries.is_empty() {
            return input;
        }
        let clamped = if input < 0 {
            0
        } else if input > Q16_ONE {
            Q16_ONE
        } else {
            input
        };
        let max_idx = (self.entries.len() - 1) as i32;
        let idx_q16 = q16_mul(clamped, q16_from_int(max_idx));
        let idx = (idx_q16 >> 16) as usize;
        let frac = idx_q16 & 0xFFFF;

        if idx >= self.entries.len() - 1 {
            return *self.entries.last().unwrap_or(&input);
        }
        let a = self.entries[idx];
        let b = self.entries[idx + 1];
        a + q16_mul(b - a, frac)
    }
}

/// ICC color profile
pub struct IccProfile {
    pub id: u32,
    pub name: String,
    pub color_space: ColorSpace,
    pub profile_class: ProfileClass,
    pub rendering_intent: RenderingIntent,
    pub to_xyz_matrix: ColorMatrix,   // Device RGB to CIE XYZ
    pub from_xyz_matrix: ColorMatrix, // CIE XYZ to device RGB
    pub red_trc: ToneCurve,
    pub green_trc: ToneCurve,
    pub blue_trc: ToneCurve,
    pub white_point_x: i32, // Q16 CIE x chromaticity
    pub white_point_y: i32, // Q16 CIE y chromaticity
}

impl IccProfile {
    /// Create an sRGB profile
    pub fn srgb(id: u32) -> Self {
        // sRGB to XYZ D65 matrix (Q16)
        let to_xyz = ColorMatrix {
            m: [
                27209, 21246, 9899, // 0.4124, 0.3576, 0.1805 (approximate Q16)
                13933, 36346, 4560, // 0.2126, 0.7152, 0.0722
                1568, 7220, 47674, // 0.0193, 0.1192, 0.9505
            ],
        };
        // XYZ to sRGB matrix (Q16)
        let from_xyz = ColorMatrix {
            m: [
                214739, -101556, -48103, // 3.2406, -1.5372, -0.4986
                -69721, 134830, 2840, // -0.9689, 2.0572, 0.0585
                4565, -23019, 76892, // 0.0557, -0.3176, 1.0710
            ],
        };

        IccProfile {
            id,
            name: String::from("sRGB IEC61966-2.1"),
            color_space: ColorSpace::SRGB,
            profile_class: ProfileClass::Display,
            rendering_intent: RenderingIntent::Perceptual,
            to_xyz_matrix: to_xyz,
            from_xyz_matrix: from_xyz,
            red_trc: ToneCurve::gamma(256, 144179), // gamma 2.2 in Q16
            green_trc: ToneCurve::gamma(256, 144179),
            blue_trc: ToneCurve::gamma(256, 144179),
            white_point_x: 20382, // 0.3127 in Q16
            white_point_y: 21432, // 0.3290 in Q16
        }
    }

    /// Transform a u32 ARGB pixel through this profile to linear RGB (Q16 per channel)
    pub fn to_linear(&self, pixel: u32) -> (i32, i32, i32) {
        let r_u8 = ((pixel >> 16) & 0xFF) as i32;
        let g_u8 = ((pixel >> 8) & 0xFF) as i32;
        let b_u8 = (pixel & 0xFF) as i32;
        let r_q16 = (r_u8 << 16) / 255;
        let g_q16 = (g_u8 << 16) / 255;
        let b_q16 = (b_u8 << 16) / 255;
        let r_lin = self.red_trc.sample(r_q16);
        let g_lin = self.green_trc.sample(g_q16);
        let b_lin = self.blue_trc.sample(b_q16);
        (r_lin, g_lin, b_lin)
    }
}

/// Night light / blue light filter settings
pub struct NightLight {
    pub enabled: bool,
    pub strength: i32,          // Q16: 0 = off, Q16_ONE = maximum
    pub color_temp_kelvin: u32, // Target color temperature (2700-6500K)
    pub schedule_enabled: bool,
    pub schedule_start_hour: u8,
    pub schedule_end_hour: u8,
    pub fade_duration_min: u32,
    pub active: bool,
}

impl NightLight {
    const fn new() -> Self {
        NightLight {
            enabled: false,
            strength: Q16_ONE / 2,
            color_temp_kelvin: 3400,
            schedule_enabled: true,
            schedule_start_hour: 21,
            schedule_end_hour: 7,
            fade_duration_min: 30,
            active: false,
        }
    }

    /// Get the RGB multipliers for the current temperature (each Q16)
    /// Based on simplified Planckian locus approximation
    pub fn temperature_rgb(&self) -> (i32, i32, i32) {
        let temp = self.color_temp_kelvin;
        // At 6500K (daylight): (1.0, 1.0, 1.0)
        // At 2700K (warm): (1.0, 0.75, 0.5)
        let r = Q16_ONE; // Red stays at 1.0
        let g = if temp >= 6500 {
            Q16_ONE
        } else {
            // Linear interpolation from 0.75 at 2700K to 1.0 at 6500K
            let range = 6500 - 2700;
            let t_offset = temp.saturating_sub(2700);
            let g_min = Q16_ONE * 3 / 4; // 0.75
            g_min + ((Q16_ONE - g_min) as u64 * t_offset as u64 / range as u64) as i32
        };
        let b = if temp >= 6500 {
            Q16_ONE
        } else {
            // Linear from 0.45 at 2700K to 1.0 at 6500K
            let range = 6500 - 2700;
            let t_offset = temp.saturating_sub(2700);
            let b_min = Q16_ONE * 45 / 100; // 0.45
            b_min + ((Q16_ONE - b_min) as u64 * t_offset as u64 / range as u64) as i32
        };
        (r, g, b)
    }

    /// Apply blue filter to a pixel (ARGB u32)
    pub fn filter_pixel(&self, pixel: u32) -> u32 {
        if !self.active || self.strength == 0 {
            return pixel;
        }
        let a = (pixel >> 24) & 0xFF;
        let r = ((pixel >> 16) & 0xFF) as i32;
        let g = ((pixel >> 8) & 0xFF) as i32;
        let b = (pixel & 0xFF) as i32;

        let (mr, mg, mb) = self.temperature_rgb();
        // Blend between original and filtered based on strength
        let inv_strength = Q16_ONE - self.strength;
        let fr = q16_mul(q16_from_int(r), inv_strength + q16_mul(self.strength, mr)) >> 16;
        let fg = q16_mul(q16_from_int(g), inv_strength + q16_mul(self.strength, mg)) >> 16;
        let fb = q16_mul(q16_from_int(b), inv_strength + q16_mul(self.strength, mb)) >> 16;

        let fr = if fr < 0 {
            0
        } else if fr > 255 {
            255
        } else {
            fr
        } as u32;
        let fg = if fg < 0 {
            0
        } else if fg > 255 {
            255
        } else {
            fg
        } as u32;
        let fb = if fb < 0 {
            0
        } else if fb > 255 {
            255
        } else {
            fb
        } as u32;

        (a << 24) | (fr << 16) | (fg << 8) | fb
    }

    /// Update schedule: check current hour and set active
    pub fn update_schedule(&mut self, current_hour: u8) {
        if !self.schedule_enabled {
            self.active = self.enabled;
            return;
        }
        if self.schedule_start_hour > self.schedule_end_hour {
            // Wraps midnight (e.g., 21:00 - 07:00)
            self.active =
                current_hour >= self.schedule_start_hour || current_hour < self.schedule_end_hour;
        } else {
            self.active =
                current_hour >= self.schedule_start_hour && current_hour < self.schedule_end_hour;
        }
    }
}

/// HDR tone mapping operator
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToneMapOperator {
    Reinhard,
    AcesFit,
    Uncharted2,
    Linear,
}

/// HDR settings
pub struct HdrSettings {
    pub enabled: bool,
    pub operator: ToneMapOperator,
    pub max_luminance: i32, // Q16: peak nits / 80 (SDR reference)
    pub paper_white: i32,   // Q16: SDR content brightness multiplier
    pub exposure: i32,      // Q16: exposure adjustment
}

impl HdrSettings {
    const fn new() -> Self {
        HdrSettings {
            enabled: false,
            operator: ToneMapOperator::Reinhard,
            max_luminance: Q16_ONE * 10, // 800 nits / 80 = 10.0
            paper_white: Q16_ONE * 3,    // 240 nits / 80 = 3.0
            exposure: Q16_ONE,
        }
    }

    /// Reinhard tone mapping: L_mapped = L / (1 + L)
    pub fn tonemap_reinhard(&self, l: i32) -> i32 {
        // l / (1 + l) in Q16
        let denom = Q16_ONE + l;
        if denom <= 0 {
            return 0;
        }
        ((l as i64 * Q16_ONE as i64) / denom as i64) as i32
    }

    /// Tone map a linear RGB value (Q16 per channel)
    pub fn tonemap_channel(&self, value: i32) -> i32 {
        let exposed = q16_mul(value, self.exposure);
        match self.operator {
            ToneMapOperator::Reinhard => self.tonemap_reinhard(exposed),
            ToneMapOperator::Linear => {
                if exposed > Q16_ONE {
                    Q16_ONE
                } else {
                    exposed
                }
            }
            ToneMapOperator::AcesFit => {
                // Simplified ACES: (x * (2.51x + 0.03)) / (x * (2.43x + 0.59) + 0.14)
                let a = q16_mul(
                    exposed,
                    q16_mul(exposed, q16_from_int(2) + Q16_ONE / 2) + Q16_ONE / 33,
                );
                let b = q16_mul(
                    exposed,
                    q16_mul(exposed, q16_from_int(2) + Q16_ONE * 43 / 100) + Q16_ONE * 59 / 100,
                ) + Q16_ONE * 14 / 100;
                if b <= 0 {
                    return 0;
                }
                let result = ((a as i64 * Q16_ONE as i64) / b as i64) as i32;
                if result > Q16_ONE {
                    Q16_ONE
                } else if result < 0 {
                    0
                } else {
                    result
                }
            }
            ToneMapOperator::Uncharted2 => {
                // Simplified Uncharted 2 filmic curve
                let a_coeff = Q16_ONE * 15 / 100;
                let b_coeff = Q16_ONE * 50 / 100;
                let c_coeff = Q16_ONE * 10 / 100;
                let d_coeff = Q16_ONE * 20 / 100;
                let e_coeff = Q16_ONE / 100; // 0.01
                let f_coeff = Q16_ONE * 30 / 100;

                // f(x) = ((x*(A*x+C*B)+D*E) / (x*(A*x+B)+D*F)) - E/F
                let ax = q16_mul(a_coeff, exposed);
                let num =
                    q16_mul(exposed, ax + q16_mul(c_coeff, b_coeff)) + q16_mul(d_coeff, e_coeff);
                let den = q16_mul(exposed, ax + b_coeff) + q16_mul(d_coeff, f_coeff);
                if den <= 0 {
                    return 0;
                }
                let result = ((num as i64 * Q16_ONE as i64) / den as i64) as i32;
                let ef = ((e_coeff as i64 * Q16_ONE as i64) / f_coeff as i64) as i32;
                let mapped = result - ef;
                if mapped > Q16_ONE {
                    Q16_ONE
                } else if mapped < 0 {
                    0
                } else {
                    mapped
                }
            }
        }
    }
}

/// Color management engine
pub struct ColorManager {
    pub profiles: Vec<IccProfile>,
    pub active_profile_id: u32,
    pub night_light: NightLight,
    pub hdr: HdrSettings,
    pub default_intent: RenderingIntent,
    pub gamut_warning: bool,
    pub next_profile_id: u32,
    pub global_brightness: i32, // Q16: 0..Q16_ONE
    pub global_contrast: i32,   // Q16: Q16_ONE = normal
    pub global_saturation: i32, // Q16: Q16_ONE = normal
}

impl ColorManager {
    const fn new() -> Self {
        ColorManager {
            profiles: Vec::new(),
            active_profile_id: 0,
            night_light: NightLight::new(),
            hdr: HdrSettings::new(),
            default_intent: RenderingIntent::Perceptual,
            gamut_warning: false,
            next_profile_id: 1,
            global_brightness: Q16_ONE,
            global_contrast: Q16_ONE,
            global_saturation: Q16_ONE,
        }
    }

    /// Register a color profile
    pub fn add_profile(&mut self, profile: IccProfile) -> u32 {
        let id = self.next_profile_id;
        self.next_profile_id = self.next_profile_id.saturating_add(1);
        self.profiles.push(profile);
        id
    }

    /// Set the active display profile
    pub fn set_active_profile(&mut self, id: u32) {
        self.active_profile_id = id;
    }

    /// Apply saturation adjustment to a pixel (ARGB u32)
    pub fn adjust_saturation(&self, pixel: u32) -> u32 {
        if self.global_saturation == Q16_ONE {
            return pixel;
        }

        let a = (pixel >> 24) & 0xFF;
        let r = ((pixel >> 16) & 0xFF) as i32;
        let g = ((pixel >> 8) & 0xFF) as i32;
        let b = (pixel & 0xFF) as i32;

        // Luminance (approximate: 0.299R + 0.587G + 0.114B) in Q16
        let lum = (r * 19595 + g * 38470 + b * 7471) >> 16;

        // Interpolate between grayscale and original
        let sat = self.global_saturation;
        let inv = Q16_ONE - sat;
        let nr = (q16_mul(q16_from_int(r), sat) + q16_mul(q16_from_int(lum), inv)) >> 16;
        let ng = (q16_mul(q16_from_int(g), sat) + q16_mul(q16_from_int(lum), inv)) >> 16;
        let nb = (q16_mul(q16_from_int(b), sat) + q16_mul(q16_from_int(lum), inv)) >> 16;

        let nr = if nr < 0 {
            0
        } else if nr > 255 {
            255
        } else {
            nr
        } as u32;
        let ng = if ng < 0 {
            0
        } else if ng > 255 {
            255
        } else {
            ng
        } as u32;
        let nb = if nb < 0 {
            0
        } else if nb > 255 {
            255
        } else {
            nb
        } as u32;

        (a << 24) | (nr << 16) | (ng << 8) | nb
    }

    /// Apply brightness and contrast to a pixel (ARGB u32)
    pub fn adjust_brightness_contrast(&self, pixel: u32) -> u32 {
        if self.global_brightness == Q16_ONE && self.global_contrast == Q16_ONE {
            return pixel;
        }
        let a = (pixel >> 24) & 0xFF;
        let r = ((pixel >> 16) & 0xFF) as i32;
        let g = ((pixel >> 8) & 0xFF) as i32;
        let b = (pixel & 0xFF) as i32;

        // Brightness: multiply
        let r2 = q16_mul(q16_from_int(r), self.global_brightness) >> 16;
        let g2 = q16_mul(q16_from_int(g), self.global_brightness) >> 16;
        let b2 = q16_mul(q16_from_int(b), self.global_brightness) >> 16;

        // Contrast: (channel - 128) * contrast + 128
        let mid = 128;
        let r3 = ((q16_mul(q16_from_int(r2 - mid), self.global_contrast)) >> 16) + mid;
        let g3 = ((q16_mul(q16_from_int(g2 - mid), self.global_contrast)) >> 16) + mid;
        let b3 = ((q16_mul(q16_from_int(b2 - mid), self.global_contrast)) >> 16) + mid;

        let clamp = |v: i32| -> u32 {
            if v < 0 {
                0
            } else if v > 255 {
                255
            } else {
                v as u32
            }
        };
        (a << 24) | (clamp(r3) << 16) | (clamp(g3) << 8) | clamp(b3)
    }
}

static COLOR_MGR: Mutex<ColorManager> = Mutex::new(ColorManager::new());

/// Initialize the color management system
pub fn init() {
    serial_println!("    [color-mgr] Color management initialized (ICC profiles, night light, HDR tone mapping)");
}

/// Enable night light
pub fn enable_night_light(enabled: bool) {
    let mut mgr = COLOR_MGR.lock();
    mgr.night_light.enabled = enabled;
    mgr.night_light.active = enabled;
}

/// Set night light color temperature (2700-6500K)
pub fn set_color_temperature(kelvin: u32) {
    COLOR_MGR.lock().night_light.color_temp_kelvin = kelvin;
}

/// Set night light strength (Q16: 0..Q16_ONE)
pub fn set_night_strength(strength: i32) {
    COLOR_MGR.lock().night_light.strength = strength;
}

/// Apply night light filter to a pixel
pub fn apply_night_filter(pixel: u32) -> u32 {
    COLOR_MGR.lock().night_light.filter_pixel(pixel)
}

/// Enable/disable HDR
pub fn enable_hdr(enabled: bool) {
    COLOR_MGR.lock().hdr.enabled = enabled;
}

/// Set HDR tone mapping operator
pub fn set_tone_map(op: ToneMapOperator) {
    COLOR_MGR.lock().hdr.operator = op;
}

/// Set global brightness (Q16)
pub fn set_brightness(brightness: i32) {
    COLOR_MGR.lock().global_brightness = brightness;
}

/// Set global contrast (Q16)
pub fn set_contrast(contrast: i32) {
    COLOR_MGR.lock().global_contrast = contrast;
}

/// Set global saturation (Q16)
pub fn set_saturation(saturation: i32) {
    COLOR_MGR.lock().global_saturation = saturation;
}

/// Update night light schedule
pub fn update_schedule(current_hour: u8) {
    COLOR_MGR.lock().night_light.update_schedule(current_hour);
}
