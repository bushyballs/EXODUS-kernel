use crate::sync::Mutex;

/// HDR transfer function
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdrTransfer {
    Sdr,
    Pq,  // Perceptual Quantizer (HDR10, SMPTE ST 2084)
    Hlg, // Hybrid Log-Gamma (HLG, ARIB STD-B67)
}

/// Mastering display metadata (SMPTE ST 2086)
#[derive(Debug, Clone, Copy)]
pub struct MasteringDisplayInfo {
    /// Display primaries in CIE 1931 xy chromaticity (16.16 fixed-point)
    pub red_x: u32,
    pub red_y: u32,
    pub green_x: u32,
    pub green_y: u32,
    pub blue_x: u32,
    pub blue_y: u32,
    pub white_x: u32,
    pub white_y: u32,
    /// Max luminance in cd/m^2 (nits)
    pub max_luminance: u32,
    /// Min luminance in 0.0001 cd/m^2 units
    pub min_luminance: u32,
}

impl MasteringDisplayInfo {
    fn default_sdr() -> Self {
        Self {
            red_x: 42739,   // 0.652 * 65536
            red_y: 22282,   // 0.340 * 65536
            green_x: 19661, // 0.300 * 65536
            green_y: 39322, // 0.600 * 65536
            blue_x: 9830,   // 0.150 * 65536
            blue_y: 3932,   // 0.060 * 65536
            white_x: 20643, // 0.3127 * 65536
            white_y: 21627, // 0.3290 * 65536
            max_luminance: 100,
            min_luminance: 500, // 0.05 nits
        }
    }
}

/// Content light level info (CTA-861.3)
#[derive(Debug, Clone, Copy)]
pub struct ContentLightLevel {
    /// Maximum content light level in cd/m^2
    pub max_cll: u16,
    /// Maximum frame-average light level in cd/m^2
    pub max_fall: u16,
}

/// Tone mapping algorithm selection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToneMapAlgorithm {
    /// Simple Reinhard global operator
    Reinhard,
    /// ACES filmic curve approximation
    AcesFilmic,
    /// Hable/Uncharted 2 filmic curve
    Hable,
    /// Simple clamp (no mapping)
    Clamp,
}

/// HDR display metadata and tone mapper
pub struct HdrManager {
    pub transfer: HdrTransfer,
    pub max_luminance: u32,
    pub min_luminance: u32,
    pub enabled: bool,
    mastering_info: MasteringDisplayInfo,
    content_light: ContentLightLevel,
    tone_map_algo: ToneMapAlgorithm,
    display_max_nits: u32,
    display_min_nits: u32,
    /// Paper white reference level in nits (SDR content maps to this)
    paper_white_nits: u32,
    /// Knee point for Reinhard mapping (fixed-point 8.8)
    knee_point: u32,
}

impl HdrManager {
    pub fn new() -> Self {
        crate::serial_println!("[hdr] manager created, SDR mode");
        Self {
            transfer: HdrTransfer::Sdr,
            max_luminance: 100,
            min_luminance: 0,
            enabled: false,
            mastering_info: MasteringDisplayInfo::default_sdr(),
            content_light: ContentLightLevel {
                max_cll: 100,
                max_fall: 80,
            },
            tone_map_algo: ToneMapAlgorithm::Reinhard,
            display_max_nits: 400,
            display_min_nits: 0,
            paper_white_nits: 200,
            knee_point: 384, // 1.5 in 8.8 fixed-point
        }
    }

    /// Enable HDR with the given transfer function and display capabilities
    pub fn enable(&mut self, transfer: HdrTransfer, max_nits: u32, min_nits: u32) {
        self.transfer = transfer;
        self.display_max_nits = max_nits;
        self.display_min_nits = min_nits;
        self.enabled = true;
        self.max_luminance = max_nits;
        self.min_luminance = min_nits;
        crate::serial_println!(
            "[hdr] enabled: {:?}, display range {}-{} nits",
            transfer,
            min_nits,
            max_nits
        );
    }

    /// Set mastering display metadata for current content
    pub fn set_mastering_info(&mut self, info: MasteringDisplayInfo) {
        self.mastering_info = info;
        crate::serial_println!(
            "[hdr] mastering info updated: max={} nits, min=0.{:04} nits",
            info.max_luminance,
            info.min_luminance
        );
    }

    /// Set content light level info
    pub fn set_content_light_level(&mut self, max_cll: u16, max_fall: u16) {
        self.content_light = ContentLightLevel { max_cll, max_fall };
    }

