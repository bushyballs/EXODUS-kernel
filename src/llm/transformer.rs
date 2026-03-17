use crate::sync::Mutex;
/// Transformer decoder — the core neural architecture
///
/// GPT-style causal transformer built from scratch.
/// No external libraries, pure Rust integer/fixed-point math.
///
/// Architecture:
///   - Embedding layer (token + position)
///   - N decoder blocks (attention + FFN + LayerNorm)
///   - RMSNorm (faster than LayerNorm)
///   - SwiGLU activation (better than ReLU/GELU)
///   - Output projection -> logits
///
/// Configurable for any size:
///   - Small:  12 layers, 768 dim, 12 heads  (~125M params)
///   - Medium: 24 layers, 1024 dim, 16 heads (~350M params)
///   - Large:  32 layers, 2048 dim, 32 heads (~1.3B params)
///   - XL:    48 layers, 4096 dim, 64 heads  (~7B params)
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Model configuration
#[derive(Clone, Copy)]
pub struct TransformerConfig {
    pub vocab_size: u32,
    pub dim: u32,            // Hidden dimension (d_model)
    pub n_layers: u32,       // Number of decoder blocks
    pub n_heads: u32,        // Number of attention heads
    pub n_kv_heads: u32,     // KV heads (for GQA — grouped query attention)
    pub head_dim: u32,       // dim / n_heads
    pub ffn_dim: u32,        // Feed-forward hidden dim (typically 4 * dim)
    pub max_seq_len: u32,    // Maximum sequence length
    pub rope_theta: u32,     // RoPE base frequency (x100, e.g., 1000000 = 10000.00)
    pub norm_eps: u32,       // RMSNorm epsilon (x1e8, e.g., 100000 = 1e-5 * 1e8)
    pub use_gqa: bool,       // Grouped Query Attention
    pub sliding_window: u32, // 0 = full attention, >0 = sliding window size
}

/// Fixed-point number: value * 2^-16 (Q16.16 format)
/// This gives us ~4 decimal digits of precision without floats
pub type Q16 = i32;

pub fn q16_mul(a: Q16, b: Q16) -> Q16 {
    ((a as i64 * b as i64) >> 16) as Q16
}

pub fn q16_from_int(x: i32) -> Q16 {
    x << 16
}

pub fn q16_to_int(x: Q16) -> i32 {
    x >> 16
}

/// Weight tensor — stored as quantized i8 with scale factor
#[derive(Clone)]
pub struct WeightTensor {
    pub data: Vec<i8>, // Quantized weights
    pub scale: Q16,    // Dequantization scale
    pub rows: u32,
    pub cols: u32,
}

/// Activation buffer — full precision Q16
#[derive(Clone)]
pub struct ActivationBuffer {
    pub data: Vec<Q16>,
    pub size: u32,
}

/// A single decoder block
pub struct DecoderBlock {
    // Attention weights
    pub wq: WeightTensor, // Query projection
    pub wk: WeightTensor, // Key projection
    pub wv: WeightTensor, // Value projection
    pub wo: WeightTensor, // Output projection
    // FFN weights (SwiGLU: gate * swish(up) then down)
    pub w_gate: WeightTensor,
    pub w_up: WeightTensor,
    pub w_down: WeightTensor,
    // Norms
    pub attn_norm_weight: Vec<Q16>, // RMSNorm for attention
    pub ffn_norm_weight: Vec<Q16>,  // RMSNorm for FFN
}

/// The full transformer model
pub struct Transformer {
    pub config: TransformerConfig,
    pub token_embedding: WeightTensor,
    pub blocks: Vec<DecoderBlock>,
    pub final_norm: Vec<Q16>,
    pub output_proj: WeightTensor, // lm_head
    // Runtime buffers
    pub hidden: ActivationBuffer,
    pub logits: Vec<Q16>,
    // Stats
    pub total_forward_passes: u64,
    pub total_tokens_generated: u64,
}

static TRANSFORMER: Mutex<Option<Transformer>> = Mutex::new(None);

impl WeightTensor {
    pub fn zeros(rows: u32, cols: u32) -> Self {
        WeightTensor {
            data: alloc::vec![0i8; (rows * cols) as usize],
            scale: q16_from_int(1),
            rows,
            cols,
        }
    }

