use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
/// Mixture of Experts (MoE) routing
///
/// Part of the AIOS LLM layer. Implements the gating / routing
/// mechanism used in Mixture-of-Experts transformer layers where each
/// token is dispatched to a subset of "expert" FFN sub-networks based
/// on learned gate weights.
///
/// The router computes gate scores for every expert by multiplying the
/// hidden state with a learnable gate matrix, applies softmax, then
/// selects the top-k experts. A load-balancing auxiliary loss encourages
/// even utilisation across experts.
///
/// Architecture:
///   hidden [d] -> gate_weights [num_experts x d] -> scores [num_experts]
///     -> softmax -> top_k selection -> (expert_id, gate_value) pairs
///
/// The capacity factor limits how many tokens each expert can process
/// per batch step to prevent memory overflow.
use alloc::vec::Vec;

/// Manual ceil for f32 (no_std compatible).
fn ceil_f32(x: f32) -> f32 {
    let i = x as i32;
    if x > i as f32 {
        (i + 1) as f32
    } else {
        i as f32
    }
}

/// Routes tokens to top-k expert FFN layers
pub struct MoeRouter {
    /// Total number of expert sub-networks
    pub num_experts: usize,
    /// How many experts each token is routed to
    pub top_k: usize,
    /// Gate weight matrix, shape [num_experts * hidden_dim], row-major
    pub gate_weights: Vec<f32>,
    /// Hidden dimension of the model
    pub hidden_dim: usize,
    /// Capacity factor: max tokens per expert = capacity_factor * (tokens / num_experts)
    pub capacity_factor: f32,
    /// Running load balance statistics: how many tokens each expert has processed
    pub expert_load: Vec<u64>,
    /// Total tokens routed through this router
    pub total_routed: u64,
    /// Cumulative load-balancing auxiliary loss
    pub aux_loss_sum: f32,
    /// Whether to use noise for exploration (during training)
    pub use_noise: bool,
    /// PRNG state for jitter noise
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

fn softmax_vec(v: &mut [f32]) {
    if v.is_empty() {
        return;
    }
    let mut max_v = v[0];
    for &x in v.iter() {
        if x > max_v {
            max_v = x;
        }
    }
    let mut sum = 0.0_f32;
    for x in v.iter_mut() {
        *x = fast_exp(*x - max_v);
        sum += *x;
    }
    if sum > 0.0 {
        let inv = 1.0 / sum;
        for x in v.iter_mut() {
            *x *= inv;
        }
    }
}

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

impl MoeRouter {
    /// Create a new MoE router.
    ///
    /// `num_experts` is the total expert count (e.g. 8, 16, 64).
    /// `top_k` is the number of experts each token is routed to (e.g. 2).
    pub fn new(num_experts: usize, top_k: usize) -> Self {
        Self::with_hidden(num_experts, top_k, 256)
    }

    /// Create with an explicit hidden dimension.
    pub fn with_hidden(num_experts: usize, top_k: usize, hidden_dim: usize) -> Self {
        // Initialise gate weights with small random values
        let gate_size = num_experts * hidden_dim;
        let mut gate_weights = Vec::with_capacity(gate_size);
        let mut rng = 0xFACE_D00D_u64;
        let scale = 1.0 / (hidden_dim as f32).max(1.0);
        for _ in 0..gate_size {
            let r = rand_f32(&mut rng) * 2.0 * scale - scale;
            gate_weights.push(r);
        }

        serial_println!(
            "    [moe] Created router: experts={}, top_k={}, hidden={}",
            num_experts,
            top_k,
            hidden_dim
        );

        MoeRouter {
            num_experts,
            top_k: top_k.min(num_experts),
            gate_weights,
            hidden_dim,
            capacity_factor: 1.25,
            expert_load: vec![0u64; num_experts],
            total_routed: 0,
            aux_loss_sum: 0.0,
            use_noise: false,
            rng_state: 0xABCD_1234_5678_EF01,
        }
    }