    /// Select the tone mapping algorithm
    pub fn set_tone_map_algorithm(&mut self, algo: ToneMapAlgorithm) {
        self.tone_map_algo = algo;
        crate::serial_println!("[hdr] tone map algorithm: {:?}", algo);
    }

    /// Apply PQ (SMPTE ST 2084) EOTF to convert PQ-encoded value to linear light.
    /// Input range: 0.0..1.0 (PQ encoded), Output: linear luminance 0..10000 nits.
    fn pq_eotf(&self, pq: f32) -> f32 {
        // PQ constants
        let _m1: f32 = 0.1593017578125;
        let _m2: f32 = 78.84375;
        let _c1: f32 = 0.8359375;
        let _c2: f32 = 18.8515625;
        let _c3: f32 = 18.6875;

        // Approximate using simplified math
        // E = ((max(E'^(1/m2) - c1, 0)) / (c2 - c3 * E'^(1/m2)))^(1/m1)
        // Using piecewise linear approximation for no_std
        let pq_clamped = if pq < 0.0 {
            0.0
        } else if pq > 1.0 {
            1.0
        } else {
            pq
        };

        // Simplified PQ curve approximation
        // The curve is roughly: luminance = 10000 * pq^(2.4 * m2/m1)
        // We use a polynomial approximation instead
        let x = pq_clamped;
        let x2 = x * x;
        let x4 = x2 * x2;
        // Approximate: output_nits ~ 10000 * x^6 (rough PQ shape)
        let approx_linear = x4 * x2 * 10000.0;
        approx_linear
    }

    /// Apply HLG OETF inverse to convert HLG signal to linear light.
    fn hlg_eotf(&self, hlg: f32) -> f32 {
        let clamped = if hlg < 0.0 {
            0.0
        } else if hlg > 1.0 {
            1.0
        } else {
            hlg
        };
        if clamped <= 0.5 {
            // Linear segment: E = (E')^2 / 3
            (clamped * clamped) / 3.0
        } else {
            // Log segment: approximate exp-based curve
            let a = 0.17883277;
            let b = 0.28466892;
            let c_const = 0.55991073;
            // E = (exp((E' - c) / a) + b) / 12
            let diff = clamped - c_const;
            let exp_approx = 1.0 + diff / a + (diff * diff) / (2.0 * a * a);
            (exp_approx + b) / 12.0
        }
    }

    /// Reinhard tone mapping operator.
    /// Maps HDR luminance to [0, 1] display range.
    fn tone_map_reinhard(&self, luminance_nits: f32) -> f32 {
        let l = luminance_nits / self.display_max_nits as f32;
        // Extended Reinhard with white point
        let l_white = self.mastering_info.max_luminance as f32 / self.display_max_nits as f32;
        let numerator = l * (1.0 + l / (l_white * l_white));
        let result = numerator / (1.0 + l);
        if result < 0.0 {
            0.0
        } else if result > 1.0 {
            1.0
        } else {
            result
        }
    }

    /// ACES filmic tone mapping approximation (Narkowicz 2015)
    fn tone_map_aces(&self, luminance_nits: f32) -> f32 {
        let x = luminance_nits / self.display_max_nits as f32;
        let a = 2.51;
        let b = 0.03;
        let c_val = 2.43;
        let d = 0.59;
        let e = 0.14;
        let numerator = x * (a * x + b);
        let denominator = x * (c_val * x + d) + e;
        let result = if denominator > 0.0001 {
            numerator / denominator
        } else {
            0.0
        };
        if result < 0.0 {
            0.0
        } else if result > 1.0 {
            1.0
        } else {
            result
        }
    }

    /// Hable/Uncharted 2 filmic curve
    fn tone_map_hable(&self, luminance_nits: f32) -> f32 {
        let x = luminance_nits / self.display_max_nits as f32;
        // Uncharted 2 curve: ((x*(A*x+C*B)+D*E)/(x*(A*x+B)+D*F))-E/F
        let a = 0.15;
        let b = 0.50;
        let c_val = 0.10;
        let d = 0.20;
        let e = 0.02;
        let f = 0.30;

        let map = |v: f32| -> f32 {
            ((v * (a * v + c_val * b) + d * e) / (v * (a * v + b) + d * f)) - e / f
        };

        let white = self.mastering_info.max_luminance as f32 / self.display_max_nits as f32;
        let result = map(x) / map(white);
        if result < 0.0 {
            0.0
        } else if result > 1.0 {
            1.0
        } else {
            result
        }
    }

