use crate::sync::Mutex;
use alloc::string::String;
/// Neural network builder for Genesis ML runtime
///
/// Provides a graph-based neural network construction API with support for
/// dense, convolutional, LSTM, and attention layers. Includes forward and
/// backward pass execution, weight initialization strategies, and activation
/// functions — all using Q16 fixed-point arithmetic.
///
/// Inspired by: PyTorch nn.Module, Keras Sequential/Functional. All code is original.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 fixed-point constants and helpers
// ---------------------------------------------------------------------------

const Q16_ONE: i32 = 65536;
const Q16_HALF: i32 = 32768;
const Q16_ZERO: i32 = 0;
const Q16_NEG_ONE: i32 = -65536;

/// Multiply two Q16 values: (a * b) >> 16
fn q16_mul(a: i32, b: i32) -> i32 {
    (((a as i64) * (b as i64)) >> 16) as i32
}

/// Divide two Q16 values: (a << 16) / b
fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << 16) / (b as i64)) as i32
}

/// Convert integer to Q16
fn q16_from_int(x: i32) -> i32 {
    x << 16
}

/// Approximate Q16 square root via Newton's method
fn q16_sqrt(x: i32) -> i32 {
    if x <= 0 {
        return 0;
    }
    let mut guess = x >> 1;
    if guess == 0 {
        guess = Q16_ONE;
    }
    for _ in 0..12 {
        if guess == 0 {
            return 0;
        }
        guess = (guess + q16_div(x, guess)) >> 1;
    }
    guess
}

/// Q16 approximate exp via polynomial: 1 + x + x^2/2 + x^3/6 (clamped)
fn q16_exp(x: i32) -> i32 {
    if x > q16_from_int(10) {
        return q16_from_int(20000);
    }
    if x < q16_from_int(-10) {
        return 0;
    }
    let x2 = q16_mul(x, x);
    let x3 = q16_mul(x2, x);
    let term2 = q16_div(x2, q16_from_int(2));
    let term3 = q16_div(x3, q16_from_int(6));
    let result = Q16_ONE + x + term2 + term3;
    if result < 0 {
        0
    } else {
        result
    }
}

/// Q16 approximate tanh via (exp(2x)-1)/(exp(2x)+1)
fn q16_tanh(x: i32) -> i32 {
    let two_x = x << 1;
    let e = q16_exp(two_x);
    let num = e - Q16_ONE;
    let den = e + Q16_ONE;
    if den == 0 {
        return 0;
    }
    q16_div(num, den)
}

/// Q16 sigmoid: 1/(1+exp(-x))
fn q16_sigmoid(x: i32) -> i32 {
    let neg_x = -x;
    let e = q16_exp(neg_x);
    let den = Q16_ONE + e;
    if den == 0 {
        return Q16_HALF;
    }
    q16_div(Q16_ONE, den)
}

// ---------------------------------------------------------------------------
// Activation functions
// ---------------------------------------------------------------------------

/// Supported activation functions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    None,
    ReLU,
    Sigmoid,
    Tanh,
    LeakyReLU,
    Swish,
    GELU,
    Softmax,
}

/// Apply activation to a single Q16 value (non-softmax)
fn apply_activation_scalar(x: i32, act: Activation) -> i32 {
    match act {
        Activation::None => x,
        Activation::ReLU => {
            if x > 0 {
                x
            } else {
                0
            }
        }
        Activation::Sigmoid => q16_sigmoid(x),
        Activation::Tanh => q16_tanh(x),
        Activation::LeakyReLU => {
            if x > 0 {
                x
            } else {
                q16_mul(x, Q16_ONE / 100)
            }
        }
        Activation::Swish => q16_mul(x, q16_sigmoid(x)),
        Activation::GELU => {
            // Approximate: x * sigmoid(1.702 * x)
            let coeff = 111543; // 1.702 in Q16
            let sx = q16_sigmoid(q16_mul(coeff, x));
            q16_mul(x, sx)
        }
        Activation::Softmax => x, // handled separately on vectors
    }
}

/// Apply activation to an entire vector
pub fn apply_activation(data: &mut Vec<i32>, act: Activation) {
    if act == Activation::Softmax {
        apply_softmax(data);
        return;
    }
    for v in data.iter_mut() {
        *v = apply_activation_scalar(*v, act);
    }
}

