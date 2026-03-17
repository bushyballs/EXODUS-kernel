use crate::sync::Mutex;
use crate::{serial_print, serial_println};
/// Token sampling strategies (top-k, top-p, min-p, typical)
///
/// Part of the AIOS LLM layer. Implements the full sampling pipeline:
///
///   raw logits -> repetition penalty -> temperature scaling -> softmax
///     -> top-k filter -> top-p (nucleus) filter -> min-p filter
///     -> typical-p filter -> renormalise -> weighted random pick
///
/// A deterministic PRNG is used so that sampling is reproducible when
/// the same seed is supplied. The PRNG is a simple xorshift64.
use alloc::vec::Vec;

/// Sampling configuration
pub struct SamplingParams {
    /// Temperature (> 0). 1.0 = neutral; < 1 = sharper; > 1 = flatter
    pub temperature: f32,
    /// Keep only the top-k most probable tokens (0 = disabled)
    pub top_k: usize,
    /// Nucleus sampling: keep tokens with cumulative prob <= top_p
    pub top_p: f32,
    /// Min-p: discard tokens with prob < min_p * max_prob
    pub min_p: f32,
    /// Typical sampling threshold (0 = disabled)
    pub typical_p: f32,
    /// Repetition penalty multiplier (1.0 = no penalty)
    pub repetition_penalty: f32,
    /// Frequency penalty coefficient (additive)
    pub frequency_penalty: f32,
    /// Presence penalty coefficient (additive, binary)
    pub presence_penalty: f32,
}

impl SamplingParams {
    /// Greedy / argmax decoding
    pub fn greedy() -> Self {
        SamplingParams {
            temperature: 0.0,
            top_k: 1,
            top_p: 1.0,
            min_p: 0.0,
            typical_p: 0.0,
            repetition_penalty: 1.0,
            frequency_penalty: 0.0,
            presence_penalty: 0.0,
        }
    }

    /// Creative / chatbot defaults
    pub fn default_creative() -> Self {
        SamplingParams {
            temperature: 0.8,
            top_k: 40,
            top_p: 0.95,
            min_p: 0.05,
            typical_p: 0.0,
            repetition_penalty: 1.1,
            frequency_penalty: 0.0,
            presence_penalty: 0.0,
        }
    }

    /// Balanced defaults for code generation
    pub fn code() -> Self {
        SamplingParams {
            temperature: 0.2,
            top_k: 20,
            top_p: 0.9,
            min_p: 0.0,
            typical_p: 0.0,
            repetition_penalty: 1.05,
            frequency_penalty: 0.1,
            presence_penalty: 0.0,
        }
    }
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

fn fast_ln(x: f32) -> f32 {
    if x <= 0.0 {
        return -1e30;
    }
    let bits = x.to_bits();
    let e = ((bits >> 23) & 0xFF) as i32 - 127;
    let m_bits = (bits & 0x007F_FFFF) | (127 << 23);
    let m = f32::from_bits(m_bits);
    let t = (m - 1.0) / (m + 1.0);
    let t2 = t * t;
    let ln_m = 2.0 * t * (1.0 + t2 / 3.0 + t2 * t2 / 5.0);
    ln_m + (e as f32) * 0.693147180559945
}

// ── PRNG (xorshift64) ──────────────────────────────────────────────

struct Xorshift64 {
    state: u64,
}

impl Xorshift64 {
    fn new(seed: u64) -> Self {
        Xorshift64 {
            state: if seed == 0 {
                0xDEAD_BEEF_CAFE_1234
            } else {
                seed
            },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Uniform f32 in [0, 1)
    fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}

// ── Softmax ────────────────────────────────────────────────────────

fn softmax_inplace(logits: &mut [f32]) {
    if logits.is_empty() {
        return;
    }
    // Numerically stable: subtract max first
    let mut max_val = logits[0];
    for &v in logits.iter() {
        if v > max_val {
            max_val = v;
        }
    }
    let mut sum = 0.0_f32;
    for v in logits.iter_mut() {
        *v = fast_exp(*v - max_val);
        sum += *v;
    }
    if sum > 0.0 {
        let inv_sum = 1.0 / sum;
        for v in logits.iter_mut() {
            *v *= inv_sum;
        }
    }
}

// ── Core sampling function ─────────────────────────────────────────

/// Sample a token index from logits using the given params.
///
/// `logits` is borrowed and will NOT be mutated. A working copy is made.
/// Returns the chosen token index (0-based into the vocabulary).
pub fn sample(logits: &[f32], params: &SamplingParams) -> u32 {
    if logits.is_empty() {
        return 0;
    }

    let _vocab = logits.len();
    let mut work: Vec<f32> = logits.to_vec();

    // ── 1. Repetition / frequency / presence penalty ───────────────
    // (No context history in this standalone call -- callers can
    // pre-apply penalties to the logits slice before calling.)

    // ── 2. Temperature scaling ─────────────────────────────────────
    if params.temperature <= 0.0 || params.top_k == 1 {
        // Greedy: return argmax
        return argmax(&work);
    }
    let inv_temp = 1.0 / params.temperature;
    for v in work.iter_mut() {
        *v *= inv_temp;
    }

    // ── 3. Softmax ─────────────────────────────────────────────────
    softmax_inplace(&mut work);

    // Build (index, prob) pairs and sort descending by prob
    let mut candidates: Vec<(u32, f32)> = work
        .iter()
        .enumerate()
        .map(|(i, &p)| (i as u32, p))
        .collect();
    candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));

