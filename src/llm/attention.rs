use crate::sync::Mutex;
use alloc::vec;
/// Multi-head attention with RoPE — built from scratch
///
/// Features:
///   - Scaled dot-product attention in Q16 fixed-point
///   - Rotary Position Embeddings (RoPE) with precomputed sin/cos tables
///   - Multi-head attention with head splitting and recombination
///   - Grouped Query Attention (GQA) for memory efficiency
///   - Causal masking (no peeking at future tokens)
///   - Sliding window attention for long contexts
///   - Attention score computation and stable softmax
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

use super::transformer::{q16_from_int, q16_mul, Q16};

// Q16 constants
const Q16_ONE: Q16 = 65536; // 1.0 in Q16
const Q16_HALF: Q16 = 32768; // 0.5 in Q16
const Q16_NEG_INF: Q16 = i32::MIN / 2; // Large negative for masking

/// Precomputed RoPE frequency tables
struct RopeCache {
    /// cos(m * theta_i) for each position m and half-dim index i
    /// Layout: [m * half_dim + i]
    cos_cache: Vec<Q16>,
    /// sin(m * theta_i), same layout
    sin_cache: Vec<Q16>,
    /// Number of positions precomputed
    max_cached_pos: u32,
    /// Full head dimension (rotary applies to pairs, so half_dim = head_dim/2)
    head_dim: u32,
    /// Base theta value (x100 to avoid floats)
    theta_x100: u32,
}

/// Configuration for the attention engine
#[derive(Clone, Copy)]
pub struct AttentionConfig {
    pub head_dim: u32,
    pub n_heads: u32,
    pub n_kv_heads: u32,
    pub max_seq_len: u32,
    pub rope_theta: u32,
    pub sliding_window: u32, // 0 = full attention
}

/// Per-head attention output buffer
struct HeadOutput {
    data: Vec<Q16>,
}

/// Attention computation engine
struct AttentionEngine {
    rope: RopeCache,
    config: AttentionConfig,
    /// Scratch buffer for attention scores (max_seq_len)
    scores: Vec<Q16>,
    /// Scratch buffer for softmax intermediate values
    softmax_buf: Vec<Q16>,
    /// Per-head output buffers (n_heads * head_dim)
    head_outputs: Vec<Q16>,
    /// GQA group size: how many query heads share one KV head
    gqa_group_size: u32,
    /// Statistics
    total_attention_ops: u64,
    total_tokens_attended: u64,
    max_seq_seen: u32,
}

static ATTENTION: Mutex<Option<AttentionEngine>> = Mutex::new(None);

// =============================================================================
// Trigonometric approximations in Q16 fixed-point
// =============================================================================

/// Reduce angle to [-pi, pi] range in Q16.
/// pi in Q16 ~ 205887 (3.14159 * 65536)
fn angle_reduce(x: Q16) -> Q16 {
    const PI_Q16: i32 = 205887;
    const TWO_PI_Q16: i32 = 411775;
    let mut a = x;
    // Coarse reduction by repeated subtraction/addition
    while a > PI_Q16 {
        a -= TWO_PI_Q16;
    }
    while a < -PI_Q16 {
        a += TWO_PI_Q16;
    }
    a
}

/// Integer cosine approximation (input in Q16 radians, output Q16)
/// Uses 5th-order polynomial for better accuracy:
/// cos(x) ~ 1 - x^2/2 + x^4/24 - x^6/720
fn cos_approx(x: Q16) -> Q16 {
    let x = angle_reduce(x);
    let x2 = q16_mul(x, x);
    let x4 = q16_mul(x2, x2);
    let x6 = q16_mul(x4, x2);
    // Term coefficients:
    // 1.0 = Q16_ONE
    // x^2/2: shift right 1
    // x^4/24: divide by 24
    // x^6/720: divide by 720
    let term2 = x2 >> 1;
    let term4 = x4 / 24;
    let term6 = x6 / 720;
    Q16_ONE - term2 + term4 - term6
}

