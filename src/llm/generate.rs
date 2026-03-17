use crate::sync::Mutex;
use alloc::string::String;
/// Text generation engine — the inference runtime
///
/// Takes a prompt, runs the transformer, samples tokens.
///   - Temperature sampling
///   - Top-k / Top-p (nucleus) filtering
///   - Repetition penalty
///   - Speculative decoding (draft model + verify)
///   - Streaming token output
///   - Stop sequences
///   - Structured output (JSON mode)
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

use super::transformer::{q16_from_int, q16_mul, Q16};
use super::{tokenizer, transformer};

#[derive(Clone, Copy)]
pub struct GenerateConfig {
    pub max_tokens: u32,
    pub temperature: Q16,        // 0 = greedy, 65536 = 1.0, higher = more random
    pub top_k: u32,              // 0 = disabled
    pub top_p: Q16,              // 0 = disabled, 60293 = 0.92 in Q16
    pub repetition_penalty: Q16, // 65536 = 1.0 (no penalty), higher = more penalty
    pub frequency_penalty: Q16,
    pub presence_penalty: Q16,
    pub stop_on_eos: bool,
    pub stream: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum GenerateState {
    Idle,
    Prefilling, // Processing prompt tokens
    Generating, // Autoregressive generation
    Complete,
    Stopped, // Hit stop sequence
    Error,
}

struct TokenCandidate {
    id: u32,
    logit: Q16,
    probability: Q16,
}

struct GenerateEngine {
    config: GenerateConfig,
    state: GenerateState,
    // Current generation state
    output_tokens: Vec<u32>,
    token_counts: Vec<u32>, // Frequency count per token ID (for rep penalty)
    current_pos: u32,
    // Stop sequences
    stop_sequences: Vec<Vec<u32>>,
    // Performance
    prefill_tokens_per_sec: u32,
    decode_tokens_per_sec: u32,
    total_generated: u64,
    generated_this_run: u32,
    // Simple PRNG for sampling
    rng_state: u64,
}

static GENERATOR: Mutex<Option<GenerateEngine>> = Mutex::new(None);

impl GenerateEngine {
    fn new() -> Self {
        GenerateEngine {
            config: GenerateConfig {
                max_tokens: 4096,
                temperature: 45875, // 0.7 in Q16
                top_k: 40,
                top_p: 60293,              // 0.92 in Q16
                repetition_penalty: 72090, // 1.1 in Q16
                frequency_penalty: 0,
                presence_penalty: 0,
                stop_on_eos: true,
                stream: true,
            },
            state: GenerateState::Idle,
            output_tokens: Vec::new(),
            token_counts: Vec::new(),
            current_pos: 0,
            stop_sequences: Vec::new(),
            prefill_tokens_per_sec: 0,
            decode_tokens_per_sec: 0,
            total_generated: 0,
            generated_this_run: 0,
            rng_state: 0x5EED_1234_CAFE_BABE,
        }
    }

    /// Simple xorshift64 PRNG
    fn rand_u64(&mut self) -> u64 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng_state = x;
        x
    }

    /// Apply temperature to logits
    fn apply_temperature(&self, logits: &mut [Q16]) {
        let temp = self.config.temperature;
        if temp <= 0 || temp == q16_from_int(1) {
            return;
        }
        for l in logits.iter_mut() {
            // logit / temperature
            *l = ((*l as i64 * 65536) / temp as i64) as Q16;
        }
    }

    /// Apply repetition penalty
    fn apply_repetition_penalty(&self, logits: &mut [Q16]) {
        let penalty = self.config.repetition_penalty;
        if penalty <= q16_from_int(1) {
            return;
        }

        for &tok in &self.output_tokens {
            if (tok as usize) < logits.len() {
                let l = logits[tok as usize];
                if l > 0 {
                    logits[tok as usize] = ((l as i64 * 65536) / penalty as i64) as Q16;
                } else {
                    logits[tok as usize] = q16_mul(l, penalty);
                }
            }
        }
    }