    // ── 4. Top-k filter ────────────────────────────────────────────
    if params.top_k > 0 && params.top_k < candidates.len() {
        candidates.truncate(params.top_k);
    }

    // ── 5. Min-p filter ────────────────────────────────────────────
    if params.min_p > 0.0 && !candidates.is_empty() {
        let max_prob = candidates[0].1;
        let threshold = params.min_p * max_prob;
        candidates.retain(|&(_, p)| p >= threshold);
        if candidates.is_empty() {
            // Fallback: keep the top token
            return argmax(logits);
        }
    }

    // ── 6. Top-p (nucleus) filter ──────────────────────────────────
    if params.top_p < 1.0 && params.top_p > 0.0 {
        let mut cumulative = 0.0_f32;
        let mut cutoff = candidates.len();
        for (i, &(_, p)) in candidates.iter().enumerate() {
            cumulative += p;
            if cumulative >= params.top_p {
                cutoff = i + 1;
                break;
            }
        }
        candidates.truncate(cutoff);
    }

    // ── 7. Typical-p filter ────────────────────────────────────────
    if params.typical_p > 0.0 && params.typical_p < 1.0 {
        // Compute entropy
        let entropy: f32 = candidates
            .iter()
            .map(|&(_, p)| if p > 0.0 { -p * fast_ln(p) } else { 0.0 })
            .sum();

        // Sort by |info - entropy| ascending (most "typical" first)
        let mut typical_candidates = candidates.clone();
        typical_candidates.sort_by(|a, b| {
            let info_a = if a.1 > 0.0 { -fast_ln(a.1) } else { 1e30 };
            let info_b = if b.1 > 0.0 { -fast_ln(b.1) } else { 1e30 };
            let diff_a = (info_a - entropy).abs();
            let diff_b = (info_b - entropy).abs();
            diff_a
                .partial_cmp(&diff_b)
                .unwrap_or(core::cmp::Ordering::Equal)
        });

        let mut cumulative = 0.0_f32;
        let mut cutoff = typical_candidates.len();
        for (i, &(_, p)) in typical_candidates.iter().enumerate() {
            cumulative += p;
            if cumulative >= params.typical_p {
                cutoff = i + 1;
                break;
            }
        }
        typical_candidates.truncate(cutoff);
        candidates = typical_candidates;
    }