/// Integer sine approximation (input in Q16 radians, output Q16)
/// sin(x) ~ x - x^3/6 + x^5/120 - x^7/5040
fn sin_approx(x: Q16) -> Q16 {
    let x = angle_reduce(x);
    let x2 = q16_mul(x, x);
    let x3 = q16_mul(x2, x);
    let x5 = q16_mul(x3, x2);
    let x7 = q16_mul(x5, x2);
    let term3 = x3 / 6;
    let term5 = x5 / 120;
    let term7 = x7 / 5040;
    x - term3 + term5 - term7
}

/// Fast exp approximation for softmax (input in Q16, output Q16).
/// For x in roughly [-8, 0], we use a degree-4 polynomial fit.
/// For very negative x, returns 0. For x > 0, clips.
fn exp_approx(x: Q16) -> Q16 {
    // exp(x) for softmax: values are shifted so max is 0
    // So x is always <= 0 after max-subtraction.
    if x < q16_from_int(-10) {
        return 0; // Effectively zero
    }
    if x > q16_from_int(4) {
        // Clip to avoid overflow
        return q16_from_int(55); // exp(4) ~ 54.6
    }
    // Polynomial: exp(x) ~ 1 + x + x^2/2 + x^3/6 + x^4/24
    let x2 = q16_mul(x, x);
    let x3 = q16_mul(x2, x);
    let x4 = q16_mul(x3, x);
    let result = Q16_ONE + x + (x2 >> 1) + (x3 / 6) + (x4 / 24);
    if result < 0 {
        0
    } else {
        result
    }
}

// =============================================================================
// RoPE (Rotary Position Embeddings)
// =============================================================================

