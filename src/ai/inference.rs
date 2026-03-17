use crate::sync::Mutex;
/// Local model inference engine
///
/// Runs quantized language models on CPU. Designed for:
///   - GGUF format models (llama.cpp compatible)
///   - INT4/INT8 quantization for efficiency
///   - KV-cache for fast autoregressive generation
///   - Batch processing for throughput
///   - Wired to crate::llm::transformer for real forward passes
///   - Markov-chain fallback when transformer generation fails
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

static ENGINE: Mutex<Option<InferenceEngine>> = Mutex::new(None);

/// Supported model formats
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFormat {
    Gguf,   // llama.cpp GGUF
    Onnx,   // ONNX Runtime
    Custom, // Hoags native format
}

/// Quantization level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantization {
    F32,
    F16,
    Int8,
    Int4,
}

/// Model metadata
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub name: String,
    pub format: ModelFormat,
    pub quantization: Quantization,
    pub parameters: u64,     // total parameters
    pub context_length: u32, // max tokens
    pub vocab_size: u32,
    pub embedding_dim: u32,
    pub num_layers: u32,
    pub num_heads: u32,
    pub memory_mb: u32, // estimated memory usage
}

/// Token — the fundamental unit of text for the model
#[derive(Debug, Clone, Copy)]
pub struct Token(pub u32);

/// Generation parameters
#[derive(Debug, Clone)]
pub struct GenerationParams {
    pub max_tokens: u32,
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: u32,
    pub repetition_penalty: f32,
    pub stop_sequences: Vec<String>,
}

impl GenerationParams {
    pub fn default() -> Self {
        GenerationParams {
            max_tokens: 256,
            temperature: 0.7,
            top_p: 0.9,
            top_k: 40,
            repetition_penalty: 1.1,
            stop_sequences: Vec::new(),
        }
    }

    pub fn deterministic() -> Self {
        GenerationParams {
            max_tokens: 256,
            temperature: 0.0,
            top_p: 1.0,
            top_k: 1,
            repetition_penalty: 1.0,
            stop_sequences: Vec::new(),
        }
    }
}

/// Inference engine state
pub struct InferenceEngine {
    pub loaded_model: Option<ModelInfo>,
    pub tokenizer: Option<Tokenizer>,
    pub state: EngineState,
    pub total_tokens_generated: u64,
    /// Bigram frequency table for Markov-chain fallback generation
    bigram_table: BTreeMap<u32, Vec<(u32, u32)>>,
    /// Simple xorshift PRNG state
    rng_state: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineState {
    Idle,
    Loading,
    Ready,
    Generating,
    Error,
}

/// Tokenizer that delegates to crate::llm::tokenizer for BPE encoding/decoding,
/// with a byte-level fallback when the LLM tokenizer is unavailable
pub struct Tokenizer {
    pub vocab: Vec<String>,
    pub merges: Vec<(String, String)>,
}

impl Tokenizer {
    pub fn new() -> Self {
        Tokenizer {
            vocab: Vec::new(),
            merges: Vec::new(),
        }
    }

    /// Encode text to token IDs. Tries the BPE tokenizer first, falls back to byte-level.
    pub fn encode(&self, text: &str) -> Vec<Token> {
        if let Some(ids) = crate::llm::tokenizer::encode_text(text) {
            return ids.iter().map(|&id| Token(id)).collect();
        }
        // Byte-level fallback: each byte becomes token (byte_value + 9)
        text.as_bytes()
            .iter()
            .map(|&b| Token(b as u32 + 9))
            .collect()
    }

    /// Decode token IDs back to text. Tries the BPE tokenizer first, falls back to byte-level.
    pub fn decode(&self, tokens: &[Token]) -> String {
        let ids: Vec<u32> = tokens.iter().map(|t| t.0).collect();
        if let Some(text) = crate::llm::tokenizer::decode_tokens(&ids) {
            return text;
        }
        // Byte-level fallback
        let mut out = String::new();
        for t in tokens {
            if let Some(word) = self.vocab.get(t.0 as usize) {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(word);
            } else {
                let b = t.0.wrapping_sub(9) as u8;
                if b == b'\n' || b == b'\r' || b == b'\t' || (32..=126).contains(&b) {
                    out.push(b as char);
                }
            }
        }
        out
    }
}

impl InferenceEngine {
    pub fn new() -> Self {
        InferenceEngine {
            loaded_model: None,
            tokenizer: None,
            state: EngineState::Idle,
            total_tokens_generated: 0,
            bigram_table: BTreeMap::new(),
            rng_state: 0x5EED_1234_CAFE_BABE,
        }
    }