    // ── 8. Renormalise ─────────────────────────────────────────────
    let sum: f32 = candidates.iter().map(|&(_, p)| p).sum();
    if sum <= 0.0 || candidates.is_empty() {
        return argmax(logits);
    }
    let inv_sum = 1.0 / sum;
    for c in candidates.iter_mut() {
        c.1 *= inv_sum;
    }

    // ── 9. Weighted random pick ────────────────────────────────────
    // Use a simple PRNG seeded from the first few logit bits
    let seed_bits = if !logits.is_empty() {
        logits[0].to_bits() as u64 ^ (logits.len() as u64) << 32
    } else {
        42
    };
    let mut rng = Xorshift64::new(seed_bits);
    let r = rng.next_f32();

    let mut cumulative = 0.0_f32;
    for &(idx, p) in candidates.iter() {
        cumulative += p;
        if r < cumulative {
            return idx;
        }
    }

    // Fallback: return last candidate
    candidates.last().map(|&(idx, _)| idx).unwrap_or(0)
}

/// Apply repetition penalty to logits given a list of previously generated tokens.
pub fn apply_repetition_penalty(logits: &mut [f32], prev_tokens: &[u32], params: &SamplingParams) {
    for &tok in prev_tokens {
        let idx = tok as usize;
        if idx >= logits.len() {
            continue;
        }
        // Multiplicative repetition penalty
        if logits[idx] > 0.0 {
            logits[idx] /= params.repetition_penalty;
        } else {
            logits[idx] *= params.repetition_penalty;
        }
        // Additive frequency penalty (applied once per occurrence)
        logits[idx] -= params.frequency_penalty;
    }

    // Presence penalty: penalise any token that appeared at all
    if params.presence_penalty != 0.0 {
        let mut seen = Vec::new();
        for &tok in prev_tokens {
            let idx = tok as usize;
            if idx < logits.len() && !seen.contains(&idx) {
                logits[idx] -= params.presence_penalty;
                seen.push(idx);
            }
        }
    }
}

/// Simple argmax over a slice
fn argmax(v: &[f32]) -> u32 {
    if v.is_empty() {
        return 0;
    }
    let mut best = 0u32;
    let mut best_val = v[0];
    for (i, &val) in v.iter().enumerate().skip(1) {
        if val > best_val {
            best_val = val;
            best = i as u32;
        }
    }
    best
}

/// Sample multiple tokens (beam-free), returning (token, probability) pairs.
pub fn sample_n(logits: &[f32], params: &SamplingParams, n: usize) -> Vec<(u32, f32)> {
    let mut results = Vec::with_capacity(n);
    let mut work = logits.to_vec();
    for _ in 0..n {
        let tok = sample(&work, params);
        let prob = if (tok as usize) < work.len() {
            work[tok as usize]
        } else {
            0.0
        };
        results.push((tok, prob));
        // Suppress the chosen token for diversity
        if (tok as usize) < work.len() {
            work[tok as usize] = -1e30;
        }
    }
    results
}

// ── Global Singleton ────────────────────────────────────────────────

struct SamplingState {
    default_params: SamplingParams,
    rng: Xorshift64,
}

static SAMPLING: Mutex<Option<SamplingState>> = Mutex::new(None);

pub fn init() {
    let state = SamplingState {
        default_params: SamplingParams::default_creative(),
        rng: Xorshift64::new(0xABCD_EF01_2345_6789),
    };
    let mut guard = SAMPLING.lock();
    *guard = Some(state);
    serial_println!(
        "    [sampling] Token sampling subsystem initialised (top_k=40, top_p=0.95, temp=0.8)"
    );
}

/// Sample using the global default parameters.
pub fn sample_global(logits: &[f32]) -> u32 {
    let guard = SAMPLING.lock();
    if let Some(state) = guard.as_ref() {
        sample(logits, &state.default_params)
    } else {
        argmax(logits)
    }
}