impl RopeCache {
    /// Build the RoPE frequency table.
    ///
    /// For dimension pair i (0..head_dim/2):
    ///   theta_i = 1 / (base ^ (2i / head_dim))
    /// For position m:
    ///   cos_cache[m * half_dim + i] = cos(m * theta_i)
    ///   sin_cache[m * half_dim + i] = sin(m * theta_i)
    ///
    /// We approximate theta^(-2i/d) using integer math:
    ///   log(theta) * (2i/d) then exponentiate.
    /// For practical purposes with theta=10000:
    ///   freq_i = m / (theta_base ^ (2i / d))
    /// We compute the divisor as a table lookup.
    fn new(head_dim: u32, max_pos: u32, theta_x100: u32) -> Self {
        let half_dim = head_dim / 2;
        let total = (max_pos as usize) * (half_dim as usize);
        let mut cos_cache = vec![Q16_ONE; total];
        let mut sin_cache = vec![0i32; total];

        // Precompute inverse frequency for each dimension pair
        // inv_freq[i] = 1.0 / (theta ^ (2*i / head_dim))
        // In Q16: we store the frequency divisor
        let mut inv_freq = vec![0i64; half_dim as usize];
        for i in 0..half_dim {
            // Compute theta^(2i/d) using repeated squaring approximation
            // For theta=10000 (theta_x100=1000000):
            //   When i=0: divisor = 1
            //   When i=half_dim-1: divisor = theta
            // Linearly interpolate the exponent in log space
            // divisor = theta^(2i/d) ~ exp((2i/d) * ln(theta))
            // We approximate: divisor grows geometrically from 1 to theta
            //
            // Practical approach: compute step ratio per dimension
            // ratio = theta^(2/d) and divisor[i] = ratio^i
            // Start with divisor = Q16_ONE and multiply by ratio each step

            let exponent_num = 2 * i;
            let exponent_den = head_dim;

            // theta_base = theta_x100 / 100
            let theta_base = (theta_x100 / 100) as i64;

            // Compute theta^(2i/d) via integer power approximation
            // Use: divisor = 1 + (theta - 1) * (2i/d)  for small exponents (linear approx)
            // Or better: use iterative approach
            // For each step of 2/d in the exponent, multiply by theta^(2/d)
            //
            // theta^(2/d) for theta=10000, d=64: 10000^(1/32) ~ 1.318
            // In Q16: 1.318 * 65536 ~ 86378

            // Compute theta^(1/(d/2)) iteratively via Newton's method
            // Simpler: precompute divisor as theta_base^(exponent_num/exponent_den)
            // Using the identity: a^(p/q) via integer log steps

            // Practical fallback: linear interpolation in log space
            // ln(1) = 0, ln(theta) at i=d/2
            // fractional position: f = 2i/d
            // divisor ~ exp(f * ln(theta_base))
            // We approximate exp and ln using integer math

            // Simpler approach that works well:
            // divisor[i] = 1 + (theta_base - 1) * exponent_num / exponent_den
            // This is a linear interpolation that works for small dim but underestimates for large

            // Better: piecewise geometric
            // divisor = product of ratio for each step
            // ratio per step = theta_base^(2/head_dim)
            // We compute ratio once, then iterate

            if exponent_den == 0 {
                inv_freq[i as usize] = Q16_ONE as i64;
                continue;
            }

            // For the frequency computation, we directly compute the angle:
            // angle(m, i) = m / divisor(i)
            // where divisor(i) = theta^(2i/d)
            //
            // We'll store inv_freq[i] as the divisor in Q16.
            // divisor[i] = theta^(2i/d)
            //
            // Approximate via: start with 1, multiply by theta^(2/d) at each step
            // theta^(2/d) with theta=10000, d=64 => 10000^(1/32)
            // We approximate 10000^(1/32) ~ 1.318 => Q16 = 86379

            // Compute theta^(2/d) using iterative sqrt-like approach:
            // theta^(1/2) = sqrt(theta), theta^(1/4) = sqrt(sqrt(theta)), etc.
            // Then combine bits of the exponent.

            // For simplicity and correctness, use the formula directly:
            // divisor_i = 1 + (theta - 1) * i * 2 / d   (linear)
            // Then refine with quadratic correction for geometric growth

            let f_num = exponent_num as i64;
            let f_den = exponent_den as i64;

            // Geometric interpolation: divisor = theta^(f_num/f_den)
            // Use integer power: take theta, raise to f_num, then take f_den-th root
            // This is expensive. Instead, use log-linear:
            //
            // ln(divisor) = (f_num / f_den) * ln(theta)
            // For theta = 10000: ln(10000) ~ 9.21 => Q16 = 603504
            // For theta = 500000 (theta_x100 = 50000000): ln = ~13.12

            // Approximate ln(theta_base) in Q16
            let ln_theta = int_ln_q16(theta_base);
            let ln_divisor = (ln_theta * f_num) / f_den;
            let divisor = int_exp_q16(ln_divisor);
            inv_freq[i as usize] = if divisor > 0 { divisor } else { 1 };
        }

        // Now compute cos/sin for each position and dimension
        for m in 0..max_pos {
            for i in 0..half_dim {
                let idx = (m * half_dim + i) as usize;
                // angle = m / divisor[i], in Q16
                let divisor = inv_freq[i as usize];
                let angle = if divisor > 0 {
                    ((m as i64 * Q16_ONE as i64) / divisor) as Q16
                } else {
                    0
                };
                cos_cache[idx] = cos_approx(angle);
                sin_cache[idx] = sin_approx(angle);
            }
        }

        RopeCache {
            cos_cache,
            sin_cache,
            max_cached_pos: max_pos,
            head_dim,
            theta_x100,
        }
    }