    /// Matrix-vector multiply: output = W * input
    pub fn matvec(&self, input: &[Q16], output: &mut [Q16]) {
        for r in 0..self.rows as usize {
            let mut sum: i64 = 0;
            let row_start = r * self.cols as usize;
            for c in 0..self.cols as usize {
                let w = self.data[row_start + c] as i64;
                let x = input[c] as i64;
                sum += w * x;
            }
            // Apply scale and convert back to Q16
            output[r] = ((sum * self.scale as i64) >> 24) as Q16;
        }
    }
}

impl ActivationBuffer {
    pub fn new(size: u32) -> Self {
        ActivationBuffer {
            data: alloc::vec![0i32; size as usize],
            size,
        }
    }
}

/// RMSNorm: x * rsqrt(mean(x^2) + eps) * weight
pub fn rms_norm(output: &mut [Q16], input: &[Q16], weight: &[Q16], eps: Q16) {
    let n = input.len();
    // Compute mean of squares
    let mut sum_sq: i64 = 0;
    for &x in input {
        sum_sq += (x as i64 * x as i64) >> 16;
    }
    let mean_sq = (sum_sq / n as i64) as Q16;

    // rsqrt approximation: 1/sqrt(x) using Newton-Raphson
    let x = mean_sq + eps;
    let mut rsqrt = q16_from_int(1); // Initial guess
    if x > 0 {
        // Newton iteration: y = y * (3 - x * y * y) / 2
        for _ in 0..4 {
            let y2 = q16_mul(rsqrt, rsqrt);
            let xy2 = q16_mul(x, y2);
            let three = q16_from_int(3);
            let diff = three - xy2;
            rsqrt = q16_mul(rsqrt, diff) >> 1;
        }
    }

    // Apply: output = input * rsqrt * weight
    for i in 0..n {
        let normed = q16_mul(input[i], rsqrt);
        output[i] = q16_mul(normed, weight[i]);
    }
}

/// SwiGLU activation: gate * silu(up)
/// silu(x) = x * sigmoid(x)
pub fn swiglu(output: &mut [Q16], gate: &[Q16], up: &[Q16]) {
    for i in 0..output.len() {
        // sigmoid approximation: 1 / (1 + exp(-x))
        // Using piecewise linear: clamp to [-4,4] then linear approx
        let x = up[i];
        let sigmoid = if x > q16_from_int(4) {
            q16_from_int(1)
        } else if x < q16_from_int(-4) {
            0
        } else {
            // Linear approx: 0.5 + x/8
            (q16_from_int(1) >> 1) + (x >> 3)
        };
        let silu = q16_mul(x, sigmoid);
        output[i] = q16_mul(gate[i], silu);
    }
}

impl Transformer {
    pub fn new(config: TransformerConfig) -> Self {
        let dim = config.dim;
        let ffn = config.ffn_dim;

        let mut blocks = Vec::new();
        for _ in 0..config.n_layers {
            blocks.push(DecoderBlock {
                wq: WeightTensor::zeros(dim, dim),
                wk: WeightTensor::zeros(config.n_kv_heads * config.head_dim, dim),
                wv: WeightTensor::zeros(config.n_kv_heads * config.head_dim, dim),
                wo: WeightTensor::zeros(dim, dim),
                w_gate: WeightTensor::zeros(ffn, dim),
                w_up: WeightTensor::zeros(ffn, dim),
                w_down: WeightTensor::zeros(dim, ffn),
                attn_norm_weight: alloc::vec![q16_from_int(1); dim as usize],
                ffn_norm_weight: alloc::vec![q16_from_int(1); dim as usize],
            });
        }

        Transformer {
            token_embedding: WeightTensor::zeros(config.vocab_size, dim),
            blocks,
            final_norm: alloc::vec![q16_from_int(1); dim as usize],
            output_proj: WeightTensor::zeros(config.vocab_size, dim),
            hidden: ActivationBuffer::new(dim),
            logits: alloc::vec![0; config.vocab_size as usize],
            config,
            total_forward_passes: 0,
            total_tokens_generated: 0,
        }
    }