/// Softmax on Q16 vector (in-place)
fn apply_softmax(data: &mut Vec<i32>) {
    if data.is_empty() {
        return;
    }
    let max_val = *data.iter().max().unwrap_or(&0);
    let mut exps = Vec::with_capacity(data.len());
    let mut sum: i64 = 0;
    for &v in data.iter() {
        let e = q16_exp(v - max_val);
        exps.push(e);
        sum += e as i64;
    }
    if sum == 0 {
        sum = 1;
    }
    for (i, e) in exps.iter().enumerate() {
        data[i] = ((((*e) as i64) << 16) / sum) as i32;
    }
}

// ---------------------------------------------------------------------------
// Weight initialization
// ---------------------------------------------------------------------------

/// Weight initialization strategies
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeightInit {
    Zeros,
    Ones,
    Uniform,
    XavierUniform,
    HeNormal,
    LeCunNormal,
    Orthogonal,
}

/// Simple PRNG (xorshift32) for weight init
struct Rng {
    state: u32,
}

impl Rng {
    const fn new(seed: u32) -> Self {
        Rng {
            state: if seed == 0 { 0xDEAD_BEEF } else { seed },
        }
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    /// Return a Q16 value in [-range, +range]
    fn next_q16_range(&mut self, range: i32) -> i32 {
        let r = self.next_u32();
        let normalized = (r % 65536) as i32 - 32768; // [-32768, 32767]
        q16_mul(normalized * 2, range) >> 16
    }
}

static RNG: Mutex<Rng> = Mutex::new(Rng::new(0xABCD_1234));

/// Generate initial weights for a given shape
pub fn init_weights(shape: &[usize], strategy: WeightInit) -> Vec<i32> {
    let total: usize = shape.iter().product();
    if total == 0 {
        return Vec::new();
    }

    let mut rng = RNG.lock();
    match strategy {
        WeightInit::Zeros => alloc::vec![Q16_ZERO; total],
        WeightInit::Ones => alloc::vec![Q16_ONE; total],
        WeightInit::Uniform => {
            let mut w = Vec::with_capacity(total);
            for _ in 0..total {
                w.push(rng.next_q16_range(Q16_ONE));
            }
            w
        }
        WeightInit::XavierUniform => {
            let fan = if shape.len() >= 2 {
                shape[0] + shape[1]
            } else {
                shape[0]
            };
            let limit = q16_sqrt(q16_div(q16_from_int(6), q16_from_int(fan as i32)));
            let mut w = Vec::with_capacity(total);
            for _ in 0..total {
                w.push(rng.next_q16_range(limit));
            }
            w
        }
        WeightInit::HeNormal => {
            let fan_in = if shape.len() >= 2 { shape[0] } else { 1 };
            let std_dev = q16_sqrt(q16_div(q16_from_int(2), q16_from_int(fan_in as i32)));
            let mut w = Vec::with_capacity(total);
            for _ in 0..total {
                w.push(q16_mul(rng.next_q16_range(Q16_ONE), std_dev));
            }
            w
        }
        WeightInit::LeCunNormal => {
            let fan_in = if shape.len() >= 2 { shape[0] } else { 1 };
            let std_dev = q16_sqrt(q16_div(Q16_ONE, q16_from_int(fan_in as i32)));
            let mut w = Vec::with_capacity(total);
            for _ in 0..total {
                w.push(q16_mul(rng.next_q16_range(Q16_ONE), std_dev));
            }
            w
        }
        WeightInit::Orthogonal => {
            // Approximate orthogonal init with scaled random values
            let scale = if shape.len() >= 2 {
                q16_sqrt(q16_div(Q16_ONE, q16_from_int(shape[1] as i32)))
            } else {
                Q16_ONE
            };
            let mut w = Vec::with_capacity(total);
            for _ in 0..total {
                w.push(q16_mul(rng.next_q16_range(Q16_ONE), scale));
            }
            w
        }
    }
}

// ---------------------------------------------------------------------------
// Layer definitions
// ---------------------------------------------------------------------------

/// Configuration for a neural network layer
#[derive(Debug, Clone)]
pub enum LayerConfig {
    Dense {
        in_features: usize,
        out_features: usize,
        activation: Activation,
        use_bias: bool,
    },
    Conv1d {
        in_channels: usize,
        out_channels: usize,
        kernel_size: usize,
        stride: usize,
        padding: usize,
        activation: Activation,
    },
    Conv2d {
        in_channels: usize,
        out_channels: usize,
        kernel_h: usize,
        kernel_w: usize,
        stride: usize,
        padding: usize,
        activation: Activation,
    },
    LSTM {
        input_size: usize,
        hidden_size: usize,
        num_layers: usize,
        bidirectional: bool,
    },
    Attention {
        embed_dim: usize,
        num_heads: usize,
        dropout_rate: i32, // Q16 fraction
    },
    BatchNorm {
        features: usize,
        momentum: i32, // Q16
        epsilon: i32,  // Q16
    },
    LayerNorm {
        features: usize,
        epsilon: i32, // Q16
    },
    Dropout {
        rate: i32, // Q16 fraction
    },
    Flatten,
    Reshape {
        target_shape: Vec<usize>,
    },
}

/// A single layer with its weights and configuration
pub struct NetLayer {
    pub config: LayerConfig,
    pub name: String,
    pub weights: Vec<i32>,
    pub bias: Vec<i32>,
    /// Gradient storage for backward pass
    pub weight_grad: Vec<i32>,
    pub bias_grad: Vec<i32>,
    /// Cached input for backward pass
    pub cached_input: Vec<i32>,
    /// Cached output (pre-activation) for backward pass
    pub cached_pre_act: Vec<i32>,
}

impl NetLayer {
    pub fn new(name: &str, config: LayerConfig, init: WeightInit) -> Self {
        let (w_shape, b_size) = match &config {
            LayerConfig::Dense {
                in_features,
                out_features,
                use_bias,
                ..
            } => (
                alloc::vec![*in_features, *out_features],
                if *use_bias { *out_features } else { 0 },
            ),
            LayerConfig::Conv1d {
                in_channels,
                out_channels,
                kernel_size,
                ..
            } => (
                alloc::vec![*out_channels, *in_channels * *kernel_size],
                *out_channels,
            ),
            LayerConfig::Conv2d {
                in_channels,
                out_channels,
                kernel_h,
                kernel_w,
                ..
            } => (
                alloc::vec![*out_channels, *in_channels * *kernel_h * *kernel_w],
                *out_channels,
            ),
            LayerConfig::LSTM {
                input_size,
                hidden_size,
                ..
            } => {
                // 4 gates: input, forget, cell, output
                let gate_size = 4 * *hidden_size;
                (
                    alloc::vec![*input_size + *hidden_size, gate_size],
                    gate_size,
                )
            }
            LayerConfig::Attention {
                embed_dim,
                num_heads,
                ..
            } => {
                let _ = num_heads;
                // Q, K, V projection matrices + output projection
                (alloc::vec![*embed_dim, *embed_dim * 4], *embed_dim * 4)
            }
            LayerConfig::BatchNorm { features, .. } => {
                (alloc::vec![*features], *features) // gamma and beta
            }
            LayerConfig::LayerNorm { features, .. } => (alloc::vec![*features], *features),
            _ => (alloc::vec![], 0),
        };

        let w_total: usize = w_shape.iter().product();
        let weights = if w_total > 0 {
            init_weights(&w_shape, init)
        } else {
            Vec::new()
        };
        let bias = if b_size > 0 {
            alloc::vec![Q16_ZERO; b_size]
        } else {
            Vec::new()
        };
        let weight_grad = alloc::vec![Q16_ZERO; w_total];
        let bias_grad = alloc::vec![Q16_ZERO; b_size];

        NetLayer {
            config,
            name: String::from(name),
            weights,
            bias,
            weight_grad,
            bias_grad,
            cached_input: Vec::new(),
            cached_pre_act: Vec::new(),
        }
    }