    /// Apply RoPE rotation to a single head's Q or K vector at given position.
    /// Rotates pairs (x[i], x[i + half_dim]) by the precomputed angle.
    fn apply(&self, x: &mut [Q16], pos: u32) {
        let half = self.head_dim as usize / 2;
        if pos >= self.max_cached_pos {
            return; // Position beyond precomputed range
        }
        let base = pos as usize * half;
        if base + half > self.cos_cache.len() || x.len() < self.head_dim as usize {
            return;
        }

        for i in 0..half {
            let x0 = x[i];
            let x1 = x[i + half];
            let cos = self.cos_cache[base + i];
            let sin = self.sin_cache[base + i];
            // 2D rotation: [cos, -sin; sin, cos] * [x0; x1]
            x[i] = q16_mul(x0, cos) - q16_mul(x1, sin);
            x[i + half] = q16_mul(x0, sin) + q16_mul(x1, cos);
        }
    }

    /// Apply RoPE to all heads in a packed QKV buffer.
    /// `packed` has shape [n_heads * head_dim]. Each contiguous head_dim
    /// chunk is one head.
    fn apply_all_heads(&self, packed: &mut [Q16], n_heads: u32, pos: u32) {
        let hd = self.head_dim as usize;
        for h in 0..n_heads as usize {
            let start = h * hd;
            let end = start + hd;
            if end <= packed.len() {
                self.apply(&mut packed[start..end], pos);
            }
        }
    }
}

/// Approximate natural log in Q16 for positive integer input.
/// Returns ln(x) * 65536.
fn int_ln_q16(x: i64) -> i64 {
    if x <= 0 {
        return 0;
    }
    if x == 1 {
        return 0;
    }
    // ln(x) = number_of_bits(x) * ln(2) + correction
    // ln(2) in Q16 = 45426
    const LN2_Q16: i64 = 45426;
    let mut bits = 0i64;
    let mut v = x;
    while v > 1 {
        v >>= 1;
        bits += 1;
    }
    // Rough correction for the fractional part
    let remainder = x - (1i64 << bits);
    let fraction = if bits > 0 {
        (remainder * Q16_ONE as i64) / (1i64 << bits)
    } else {
        0
    };
    // ln(1 + f) ~ f - f^2/2 for small f
    let f_correction = fraction - ((fraction * fraction) >> 17);
    bits * LN2_Q16 + f_correction
}

/// Approximate exp in Q16 for Q16 input.
/// Returns exp(x/65536) * 65536.
fn int_exp_q16(x: i64) -> i64 {
    if x <= 0 {
        return Q16_ONE as i64;
    }
    // exp(x) = 2^(x / ln(2))
    const LN2_Q16: i64 = 45426;
    // power_of_2 = x / ln(2) in Q16
    let power = (x * Q16_ONE as i64) / LN2_Q16;
    let int_part = power >> 16;
    let frac_part = power & 0xFFFF;

    // 2^int_part
    if int_part >= 30 {
        return i64::MAX / 2;
    } // Overflow guard
    let base = 1i64 << int_part;

    // 2^frac ~ 1 + frac * ln(2) / 65536
    let frac_mult = Q16_ONE as i64 + ((frac_part * LN2_Q16) >> 16);
    (base * frac_mult) >> 16
}

// =============================================================================
// Softmax computation
// =============================================================================

/// Compute softmax over `scores[0..len]` in-place.
/// Uses the numerically stable version: subtract max first, then exp, then normalize.
fn softmax_inplace(scores: &mut [Q16], len: usize) {
    if len == 0 {
        return;
    }

    // Find maximum score
    let mut max_score: Q16 = Q16_NEG_INF;
    for i in 0..len {
        if scores[i] > max_score {
            max_score = scores[i];
        }
    }

    // Subtract max and compute exp
    let mut sum_exp: i64 = 0;
    for i in 0..len {
        let shifted = scores[i] - max_score;
        let e = exp_approx(shifted);
        scores[i] = e;
        sum_exp += e as i64;
    }

    // Normalize: each score = score / sum
    if sum_exp > 0 {
        for i in 0..len {
            scores[i] = ((scores[i] as i64 * Q16_ONE as i64) / sum_exp) as Q16;
        }
    }
}

// =============================================================================
// Attention Engine
// =============================================================================