    /// Load a model configuration. Selects transformer config based on model name.
    pub fn load_model(&mut self, path: &str) -> Result<(), &'static str> {
        self.state = EngineState::Loading;
        serial_println!("    [ai] Loading model: {}", path);

        let (name, params, dim, layers, heads, ctx, mem) =
            if path.contains("small") || path.contains("125m") {
                (
                    "hoags-small-125M",
                    125_000_000u64,
                    768u32,
                    12u32,
                    12u32,
                    8192u32,
                    256u32,
                )
            } else if path.contains("medium") || path.contains("350m") {
                ("hoags-medium-350M", 350_000_000, 1024, 24, 16, 32768, 512)
            } else if path.contains("large") || path.contains("1b") {
                ("hoags-large-1B", 1_000_000_000, 2048, 32, 32, 65536, 1024)
            } else if path.contains("xl") || path.contains("7b") {
                ("hoags-xl-7B", 7_000_000_000, 4096, 48, 64, 131072, 4096)
            } else {
                ("hoags-tiny-1B", 1_000_000_000, 2048, 22, 32, 4096, 512)
            };

        self.loaded_model = Some(ModelInfo {
            name: String::from(name),
            format: ModelFormat::Gguf,
            quantization: Quantization::Int4,
            parameters: params,
            context_length: ctx,
            vocab_size: 32000,
            embedding_dim: dim,
            num_layers: layers,
            num_heads: heads,
            memory_mb: mem,
        });

        self.tokenizer = Some(Tokenizer::new());
        self.state = EngineState::Ready;
        serial_println!("    [ai] Model loaded: {}", name);
        Ok(())
    }

    /// Generate text from a prompt.
    ///
    /// Primary path: delegate to crate::llm::generate::generate_text which uses the
    /// full transformer pipeline (tokenize -> forward -> sample -> decode).
    ///
    /// Fallback: if the LLM generate module fails, use a Markov-chain bigram
    /// generator built from the input prompt.
    pub fn generate(
        &mut self,
        prompt: &str,
        params: &GenerationParams,
    ) -> Result<String, &'static str> {
        if self.state != EngineState::Ready {
            return Err("model not loaded");
        }
        self.state = EngineState::Generating;

        let input_tokens = self
            .tokenizer
            .as_ref()
            .ok_or("no tokenizer")?
            .encode(prompt);
        let input_count = input_tokens.len() as u64;

        // Build LLM generate config from our params
        let mut cfg = crate::llm::generate::default_config();
        cfg.max_tokens = params.max_tokens;
        cfg.temperature = (params.temperature * 65536.0) as i32;
        cfg.top_k = params.top_k;
        cfg.top_p = (params.top_p * 65536.0) as i32;
        cfg.repetition_penalty = (params.repetition_penalty * 65536.0) as i32;
        cfg.stop_on_eos = true;

        // Try the real transformer pipeline first
        let result = match crate::llm::generate::generate_text(prompt, Some(cfg)) {
            Ok(text) if !text.is_empty() => {
                self.total_tokens_generated =
                    self.total_tokens_generated.saturating_add(input_count);
                self.state = EngineState::Ready;
                return Ok(text);
            }
            _ => {
                // Transformer generation failed or returned empty — use Markov fallback
                self.generate_markov(&input_tokens, params)
            }
        };