    /// Number of trainable parameters in this layer
    pub fn param_count(&self) -> usize {
        self.weights.len() + self.bias.len()
    }
}

// ---------------------------------------------------------------------------
// Forward pass implementations
// ---------------------------------------------------------------------------

/// Dense (fully connected) forward pass
fn forward_dense(
    input: &[i32],
    weights: &[i32],
    bias: &[i32],
    in_f: usize,
    out_f: usize,
) -> Vec<i32> {
    let batch = input.len() / in_f;
    let mut output = alloc::vec![Q16_ZERO; batch * out_f];
    for b in 0..batch {
        for o in 0..out_f {
            let mut sum: i64 = 0;
            for i in 0..in_f {
                sum += (input[b * in_f + i] as i64) * (weights[i * out_f + o] as i64);
            }
            let mut val = (sum >> 16) as i32;
            if !bias.is_empty() {
                val += bias[o];
            }
            output[b * out_f + o] = val;
        }
    }
    output
}

/// 1D convolution forward pass
fn forward_conv1d(
    input: &[i32],
    weights: &[i32],
    bias: &[i32],
    in_ch: usize,
    out_ch: usize,
    kernel: usize,
    stride: usize,
    padding: usize,
    in_len: usize,
) -> Vec<i32> {
    let padded_len = in_len + 2 * padding;
    let out_len = (padded_len - kernel) / stride + 1;
    let mut output = alloc::vec![Q16_ZERO; out_ch * out_len];

    for oc in 0..out_ch {
        for pos in 0..out_len {
            let mut sum: i64 = 0;
            for ic in 0..in_ch {
                for k in 0..kernel {
                    let in_pos = pos * stride + k;
                    let in_val = if in_pos >= padding && in_pos < padding + in_len {
                        input[ic * in_len + (in_pos - padding)]
                    } else {
                        Q16_ZERO
                    };
                    let w_idx = oc * (in_ch * kernel) + ic * kernel + k;
                    sum += (in_val as i64) * (weights[w_idx] as i64);
                }
            }
            let mut val = (sum >> 16) as i32;
            if !bias.is_empty() {
                val += bias[oc];
            }
            output[oc * out_len + pos] = val;
        }
    }
    output
}

/// 2D convolution forward pass
fn forward_conv2d(
    input: &[i32],
    weights: &[i32],
    bias: &[i32],
    in_ch: usize,
    out_ch: usize,
    kh: usize,
    kw: usize,
    stride: usize,
    padding: usize,
    in_h: usize,
    in_w: usize,
) -> Vec<i32> {
    let out_h = (in_h + 2 * padding - kh) / stride + 1;
    let out_w = (in_w + 2 * padding - kw) / stride + 1;
    let mut output = alloc::vec![Q16_ZERO; out_ch * out_h * out_w];

    for oc in 0..out_ch {
        for oh in 0..out_h {
            for ow in 0..out_w {
                let mut sum: i64 = 0;
                for ic in 0..in_ch {
                    for fh in 0..kh {
                        for fw in 0..kw {
                            let ih = oh * stride + fh;
                            let iw = ow * stride + fw;
                            let in_val = if ih >= padding
                                && ih < padding + in_h
                                && iw >= padding
                                && iw < padding + in_w
                            {
                                input[ic * in_h * in_w + (ih - padding) * in_w + (iw - padding)]
                            } else {
                                Q16_ZERO
                            };
                            let w_idx = oc * (in_ch * kh * kw) + ic * (kh * kw) + fh * kw + fw;
                            sum += (in_val as i64) * (weights[w_idx] as i64);
                        }
                    }
                }
                let mut val = (sum >> 16) as i32;
                if !bias.is_empty() {
                    val += bias[oc];
                }
                output[oc * out_h * out_w + oh * out_w + ow] = val;
            }
        }
    }
    output
}

/// LSTM forward pass for a single time step
fn forward_lstm_step(
    input: &[i32],
    h_prev: &[i32],
    c_prev: &[i32],
    weights: &[i32],
    bias: &[i32],
    input_size: usize,
    hidden_size: usize,
) -> (Vec<i32>, Vec<i32>) {
    let gate_size = 4 * hidden_size;
    let _combined_size = input_size + hidden_size;
    let mut gates = alloc::vec![Q16_ZERO; gate_size];

    // Compute all 4 gates: [i, f, g, o]
    for g in 0..gate_size {
        let mut sum: i64 = 0;
        for j in 0..input_size {
            sum += (input[j] as i64) * (weights[j * gate_size + g] as i64);
        }
        for j in 0..hidden_size {
            let w_idx = (input_size + j) * gate_size + g;
            if w_idx < weights.len() {
                sum += (h_prev[j] as i64) * (weights[w_idx] as i64);
            }
        }
        gates[g] = (sum >> 16) as i32;
        if !bias.is_empty() && g < bias.len() {
            gates[g] += bias[g];
        }
    }

    // Split into 4 gates and apply activations
    let mut h_new = alloc::vec![Q16_ZERO; hidden_size];
    let mut c_new = alloc::vec![Q16_ZERO; hidden_size];

    for j in 0..hidden_size {
        let i_gate = q16_sigmoid(gates[j]);
        let f_gate = q16_sigmoid(gates[hidden_size + j]);
        let g_gate = q16_tanh(gates[2 * hidden_size + j]);
        let o_gate = q16_sigmoid(gates[3 * hidden_size + j]);

        // c_new = f * c_prev + i * g
        c_new[j] = q16_mul(f_gate, c_prev[j]) + q16_mul(i_gate, g_gate);
        // h_new = o * tanh(c_new)
        h_new[j] = q16_mul(o_gate, q16_tanh(c_new[j]));
    }

    (h_new, c_new)
}

/// Single-head attention forward pass
fn forward_attention(
    input: &[i32],
    weights: &[i32],
    _bias: &[i32],
    embed_dim: usize,
    num_heads: usize,
    seq_len: usize,
) -> Vec<i32> {
    let head_dim = embed_dim / num_heads;
    if head_dim == 0 {
        return input.to_vec();
    }

    // Q, K, V projections (simplified: weights laid out as [embed_dim, embed_dim*3])
    let mut q_proj = alloc::vec![Q16_ZERO; seq_len * embed_dim];
    let mut k_proj = alloc::vec![Q16_ZERO; seq_len * embed_dim];
    let mut v_proj = alloc::vec![Q16_ZERO; seq_len * embed_dim];

    for s in 0..seq_len {
        for d in 0..embed_dim {
            let mut sq: i64 = 0;
            let mut sk: i64 = 0;
            let mut sv: i64 = 0;
            for e in 0..embed_dim {
                let in_val = input[s * embed_dim + e] as i64;
                sq += in_val * (weights[e * embed_dim * 4 + d] as i64);
                sk += in_val * (weights[e * embed_dim * 4 + embed_dim + d] as i64);
                sv += in_val * (weights[e * embed_dim * 4 + 2 * embed_dim + d] as i64);
            }
            q_proj[s * embed_dim + d] = (sq >> 16) as i32;
            k_proj[s * embed_dim + d] = (sk >> 16) as i32;
            v_proj[s * embed_dim + d] = (sv >> 16) as i32;
        }
    }

    // Scaled dot-product attention per head
    let scale = q16_sqrt(q16_from_int(head_dim as i32));
    let mut output = alloc::vec![Q16_ZERO; seq_len * embed_dim];

    for h in 0..num_heads {
        let offset = h * head_dim;
        // Attention scores: Q * K^T / sqrt(d_k)
        let mut scores = alloc::vec![Q16_ZERO; seq_len * seq_len];
        for i in 0..seq_len {
            for j in 0..seq_len {
                let mut dot: i64 = 0;
                for d in 0..head_dim {
                    dot += (q_proj[i * embed_dim + offset + d] as i64)
                        * (k_proj[j * embed_dim + offset + d] as i64);
                }
                let raw = (dot >> 16) as i32;
                scores[i * seq_len + j] = if scale != 0 { q16_div(raw, scale) } else { raw };
            }
            // Softmax across j for each i
            let start = i * seq_len;
            let end = start + seq_len;
            let mut row: Vec<i32> = scores[start..end].to_vec();
            apply_softmax(&mut row);
            scores[start..end].copy_from_slice(&row);
        }

        // Weighted sum of V
        for i in 0..seq_len {
            for d in 0..head_dim {
                let mut sum: i64 = 0;
                for j in 0..seq_len {
                    sum += (scores[i * seq_len + j] as i64)
                        * (v_proj[j * embed_dim + offset + d] as i64);
                }
                output[i * embed_dim + offset + d] = (sum >> 16) as i32;
            }
        }
    }

    // Output projection (last embed_dim columns of weight matrix)
    let mut proj_out = alloc::vec![Q16_ZERO; seq_len * embed_dim];
    for s in 0..seq_len {
        for d in 0..embed_dim {
            let mut sum: i64 = 0;
            for e in 0..embed_dim {
                let w_idx = e * embed_dim * 4 + 3 * embed_dim + d;
                if w_idx < weights.len() {
                    sum += (output[s * embed_dim + e] as i64) * (weights[w_idx] as i64);
                }
            }
            proj_out[s * embed_dim + d] = (sum >> 16) as i32;
        }
    }
    proj_out
}

/// Backward pass for dense layer — returns (input_grad, weight_grad, bias_grad)
fn backward_dense(
    output_grad: &[i32],
    cached_input: &[i32],
    weights: &[i32],
    in_f: usize,
    out_f: usize,
) -> (Vec<i32>, Vec<i32>, Vec<i32>) {
    let batch = cached_input.len() / in_f;
    let mut input_grad = alloc::vec![Q16_ZERO; batch * in_f];
    let mut weight_grad = alloc::vec![Q16_ZERO; in_f * out_f];
    let mut bias_grad = alloc::vec![Q16_ZERO; out_f];

    for b in 0..batch {
        for o in 0..out_f {
            let grad = output_grad[b * out_f + o];
            bias_grad[o] += grad;
            for i in 0..in_f {
                weight_grad[i * out_f + o] += q16_mul(cached_input[b * in_f + i], grad);
                input_grad[b * in_f + i] += q16_mul(weights[i * out_f + o], grad);
            }
        }
    }
    (input_grad, weight_grad, bias_grad)
}

// ---------------------------------------------------------------------------
// Neural network graph
// ---------------------------------------------------------------------------

/// A complete neural network with layers and metadata
pub struct NeuralNet {
    pub name: String,
    pub layers: Vec<NetLayer>,
    pub input_shape: Vec<usize>,
    pub output_shape: Vec<usize>,
    pub is_training: bool,
}

impl NeuralNet {
    pub fn new(name: &str) -> Self {
        NeuralNet {
            name: String::from(name),
            layers: Vec::new(),
            input_shape: Vec::new(),
            output_shape: Vec::new(),
            is_training: false,
        }
    }