impl AttentionEngine {
    fn new(config: AttentionConfig) -> Self {
        let gqa_group_size = if config.n_kv_heads > 0 {
            config.n_heads / config.n_kv_heads
        } else {
            1
        };

        let max_precompute = config.max_seq_len.min(8192);

        AttentionEngine {
            rope: RopeCache::new(config.head_dim, max_precompute, config.rope_theta),
            config,
            scores: vec![0; config.max_seq_len as usize],
            softmax_buf: vec![0; config.max_seq_len as usize],
            head_outputs: vec![0; (config.n_heads * config.head_dim) as usize],
            gqa_group_size,
            total_attention_ops: 0,
            total_tokens_attended: 0,
            max_seq_seen: 0,
        }
    }

    /// Apply RoPE to a query or key vector for a specific head and position.
    fn apply_rope_single(&self, head_vec: &mut [Q16], pos: u32) {
        self.rope.apply(head_vec, pos);
    }

    /// Apply RoPE to all query heads (packed as [n_heads * head_dim]).
    fn apply_rope_queries(&self, q: &mut [Q16], pos: u32) {
        self.rope.apply_all_heads(q, self.config.n_heads, pos);
    }

    /// Apply RoPE to all KV heads (packed as [n_kv_heads * head_dim]).
    fn apply_rope_keys(&self, k: &mut [Q16], pos: u32) {
        self.rope.apply_all_heads(k, self.config.n_kv_heads, pos);
    }

    /// Compute attention for a single query head against cached KV.
    ///
    /// Arguments:
    /// - `q`: query vector for this head, length = head_dim
    /// - `keys`: cached keys, flat [seq_len * head_dim]
    /// - `values`: cached values, flat [seq_len * head_dim]
    /// - `seq_len`: number of cached positions
    /// - `query_pos`: absolute position of the query (for causal masking)
    /// - `key_positions`: absolute positions of each cached key
    ///
    /// Returns: output vector of length head_dim
    fn attention_single_head(
        &mut self,
        q: &[Q16],
        keys: &[Q16],
        values: &[Q16],
        seq_len: usize,
        query_pos: u32,
        key_positions: &[u32],
    ) -> Vec<Q16> {
        let d = self.config.head_dim as usize;
        if seq_len == 0 || q.len() < d {
            return vec![0; d];
        }

        self.total_attention_ops = self.total_attention_ops.saturating_add(1);

        // Compute scale factor: 1/sqrt(head_dim)
        // For head_dim=64: sqrt(64)=8, so scale = 1/8 = 0.125
        // In Q16: 0.125 * 65536 = 8192
        let scale = isqrt_inv_q16(self.config.head_dim);

        let mut effective_len = 0usize;

        // Compute Q * K^T / sqrt(d) for each cached position
        for t in 0..seq_len {
            // Causal mask: skip future positions
            if key_positions[t] > query_pos {
                self.scores[t] = Q16_NEG_INF;
                continue;
            }

            // Sliding window mask
            if self.config.sliding_window > 0 {
                let window_start = if query_pos >= self.config.sliding_window {
                    query_pos - self.config.sliding_window + 1
                } else {
                    0
                };
                if key_positions[t] < window_start {
                    self.scores[t] = Q16_NEG_INF;
                    continue;
                }
            }

            // Dot product: Q . K[t]
            let key_offset = t * d;
            let mut dot: i64 = 0;
            let key_end = (key_offset + d).min(keys.len());
            for i in 0..d.min(key_end - key_offset) {
                dot += q[i] as i64 * keys[key_offset + i] as i64;
            }
            // Scale: (dot >> 16) gives Q16 dot product, then multiply by scale
            let scaled = ((dot >> 16) * scale as i64) >> 16;
            self.scores[t] = scaled as Q16;
            effective_len = t + 1;
        }

        // Softmax over valid scores
        softmax_inplace(&mut self.scores[..], effective_len);

        // Weighted sum of values
        let mut output = vec![0i32; d];
        for t in 0..effective_len {
            let weight = self.scores[t];
            if weight == 0 {
                continue;
            }
            let val_offset = t * d;
            let val_end = (val_offset + d).min(values.len());
            for i in 0..d.min(val_end - val_offset) {
                output[i] += q16_mul(weight, values[val_offset + i]);
            }
        }

        self.total_tokens_attended += effective_len as u64;
        if effective_len as u32 > self.max_seq_seen {
            self.max_seq_seen = effective_len as u32;
        }

        output
    }