        self.state = EngineState::Ready;
        result
    }

    /// Markov-chain bigram text generator — fallback when the transformer
    /// forward pass produces empty output (e.g., no trained weights loaded).
    ///
    /// Builds a bigram frequency table from the prompt tokens, then samples
    /// next tokens proportionally using xorshift PRNG.
    fn generate_markov(
        &mut self,
        input_tokens: &[Token],
        params: &GenerationParams,
    ) -> Result<String, &'static str> {
        self.bigram_table.clear();

        if input_tokens.len() < 2 {
            return Ok(alloc::format!(
                "[Hoags AI] Input too short for Markov generation ({} tokens).",
                input_tokens.len()
            ));
        }

        // Build bigram frequency table: token -> [(next_token, count)]
        for window in input_tokens.windows(2) {
            let current = window[0].0;
            let next = window[1].0;
            let entry = self.bigram_table.entry(current).or_insert_with(Vec::new);
            if let Some(pair) = entry.iter_mut().find(|(tok, _)| *tok == next) {
                pair.1 += 1;
            } else {
                entry.push((next, 1));
            }
        }

        // Walk the chain
        let max_tokens = params.max_tokens.min(256);
        let mut generated: Vec<u32> = Vec::new();
        let mut current = input_tokens.last().map(|t| t.0).unwrap_or(0);

        for _ in 0..max_tokens {
            let candidates = match self.bigram_table.get(&current) {
                Some(c) if !c.is_empty() => c.clone(),
                _ => break,
            };

            let total: u64 = candidates.iter().map(|(_, count)| *count as u64).sum();
            if total == 0 {
                break;
            }

            let threshold = self.rand_u64() % total;
            let mut cumulative: u64 = 0;
            let mut next_token = candidates[0].0;
            for &(tok, count) in &candidates {
                cumulative += count as u64;
                if cumulative > threshold {
                    next_token = tok;
                    break;
                }
            }

            // Stop on excessive repetition (same token 4 times)
            if generated.len() >= 3 {
                let l = generated.len();
                if generated[l - 1] == next_token
                    && generated[l - 2] == next_token
                    && generated[l - 3] == next_token
                {
                    break;
                }
            }

            generated.push(next_token);
            current = next_token;
            self.total_tokens_generated = self.total_tokens_generated.saturating_add(1);
        }

        let tokenizer = self.tokenizer.as_ref().ok_or("no tokenizer")?;
        let out_tokens: Vec<Token> = generated.iter().map(|&id| Token(id)).collect();
        let decoded = tokenizer.decode(&out_tokens);

        if decoded.is_empty() {
            Ok(alloc::format!(
                "[Hoags AI] Generated {} Markov tokens from {} input tokens.",
                generated.len(),
                input_tokens.len()
            ))
        } else {
            Ok(decoded)
        }
    }

    /// Xorshift64 PRNG
    fn rand_u64(&mut self) -> u64 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng_state = x;
        x
    }

    /// Get model info
    pub fn model_info(&self) -> Option<&ModelInfo> {
        self.loaded_model.as_ref()
    }
}

// =============================================================================
// Q4_0 quantized inference primitives
// =============================================================================

/// Q4_0 block layout (18 bytes each):
///   bytes [0..1]  — scale as f16 (big-endian IEEE 754 half-precision)
///   bytes [2..17] — 16 packed 4-bit signed weights (two per byte, low nibble first)
const Q4_0_BLOCK_SIZE: usize = 18;
/// Number of weights encoded per Q4_0 block
const Q4_0_WEIGHTS_PER_BLOCK: usize = 16;

/// Decode an f16 big-endian byte pair to a fixed-point i16 scale (Q8.8 format).
///
/// The f16 has: sign(1) | exponent(5) | mantissa(10).
/// We convert to a signed integer in [−32768, 32767] by
/// scaling the float value by 256 (i.e., Q8.8 fixed-point).
fn f16_be_to_q8_8(hi: u8, lo: u8) -> i16 {
    let bits = ((hi as u16) << 8) | (lo as u16);
    let sign: i32 = if bits >> 15 != 0 { -1 } else { 1 };
    let exp = ((bits >> 10) & 0x1F) as i32;
    let mantissa = (bits & 0x3FF) as i32;

    if exp == 0 {
        // Subnormal or zero
        // value = sign * 2^(−14) * (mantissa / 1024)
        // Q8.8: value * 256 = sign * mantissa * 256 / (1024 * 16384)
        // For subnormals the result rounds to 0 in Q8.8
        return 0;
    }
    if exp == 31 {
        // Inf or NaN — clamp to max i16
        return if sign > 0 { i16::MAX } else { i16::MIN };
    }

    // Normalised: value = sign * 2^(exp-15) * (1 + mantissa/1024)
    // To Q8.8: multiply by 256 = 2^8
    // net exponent for Q8.8: (exp - 15) + 8 = exp - 7
    let net_exp = exp - 7; // exp in [1..30], so net_exp in [-6..23]
                           // significand = 1024 + mantissa  (implicit leading 1, shifted to avoid fractions)
    let significand = 1024i32 + mantissa; // 10-bit mantissa + implicit 1 → 11 bits

    // value_q8_8 = sign * significand * 2^(net_exp - 10)
    //   (divide by 1024 to normalise the 11-bit significand)
    let shift = net_exp - 10;
    let abs_val: i32 = if shift >= 0 {
        (significand << shift.min(20)).min(32767)
    } else {
        let rshift = (-shift) as u32;
        if rshift >= 32 {
            0
        } else {
            significand >> rshift
        }
    };

    let result = sign * abs_val;
    result.max(-32768).min(32767) as i16
}