    /// Add a layer with specified weight init
    pub fn add_layer(&mut self, name: &str, config: LayerConfig, init: WeightInit) {
        let layer = NetLayer::new(name, config, init);
        self.layers.push(layer);
    }

    /// Add a dense (fully connected) layer
    pub fn add_dense(&mut self, name: &str, in_f: usize, out_f: usize, act: Activation) {
        self.add_layer(
            name,
            LayerConfig::Dense {
                in_features: in_f,
                out_features: out_f,
                activation: act,
                use_bias: true,
            },
            WeightInit::XavierUniform,
        );
    }

    /// Add a 2D convolutional layer
    pub fn add_conv2d(
        &mut self,
        name: &str,
        in_ch: usize,
        out_ch: usize,
        kernel: usize,
        stride: usize,
        act: Activation,
    ) {
        self.add_layer(
            name,
            LayerConfig::Conv2d {
                in_channels: in_ch,
                out_channels: out_ch,
                kernel_h: kernel,
                kernel_w: kernel,
                stride,
                padding: kernel / 2,
                activation: act,
            },
            WeightInit::HeNormal,
        );
    }

    /// Add an LSTM layer
    pub fn add_lstm(&mut self, name: &str, input_size: usize, hidden_size: usize) {
        self.add_layer(
            name,
            LayerConfig::LSTM {
                input_size,
                hidden_size,
                num_layers: 1,
                bidirectional: false,
            },
            WeightInit::XavierUniform,
        );
    }

