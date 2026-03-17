use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;

/// Default tile size for the blocked algorithm
const DEFAULT_BLOCK_SIZE: usize = 64;

/// Flash Attention block processor
pub struct FlashAttention {
    /// Tile / block size (tokens per block)
    pub block_size: usize,
    /// Number of attention heads
    pub num_heads: usize,
    /// Dimension of each head (d_k)
    pub head_dim: usize,
    /// Pre-computed 1/sqrt(head_dim) scaling factor
    pub scale: f32,
    /// Whether to apply causal (auto-regressive) masking
    pub causal: bool,
}

// ── Maths helpers (no libm) ────────────────────────────────────────

/// Approximate exp(x) for softmax. Same polynomial as in rope.rs.
fn fast_exp(x: f32) -> f32 {
    if x > 88.0 {
        return f32::MAX;
    }
    if x < -88.0 {
        return 0.0;
    }
    let v = x * 1.442695040888963; // 1/ln2
    let floor_v = if v >= 0.0 { v as i32 } else { v as i32 - 1 };
    let frac = v - floor_v as f32;
    let ln2 = 0.693147180559945_f32;
    let t = frac * ln2;
    let t2 = t * t;
    let t3 = t2 * t;
    let exp_frac = 1.0 + t + t2 / 2.0 + t3 / 6.0 + t2 * t2 / 24.0;
    if floor_v > 127 {
        return f32::MAX;
    }
    if floor_v < -126 {
        return 0.0;
    }
    let pow2 = f32::from_bits(((floor_v + 127) as u32) << 23);
    pow2 * exp_frac
}

/// Approximate 1/sqrt(x) using the fast inverse-square-root trick.
fn fast_inv_sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let half = 0.5 * x;
    let mut i = x.to_bits();
    i = 0x5f37_59df - (i >> 1); // magic constant
    let y = f32::from_bits(i);
    // One Newton-Raphson iteration
    let y = y * (1.5 - half * y * y);
    // Second iteration for better accuracy
    y * (1.5 - half * y * y)
}

impl FlashAttention {
    /// Create a new Flash Attention engine.
    pub fn new(num_heads: usize, head_dim: usize) -> Self {
        Self::with_block_size(num_heads, head_dim, DEFAULT_BLOCK_SIZE, true)
    }

    /// Create with explicit block size and causal flag.
    pub fn with_block_size(
        num_heads: usize,
        head_dim: usize,
        block_size: usize,
        causal: bool,
    ) -> Self {
        let scale = fast_inv_sqrt(head_dim as f32);
        serial_println!(
            "    [flash-attn] heads={}, d_k={}, block={}, causal={}, scale={}",
            num_heads,
            head_dim,
            block_size,
            causal,
            (scale * 1000.0) as i32
        );
        FlashAttention {
            block_size,
            num_heads,
            head_dim,
            scale,
            causal,
        }
    }

    /// Run multi-head flash attention.
    ///
    /// All tensors are flattened: length = seq_len * num_heads * head_dim.
    /// Layout: `[token][head][dim]` (seq-major, head-minor).
    /// `out` is written with the same layout.
    pub fn forward(&self, q: &[f32], k: &[f32], v: &[f32], out: &mut [f32]) {
        let d = self.head_dim;
        let h = self.num_heads;
        let stride = h * d;

        // Derive sequence lengths from tensor sizes
        let seq_q = if stride == 0 { 0 } else { q.len() / stride };
        let seq_k = if stride == 0 { 0 } else { k.len() / stride };
        if seq_q == 0 || seq_k == 0 || out.len() < seq_q * stride {
            return;
        }

        // Process each head independently
        for head in 0..h {
            self.flash_attn_head(q, k, v, out, seq_q, seq_k, head, stride, d);
        }
    }

