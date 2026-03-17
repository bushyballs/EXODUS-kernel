use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
/// Speculative decoding -- draft model + verify
///
/// Part of the AIOS LLM layer. Implements speculative decoding where a
/// small, fast "draft" model generates K candidate tokens ahead, then
/// the full "target" model verifies them in a single batched forward
/// pass. Accepted tokens are kept; the first rejected token is replaced
/// with the target model's own sample, and drafting restarts from there.
///
/// This reduces total forward-pass count because verifying K tokens in
/// parallel is almost as cheap as generating 1 token in the target model,
/// while the draft model is much cheaper per token.
///
/// The acceptance criterion follows the original DeepMind paper:
///   accept token t if  p_target(t) / p_draft(t)  >= uniform(0,1)
/// which preserves the target model's distribution exactly.
use alloc::vec::Vec;

/// Speculative decoding engine
pub struct SpeculativeDecoder {
    /// How many tokens the draft model generates ahead
    pub draft_ahead: usize,
    /// Minimum acceptance probability threshold
    pub acceptance_threshold: f32,
    /// Simple simulated "draft model" weights (small FFN)
    draft_weights: Vec<f32>,
    /// Vocabulary size for both models
    vocab_size: usize,
    /// Hidden dimension of the draft model
    draft_hidden: usize,
    /// Running statistics
    pub total_drafted: u64,
    pub total_accepted: u64,
    /// PRNG state for stochastic acceptance
    rng_state: u64,
}

// ── Math helpers ────────────────────────────────────────────────────