    /// Add a multi-head attention layer
    pub fn add_attention(&mut self, name: &str, embed_dim: usize, num_heads: usize) {
        self.add_layer(
            name,
            LayerConfig::Attention {
                embed_dim,
                num_heads,
                dropout_rate: Q16_ZERO,
            },
            WeightInit::XavierUniform,
        );
    }

    /// Total trainable parameters
    pub fn param_count(&self) -> usize {
        self.layers.iter().map(|l| l.param_count()).sum()
    }

    /// Forward pass through the entire network
    pub fn forward(&mut self, input: &[i32]) -> Vec<i32> {
        let mut current = input.to_vec();

        for layer in self.layers.iter_mut() {
            if self.is_training {
                layer.cached_input = current.clone();
            }

            current = match &layer.config {
                LayerConfig::Dense {
                    in_features,
                    out_features,
                    activation,
                    ..
                } => {
                    let mut out = forward_dense(
                        &current,
                        &layer.weights,
                        &layer.bias,
                        *in_features,
                        *out_features,
                    );
                    if self.is_training {
                        layer.cached_pre_act = out.clone();
                    }
                    apply_activation(&mut out, *activation);
                    out
                }
                LayerConfig::Conv1d {
                    in_channels,
                    out_channels,
                    kernel_size,
                    stride,
                    padding,
                    activation,
                } => {
                    let in_len = if *in_channels > 0 {
                        current.len() / *in_channels
                    } else {
                        0
                    };
                    let mut out = forward_conv1d(
                        &current,
                        &layer.weights,
                        &layer.bias,
                        *in_channels,
                        *out_channels,
                        *kernel_size,
                        *stride,
                        *padding,
                        in_len,
                    );
                    apply_activation(&mut out, *activation);
                    out
                }
                LayerConfig::Conv2d {
                    in_channels,
                    out_channels,
                    kernel_h,
                    kernel_w,
                    stride,
                    padding,
                    activation,
                } => {
                    // Infer spatial dims from input size
                    let spatial = if *in_channels > 0 {
                        current.len() / *in_channels
                    } else {
                        0
                    };
                    let side = q16_isqrt(spatial);
                    let mut out = forward_conv2d(
                        &current,
                        &layer.weights,
                        &layer.bias,
                        *in_channels,
                        *out_channels,
                        *kernel_h,
                        *kernel_w,
                        *stride,
                        *padding,
                        side,
                        side,
                    );
                    apply_activation(&mut out, *activation);
                    out
                }
                LayerConfig::LSTM {
                    input_size,
                    hidden_size,
                    num_layers: _,
                    ..
                } => {
                    let seq_len = if *input_size > 0 {
                        current.len() / *input_size
                    } else {
                        0
                    };
                    let mut h = alloc::vec![Q16_ZERO; *hidden_size];
                    let mut c = alloc::vec![Q16_ZERO; *hidden_size];
                    for t in 0..seq_len {
                        let start = t * *input_size;
                        let end = start + *input_size;
                        let step_input = &current[start..end];
                        let (h_new, c_new) = forward_lstm_step(
                            step_input,
                            &h,
                            &c,
                            &layer.weights,
                            &layer.bias,
                            *input_size,
                            *hidden_size,
                        );
                        h = h_new;
                        c = c_new;
                    }
                    h // Return final hidden state
                }
                LayerConfig::Attention {
                    embed_dim,
                    num_heads,
                    ..
                } => {
                    let seq_len = if *embed_dim > 0 {
                        current.len() / *embed_dim
                    } else {
                        0
                    };
                    forward_attention(
                        &current,
                        &layer.weights,
                        &layer.bias,
                        *embed_dim,
                        *num_heads,
                        seq_len,
                    )
                }
                LayerConfig::BatchNorm {
                    features,
                    momentum: _,
                    epsilon,
                } => batch_norm_forward(&current, &layer.weights, &layer.bias, *features, *epsilon),
                LayerConfig::LayerNorm { features, epsilon } => {
                    layer_norm_forward(&current, &layer.weights, &layer.bias, *features, *epsilon)
                }
                LayerConfig::Dropout { rate: _ } => {
                    // During training, zero out with probability rate
                    // During inference, pass through
                    current.clone()
                }
                LayerConfig::Flatten => current.clone(),
                LayerConfig::Reshape { target_shape: _ } => current.clone(),
            };
        }
        current
    }