    /// Route a single hidden state to the top-k experts.
    ///
    /// `hidden` has length `hidden_dim`.
    /// Returns a sorted vec of (expert_index, gate_weight) pairs,
    /// with length <= top_k. Gate weights are normalised to sum to 1.
    pub fn route(&self, hidden: &[f32]) -> Vec<(usize, f32)> {
        if hidden.len() < self.hidden_dim || self.num_experts == 0 {
            return Vec::new();
        }

        // Compute gate scores: score[e] = gate_weights[e] . hidden
        let mut scores = vec![0.0_f32; self.num_experts];
        for e in 0..self.num_experts {
            let offset = e * self.hidden_dim;
            let mut dot = 0.0_f32;
            for j in 0..self.hidden_dim {
                if offset + j < self.gate_weights.len() {
                    dot += self.gate_weights[offset + j] * hidden[j];
                }
            }
            scores[e] = dot;
        }

        // Optionally add noise for load-balancing exploration
        if self.use_noise {
            let mut rng = self.rng_state;
            for score in scores.iter_mut() {
                let noise = (rand_f32(&mut rng) - 0.5) * 0.1;
                *score += noise;
            }
        }

        // Apply softmax to get gate probabilities
        softmax_vec(&mut scores);

        // Select top-k experts
        let mut indexed: Vec<(usize, f32)> =
            scores.iter().enumerate().map(|(i, &s)| (i, s)).collect();
        indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
        indexed.truncate(self.top_k);

        // Renormalise the selected gate weights to sum to 1
        let sum: f32 = indexed.iter().map(|&(_, w)| w).sum();
        if sum > 0.0 {
            let inv = 1.0 / sum;
            for item in indexed.iter_mut() {
                item.1 *= inv;
            }
        }

        indexed
    }

    /// Route a batch of hidden states and return per-token expert assignments.
    ///
    /// `hidden_batch` is flattened: length = batch_size * hidden_dim.
    /// Returns a vec of vec: outer is per-token, inner is (expert_id, weight).
    pub fn route_batch(
        &mut self,
        hidden_batch: &[f32],
        batch_size: usize,
    ) -> Vec<Vec<(usize, f32)>> {
        let mut assignments = Vec::with_capacity(batch_size);
        let mut expert_counts = vec![0usize; self.num_experts];
        let cap_raw = (batch_size as f32 * self.capacity_factor) / self.num_experts as f32;
        let capacity = ceil_f32(cap_raw) as usize;

        for t in 0..batch_size {
            let offset = t * self.hidden_dim;
            let end = (offset + self.hidden_dim).min(hidden_batch.len());
            if offset >= hidden_batch.len() {
                assignments.push(Vec::new());
                continue;
            }

            let routes = self.route(&hidden_batch[offset..end]);

            // Apply capacity factor: skip experts that are full
            let mut filtered = Vec::new();
            for (expert_id, weight) in routes {
                if expert_counts[expert_id] < capacity {
                    expert_counts[expert_id] += 1;
                    filtered.push((expert_id, weight));
                }
            }

            // If all selected experts are full, route to least-loaded expert
            if filtered.is_empty() && self.num_experts > 0 {
                let mut min_load = usize::MAX;
                let mut min_expert = 0;
                for (e, &count) in expert_counts.iter().enumerate() {
                    if count < min_load {
                        min_load = count;
                        min_expert = e;
                    }
                }
                expert_counts[min_expert] += 1;
                filtered.push((min_expert, 1.0));
            }

            assignments.push(filtered);
        }

        // Update load statistics
        self.total_routed += batch_size as u64;
        for (e, &count) in expert_counts.iter().enumerate() {
            if e < self.expert_load.len() {
                self.expert_load[e] += count as u64;
            }
        }

        // Compute auxiliary load-balancing loss
        self.aux_loss_sum += self.compute_balance_loss(&expert_counts, batch_size);

        assignments
    }