fn fast_exp(x: f32) -> f32 {
    if x > 88.0 {
        return f32::MAX;
    }
    if x < -88.0 {
        return 0.0;
    }
    let v = x * 1.442695040888963;
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

fn softmax(logits: &mut [f32]) {
    if logits.is_empty() {
        return;
    }
    let mut max_v = logits[0];
    for &v in logits.iter() {
        if v > max_v {
            max_v = v;
        }
    }
    let mut sum = 0.0_f32;
    for v in logits.iter_mut() {
        *v = fast_exp(*v - max_v);
        sum += *v;
    }
    if sum > 0.0 {
        let inv = 1.0 / sum;
        for v in logits.iter_mut() {
            *v *= inv;
        }
    }
}

fn argmax(v: &[f32]) -> u32 {
    let mut best = 0u32;
    let mut best_val = f32::MIN;
    for (i, &val) in v.iter().enumerate() {
        if val > best_val {
            best_val = val;
            best = i as u32;
        }
    }
    best
}

// ── PRNG ────────────────────────────────────────────────────────────

fn xorshift64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

fn rand_f32(state: &mut u64) -> f32 {
    (xorshift64(state) >> 40) as f32 / (1u64 << 24) as f32
}

impl SpeculativeDecoder {
    /// Create a new speculative decoder.
    ///
    /// `draft_ahead` is the number of tokens to speculatively generate
    /// before verification (typically 3-5).
    pub fn new(draft_ahead: usize) -> Self {
        Self::with_vocab(draft_ahead, 256, 64)
    }

    /// Create with explicit vocab size and draft hidden dimension.
    pub fn with_vocab(draft_ahead: usize, vocab_size: usize, draft_hidden: usize) -> Self {
        // Initialise a tiny draft model: single hidden layer FFN
        // weights: [vocab_size * draft_hidden] (embed) + [draft_hidden * vocab_size] (head)
        let total_weights = vocab_size * draft_hidden + draft_hidden * vocab_size;
        let mut weights = Vec::with_capacity(total_weights);
        let mut rng = 0xBEEF_CAFE_u64;
        for _ in 0..total_weights {
            let r = rand_f32(&mut rng) * 0.02 - 0.01; // small random init
            weights.push(r);
        }

        serial_println!(
            "    [speculative] Created decoder: ahead={}, vocab={}, hidden={}",
            draft_ahead,
            vocab_size,
            draft_hidden
        );

        SpeculativeDecoder {
            draft_ahead,
            acceptance_threshold: 0.0,
            draft_weights: weights,
            vocab_size,
            draft_hidden,
            total_drafted: 0,
            total_accepted: 0,
            rng_state: 0xDEAD_BEEF_1234_5678,
        }
    }

    /// Run the draft model on a token to produce logits.
    ///
    /// This is a minimal single-layer FFN:
    ///   embed = embedding_table[token]  (size: draft_hidden)
    ///   logits = embed @ head_weights   (size: vocab_size)
    fn draft_forward(&self, token: u32) -> Vec<f32> {
        let tok = (token as usize) % self.vocab_size;
        let embed_offset = tok * self.draft_hidden;
        let head_offset = self.vocab_size * self.draft_hidden;

        // Extract embedding
        let mut hidden = vec![0.0_f32; self.draft_hidden];
        for j in 0..self.draft_hidden {
            let idx = embed_offset + j;
            hidden[j] = if idx < head_offset {
                self.draft_weights[idx]
            } else {
                0.0
            };
        }

        // Apply ReLU activation
        for h in hidden.iter_mut() {
            if *h < 0.0 {
                *h = 0.0;
            }
        }

        // Linear head: logits[v] = sum_j(hidden[j] * head[j][v])
        let mut logits = vec![0.0_f32; self.vocab_size];
        for v in 0..self.vocab_size {
            let mut sum = 0.0_f32;
            for j in 0..self.draft_hidden {
                let idx = head_offset + j * self.vocab_size + v;
                if idx < self.draft_weights.len() {
                    sum += hidden[j] * self.draft_weights[idx];
                }
            }
            logits[v] = sum;
        }

        logits
    }

    /// Generate tokens using speculative decoding.
    ///
    /// `prompt_tokens` is the input sequence.
    /// `max_tokens` is the maximum number of new tokens to generate.
    ///
    /// Returns the generated token sequence.
    pub fn generate(&self, prompt_tokens: &[u32], max_tokens: usize) -> Vec<u32> {
        if prompt_tokens.is_empty() || max_tokens == 0 {
            return Vec::new();
        }

        let mut output = Vec::with_capacity(max_tokens);
        let mut last_token = *prompt_tokens.last().unwrap_or(&0);
        let mut rng = self.rng_state;

        while output.len() < max_tokens {
            // ── Draft phase: generate K candidate tokens ────────────
            let mut draft_tokens = Vec::with_capacity(self.draft_ahead);
            let mut draft_probs = Vec::with_capacity(self.draft_ahead);
            let mut current = last_token;

            for _ in 0..self.draft_ahead {
                if output.len() + draft_tokens.len() >= max_tokens {
                    break;
                }
                let mut logits = self.draft_forward(current);
                softmax(&mut logits);

                let tok = argmax(&logits);
                let prob = logits[tok as usize];
                draft_tokens.push(tok);
                draft_probs.push(prob);
                current = tok;
            }

            if draft_tokens.is_empty() {
                break;
            }

            // ── Verify phase: run target model on all candidates ───
            // (In a real system, this calls the full model in one batch.
            //  Here we simulate target logits as slightly perturbed
            //  draft logits to demonstrate the verification algorithm.)
            let mut accepted_count = 0;
            let mut verify_token = last_token;

            for i in 0..draft_tokens.len() {
                let mut target_logits = self.draft_forward(verify_token);
                // Simulate target being slightly different
                for (j, l) in target_logits.iter_mut().enumerate() {
                    *l += (j as f32 * 0.001) - 0.0005;
                }
                softmax(&mut target_logits);

                let draft_tok = draft_tokens[i];
                let target_prob = target_logits[draft_tok as usize];
                let draft_prob = draft_probs[i];

                // Acceptance criterion: accept if p_target >= r * p_draft
                let r = rand_f32(&mut rng);
                let ratio = if draft_prob > 0.0 {
                    target_prob / draft_prob
                } else {
                    0.0
                };

                if ratio >= r {
                    // Accept this draft token
                    output.push(draft_tok);
                    verify_token = draft_tok;
                    accepted_count += 1;
                } else {
                    // Reject: sample from adjusted distribution
                    // p_adjusted = max(0, p_target - p_draft) / Z
                    let mut adjusted = vec![0.0_f32; self.vocab_size];
                    let d_logits = self.draft_forward(verify_token);
                    let mut d_probs = d_logits;
                    softmax(&mut d_probs);

                    let mut sum = 0.0_f32;
                    for v in 0..self.vocab_size {
                        let diff = target_logits[v] - d_probs[v];
                        adjusted[v] = if diff > 0.0 { diff } else { 0.0 };
                        sum += adjusted[v];
                    }

                    let sampled = if sum > 0.0 {
                        let inv = 1.0 / sum;
                        for a in adjusted.iter_mut() {
                            *a *= inv;
                        }
                        // Weighted sample
                        let u = rand_f32(&mut rng);
                        let mut cum = 0.0_f32;
                        let mut chosen = 0u32;
                        for (v, &p) in adjusted.iter().enumerate() {
                            cum += p;
                            if u < cum {
                                chosen = v as u32;
                                break;
                            }
                        }
                        chosen
                    } else {
                        argmax(&target_logits)
                    };

                    output.push(sampled);
                    verify_token = sampled;
                    break; // Restart drafting from here
                }
            }

            last_token = if let Some(&t) = output.last() {
                t
            } else {
                break;
            };

            // If all drafts were accepted, we also get one bonus token
            // from the target model's distribution at the last position
            if accepted_count == draft_tokens.len() && output.len() < max_tokens {
                let mut bonus_logits = self.draft_forward(last_token);
                softmax(&mut bonus_logits);
                let bonus = argmax(&bonus_logits);
                output.push(bonus);
                last_token = bonus;
            }
        }

        output.truncate(max_tokens);
        output
    }

    /// Verify a batch of draft tokens against target model logits.
    ///
    /// `draft` is the sequence of drafted tokens.
    /// `target_logits` is the flattened target model logits for each
    /// position: length = draft.len() * vocab_size.
    ///
    /// Returns the number of tokens accepted from the front of `draft`.
    pub fn verify_batch(&self, draft: &[u32], target_logits: &[f32]) -> usize {
        let mut accepted = 0;
        let mut rng = self.rng_state;

        for (i, &tok) in draft.iter().enumerate() {
            let offset = i * self.vocab_size;
            if offset + self.vocab_size > target_logits.len() {
                break;
            }

            let mut probs = target_logits[offset..offset + self.vocab_size].to_vec();
            softmax(&mut probs);

            let target_prob = if (tok as usize) < probs.len() {
                probs[tok as usize]
            } else {
                0.0
            };

            // Draft probability: run draft model to get it
            let mut draft_logits = self.draft_forward(if i == 0 { 0 } else { draft[i - 1] });
            softmax(&mut draft_logits);
            let draft_prob = if (tok as usize) < draft_logits.len() {
                draft_logits[tok as usize]
            } else {
                0.0
            };

            let ratio = if draft_prob > 0.0 {
                target_prob / draft_prob
            } else {
                0.0
            };
            let r = rand_f32(&mut rng);

            if ratio >= r {
                accepted += 1;
            } else {
                break;
            }
        }

        accepted
    }

    /// Get the acceptance rate so far.
    pub fn acceptance_rate(&self) -> f32 {
        if self.total_drafted == 0 {
            return 0.0;
        }
        self.total_accepted as f32 / self.total_drafted as f32
    }

    /// Get the effective speedup factor estimate.
    /// Assumes target model is `draft_ahead` times slower than draft.
    pub fn estimated_speedup(&self) -> f32 {
        let rate = self.acceptance_rate();
        let _k = self.draft_ahead as f32;
        // Expected accepted per round: sum_i=0..k of rate^i
        let mut expected = 0.0_f32;
        let mut ri = 1.0_f32;
        for _ in 0..self.draft_ahead {
            expected += ri;
            ri *= rate;
        }
        // Speedup = expected_tokens / (1 + k/cost_ratio)
        // Simplified: if draft is free, speedup ~ expected + 1
        expected + 1.0
    }
}

// ── Global Singleton ────────────────────────────────────────────────

struct SpecState {
    decoder: SpeculativeDecoder,
}

static SPECULATIVE: Mutex<Option<SpecState>> = Mutex::new(None);

const DEFAULT_DRAFT_AHEAD: usize = 4;

pub fn init() {
    let decoder = SpeculativeDecoder::new(DEFAULT_DRAFT_AHEAD);
    let mut guard = SPECULATIVE.lock();
    *guard = Some(SpecState { decoder });
    serial_println!(
        "    [speculative] Subsystem initialised (draft_ahead={})",
        DEFAULT_DRAFT_AHEAD
    );
}

/// Generate tokens using the global speculative decoder.
pub fn generate_global(prompt: &[u32], max_tokens: usize) -> Vec<u32> {
    let guard = SPECULATIVE.lock();
    if let Some(state) = guard.as_ref() {
        state.decoder.generate(prompt, max_tokens)
    } else {
        Vec::new()
    }
}
