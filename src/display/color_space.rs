use crate::sync::Mutex;
/// Color space conversion (sRGB, P3, etc.)
///
/// Part of the Hoags Display Server. Converts pixel data
/// between color spaces for accurate color reproduction.
/// Supports sRGB, Display P3, Adobe RGB, Rec.2020, and linear light.
use alloc::vec::Vec;

/// Supported color space identifiers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    Srgb,
    DisplayP3,
    AdobeRgb,
    Rec2020,
    Linear,
}

/// A 3x3 matrix used for color space conversions (row-major, fixed-point 16.16)
#[derive(Debug, Clone, Copy)]
struct Matrix3x3 {
    m: [[i64; 3]; 3],
}

impl Matrix3x3 {
    /// Multiply this matrix by a column vector [r, g, b] in 16.16 fixed-point
    fn transform(&self, r: i64, g: i64, b: i64) -> (i64, i64, i64) {
        let out_r = (self.m[0][0] * r + self.m[0][1] * g + self.m[0][2] * b) >> 16;
        let out_g = (self.m[1][0] * r + self.m[1][1] * g + self.m[1][2] * b) >> 16;
        let out_b = (self.m[2][0] * r + self.m[2][1] * g + self.m[2][2] * b) >> 16;
        (out_r, out_g, out_b)
    }

    /// Compose two matrices: self * other
    fn multiply(&self, other: &Matrix3x3) -> Matrix3x3 {
        let mut result = Matrix3x3 { m: [[0i64; 3]; 3] };
        for i in 0..3 {
            for j in 0..3 {
                let mut sum: i64 = 0;
                for k in 0..3 {
                    sum += (self.m[i][k] * other.m[k][j]) >> 16;
                }
                result.m[i][j] = sum;
            }
        }
        result
    }
}

/// Identity matrix in 16.16 fixed-point
const IDENTITY: Matrix3x3 = Matrix3x3 {
    m: [[65536, 0, 0], [0, 65536, 0], [0, 0, 65536]],
};

/// sRGB to XYZ D65 matrix (16.16 fixed-point)
const SRGB_TO_XYZ: Matrix3x3 = Matrix3x3 {
    m: [
        [27209, 22768, 12157],
        [14043, 42926, 8567],
        [1276, 7607, 62654],
    ],
};

/// XYZ D65 to sRGB matrix (16.16 fixed-point)
const XYZ_TO_SRGB: Matrix3x3 = Matrix3x3 {
    m: [
        [213930, -109894, -44560],
        [-63108, 130588, -2944],
        [4540, -27536, 103432],
    ],
};

/// Display P3 to XYZ D65 matrix (16.16 fixed-point)
const P3_TO_XYZ: Matrix3x3 = Matrix3x3 {
    m: [
        [31064, 19240, 11840],
        [14570, 42236, 8730],
        [720, 4928, 65888],
    ],
};

/// XYZ D65 to Display P3 matrix (16.16 fixed-point)
const XYZ_TO_P3: Matrix3x3 = Matrix3x3 {
    m: [
        [169344, -79040, -30784],
        [-56896, 132960, -1792],
        [3072, -19968, 87168],
    ],
};

/// Adobe RGB to XYZ D65 matrix (16.16 fixed-point)
const ADOBE_TO_XYZ: Matrix3x3 = Matrix3x3 {
    m: [
        [37950, 19890, 4304],
        [19557, 40730, 5249],
        [1773, 6636, 63127],
    ],
};

/// XYZ to Adobe RGB (16.16 fixed-point)
const XYZ_TO_ADOBE: Matrix3x3 = Matrix3x3 {
    m: [
        [131072, -38912, -8192],
        [-63488, 131072, 4096],
        [2048, -8192, 69632],
    ],
};

/// Rec.2020 to XYZ D65 matrix (16.16 fixed-point)
const REC2020_TO_XYZ: Matrix3x3 = Matrix3x3 {
    m: [
        [41328, 19060, 1756],
        [14760, 46152, 4624],
        [468, 3660, 67408],
    ],
};

/// XYZ to Rec.2020 (16.16 fixed-point)
const XYZ_TO_REC2020: Matrix3x3 = Matrix3x3 {
    m: [
        [107904, -33536, -11776],
        [-32128, 105472, 1408],
        [1152, -5632, 64512],
    ],
};

/// Get the matrix to convert from a color space to XYZ
fn to_xyz_matrix(cs: ColorSpace) -> Matrix3x3 {
    match cs {
        ColorSpace::Srgb => SRGB_TO_XYZ,
        ColorSpace::DisplayP3 => P3_TO_XYZ,
        ColorSpace::AdobeRgb => ADOBE_TO_XYZ,
        ColorSpace::Rec2020 => REC2020_TO_XYZ,
        ColorSpace::Linear => IDENTITY,
    }
}

/// Get the matrix to convert from XYZ to a color space
fn from_xyz_matrix(cs: ColorSpace) -> Matrix3x3 {
    match cs {
        ColorSpace::Srgb => XYZ_TO_SRGB,
        ColorSpace::DisplayP3 => XYZ_TO_P3,
        ColorSpace::AdobeRgb => XYZ_TO_ADOBE,
        ColorSpace::Rec2020 => XYZ_TO_REC2020,
        ColorSpace::Linear => IDENTITY,
    }
}

/// Apply sRGB gamma (approximate using piecewise linear + power curve)
/// Input and output in 0.0..1.0 range represented as f32
fn srgb_gamma_encode(linear: f32) -> f32 {
    if linear <= 0.0031308 {
        linear * 12.92
    } else {
        // Approximate pow(linear, 1/2.4) using sqrt-based approximation
        // 1.055 * linear^(1/2.4) - 0.055
        let sqrt_val = fast_sqrt(linear);
        let cbrt_approx = fast_sqrt(sqrt_val); // ~linear^0.25, close to ^0.4167
        1.055 * cbrt_approx - 0.055
    }
}

