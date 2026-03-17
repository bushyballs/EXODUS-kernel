use crate::sync::Mutex;
use crate::{serial_print, serial_println};
/// Rotary Position Embedding (RoPE)
///
/// Part of the AIOS LLM layer. Implements pre-computed frequency tables
/// and the rotation operation used to inject positional information into
/// query/key vectors inside every attention head.
///
/// The frequencies follow theta_i = base^(-2i/d) where base defaults to
/// 10000 and d is the head dimension. For each sequence position `pos`,
/// we store cos(pos * theta_i) and sin(pos * theta_i). At apply time the
/// even/odd pairs of each head vector are rotated:
///   x'[2i]   = x[2i]*cos - x[2i+1]*sin
///   x'[2i+1] = x[2i]*sin + x[2i+1]*cos
use alloc::vec::Vec;

/// Default base frequency for vanilla RoPE
const ROPE_BASE: f32 = 10000.0;

/// Precomputed RoPE frequency table
pub struct RopeTable {
    /// cos(pos * freq) for each (pos, dim_pair)
    pub cos_cache: Vec<f32>,
    /// sin(pos * freq) for each (pos, dim_pair)
    pub sin_cache: Vec<f32>,
    /// Head dimension (must be even)
    pub dim: usize,
    /// Maximum sequence length the cache covers
    pub max_seq_len: usize,
    /// Base frequency (default 10000)
    pub base: f32,
    /// NTK-aware scaling factor for extended contexts
    pub scaling_factor: f32,
}

// ── Minimal math helpers (no libm) ─────────────────────────────────

/// Fast sine approximation using a 5th-order polynomial (Bhaskara-style).
/// Accepts any f32; internally maps to [-pi, pi].
fn fast_sin(mut x: f32) -> f32 {
    use core::f32::consts::PI;
    const TWO_PI: f32 = 2.0 * PI;

    // Reduce to [-pi, pi]
    x = x % TWO_PI;
    if x > PI {
        x -= TWO_PI;
    } else if x < -PI {
        x += TWO_PI;
    }

    // Polynomial: sin(x) ~ x - x^3/6 + x^5/120
    let x2 = x * x;
    let x3 = x2 * x;
    let x5 = x3 * x2;
    x - x3 / 6.0 + x5 / 120.0
}

/// Fast cosine via sin(x + pi/2)
fn fast_cos(x: f32) -> f32 {
    fast_sin(x + core::f32::consts::FRAC_PI_2)
}

/// Approximate natural log: ln(x) using a rational approximation around 1.
/// For computing base^(-2i/d) we use exp(-2i/d * ln(base)).
fn fast_ln(x: f32) -> f32 {
    // Use the identity: ln(x) = 2 * atanh((x-1)/(x+1))
    // atanh(t) ~ t + t^3/3 + t^5/5 for |t| < 1
    if x <= 0.0 {
        return -1e30; // sentinel for invalid
    }
    // Decompose: x = m * 2^e  =>  ln(x) = e*ln(2) + ln(m)
    let bits = x.to_bits();
    let e = ((bits >> 23) & 0xFF) as i32 - 127;
    let m_bits = (bits & 0x007F_FFFF) | (127 << 23);
    let m = f32::from_bits(m_bits);

    let t = (m - 1.0) / (m + 1.0);
    let t2 = t * t;
    let ln_m = 2.0 * t * (1.0 + t2 / 3.0 + t2 * t2 / 5.0 + t2 * t2 * t2 / 7.0);
    ln_m + (e as f32) * 0.693147180559945 // ln(2)
}

/// Approximate exp(x) via 2^(x / ln2) and bit manipulation
fn fast_exp(x: f32) -> f32 {
    if x > 88.0 {
        return f32::MAX;
    }
    if x < -88.0 {
        return 0.0;
    }
    // exp(x) = 2^(x / ln2)
    let v = x * 1.442695040888963; // 1/ln(2)
                                   // 2^v using the classic bit trick + polynomial refinement
    let floor_v = if v >= 0.0 { v as i32 } else { v as i32 - 1 };
    let frac = v - floor_v as f32;
    // 2^frac ~ 1 + frac*ln2 + (frac*ln2)^2/2 + ...
    let ln2 = 0.693147180559945_f32;
    let t = frac * ln2;
    let t2 = t * t;
    let t3 = t2 * t;
    let exp_frac = 1.0 + t + t2 / 2.0 + t3 / 6.0 + t2 * t2 / 24.0;

    // Construct 2^floor_v by bit manipulation
    if floor_v > 127 {
        return f32::MAX;
    }
    if floor_v < -126 {
        return 0.0;
    }
    let pow2 = f32::from_bits(((floor_v + 127) as u32) << 23);
    pow2 * exp_frac
}

/// Compute base^(-2i/d) = exp(-2i/d * ln(base))
fn inv_freq(i: usize, dim: usize, base: f32) -> f32 {
    let exponent = -2.0 * (i as f32) / (dim as f32);
    fast_exp(exponent * fast_ln(base))
}

impl RopeTable {
    /// Create a new RoPE table with precomputed cos/sin caches.
    ///
    /// `dim` is the per-head dimension (must be even).
    /// `max_seq_len` is the maximum sequence position to pre-cache.
    pub fn new(dim: usize, max_seq_len: usize) -> Self {
        Self::with_base(dim, max_seq_len, ROPE_BASE, 1.0)
    }