    /// Full multi-head attention for one query position.
    ///
    /// Arguments:
    /// - `q_packed`: all query heads packed [n_heads * head_dim], after RoPE
    /// - `k_cache`: cached keys for the relevant KV heads [seq_len * n_kv_heads * head_dim]
    /// - `v_cache`: cached values, same layout
    /// - `seq_len`: number of cached positions
    /// - `query_pos`: current query position
    /// - `key_positions`: positions of cached keys
    ///
    /// Returns: concatenated output of all heads [n_heads * head_dim]
    fn multi_head_attention(
        &mut self,
        q_packed: &[Q16],
        k_cache: &[Q16],
        v_cache: &[Q16],
        seq_len: usize,
        query_pos: u32,
        key_positions: &[u32],
    ) -> Vec<Q16> {
        let hd = self.config.head_dim as usize;
        let n_heads = self.config.n_heads as usize;
        let n_kv = self.config.n_kv_heads as usize;
        let group = self.gqa_group_size as usize;

        let mut output = vec![0i32; n_heads * hd];

        for h in 0..n_heads {
            // Determine which KV head this query head uses (GQA)
            let kv_head = if group > 0 { h / group } else { h };
            let kv_head = kv_head.min(n_kv.saturating_sub(1));

            // Extract this query head
            let q_start = h * hd;
            let q_end = q_start + hd;
            if q_end > q_packed.len() {
                break;
            }
            let q_head = &q_packed[q_start..q_end];

            // Build key/value slices for this KV head from the cache.
            // Cache layout: for each cached position t, KV heads are interleaved:
            //   k_cache[t * n_kv * hd + kv_head * hd .. + hd]
            let kv_stride = n_kv * hd;

            // Extract per-head keys and values into contiguous buffers
            let mut head_keys = vec![0i32; seq_len * hd];
            let mut head_values = vec![0i32; seq_len * hd];

            for t in 0..seq_len {
                let src_offset = t * kv_stride + kv_head * hd;
                let dst_offset = t * hd;
                for d in 0..hd {
                    if src_offset + d < k_cache.len() {
                        head_keys[dst_offset + d] = k_cache[src_offset + d];
                    }
                    if src_offset + d < v_cache.len() {
                        head_values[dst_offset + d] = v_cache[src_offset + d];
                    }
                }
            }

            // Compute single-head attention
            let head_out = self.attention_single_head(
                q_head,
                &head_keys,
                &head_values,
                seq_len,
                query_pos,
                key_positions,
            );

            // Write into output
            let out_start = h * hd;
            for d in 0..hd.min(head_out.len()) {
                output[out_start + d] = head_out[d];
            }
        }

        output
    }

    /// Compute scaled dot-product attention for one head (no RoPE, no masking).
    /// Useful for cross-attention or simple attention patterns.
    fn scaled_dot_product(&mut self, q: &[Q16], k: &[Q16], v: &[Q16], seq_len: usize) -> Vec<Q16> {
        let d = self.config.head_dim as usize;
        if seq_len == 0 || d == 0 {
            return vec![0; d];
        }

        let scale = isqrt_inv_q16(self.config.head_dim);

        // Q * K^T
        for t in 0..seq_len {
            let k_off = t * d;
            let mut dot: i64 = 0;
            for i in 0..d {
                if k_off + i < k.len() {
                    dot += q[i] as i64 * k[k_off + i] as i64;
                }
            }
            self.scores[t] = (((dot >> 16) * scale as i64) >> 16) as Q16;
        }

        softmax_inplace(&mut self.scores[..], seq_len);

        let mut output = vec![0i32; d];
        for t in 0..seq_len {
            let weight = self.scores[t];
            if weight == 0 {
                continue;
            }
            let v_off = t * d;
            for i in 0..d {
                if v_off + i < v.len() {
                    output[i] += q16_mul(weight, v[v_off + i]);
                }
            }
        }
        output
    }