/// Remove sRGB gamma to get linear light
fn srgb_gamma_decode(encoded: f32) -> f32 {
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        let base = (encoded + 0.055) / 1.055;
        // Approximate pow(base, 2.4) using base * base * sqrt(base)
        base * base * fast_sqrt(base)
    }
}

/// Fast approximate square root using Newton's method
fn fast_sqrt(val: f32) -> f32 {
    if val <= 0.0 {
        return 0.0;
    }
    // Initial guess
    let mut guess = val;
    // 5 iterations of Newton's method for sqrt
    for _ in 0..5 {
        guess = (guess + val / guess) * 0.5;
    }
    guess
}

/// Clamp a floating-point value to [0.0, 1.0]
fn clamp01(v: f32) -> f32 {
    if v < 0.0 {
        0.0
    } else if v > 1.0 {
        1.0
    } else {
        v
    }
}

pub struct ColorConverter {
    pub source: ColorSpace,
    pub target: ColorSpace,
    combined_matrix: Matrix3x3,
    needs_gamma_decode: bool,
    needs_gamma_encode: bool,
}

impl ColorConverter {
    pub fn new(source: ColorSpace, target: ColorSpace) -> Self {
        // Build combined matrix: from_xyz(target) * to_xyz(source)
        let src_to_xyz = to_xyz_matrix(source);
        let xyz_to_dst = from_xyz_matrix(target);
        let combined_matrix = xyz_to_dst.multiply(&src_to_xyz);

        // sRGB, P3, AdobeRGB all use gamma encoding; Linear and Rec2020 are linear-ish
        let needs_gamma_decode = matches!(
            source,
            ColorSpace::Srgb | ColorSpace::DisplayP3 | ColorSpace::AdobeRgb
        );
        let needs_gamma_encode = matches!(
            target,
            ColorSpace::Srgb | ColorSpace::DisplayP3 | ColorSpace::AdobeRgb
        );

        crate::serial_println!("[color_space] converter: {:?} -> {:?}", source, target);
        Self {
            source,
            target,
            combined_matrix,
            needs_gamma_decode,
            needs_gamma_encode,
        }
    }

    /// Convert a single RGB triplet between color spaces.
    /// Input and output are 0.0..1.0 floating-point per channel.
    pub fn convert(&self, r: f32, g: f32, b: f32) -> (f32, f32, f32) {
        if self.source == self.target {
            return (r, g, b);
        }

        // Step 1: decode gamma to linear light if needed
        let (lr, lg, lb) = if self.needs_gamma_decode {
            (
                srgb_gamma_decode(r),
                srgb_gamma_decode(g),
                srgb_gamma_decode(b),
            )
        } else {
            (r, g, b)
        };

        // Step 2: convert to 16.16 fixed-point and apply combined matrix
        let fr = (lr * 65536.0) as i64;
        let fg = (lg * 65536.0) as i64;
        let fb = (lb * 65536.0) as i64;

        let (xr, xg, xb) = self.combined_matrix.transform(fr, fg, fb);

        // Step 3: convert back to float
        let out_r = xr as f32 / 65536.0;
        let out_g = xg as f32 / 65536.0;
        let out_b = xb as f32 / 65536.0;

        // Step 4: re-encode gamma if the target space uses it
        let (er, eg, eb) = if self.needs_gamma_encode {
            (
                srgb_gamma_encode(clamp01(out_r)),
                srgb_gamma_encode(clamp01(out_g)),
                srgb_gamma_encode(clamp01(out_b)),
            )
        } else {
            (clamp01(out_r), clamp01(out_g), clamp01(out_b))
        };

        (er, eg, eb)
    }

    /// Batch-convert a pixel buffer in place.
    /// Expects RGBA format (4 bytes per pixel). Alpha is preserved.
    pub fn convert_buffer(&self, pixels: &mut [u8]) {
        if self.source == self.target {
            return;
        }
        let pixel_count = pixels.len() / 4;
        for i in 0..pixel_count {
            let base = i * 4;
            let r = pixels[base] as f32 / 255.0;
            let g = pixels[base + 1] as f32 / 255.0;
            let b = pixels[base + 2] as f32 / 255.0;
            // alpha at base+3 is preserved

            let (cr, cg, cb) = self.convert(r, g, b);

            pixels[base] = (clamp01(cr) * 255.0) as u8;
            pixels[base + 1] = (clamp01(cg) * 255.0) as u8;
            pixels[base + 2] = (clamp01(cb) * 255.0) as u8;
        }
        crate::serial_println!(
            "[color_space] converted {} pixels {:?}->{:?}",
            pixel_count,
            self.source,
            self.target
        );
    }
}

/// Active display color profile
static DISPLAY_PROFILE: Mutex<Option<ColorSpace>> = Mutex::new(None);

pub fn init() {
    // Detect display color profile; default to sRGB
    let mut profile = DISPLAY_PROFILE.lock();
    *profile = Some(ColorSpace::Srgb);
    crate::serial_println!("[color_space] initialized, display profile: sRGB");
}

/// Get the current display color profile
pub fn display_profile() -> ColorSpace {
    let profile = DISPLAY_PROFILE.lock();
    profile.unwrap_or(ColorSpace::Srgb)
}

/// Set the display color profile
pub fn set_display_profile(cs: ColorSpace) {
    let mut profile = DISPLAY_PROFILE.lock();
    *profile = Some(cs);
    crate::serial_println!("[color_space] display profile set to {:?}", cs);
}
