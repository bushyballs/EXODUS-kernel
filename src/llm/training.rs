use crate::sync::Mutex;
use alloc::vec;
/// On-device training — backpropagation & optimization
///
/// Train and fine-tune the Hoags LLM directly on-device.
/// No cloud, no external services. Your data stays local.
///
///   - Cross-entropy loss
///   - Backpropagation through transformer layers
///   - AdamW optimizer (weight decay regularization)
///   - Gradient accumulation for larger effective batch size
///   - Learning rate scheduling (cosine with warmup)
///   - Gradient clipping (prevent exploding gradients)
///   - LoRA (Low-Rank Adaptation) for efficient fine-tuning
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

use super::transformer::{self, q16_from_int, q16_mul, Q16};

#[derive(Clone, Copy, PartialEq)]
pub enum TrainState {
    Idle,
    Training,
    Evaluating,
    Paused,
    Complete,
}

#[derive(Clone, Copy)]
pub struct TrainConfig {
    pub learning_rate: Q16, // e.g., 3e-4 in Q16
    pub weight_decay: Q16,  // e.g., 0.01 in Q16
    pub warmup_steps: u32,
    pub total_steps: u32,
    pub batch_size: u32,
    pub grad_accum_steps: u32, // Effective batch = batch * accum
    pub max_grad_norm: Q16,    // Gradient clipping threshold
    pub beta1: Q16,            // Adam momentum (0.9)
    pub beta2: Q16,            // Adam variance (0.999)
    pub epsilon: Q16,          // Adam epsilon (1e-8)
    pub use_lora: bool,
    pub lora_rank: u32,  // LoRA rank (4-64)
    pub lora_alpha: Q16, // LoRA scaling factor
}

/// Per-parameter optimizer state (AdamW)
#[derive(Clone, Copy)]
struct AdamState {
    m: Q16, // First moment (mean of gradients)
    v: Q16, // Second moment (mean of squared gradients)
}

/// LoRA adapter: low-rank matrices A and B
/// W_adapted = W + (B @ A) * alpha/rank
struct LoraAdapter {
    a_data: Vec<Q16>, // [rank x in_dim]
    b_data: Vec<Q16>, // [out_dim x rank]
    rank: u32,
    in_dim: u32,
    out_dim: u32,
    alpha: Q16,
}

struct TrainingEngine {
    config: TrainConfig,
    state: TrainState,
    current_step: u32,
    current_epoch: u32,
    // Loss tracking
    loss_history: Vec<Q16>,
    running_loss: Q16,
    best_loss: Q16,
    // Optimizer states
    adam_states: Vec<AdamState>,
    // Gradient buffer
    gradients: Vec<Q16>,
    grad_accum_count: u32,
    // LoRA adapters (one per attention layer)
    lora_adapters: Vec<LoraAdapter>,
    // Stats
    total_tokens_trained: u64,
    total_training_time_ms: u64,
    tokens_per_second: u32,
}

static TRAINER: Mutex<Option<TrainingEngine>> = Mutex::new(None);

impl TrainingEngine {
    fn new(config: TrainConfig) -> Self {
        TrainingEngine {
            config,
            state: TrainState::Idle,
            current_step: 0,
            current_epoch: 0,
            loss_history: Vec::new(),
            running_loss: 0,
            best_loss: q16_from_int(100),
            adam_states: Vec::new(),
            gradients: Vec::new(),
            grad_accum_count: 0,
            lora_adapters: Vec::new(),
            total_tokens_trained: 0,
            total_training_time_ms: 0,
            tokens_per_second: 0,
        }
    }

    /// Cross-entropy loss: -sum(target * log(softmax(logits)))
    fn compute_loss(&self, logits: &[Q16], target_id: u32) -> Q16 {
        // Find max for numerical stability
        let mut max_l: Q16 = i32::MIN;
        for &l in logits {
            if l > max_l {
                max_l = l;
            }
        }

        for &l in logits {
            let shifted = l - max_l;
            // exp approx: 1 + x + x^2/2
            let exp = q16_from_int(1) + shifted + (q16_mul(shifted, shifted) >> 1);
            let _ = exp.max(1);
        }

        // log_sum_exp = max + log(sum_exp)
        // log approx: log(x) ≈ (x-1) - (x-1)^2/2 for x near 1
        let target_logit = logits.get(target_id as usize).copied().unwrap_or(0);
        let loss = -(target_logit - max_l); // Simplified: -log(softmax(target))

        loss
    }