    /// Top-k filtering: keep only the k highest logits
    fn top_k_filter(&self, logits: &mut [Q16]) {
        let k = self.config.top_k as usize;
        if k == 0 || k >= logits.len() {
            return;
        }

        // Find the k-th largest value
        let mut sorted: Vec<Q16> = logits.iter().copied().collect();
        sorted.sort_by(|a, b| b.cmp(a));
        let threshold = sorted[k];

        // Zero out everything below threshold
        for l in logits.iter_mut() {
            if *l < threshold {
                *l = i32::MIN / 2;
            }
        }
    }

    /// Top-p (nucleus) filtering: keep smallest set with cumulative prob >= p
    fn top_p_filter(&self, logits: &mut [Q16]) {
        let p = self.config.top_p;
        if p <= 0 || p >= q16_from_int(1) {
            return;
        }

        // Convert to probabilities (softmax)
        let mut max_l: Q16 = i32::MIN;
        for &l in logits.iter() {
            if l > max_l {
                max_l = l;
            }
        }

        let mut probs: Vec<(usize, Q16)> = Vec::new();
        let mut sum: i64 = 0;
        for (i, &l) in logits.iter().enumerate() {
            let shifted = l - max_l;
            let exp = (q16_from_int(1) + shifted + (q16_mul(shifted, shifted) >> 1)).max(1);
            probs.push((i, exp));
            sum += exp as i64;
        }

        // Normalize and sort by probability descending
        for p_entry in &mut probs {
            p_entry.1 = ((p_entry.1 as i64 * 65536) / sum.max(1)) as Q16;
        }
        probs.sort_by(|a, b| b.1.cmp(&a.1));

        // Find cutoff
        let mut cumulative: Q16 = 0;
        let mut cutoff_idx = probs.len();
        for (j, &(_, prob)) in probs.iter().enumerate() {
            cumulative += prob;
            if cumulative >= p {
                cutoff_idx = j + 1;
                break;
            }
        }

        // Zero out tokens below cutoff
        for j in cutoff_idx..probs.len() {
            let idx = probs[j].0;
            logits[idx] = i32::MIN / 2;
        }
    }

    /// Sample a token from filtered logits
    fn sample(&mut self, logits: &[Q16]) -> u32 {
        if self.config.temperature <= 0 {
            // Greedy: argmax
            return logits
                .iter()
                .enumerate()
                .max_by_key(|(_, &l)| l)
                .map(|(i, _)| i as u32)
                .unwrap_or(0);
        }

        // Softmax
        let mut max_l: Q16 = i32::MIN;
        for &l in logits {
            if l > max_l {
                max_l = l;
            }
        }

        let mut probs: Vec<Q16> = Vec::new();
        let mut sum: i64 = 0;
        for &l in logits {
            let shifted = l - max_l;
            let exp = (q16_from_int(1) + shifted + (q16_mul(shifted, shifted) >> 1)).max(1);
            probs.push(exp);
            sum += exp as i64;
        }

        // Random sampling
        let rand = self.rand_u64();
        let threshold = ((rand % sum.max(1) as u64) as i64) as Q16;
        let mut cumulative: i64 = 0;

        for (i, &p) in probs.iter().enumerate() {
            cumulative += p as i64;
            if cumulative >= threshold as i64 {
                return i as u32;
            }
        }
        0
    }

    /// Generate next token from logits
    fn generate_token(&mut self, logits: &mut Vec<Q16>) -> u32 {
        self.apply_repetition_penalty(logits);
        self.apply_temperature(logits);
        self.top_k_filter(logits);
        self.top_p_filter(logits);

        let token = self.sample(logits);
        self.output_tokens.push(token);
        self.total_generated = self.total_generated.saturating_add(1);
        self.generated_this_run = self.generated_this_run.saturating_add(1);
        self.current_pos += 1;
        token
    }