    /// Forward pass for a single token at position `pos`
    pub fn forward(&mut self, token_id: u32, _pos: u32) {
        self.total_forward_passes = self.total_forward_passes.saturating_add(1);
        let dim = self.config.dim as usize;

        // 1. Token embedding lookup
        let emb_start = token_id as usize * dim;
        let emb_end = emb_start + dim;
        if emb_end <= self.token_embedding.data.len() {
            for i in 0..dim {
                self.hidden.data[i] = self.token_embedding.data[emb_start + i] as Q16
                    * self.token_embedding.scale as Q16;
            }
        }

        // 2. Process through each decoder block
        let mut residual = alloc::vec![0i32; dim];
        for block_idx in 0..self.config.n_layers as usize {
            // Save residual
            residual.copy_from_slice(&self.hidden.data[..dim]);

            // Attention sub-layer (simplified — full impl in attention.rs)
            let mut normed = alloc::vec![0i32; dim];
            rms_norm(
                &mut normed,
                &self.hidden.data[..dim],
                &self.blocks[block_idx].attn_norm_weight,
                self.config.norm_eps as Q16,
            );

            // Q, K, V projections
            let mut q = alloc::vec![0i32; dim];
            let mut k = alloc::vec![0i32; (self.config.n_kv_heads * self.config.head_dim) as usize];
            let mut v = alloc::vec![0i32; (self.config.n_kv_heads * self.config.head_dim) as usize];
            self.blocks[block_idx].wq.matvec(&normed, &mut q);
            self.blocks[block_idx].wk.matvec(&normed, &mut k);
            self.blocks[block_idx].wv.matvec(&normed, &mut v);

            // Attention output (simplified single-head for now)
            let mut attn_out = alloc::vec![0i32; dim];
            self.blocks[block_idx].wo.matvec(&q, &mut attn_out);

            // Residual connection
            for i in 0..dim {
                self.hidden.data[i] = residual[i] + attn_out[i];
            }

            // Save residual for FFN
            residual.copy_from_slice(&self.hidden.data[..dim]);

            // FFN sub-layer
            let mut normed2 = alloc::vec![0i32; dim];
            rms_norm(
                &mut normed2,
                &self.hidden.data[..dim],
                &self.blocks[block_idx].ffn_norm_weight,
                self.config.norm_eps as Q16,
            );

            let ffn_dim = self.config.ffn_dim as usize;
            let mut gate = alloc::vec![0i32; ffn_dim];
            let mut up = alloc::vec![0i32; ffn_dim];
            let mut ffn_out_mid = alloc::vec![0i32; ffn_dim];
            let mut ffn_out = alloc::vec![0i32; dim];

            self.blocks[block_idx].w_gate.matvec(&normed2, &mut gate);
            self.blocks[block_idx].w_up.matvec(&normed2, &mut up);
            swiglu(&mut ffn_out_mid, &gate, &up);
            self.blocks[block_idx]
                .w_down
                .matvec(&ffn_out_mid, &mut ffn_out);

            // Residual connection
            for i in 0..dim {
                self.hidden.data[i] = residual[i] + ffn_out[i];
            }
        }

        // 3. Final norm
        let mut final_hidden = alloc::vec![0i32; dim];
        rms_norm(
            &mut final_hidden,
            &self.hidden.data[..dim],
            &self.final_norm,
            self.config.norm_eps as Q16,
        );

        // 4. Output projection -> logits
        self.output_proj.matvec(&final_hidden, &mut self.logits);
    }

    pub fn param_count(&self) -> u64 {
        let d = self.config.dim as u64;
        let ff = self.config.ffn_dim as u64;
        let v = self.config.vocab_size as u64;
        let n = self.config.n_layers as u64;
        // Embedding + output
        let embed = v * d * 2;
        // Per layer: Q,K,V,O (attention) + gate,up,down (FFN) + 2 norms
        let per_layer = d * d * 4 + d * ff * 3 + d * 2;
        embed + n * per_layer
    }
}

/// Predefined model configurations
pub fn config_small() -> TransformerConfig {
    TransformerConfig {
        vocab_size: 32_000,
        dim: 768,
        n_layers: 12,
        n_heads: 12,
        n_kv_heads: 12,
        head_dim: 64,
        ffn_dim: 3072,
        max_seq_len: 8192,
        rope_theta: 1_000_000,
        norm_eps: 100_000,
        use_gqa: false,
        sliding_window: 0,
    }
}

pub fn config_medium() -> TransformerConfig {
    TransformerConfig {
        vocab_size: 32_000,
        dim: 1024,
        n_layers: 24,
        n_heads: 16,
        n_kv_heads: 8,
        head_dim: 64,
        ffn_dim: 4096,
        max_seq_len: 32_768,
        rope_theta: 1_000_000,
        norm_eps: 100_000,
        use_gqa: true,
        sliding_window: 0,
    }
}