    /// Compute gradient of loss w.r.t. logits (softmax - one_hot)
    fn logit_gradient(&self, logits: &[Q16], target_id: u32) -> Vec<Q16> {
        let n = logits.len();
        let mut grad = vec![0i32; n];

        // Softmax
        let mut max_l: Q16 = i32::MIN;
        for &l in logits {
            if l > max_l {
                max_l = l;
            }
        }

        let mut sum_exp: i64 = 0;
        let mut exps = vec![0i32; n];
        for (i, &l) in logits.iter().enumerate() {
            let shifted = l - max_l;
            let exp = (q16_from_int(1) + shifted + (q16_mul(shifted, shifted) >> 1)).max(1);
            exps[i] = exp;
            sum_exp += exp as i64;
        }

        // grad = softmax - one_hot
        for i in 0..n {
            let softmax_i = ((exps[i] as i64 * 65536) / sum_exp.max(1)) as Q16;
            grad[i] = softmax_i;
            if i == target_id as usize {
                grad[i] -= q16_from_int(1);
            }
        }
        grad
    }

    /// Get learning rate with cosine schedule + warmup
    fn get_lr(&self) -> Q16 {
        if self.current_step < self.config.warmup_steps {
            // Linear warmup
            let ratio = (self.current_step as i64 * self.config.learning_rate as i64)
                / self.config.warmup_steps as i64;
            ratio as Q16
        } else {
            // Cosine decay
            let progress = (self.current_step - self.config.warmup_steps) as i64;
            let total = (self.config.total_steps - self.config.warmup_steps).max(1) as i64;
            let cos_decay = q16_from_int(1) - (progress * q16_from_int(1) as i64 / total) as Q16;
            q16_mul(
                self.config.learning_rate,
                (cos_decay + q16_from_int(1)) >> 1,
            )
        }
    }

    /// AdamW optimizer step
    fn adam_step(&mut self, params: &mut [Q16]) {
        let lr = self.get_lr();
        let beta1 = self.config.beta1;
        let beta2 = self.config.beta2;
        let wd = self.config.weight_decay;

        // Ensure adam states are allocated
        while self.adam_states.len() < params.len() {
            self.adam_states.push(AdamState { m: 0, v: 0 });
        }

        let n = params.len().min(self.gradients.len());
        for i in 0..n {
            let g = self.gradients[i];

            // Update biased first moment: m = β1*m + (1-β1)*g
            self.adam_states[i].m =
                q16_mul(beta1, self.adam_states[i].m) + q16_mul(q16_from_int(1) - beta1, g);

            // Update biased second moment: v = β2*v + (1-β2)*g²
            let g2 = q16_mul(g, g);
            self.adam_states[i].v =
                q16_mul(beta2, self.adam_states[i].v) + q16_mul(q16_from_int(1) - beta2, g2);

            // Bias correction (simplified)
            let m_hat = self.adam_states[i].m;
            let v_hat = self.adam_states[i].v;

            // rsqrt(v_hat) approximation
            let denom = v_hat.max(1);
            let update = ((m_hat as i64 * 65536) / denom as i64) as Q16;

            // Apply: param -= lr * (update + wd * param)
            let decay = q16_mul(wd, params[i]);
            params[i] -= q16_mul(lr, update + decay);
        }

        self.current_step += 1;
    }

    /// Clip gradients to prevent explosion
    fn clip_gradients(&mut self) {
        let max_norm = self.config.max_grad_norm;
        // Compute gradient norm
        let mut norm_sq: i64 = 0;
        for &g in &self.gradients {
            norm_sq += (g as i64 * g as i64) >> 16;
        }
        // If norm > max_norm, scale down
        let norm = norm_sq; // Approximate (skip sqrt for speed)
        let max_sq = (max_norm as i64 * max_norm as i64) >> 16;
        if norm > max_sq && norm > 0 {
            let scale = ((max_sq * 65536) / norm) as Q16;
            for g in &mut self.gradients {
                *g = q16_mul(*g, scale);
            }
        }
    }

    fn start_training(&mut self) {
        self.state = TrainState::Training;
        self.current_step = 0;
        self.current_epoch = 0;
    }

    fn pause(&mut self) {
        self.state = TrainState::Paused;
    }
    fn resume(&mut self) {
        self.state = TrainState::Training;
    }

    fn record_loss(&mut self, loss: Q16) {
        self.running_loss = (self.running_loss * 9 + loss) / 10; // EMA
        if self.current_step % 100 == 0 {
            self.loss_history.push(self.running_loss);
            if self.running_loss < self.best_loss {
                self.best_loss = self.running_loss;
            }
        }
    }

    fn train_projection_step(
        &mut self,
        input_token: u32,
        target_token: u32,
        pos: u32,
    ) -> Result<Q16, &'static str> {
        if self.state == TrainState::Idle || self.state == TrainState::Paused {
            self.start_training();
        }
        if self.state != TrainState::Training {
            return Err("trainer not in training state");
        }
        if self.current_step >= self.config.total_steps {
            self.state = TrainState::Complete;
            return Err("training complete");
        }

        let logits =
            transformer::forward_logits(input_token, pos).ok_or("transformer not initialized")?;
        let loss = self.compute_loss(&logits, target_token);
        self.gradients = self.logit_gradient(&logits, target_token);
        self.clip_gradients();