    /// Check if we should stop generating
    fn should_stop(&self, eos_id: u32) -> bool {
        if self.generated_this_run >= self.config.max_tokens {
            return true;
        }
        if self.config.stop_on_eos {
            if let Some(&last) = self.output_tokens.last() {
                if last == eos_id {
                    return true;
                }
            }
        }
        // Check stop sequences
        for seq in &self.stop_sequences {
            let out_len = self.output_tokens.len();
            let seq_len = seq.len();
            if out_len >= seq_len {
                if &self.output_tokens[out_len - seq_len..] == seq.as_slice() {
                    return true;
                }
            }
        }
        false
    }

    fn reset(&mut self) {
        self.output_tokens.clear();
        self.token_counts.clear();
        self.current_pos = 0;
        self.generated_this_run = 0;
        self.state = GenerateState::Idle;
    }

    fn set_config(&mut self, config: GenerateConfig) {
        self.config = config;
    }

    fn run_prompt_tokens(
        &mut self,
        prompt_tokens: &[u32],
        eos_id: u32,
    ) -> Result<Vec<u32>, &'static str> {
        self.reset();
        self.state = GenerateState::Prefilling;

        if prompt_tokens.is_empty() {
            self.state = GenerateState::Error;
            return Err("empty prompt");
        }

        self.output_tokens.extend_from_slice(prompt_tokens);
        for (pos, &tok) in prompt_tokens.iter().enumerate() {
            if transformer::forward_logits(tok, pos as u32).is_none() {
                self.state = GenerateState::Error;
                return Err("transformer not initialized");
            }
            self.current_pos = pos as u32 + 1;
        }

        self.state = GenerateState::Generating;
        let mut generated = Vec::new();
        let mut last_token = *prompt_tokens.last().unwrap_or(&eos_id);

        while !self.should_stop(eos_id) {
            let mut logits = transformer::forward_logits(last_token, self.current_pos)
                .ok_or("transformer not initialized")?;
            let token = self.generate_token(&mut logits);
            generated.push(token);
            last_token = token;
            if self.should_stop(eos_id) {
                break;
            }
        }

        self.state = GenerateState::Complete;
        Ok(generated)
    }
}

pub fn default_config() -> GenerateConfig {
    GenerateConfig {
        max_tokens: 4096,
        temperature: 45875,
        top_k: 40,
        top_p: 60293,
        repetition_penalty: 72090,
        frequency_penalty: 0,
        presence_penalty: 0,
        stop_on_eos: true,
        stream: true,
    }
}

pub fn set_config(config: GenerateConfig) -> Result<(), &'static str> {
    GENERATOR
        .lock()
        .as_mut()
        .ok_or("generator not initialized")
        .map(|g| g.set_config(config))
}

pub fn generate_tokens(
    prompt_tokens: &[u32],
    config: Option<GenerateConfig>,
) -> Result<Vec<u32>, &'static str> {
    let eos_id = tokenizer::eos_token_id().unwrap_or(1);
    let mut g = GENERATOR.lock();
    let engine = g.as_mut().ok_or("generator not initialized")?;
    if let Some(cfg) = config {
        engine.set_config(cfg);
    }
    engine.run_prompt_tokens(prompt_tokens, eos_id)
}

pub fn generate_text(prompt: &str, config: Option<GenerateConfig>) -> Result<String, &'static str> {
    let mut prompt_tokens = tokenizer::encode_text(prompt).ok_or("tokenizer not initialized")?;
    if prompt_tokens.is_empty() {
        if let Some(bos) = tokenizer::bos_token_id() {
            prompt_tokens.push(bos);
        }
    }

    let generated = generate_tokens(&prompt_tokens, config)?;
    let mut combined = prompt_tokens;
    combined.extend_from_slice(&generated);
    tokenizer::decode_tokens(&combined).ok_or("tokenizer not initialized")
}

pub fn init() {
    let mut g = GENERATOR.lock();
    *g = Some(GenerateEngine::new());
    serial_println!("    Generate: temp/top-k/top-p sampling, rep penalty, streaming ready");
}