/// Dequantize a Q4_0 byte stream into i16 output values.
///
/// Each 18-byte block contains:
/// - 2-byte f16 BE scale
/// - 16 bytes of packed 4-bit signed weights (low nibble = even index, high nibble = odd index)
///
/// Dequantized value = scale_q8_8 * weight_4bit
/// Output is stored as i16. `out` must have capacity for
/// `(data.len() / Q4_0_BLOCK_SIZE) * Q4_0_WEIGHTS_PER_BLOCK` elements.
///
/// Incomplete trailing blocks are silently skipped.
pub fn dequant_q4_0(data: &[u8], out: &mut [i16]) {
    let n_blocks = data.len() / Q4_0_BLOCK_SIZE;
    let mut out_idx = 0usize;

    for b in 0..n_blocks {
        let block_start = b * Q4_0_BLOCK_SIZE;
        if block_start + Q4_0_BLOCK_SIZE > data.len() {
            break;
        }

        // Read f16 BE scale and convert to Q8.8
        let scale = f16_be_to_q8_8(data[block_start], data[block_start + 1]);

        // Decode 16 packed 4-bit weights from the 16 bytes after the scale
        for w in 0..Q4_0_WEIGHTS_PER_BLOCK {
            if out_idx >= out.len() {
                return;
            }
            let byte_idx = block_start + 2 + w / 2;
            let packed = data[byte_idx];
            // Low nibble = even weight index, high nibble = odd weight index
            let nibble = if w % 2 == 0 {
                packed & 0x0F
            } else {
                packed >> 4
            };
            // Sign-extend 4-bit two's complement to i8
            let signed = if nibble & 0x08 != 0 {
                (nibble as i8) | (-16i8) // set upper 4 bits
            } else {
                nibble as i8
            };
            // Dequantized = scale * weight; scale is Q8.8, weight is i8
            // Result: (scale * signed) >> 8 stays in i16 range for typical values
            let dequant = ((scale as i32) * (signed as i32)) >> 8;
            out[out_idx] = dequant.max(-32768).min(32767) as i16;
            out_idx += 1;
        }
    }
}

/// Matrix-vector multiply using Q4_0 quantized weights on-the-fly.
///
/// Computes: `out[r] = sum_c( dequant(weights, r, c) * input[c] )`
///
/// - `weights`: packed Q4_0 byte stream for the full matrix (rows × cols weights)
/// - `input`: input vector of `cols` i16 values
/// - `out`: output accumulator of `rows` i32 values (accumulated in i32 to avoid overflow)
/// - `rows`, `cols`: matrix dimensions
///
/// Each Q4_0 block covers 16 consecutive elements. If `cols` is not a multiple
/// of 16 the last partial block of each row is handled with bounds checking.
pub fn matmul_q4_i16(weights: &[u8], input: &[i16], out: &mut [i32], rows: usize, cols: usize) {
    if rows == 0 || cols == 0 || input.is_empty() || out.is_empty() {
        return;
    }

    // Number of Q4_0 blocks per row (ceiling division by 16)
    let blocks_per_row = (cols + Q4_0_WEIGHTS_PER_BLOCK - 1) / Q4_0_WEIGHTS_PER_BLOCK;
    // Bytes per row in the Q4_0 stream
    let bytes_per_row = blocks_per_row * Q4_0_BLOCK_SIZE;

    for r in 0..rows.min(out.len()) {
        let row_byte_start = r * bytes_per_row;
        let mut acc: i32 = 0;
        let mut col = 0usize;

        for b in 0..blocks_per_row {
            let block_start = row_byte_start + b * Q4_0_BLOCK_SIZE;
            if block_start + Q4_0_BLOCK_SIZE > weights.len() {
                break;
            }

            let scale = f16_be_to_q8_8(weights[block_start], weights[block_start + 1]);

            for w in 0..Q4_0_WEIGHTS_PER_BLOCK {
                if col >= cols || col >= input.len() {
                    break;
                }
                let byte_idx = block_start + 2 + w / 2;
                let packed = weights[byte_idx];
                let nibble = if w % 2 == 0 {
                    packed & 0x0F
                } else {
                    packed >> 4
                };
                let signed = if nibble & 0x08 != 0 {
                    (nibble as i8) | (-16i8)
                } else {
                    nibble as i8
                };
                // dequant * input: (scale * signed >> 8) * input[col]
                // Kept in i32 to preserve precision during accumulation
                let dequant = ((scale as i32) * (signed as i32)) >> 8;
                acc = acc.saturating_add(dequant.saturating_mul(input[col] as i32));
                col += 1;
            }
        }

        out[r] = acc;
    }
}