    /// Create a RoPE table with custom base and scaling factor.
    /// Scaling factor > 1 enables NTK-aware context extension.
    pub fn with_base(dim: usize, max_seq_len: usize, base: f32, scaling_factor: f32) -> Self {
        let half_dim = dim / 2;
        let effective_base = if scaling_factor > 1.0 {
            // NTK-aware scaling: raise the base frequency
            base * fast_exp(fast_ln(scaling_factor) * (dim as f32) / ((dim as f32) - 2.0))
        } else {
            base
        };

        let total = max_seq_len * half_dim;
        let mut cos_cache = Vec::with_capacity(total);
        let mut sin_cache = Vec::with_capacity(total);

        for pos in 0..max_seq_len {
            for i in 0..half_dim {
                let freq = inv_freq(i, dim, effective_base);
                let angle = (pos as f32) * freq;
                cos_cache.push(fast_cos(angle));
                sin_cache.push(fast_sin(angle));
            }
        }

        serial_println!(
            "    [rope] Precomputed table: dim={}, max_seq={}, base={}, scale={}",
            dim,
            max_seq_len,
            base as u32,
            (scaling_factor * 100.0) as u32
        );

        RopeTable {
            cos_cache,
            sin_cache,
            dim,
            max_seq_len,
            base,
            scaling_factor,
        }
    }

    /// Apply RoPE to query and key vectors at position `pos`.
    ///
    /// Both `q` and `k` must have length equal to `self.dim`.
    /// The rotation is applied in-place by treating consecutive
    /// even/odd element pairs.
    pub fn apply(&self, q: &mut [f32], k: &mut [f32], pos: usize) {
        if pos >= self.max_seq_len {
            return; // Position out of cached range
        }
        let half_dim = self.dim / 2;
        let cache_offset = pos * half_dim;

        // Rotate query
        self.rotate_vec(q, cache_offset, half_dim);

        // Rotate key
        self.rotate_vec(k, cache_offset, half_dim);
    }

    /// Apply RoPE to a single vector (q or k) in-place.
    fn rotate_vec(&self, v: &mut [f32], cache_offset: usize, half_dim: usize) {
        let pairs = half_dim.min(v.len() / 2);
        for i in 0..pairs {
            let cos_val = self.cos_cache[cache_offset + i];
            let sin_val = self.sin_cache[cache_offset + i];
            let x0 = v[2 * i];
            let x1 = v[2 * i + 1];
            v[2 * i] = x0 * cos_val - x1 * sin_val;
            v[2 * i + 1] = x0 * sin_val + x1 * cos_val;
        }
    }

    /// Apply RoPE to a batch of query vectors, each of length `self.dim`,
    /// starting at position `start_pos` (for incremental decoding).
    pub fn apply_batch(&self, qs: &mut [f32], ks: &mut [f32], start_pos: usize, seq_len: usize) {
        let d = self.dim;
        let half_dim = d / 2;
        for t in 0..seq_len {
            let pos = start_pos + t;
            if pos >= self.max_seq_len {
                break;
            }
            let cache_offset = pos * half_dim;
            let vec_offset = t * d;
            if vec_offset + d > qs.len() || vec_offset + d > ks.len() {
                break;
            }
            self.rotate_vec(&mut qs[vec_offset..vec_offset + d], cache_offset, half_dim);
            self.rotate_vec(&mut ks[vec_offset..vec_offset + d], cache_offset, half_dim);
        }
    }

    /// Extend the cache to cover a longer sequence length on the fly.
    pub fn extend_cache(&mut self, new_max: usize) {
        if new_max <= self.max_seq_len {
            return;
        }
        let half_dim = self.dim / 2;
        let effective_base = if self.scaling_factor > 1.0 {
            self.base
                * fast_exp(
                    fast_ln(self.scaling_factor) * (self.dim as f32) / ((self.dim as f32) - 2.0),
                )
        } else {
            self.base
        };
        for pos in self.max_seq_len..new_max {
            for i in 0..half_dim {
                let freq = inv_freq(i, self.dim, effective_base);
                let angle = (pos as f32) * freq;
                self.cos_cache.push(fast_cos(angle));
                self.sin_cache.push(fast_sin(angle));
            }
        }
        self.max_seq_len = new_max;
    }

    /// Return the number of cached positions.
    pub fn cached_positions(&self) -> usize {
        self.max_seq_len
    }
}

// ── Global Singleton ────────────────────────────────────────────────

struct RopeState {
    table: RopeTable,
}

static ROPE: Mutex<Option<RopeState>> = Mutex::new(None);

/// Default head dimension for the built-in model
const DEFAULT_DIM: usize = 64;
/// Default maximum sequence length
const DEFAULT_MAX_SEQ: usize = 4096;

pub fn init() {
    let table = RopeTable::new(DEFAULT_DIM, DEFAULT_MAX_SEQ);
    let mut guard = ROPE.lock();
    *guard = Some(RopeState { table });
    serial_println!(
        "    [rope] RoPE subsystem initialized (dim={}, max_seq={})",
        DEFAULT_DIM,
        DEFAULT_MAX_SEQ
    );
}

/// Apply RoPE from the global singleton.
pub fn apply_global(q: &mut [f32], k: &mut [f32], pos: usize) {
    let guard = ROPE.lock();
    if let Some(state) = guard.as_ref() {
        state.table.apply(q, k, pos);
    }
}