    /// Flash attention for a single head using the online-softmax
    /// blocked algorithm.
    fn flash_attn_head(
        &self,
        q: &[f32],
        k: &[f32],
        v: &[f32],
        out: &mut [f32],
        seq_q: usize,
        seq_k: usize,
        head: usize,
        stride: usize,
        d: usize,
    ) {
        let bs = self.block_size;

        // Number of blocks along Q and K dimensions
        let q_blocks = (seq_q + bs - 1) / bs;
        let k_blocks = (seq_k + bs - 1) / bs;

        // Temporary storage per query block: running max, running sum,
        // and accumulated output.
        let mut row_max = vec![-1e30_f32; bs];
        let mut row_sum = vec![0.0_f32; bs];
        let mut acc = vec![0.0_f32; bs * d];

        for qb in 0..q_blocks {
            let q_start = qb * bs;
            let q_end = (q_start + bs).min(seq_q);
            let q_len = q_end - q_start;

            // Reset accumulators for this query block
            for i in 0..q_len {
                row_max[i] = -1e30;
                row_sum[i] = 0.0;
            }
            for val in acc[..q_len * d].iter_mut() {
                *val = 0.0;
            }

            for kb in 0..k_blocks {
                let k_start = kb * bs;
                let k_end = (k_start + bs).min(seq_k);
                let k_len = k_end - k_start;

                // ── Compute S = Q_block @ K_block^T * scale ────────
                // S is q_len x k_len
                for qi in 0..q_len {
                    let q_row = q_start + qi;

                    // Find new local max for this row across the K block
                    let mut local_max = row_max[qi];

                    // First pass: compute dot products and find max
                    let mut dots = vec![0.0_f32; k_len];
                    for ki in 0..k_len {
                        let k_row = k_start + ki;

                        // Causal mask: skip future keys
                        if self.causal && k_row > q_row {
                            dots[ki] = -1e30;
                            continue;
                        }

                        // Dot product q[qi] . k[ki]
                        let mut dot = 0.0_f32;
                        let q_off = q_row * stride + head * d;
                        let k_off = k_row * stride + head * d;
                        for j in 0..d {
                            if q_off + j < q.len() && k_off + j < k.len() {
                                dot += q[q_off + j] * k[k_off + j];
                            }
                        }
                        dot *= self.scale;
                        dots[ki] = dot;
                        if dot > local_max {
                            local_max = dot;
                        }
                    }

                    // ── Online softmax correction ──────────────────
                    // If the new block has a higher max, we need to
                    // rescale the previously accumulated values.
                    let old_max = row_max[qi];
                    if local_max > old_max {
                        let correction = fast_exp(old_max - local_max);
                        row_sum[qi] *= correction;
                        // Rescale accumulated output
                        for j in 0..d {
                            acc[qi * d + j] *= correction;
                        }
                        row_max[qi] = local_max;
                    }

                    // ── Accumulate exp(s - max) * V ────────────────
                    for ki in 0..k_len {
                        let p = fast_exp(dots[ki] - row_max[qi]);
                        row_sum[qi] += p;

                        let k_row = k_start + ki;
                        let v_off = k_row * stride + head * d;
                        for j in 0..d {
                            if v_off + j < v.len() {
                                acc[qi * d + j] += p * v[v_off + j];
                            }
                        }
                    }
                }
            }

            // ── Write output: acc / row_sum ────────────────────────
            for qi in 0..q_len {
                let q_row = q_start + qi;
                let o_off = q_row * stride + head * d;
                let inv_sum = if row_sum[qi] > 0.0 {
                    1.0 / row_sum[qi]
                } else {
                    0.0
                };
                for j in 0..d {
                    if o_off + j < out.len() {
                        out[o_off + j] = acc[qi * d + j] * inv_sum;
                    }
                }
            }
        }
    }

    /// Single-query attention (for incremental decoding).
    /// `q` has length `num_heads * head_dim` (one token).
    /// `k`, `v` have length `kv_len * num_heads * head_dim`.
    pub fn forward_single(&self, q: &[f32], k: &[f32], v: &[f32], out: &mut [f32]) {
        let d = self.head_dim;
        let h = self.num_heads;
        let stride = h * d;
        let kv_len = if stride == 0 { 0 } else { k.len() / stride };
        if kv_len == 0 {
            return;
        }

        for head in 0..h {
            let mut max_val = -1e30_f32;
            let mut sum = 0.0_f32;

            // First pass: find max score
            for ki in 0..kv_len {
                let mut dot = 0.0_f32;
                let q_off = head * d;
                let k_off = ki * stride + head * d;
                for j in 0..d {
                    if q_off + j < q.len() && k_off + j < k.len() {
                        dot += q[q_off + j] * k[k_off + j];
                    }
                }
                dot *= self.scale;
                if dot > max_val {
                    max_val = dot;
                }
            }

            // Second pass: accumulate softmax * V
            let o_off = head * d;
            for j in 0..d {
                if o_off + j < out.len() {
                    out[o_off + j] = 0.0;
                }
            }

            for ki in 0..kv_len {
                let mut dot = 0.0_f32;
                let q_off = head * d;
                let k_off = ki * stride + head * d;
                for j in 0..d {
                    if q_off + j < q.len() && k_off + j < k.len() {
                        dot += q[q_off + j] * k[k_off + j];
                    }
                }
                dot *= self.scale;
                let p = fast_exp(dot - max_val);
                sum += p;

                let v_off = ki * stride + head * d;
                for j in 0..d {
                    if o_off + j < out.len() && v_off + j < v.len() {
                        out[o_off + j] += p * v[v_off + j];
                    }
                }
            }

            // Normalise
            let inv_sum = if sum > 0.0 { 1.0 / sum } else { 0.0 };
            for j in 0..d {
                if o_off + j < out.len() {
                    out[o_off + j] *= inv_sum;
                }
            }
        }
    }
}

// ── Global Singleton ────────────────────────────────────────────────

struct FlashAttnState {
    engine: FlashAttention,
}

static FLASH_ATTN: Mutex<Option<FlashAttnState>> = Mutex::new(None);

const DEFAULT_HEADS: usize = 8;
const DEFAULT_HEAD_DIM: usize = 64;

pub fn init() {
    let engine = FlashAttention::new(DEFAULT_HEADS, DEFAULT_HEAD_DIM);
    let mut guard = FLASH_ATTN.lock();
    *guard = Some(FlashAttnState { engine });
    serial_println!(
        "    [flash-attn] Subsystem initialised (heads={}, d_k={}, block={})",
        DEFAULT_HEADS,
        DEFAULT_HEAD_DIM,
        DEFAULT_BLOCK_SIZE
    );
}

/// Run flash attention from the global singleton.
pub fn forward_global(q: &[f32], k: &[f32], v: &[f32], out: &mut [f32]) {
    let guard = FLASH_ATTN.lock();
    if let Some(state) = guard.as_ref() {
        state.engine.forward(q, k, v, out);
    }
}