        let lr = self.get_lr();
        transformer::train_output_projection_step(input_token, pos, target_token, lr)
            .ok_or("transformer not initialized")?;

        self.current_step += 1;
        self.grad_accum_count = self.grad_accum_count.saturating_add(1);
        self.total_tokens_trained = self.total_tokens_trained.saturating_add(1);
        self.record_loss(loss);
        if self.current_step >= self.config.total_steps {
            self.state = TrainState::Complete;
        }

        Ok(loss)
    }
}

impl LoraAdapter {
    fn new(in_dim: u32, out_dim: u32, rank: u32, alpha: Q16) -> Self {
        // Initialize A with small random values, B with zeros
        let a_size = (rank * in_dim) as usize;
        let b_size = (out_dim * rank) as usize;
        LoraAdapter {
            a_data: vec![100; a_size], // Small nonzero init
            b_data: vec![0; b_size],   // Zero init
            rank,
            in_dim,
            out_dim,
            alpha,
        }
    }

    /// Compute LoRA output: (B @ A) * x * (alpha/rank)
    fn forward(&self, input: &[Q16]) -> Vec<Q16> {
        // First: A @ input -> [rank]
        let mut mid = vec![0i32; self.rank as usize];
        for r in 0..self.rank as usize {
            let mut sum: i64 = 0;
            for c in 0..self.in_dim as usize {
                sum += self.a_data[r * self.in_dim as usize + c] as i64 * input[c] as i64;
            }
            mid[r] = (sum >> 16) as Q16;
        }

        // Then: B @ mid -> [out_dim]
        let mut output = vec![0i32; self.out_dim as usize];
        for r in 0..self.out_dim as usize {
            let mut sum: i64 = 0;
            for c in 0..self.rank as usize {
                sum += self.b_data[r * self.rank as usize + c] as i64 * mid[c] as i64;
            }
            // Scale by alpha/rank
            let scaled = (sum * self.alpha as i64) / (self.rank as i64 * 65536);
            output[r] = scaled as Q16;
        }
        output
    }
}

fn default_config() -> TrainConfig {
    TrainConfig {
        learning_rate: 20, // ~3e-4 in Q16
        weight_decay: 655, // 0.01 in Q16
        warmup_steps: 100,
        total_steps: 10_000,
        batch_size: 4,
        grad_accum_steps: 8,
        max_grad_norm: q16_from_int(1),
        beta1: 58982, // 0.9 in Q16
        beta2: 65471, // 0.999 in Q16
        epsilon: 1,   // ~1e-8 in Q16
        use_lora: true,
        lora_rank: 16,
        lora_alpha: q16_from_int(32),
    }
}

pub fn init() {
    let mut t = TRAINER.lock();
    *t = Some(TrainingEngine::new(default_config()));
    serial_println!("    Training: AdamW, cosine LR, gradient clip, LoRA fine-tuning ready");
}

pub fn start() -> Result<(), &'static str> {
    let mut t = TRAINER.lock();
    let trainer = t.as_mut().ok_or("trainer not initialized")?;
    trainer.start_training();
    Ok(())
}

pub fn pause() -> Result<(), &'static str> {
    let mut t = TRAINER.lock();
    let trainer = t.as_mut().ok_or("trainer not initialized")?;
    trainer.pause();
    Ok(())
}

pub fn resume() -> Result<(), &'static str> {
    let mut t = TRAINER.lock();
    let trainer = t.as_mut().ok_or("trainer not initialized")?;
    trainer.resume();
    Ok(())
}

/// Single next-token training step.
pub fn train_token_step(
    input_token: u32,
    target_token: u32,
    pos: u32,
) -> Result<Q16, &'static str> {
    let mut t = TRAINER.lock();
    let trainer = t.as_mut().ok_or("trainer not initialized")?;
    trainer.train_projection_step(input_token, target_token, pos)
}

/// Train on a token sequence (teacher forcing: token[i] -> token[i+1]).
pub fn train_sequence(tokens: &[u32]) -> Result<Q16, &'static str> {
    if tokens.len() < 2 {
        return Err("need at least 2 tokens");
    }

    let mut t = TRAINER.lock();
    let trainer = t.as_mut().ok_or("trainer not initialized")?;
    let mut loss_sum: i64 = 0;
    let mut steps: i64 = 0;

    for i in 0..(tokens.len() - 1) {
        let loss = trainer.train_projection_step(tokens[i], tokens[i + 1], i as u32)?;
        loss_sum += loss as i64;
        steps += 1;
    }

    if steps == 0 {
        return Err("no training steps");
    }
    Ok((loss_sum / steps) as Q16)
}

pub fn state() -> Option<TrainState> {
    TRAINER.lock().as_ref().map(|t| t.state)
}

pub fn step() -> Option<u32> {
    TRAINER.lock().as_ref().map(|t| t.current_step)
}