    /// Tone map a single RGB triplet from content luminance to display range.
    /// Input: linear scene-referred RGB (0.0..inf).
    /// Output: display-referred RGB (0.0..1.0).
    pub fn tone_map(&self, r: f32, g: f32, b: f32) -> (f32, f32, f32) {
        if !self.enabled || matches!(self.transfer, HdrTransfer::Sdr) {
            // SDR passthrough, just clamp
            let cr = if r < 0.0 {
                0.0
            } else if r > 1.0 {
                1.0
            } else {
                r
            };
            let cg = if g < 0.0 {
                0.0
            } else if g > 1.0 {
                1.0
            } else {
                g
            };
            let cb = if b < 0.0 {
                0.0
            } else if b > 1.0 {
                1.0
            } else {
                b
            };
            return (cr, cg, cb);
        }

        // Convert from transfer function to linear luminance (nits)
        let (lr, lg, lb) = match self.transfer {
            HdrTransfer::Pq => (self.pq_eotf(r), self.pq_eotf(g), self.pq_eotf(b)),
            HdrTransfer::Hlg => {
                let max_nits = self.display_max_nits as f32;
                (
                    self.hlg_eotf(r) * max_nits,
                    self.hlg_eotf(g) * max_nits,
                    self.hlg_eotf(b) * max_nits,
                )
            }
            HdrTransfer::Sdr => (
                r * self.paper_white_nits as f32,
                g * self.paper_white_nits as f32,
                b * self.paper_white_nits as f32,
            ),
        };

        // Compute luminance for ratio-preserving tone mapping
        let luminance = 0.2126 * lr + 0.7152 * lg + 0.0722 * lb;
        if luminance < 0.001 {
            return (0.0, 0.0, 0.0);
        }

        // Apply selected tone mapping operator to luminance
        let mapped_lum = match self.tone_map_algo {
            ToneMapAlgorithm::Reinhard => self.tone_map_reinhard(luminance),
            ToneMapAlgorithm::AcesFilmic => self.tone_map_aces(luminance),
            ToneMapAlgorithm::Hable => self.tone_map_hable(luminance),
            ToneMapAlgorithm::Clamp => {
                let l = luminance / self.display_max_nits as f32;
                if l > 1.0 {
                    1.0
                } else {
                    l
                }
            }
        };

        // Preserve color ratios while applying luminance mapping
        let scale = mapped_lum / (luminance / self.display_max_nits as f32);
        let out_r = (lr / self.display_max_nits as f32) * scale;
        let out_g = (lg / self.display_max_nits as f32) * scale;
        let out_b = (lb / self.display_max_nits as f32) * scale;

        let clamp = |v: f32| {
            if v < 0.0 {
                0.0
            } else if v > 1.0 {
                1.0
            } else {
                v
            }
        };
        (clamp(out_r), clamp(out_g), clamp(out_b))
    }

    /// Tone map a buffer of RGBA pixels in place
    pub fn tone_map_buffer(&self, pixels: &mut [u8]) {
        let pixel_count = pixels.len() / 4;
        for i in 0..pixel_count {
            let base = i * 4;
            let r = pixels[base] as f32 / 255.0;
            let g = pixels[base + 1] as f32 / 255.0;
            let b = pixels[base + 2] as f32 / 255.0;
            let (mr, mg, mb) = self.tone_map(r, g, b);
            pixels[base] = (mr * 255.0) as u8;
            pixels[base + 1] = (mg * 255.0) as u8;
            pixels[base + 2] = (mb * 255.0) as u8;
        }
    }

    /// Check if HDR is currently active
    pub fn is_hdr_active(&self) -> bool {
        self.enabled && !matches!(self.transfer, HdrTransfer::Sdr)
    }
}

static HDR: Mutex<Option<HdrManager>> = Mutex::new(None);

pub fn init() {
    let manager = HdrManager::new();
    let mut h = HDR.lock();
    *h = Some(manager);
    crate::serial_println!("[hdr] subsystem initialized");
}

/// Enable HDR from external code
pub fn enable_hdr(transfer: HdrTransfer, max_nits: u32) {
    let mut h = HDR.lock();
    if let Some(ref mut mgr) = *h {
        mgr.enable(transfer, max_nits, 0);
    }
}