    /// Backward pass: compute gradients given output loss gradient
    pub fn backward(&mut self, loss_grad: &[i32]) {
        let mut grad = loss_grad.to_vec();

        for idx in (0..self.layers.len()).rev() {
            let (input_grad, w_grad, b_grad) = match &self.layers[idx].config {
                LayerConfig::Dense {
                    in_features,
                    out_features,
                    ..
                } => backward_dense(
                    &grad,
                    &self.layers[idx].cached_input,
                    &self.layers[idx].weights,
                    *in_features,
                    *out_features,
                ),
                _ => {
                    // Simplified: pass gradient through unchanged for other layer types
                    (grad.clone(), alloc::vec![], alloc::vec![])
                }
            };

            if !w_grad.is_empty() {
                let layer = &mut self.layers[idx];
                for i in 0..layer.weight_grad.len().min(w_grad.len()) {
                    layer.weight_grad[i] = w_grad[i];
                }
                for i in 0..layer.bias_grad.len().min(b_grad.len()) {
                    layer.bias_grad[i] = b_grad[i];
                }
            }
            grad = input_grad;
        }
    }

    /// Zero all gradients
    pub fn zero_grad(&mut self) {
        for layer in self.layers.iter_mut() {
            for g in layer.weight_grad.iter_mut() {
                *g = Q16_ZERO;
            }
            for g in layer.bias_grad.iter_mut() {
                *g = Q16_ZERO;
            }
        }
    }
}

/// Integer square root helper (not Q16)
fn q16_isqrt(n: usize) -> usize {
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

/// Batch normalization forward
fn batch_norm_forward(
    input: &[i32],
    gamma: &[i32],
    beta: &[i32],
    features: usize,
    epsilon: i32,
) -> Vec<i32> {
    let batch = if features > 0 {
        input.len() / features
    } else {
        0
    };
    let mut output = alloc::vec![Q16_ZERO; input.len()];

    for f in 0..features {
        // Compute mean
        let mut mean: i64 = 0;
        for b in 0..batch {
            mean += input[b * features + f] as i64;
        }
        let mean = if batch > 0 {
            (mean / (batch as i64)) as i32
        } else {
            0
        };

        // Compute variance
        let mut var: i64 = 0;
        for b in 0..batch {
            let diff = input[b * features + f] - mean;
            var += ((diff as i64) * (diff as i64)) >> 16;
        }
        let var = if batch > 0 {
            (var / (batch as i64)) as i32
        } else {
            Q16_ONE
        };

        let std = q16_sqrt(var + epsilon);
        let g = if f < gamma.len() { gamma[f] } else { Q16_ONE };
        let b_val = if f < beta.len() { beta[f] } else { Q16_ZERO };

        for b in 0..batch {
            let idx = b * features + f;
            let normalized = if std != 0 {
                q16_div(input[idx] - mean, std)
            } else {
                Q16_ZERO
            };
            output[idx] = q16_mul(g, normalized) + b_val;
        }
    }
    output
}

/// Layer normalization forward
fn layer_norm_forward(
    input: &[i32],
    gamma: &[i32],
    beta: &[i32],
    features: usize,
    epsilon: i32,
) -> Vec<i32> {
    let samples = if features > 0 {
        input.len() / features
    } else {
        0
    };
    let mut output = alloc::vec![Q16_ZERO; input.len()];

    for s in 0..samples {
        let offset = s * features;
        // Mean
        let mut mean: i64 = 0;
        for f in 0..features {
            mean += input[offset + f] as i64;
        }
        let mean = if features > 0 {
            (mean / (features as i64)) as i32
        } else {
            0
        };

        // Variance
        let mut var: i64 = 0;
        for f in 0..features {
            let diff = input[offset + f] - mean;
            var += ((diff as i64) * (diff as i64)) >> 16;
        }
        let var = if features > 0 {
            (var / (features as i64)) as i32
        } else {
            Q16_ONE
        };

        let std = q16_sqrt(var + epsilon);
        for f in 0..features {
            let idx = offset + f;
            let normalized = if std != 0 {
                q16_div(input[idx] - mean, std)
            } else {
                Q16_ZERO
            };
            let g = if f < gamma.len() { gamma[f] } else { Q16_ONE };
            let b_val = if f < beta.len() { beta[f] } else { Q16_ZERO };
            output[idx] = q16_mul(g, normalized) + b_val;
        }
    }
    output
}

// ---------------------------------------------------------------------------
// Global registry and init
// ---------------------------------------------------------------------------

/// Registry for neural networks
pub struct NetRegistry {
    nets: Vec<NeuralNet>,
}

impl NetRegistry {
    const fn new() -> Self {
        NetRegistry { nets: Vec::new() }
    }

    pub fn register(&mut self, net: NeuralNet) -> usize {
        let id = self.nets.len();
        serial_println!(
            "    [neural_net] Registered '{}' ({} layers, {} params)",
            net.name,
            net.layers.len(),
            net.param_count()
        );
        self.nets.push(net);
        id
    }

    pub fn get(&self, id: usize) -> Option<&NeuralNet> {
        self.nets.get(id)
    }

    pub fn get_mut(&mut self, id: usize) -> Option<&mut NeuralNet> {
        self.nets.get_mut(id)
    }

    pub fn count(&self) -> usize {
        self.nets.len()
    }
}

pub static NET_REGISTRY: Mutex<NetRegistry> = Mutex::new(NetRegistry::new());

pub fn register_net(net: NeuralNet) -> usize {
    NET_REGISTRY.lock().register(net)
}

pub fn forward(net_id: usize, input: &[i32]) -> Option<Vec<i32>> {
    NET_REGISTRY
        .lock()
        .get_mut(net_id)
        .map(|n| n.forward(input))
}

pub fn init() {
    serial_println!("    [neural_net] Neural network builder initialized (Q16 fixed-point)");
    serial_println!("    [neural_net] Layers: Dense, Conv1d, Conv2d, LSTM, Attention");
    serial_println!("    [neural_net] Activations: ReLU, Sigmoid, Tanh, LeakyReLU, Swish, GELU");
    serial_println!("    [neural_net] Init: Xavier, He, LeCun, Orthogonal");
}