pub fn config_large() -> TransformerConfig {
    TransformerConfig {
        vocab_size: 32_000,
        dim: 2048,
        n_layers: 32,
        n_heads: 32,
        n_kv_heads: 8,
        head_dim: 64,
        ffn_dim: 8192,
        max_seq_len: 65_536,
        rope_theta: 1_000_000,
        norm_eps: 100_000,
        use_gqa: true,
        sliding_window: 4096,
    }
}

pub fn config_xl() -> TransformerConfig {
    TransformerConfig {
        vocab_size: 32_000,
        dim: 4096,
        n_layers: 48,
        n_heads: 64,
        n_kv_heads: 8,
        head_dim: 64,
        ffn_dim: 16384,
        max_seq_len: 131_072,
        rope_theta: 1_000_000,
        norm_eps: 100_000,
        use_gqa: true,
        sliding_window: 8192,
    }
}

fn approx_cross_entropy(logits: &[Q16], target_id: usize) -> Q16 {
    if logits.is_empty() {
        return 0;
    }

    let mut max_l = i32::MIN;
    for &l in logits {
        if l > max_l {
            max_l = l;
        }
    }

    let mut sum_exp: i64 = 0;
    for &l in logits {
        let shifted = l - max_l;
        let exp = (q16_from_int(1) + shifted + (q16_mul(shifted, shifted) >> 1)).max(1);
        sum_exp += exp as i64;
    }

    let target_logit = logits.get(target_id).copied().unwrap_or(0);
    let base = (max_l - target_logit).max(0);
    let norm_term = (sum_exp / q16_from_int(1).max(1) as i64) as Q16;
    base + (norm_term >> 4)
}

/// Run a forward pass and return logits for the supplied token/position.
pub fn forward_logits(token_id: u32, pos: u32) -> Option<Vec<Q16>> {
    let mut t = TRANSFORMER.lock();
    let model = t.as_mut()?;
    model.forward(token_id, pos);
    Some(model.logits.clone())
}

/// Per-step lightweight output-head training update.
///
/// This updates only two rows in the output projection:
/// the target row (positive) and the predicted row (negative).
/// It is intentionally cheap and suitable for on-device incremental learning.
pub fn train_output_projection_step(
    input_token: u32,
    pos: u32,
    target_id: u32,
    lr: Q16,
) -> Option<(Q16, u32)> {
    let mut t = TRANSFORMER.lock();
    let model = t.as_mut()?;
    model.forward(input_token, pos);

    let logits = model.logits.clone();
    let predicted = logits
        .iter()
        .enumerate()
        .max_by_key(|(_, &l)| l)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let loss = approx_cross_entropy(&logits, target_id as usize);

    let rows = model.output_proj.rows as usize;
    let cols = model.output_proj.cols as usize;
    let hidden_len = model.hidden.data.len().min(cols);
    let step = lr.max(1);

    if (target_id as usize) < rows && predicted < rows && target_id as usize != predicted {
        let hidden = model.hidden.data[..hidden_len].to_vec();
        let target_row = target_id as usize * cols;
        let pred_row = predicted * cols;

        for i in 0..hidden_len {
            let delta = ((hidden[i] as i64 * step as i64) >> 24) as i32;
            if delta == 0 {
                continue;
            }

            let t_idx = target_row + i;
            if t_idx < model.output_proj.data.len() {
                let next = (model.output_proj.data[t_idx] as i32 + delta).clamp(-127, 127);
                model.output_proj.data[t_idx] = next as i8;
            }

            let p_idx = pred_row + i;
            if p_idx < model.output_proj.data.len() {
                let next = (model.output_proj.data[p_idx] as i32 - delta).clamp(-127, 127);
                model.output_proj.data[p_idx] = next as i8;
            }
        }
    }

    model.total_tokens_generated += 1;
    Some((loss, predicted as u32))
}

pub fn vocab_size() -> Option<u32> {
    TRANSFORMER
        .lock()
        .as_ref()
        .map(|model| model.config.vocab_size)
}

pub fn init() {
    // Initialize with small config by default (can be upgraded at runtime)
    let mut t = TRANSFORMER.lock();
    let config = config_small();
    *t = Some(Transformer::new(config));
    serial_println!("    Transformer: 12L/768d/12H (~125M params), SwiGLU, RMSNorm ready");
}