    /// Compute the load-balancing auxiliary loss.
    ///
    /// loss = num_experts * sum_e(fraction_tokens_e * mean_gate_prob_e)
    /// This encourages uniform distribution across experts.
    fn compute_balance_loss(&self, counts: &[usize], batch_size: usize) -> f32 {
        if batch_size == 0 || self.num_experts == 0 {
            return 0.0;
        }
        let n = self.num_experts as f32;
        let bs = batch_size as f32;
        let uniform = 1.0 / n;

        let mut loss = 0.0_f32;
        for &count in counts.iter() {
            let fraction = count as f32 / bs;
            // deviation from uniform
            let diff = fraction - uniform;
            loss += diff * diff;
        }
        loss * n
    }

    /// Get the load distribution as percentages.
    pub fn load_distribution(&self) -> Vec<f32> {
        let total = self.total_routed as f32;
        if total == 0.0 {
            return vec![0.0; self.num_experts];
        }
        self.expert_load
            .iter()
            .map(|&load| (load as f32 / total) * 100.0)
            .collect()
    }

    /// Get the coefficient of variation of expert load (0 = perfectly balanced).
    pub fn load_balance_cv(&self) -> f32 {
        if self.num_experts == 0 {
            return 0.0;
        }
        let n = self.num_experts as f32;
        let mean: f32 = self.expert_load.iter().map(|&x| x as f32).sum::<f32>() / n;
        if mean == 0.0 {
            return 0.0;
        }
        let variance: f32 = self
            .expert_load
            .iter()
            .map(|&x| {
                let d = x as f32 - mean;
                d * d
            })
            .sum::<f32>()
            / n;
        fast_sqrt(variance) / mean
    }

    /// Update gate weights via gradient descent for one step.
    ///
    /// `hidden` is the input, `grad_output` is the gradient from the
    /// expert outputs, `expert_assignments` is from route().
    pub fn update_gate(
        &mut self,
        hidden: &[f32],
        grad_output: &[f32],
        expert_assignments: &[(usize, f32)],
        lr: f32,
    ) {
        for &(expert_id, _gate_weight) in expert_assignments {
            let offset = expert_id * self.hidden_dim;
            for j in 0..self.hidden_dim.min(hidden.len()) {
                let grad_j = if j < grad_output.len() {
                    grad_output[j]
                } else {
                    0.0
                };
                let idx = offset + j;
                if idx < self.gate_weights.len() {
                    self.gate_weights[idx] -= lr * grad_j * hidden[j];
                }
            }
        }
    }

    /// Reset load counters.
    pub fn reset_stats(&mut self) {
        for load in self.expert_load.iter_mut() {
            *load = 0;
        }
        self.total_routed = 0;
        self.aux_loss_sum = 0.0;
    }
}

fn fast_sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let half = 0.5 * x;
    let mut i = x.to_bits();
    i = 0x5f37_59df - (i >> 1);
    let y = f32::from_bits(i);
    let y = y * (1.5 - half * y * y);
    let y = y * (1.5 - half * y * y);
    x * y
}

// ── Global Singleton ────────────────────────────────────────────────

struct MoeState {
    router: MoeRouter,
}

static MOE: Mutex<Option<MoeState>> = Mutex::new(None);

const DEFAULT_EXPERTS: usize = 8;
const DEFAULT_TOP_K: usize = 2;

pub fn init() {
    let router = MoeRouter::new(DEFAULT_EXPERTS, DEFAULT_TOP_K);
    let mut guard = MOE.lock();
    *guard = Some(MoeState { router });
    serial_println!(
        "    [moe] MoE subsystem initialised (experts={}, top_k={})",
        DEFAULT_EXPERTS,
        DEFAULT_TOP_K
    );
}

/// Route a hidden state through the global MoE router.
pub fn route_global(hidden: &[f32]) -> Vec<(usize, f32)> {
    let guard = MOE.lock();
    if let Some(state) = guard.as_ref() {
        state.router.route(hidden)
    } else {
        Vec::new()
    }
}