    /// Get attention statistics
    fn stats(&self) -> (u64, u64, u32) {
        (
            self.total_attention_ops,
            self.total_tokens_attended,
            self.max_seq_seen,
        )
    }
}

/// Compute 1/sqrt(n) in Q16 using integer Newton-Raphson.
/// For head_dim values like 32, 64, 128 this is exact enough.
fn isqrt_inv_q16(n: u32) -> Q16 {
    if n == 0 {
        return Q16_ONE;
    }
    // sqrt via integer Newton-Raphson
    let mut guess = 1u32;
    // Initial guess: half of n's bit width
    let mut tmp = n;
    while tmp > 1 {
        tmp >>= 2;
        guess <<= 1;
    }
    // Refine sqrt
    for _ in 0..8 {
        if guess == 0 {
            break;
        }
        guess = (guess + n / guess) / 2;
    }
    if guess == 0 {
        return Q16_ONE;
    }
    // 1/sqrt(n) = 65536/sqrt(n) in Q16
    (Q16_ONE as u32 / guess) as Q16
}

// =============================================================================
// Public API
// =============================================================================

pub fn init() {
    let config = AttentionConfig {
        head_dim: 64,
        n_heads: 12,
        n_kv_heads: 12,
        max_seq_len: 8192,
        rope_theta: 1_000_000,
        sliding_window: 0,
    };
    let mut a = ATTENTION.lock();
    *a = Some(AttentionEngine::new(config));
    serial_println!("    Attention: RoPE, causal mask, GQA, multi-head softmax ready");
}

/// Initialize with custom configuration
pub fn init_with_config(config: AttentionConfig) {
    let mut a = ATTENTION.lock();
    *a = Some(AttentionEngine::new(config));
}

/// Apply RoPE rotation to a query vector at the given position
pub fn apply_rope(q: &mut [Q16], pos: u32) {
    if let Some(engine) = ATTENTION.lock().as_ref() {
        engine.rope.apply(q, pos);
    }
}

/// Apply RoPE to packed query heads
pub fn apply_rope_queries(q: &mut [Q16], pos: u32) {
    if let Some(engine) = ATTENTION.lock().as_ref() {
        engine.rope.apply_all_heads(q, engine.config.n_heads, pos);
    }
}

/// Apply RoPE to packed key heads
pub fn apply_rope_keys(k: &mut [Q16], pos: u32) {
    if let Some(engine) = ATTENTION.lock().as_ref() {
        engine
            .rope
            .apply_all_heads(k, engine.config.n_kv_heads, pos);
    }
}

/// Compute full multi-head attention for one query position.
/// Returns output of shape [n_heads * head_dim].
pub fn compute_attention(
    q_packed: &[Q16],
    k_cache: &[Q16],
    v_cache: &[Q16],
    seq_len: usize,
    query_pos: u32,
    key_positions: &[u32],
) -> Vec<Q16> {
    if let Some(engine) = ATTENTION.lock().as_mut() {
        engine.multi_head_attention(
            q_packed,
            k_cache,
            v_cache,
            seq_len,
            query_pos,
            key_positions,
        )
    } else {
        Vec::new()
    }
}

/// Get attention engine statistics: (total_ops, total_tokens_attended, max_seq_seen)
pub fn stats() -> (u64, u64, u32) {
    ATTENTION.lock().as_ref().map_or((0, 0, 0), |e| e.stats())
}