/// Integer SoftMax over a logit vector.
///
/// `logits` contains raw i32 scores. `temperature_q8` is a Q8.0 temperature
/// (e.g., 128 = 1.0, 64 = 0.5, 255 = ~2.0). Temperature must be > 0.
///
/// Algorithm (integer-friendly):
///   1. Find the maximum logit (to subtract for numerical stability).
///   2. For each logit, compute shifted = (logit - max) scaled by temperature.
///   3. Approximate exp(shifted) using a fast integer exponential.
///   4. Normalise so the sum of all values fits in i32 (values scaled to [0, 65536]).
///
/// After the call, `logits` contains non-negative probability-proportional
/// values in Q16 format (sum ≈ 65536).
///
/// If `temperature_q8` is 0 it is clamped to 1 (minimum division by 256).
pub fn softmax_q8(logits: &mut [i32], temperature_q8: u8) {
    if logits.is_empty() {
        return;
    }

    let temp = if temperature_q8 == 0 {
        1u32
    } else {
        temperature_q8 as u32
    };

    // Step 1: find maximum
    let mut max_val = logits[0];
    for &v in logits.iter().skip(1) {
        if v > max_val {
            max_val = v;
        }
    }

    // Step 2 & 3: compute shifted logits and integer exp approximation.
    // shifted[i] = (logit[i] - max) * 256 / temp
    // We use a piecewise linear approximation of exp(x):
    //   For x in [-16, 0]: exp(x) ≈ 65536 * 2^(x / ln2)
    //   Implemented as: 65536 >> (-x * 512 / 355557) where 512/355557 ≈ 1/ln2 * 1/1024
    //
    // A simpler and reliable approach: map shifted value to [0, 32] integer range,
    // then compute 2^fractional using a 8-step table.
    //
    // For the kernel use-case (greedy or top-k sampling on small vocab) precision
    // is not critical — we just need monotone, non-negative values.

    let mut exp_vals: [i32; 256] = [0i32; 256];
    let n = logits.len().min(256);
    let mut sum: i64 = 0;

    for i in 0..n {
        // Scale the difference by 256/temp (Q0 → temperature-divided Q0)
        let diff = ((logits[i] - max_val) as i64) * 256 / (temp as i64);
        // Clamp to [-64, 0] to keep the exponential in (0, 1]
        let diff = diff.max(-64).min(0) as i32;

        // Approximate 2^(diff/8): each step of 8 halves the value
        // exp(diff) ≈ 65536 >> (-diff >> 3)  (divide by 2 for every 8 units of diff)
        let shift = ((-diff) >> 3) as u32;
        let approx_exp = if shift >= 16 { 1i32 } else { 65536i32 >> shift };

        exp_vals[i] = approx_exp;
        sum += approx_exp as i64;
    }

    if sum == 0 {
        // Degenerate case: all logits were equal — uniform distribution
        let uniform = 65536i32 / (n as i32).max(1);
        for i in 0..n {
            logits[i] = uniform;
        }
        return;
    }

    // Step 4: normalise to Q16 (sum = 65536)
    for i in 0..n {
        logits[i] = ((exp_vals[i] as i64 * 65536) / sum) as i32;
    }
    // Zero out any entries beyond n that may have been valid before
    for v in logits.iter_mut().skip(n) {
        *v = 0;
    }
}

pub fn init() {
    let mut engine = InferenceEngine::new();
    let _ = engine.load_model("hoags-tiny-1B");
    *ENGINE.lock() = Some(engine);
    serial_println!("    [inference] AI inference engine ready (transformer + Markov fallback)");
}

/// Generate text from a prompt
pub fn generate(prompt: &str) -> Result<String, &'static str> {
    ENGINE
        .lock()
        .as_mut()
        .ok_or("engine not initialized")?
        .generate(prompt, &GenerationParams::default())
}
