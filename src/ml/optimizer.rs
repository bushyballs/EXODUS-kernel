use crate::sync::Mutex;
/// Training optimizers for Genesis ML runtime
///
/// Implements gradient-based optimization algorithms for training neural
/// networks on-device. Supports SGD (with momentum), Adam, AdamW, learning
/// rate schedulers, gradient clipping, and weight decay — all in Q16
/// fixed-point arithmetic.
///
/// Inspired by: PyTorch optim, TensorFlow optimizers. All code is original.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

use super::neural_net::NeuralNet;

// ---------------------------------------------------------------------------
// Q16 fixed-point constants and helpers
// ---------------------------------------------------------------------------

const Q16_ONE: i32 = 65536;
const Q16_ZERO: i32 = 0;

fn q16_mul(a: i32, b: i32) -> i32 {
    (((a as i64) * (b as i64)) >> 16) as i32
}

fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << 16) / (b as i64)) as i32
}

fn q16_from_int(x: i32) -> i32 {
    x << 16
}

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

fn q16_abs(x: i32) -> i32 {
    if x < 0 {
        -x
    } else {
        x
    }
}

// ---------------------------------------------------------------------------
// Learning rate schedulers
// ---------------------------------------------------------------------------

/// Learning rate scheduling strategies
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LrSchedule {
    /// Constant learning rate
    Constant,
    /// Linear decay from initial to min_lr over total_steps
    LinearDecay,
    /// Step decay: multiply by gamma every step_size epochs
    StepDecay,
    /// Exponential decay: lr * gamma^epoch
    ExponentialDecay,
    /// Cosine annealing to min_lr
    CosineAnnealing,
    /// Warmup for warmup_steps, then constant
    Warmup,
    /// Warmup then linear decay
    WarmupLinearDecay,
    /// One-cycle policy (warmup then decay)
    OneCycle,
}

/// Learning rate scheduler state
pub struct LrScheduler {
    pub schedule: LrSchedule,
    pub initial_lr: i32,  // Q16
    pub current_lr: i32,  // Q16
    pub min_lr: i32,      // Q16
    pub gamma: i32,       // Q16 decay factor
    pub step_size: usize, // epochs between steps
    pub warmup_steps: usize,
    pub total_steps: usize,
    pub current_step: usize,
}

impl LrScheduler {
    pub fn new(schedule: LrSchedule, initial_lr: i32) -> Self {
        LrScheduler {
            schedule,
            initial_lr,
            current_lr: initial_lr,
            min_lr: initial_lr / 100,  // 1% of initial
            gamma: (Q16_ONE * 9) / 10, // 0.9 in Q16 = 58982
            step_size: 10,
            warmup_steps: 100,
            total_steps: 1000,
            current_step: 0,
        }
    }

    /// Set minimum learning rate
    pub fn with_min_lr(mut self, min_lr: i32) -> Self {
        self.min_lr = min_lr;
        self
    }

    /// Set decay factor
    pub fn with_gamma(mut self, gamma: i32) -> Self {
        self.gamma = gamma;
        self
    }

    /// Set step size for step decay
    pub fn with_step_size(mut self, step_size: usize) -> Self {
        self.step_size = step_size;
        self
    }

    /// Set warmup steps
    pub fn with_warmup(mut self, warmup_steps: usize) -> Self {
        self.warmup_steps = warmup_steps;
        self
    }

    /// Set total training steps
    pub fn with_total_steps(mut self, total: usize) -> Self {
        self.total_steps = total;
        self
    }

    /// Advance one step and return updated learning rate
    pub fn step(&mut self) -> i32 {
        self.current_step = self.current_step.saturating_add(1);
        self.current_lr = self.compute_lr();
        self.current_lr
    }

    /// Compute learning rate for current step
    fn compute_lr(&self) -> i32 {
        let step = self.current_step;
        match self.schedule {
            LrSchedule::Constant => self.initial_lr,

            LrSchedule::LinearDecay => {
                if self.total_steps == 0 {
                    return self.initial_lr;
                }
                let progress = q16_div(
                    q16_from_int(step as i32),
                    q16_from_int(self.total_steps as i32),
                );
                let range = self.initial_lr - self.min_lr;
                let decay = q16_mul(range, progress);
                let lr = self.initial_lr - decay;
                if lr < self.min_lr {
                    self.min_lr
                } else {
                    lr
                }
            }

            LrSchedule::StepDecay => {
                if self.step_size == 0 {
                    return self.initial_lr;
                }
                let num_decays = step / self.step_size;
                let mut lr = self.initial_lr;
                for _ in 0..num_decays {
                    lr = q16_mul(lr, self.gamma);
                }
                if lr < self.min_lr {
                    self.min_lr
                } else {
                    lr
                }
            }

            LrSchedule::ExponentialDecay => {
                let mut lr = self.initial_lr;
                // Apply gamma once per step (approximation)
                let decays = if step > 100 { 100 } else { step };
                for _ in 0..decays {
                    lr = q16_mul(lr, self.gamma);
                }
                if lr < self.min_lr {
                    self.min_lr
                } else {
                    lr
                }
            }

            LrSchedule::CosineAnnealing => {
                if self.total_steps == 0 {
                    return self.initial_lr;
                }
                // cos(pi * t / T) approximated with parabolic curve
                let progress = q16_div(
                    q16_from_int(step as i32),
                    q16_from_int(self.total_steps as i32),
                );
                // Parabolic approx of (1 + cos(pi*t))/2 = 1 - (t/T)^2 roughly
                let one_minus_p = Q16_ONE - progress;
                let cosine_factor = q16_mul(one_minus_p, one_minus_p);
                let range = self.initial_lr - self.min_lr;
                self.min_lr + q16_mul(range, cosine_factor)
            }

            LrSchedule::Warmup => {
                if step < self.warmup_steps && self.warmup_steps > 0 {
                    q16_div(
                        q16_mul(self.initial_lr, q16_from_int(step as i32)),
                        q16_from_int(self.warmup_steps as i32),
                    )
                } else {
                    self.initial_lr
                }
            }

            LrSchedule::WarmupLinearDecay => {
                if step < self.warmup_steps && self.warmup_steps > 0 {
                    q16_div(
                        q16_mul(self.initial_lr, q16_from_int(step as i32)),
                        q16_from_int(self.warmup_steps as i32),
                    )
                } else if self.total_steps > self.warmup_steps {
                    let decay_steps = self.total_steps - self.warmup_steps;
                    let decay_step = step - self.warmup_steps;
                    let progress = q16_div(
                        q16_from_int(decay_step as i32),
                        q16_from_int(decay_steps as i32),
                    );
                    let range = self.initial_lr - self.min_lr;
                    let lr = self.initial_lr - q16_mul(range, progress);
                    if lr < self.min_lr {
                        self.min_lr
                    } else {
                        lr
                    }
                } else {
                    self.initial_lr
                }
            }

            LrSchedule::OneCycle => {
                let half = self.total_steps / 2;
                if half == 0 {
                    return self.initial_lr;
                }
                if step <= half {
                    // Warmup phase: min_lr -> initial_lr
                    let progress = q16_div(q16_from_int(step as i32), q16_from_int(half as i32));
                    self.min_lr + q16_mul(self.initial_lr - self.min_lr, progress)
                } else {
                    // Decay phase: initial_lr -> min_lr
                    let decay_step = step - half;
                    let progress =
                        q16_div(q16_from_int(decay_step as i32), q16_from_int(half as i32));
                    let lr = self.initial_lr - q16_mul(self.initial_lr - self.min_lr, progress);
                    if lr < self.min_lr {
                        self.min_lr
                    } else {
                        lr
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Gradient clipping
// ---------------------------------------------------------------------------

/// Gradient clipping strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GradClip {
    /// No clipping
    None,
    /// Clip by absolute value
    ClipValue(i32), // Q16 max magnitude
    /// Clip by global norm
    ClipNorm(i32), // Q16 max norm
}

/// Clip gradients by value
pub fn clip_grad_value(grads: &mut [i32], max_val: i32) {
    for g in grads.iter_mut() {
        if *g > max_val {
            *g = max_val;
        } else if *g < -max_val {
            *g = -max_val;
        }
    }
}

/// Clip gradients by global L2 norm
pub fn clip_grad_norm(grads: &mut [i32], max_norm: i32) {
    // Compute L2 norm
    let mut norm_sq: i64 = 0;
    for &g in grads.iter() {
        norm_sq += ((g as i64) * (g as i64)) >> 16;
    }
    let norm = q16_sqrt(norm_sq as i32);

    if norm > max_norm && norm > 0 {
        let scale = q16_div(max_norm, norm);
        for g in grads.iter_mut() {
            *g = q16_mul(*g, scale);
        }
    }
}

/// Apply gradient clipping based on strategy
fn apply_grad_clip(grads: &mut [i32], clip: GradClip) {
    match clip {
        GradClip::None => {}
        GradClip::ClipValue(max_val) => clip_grad_value(grads, max_val),
        GradClip::ClipNorm(max_norm) => clip_grad_norm(grads, max_norm),
    }
}

// ---------------------------------------------------------------------------
// Optimizer types
// ---------------------------------------------------------------------------

/// Optimizer algorithm selection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptimizerType {
    SGD,
    SGDMomentum,
    Adam,
    AdamW,
}

/// Per-parameter optimizer state (for Adam/AdamW)
struct ParamState {
    /// First moment (mean of gradients)
    m: Vec<i32>,
    /// Second moment (mean of squared gradients)
    v: Vec<i32>,
    /// For SGD momentum
    velocity: Vec<i32>,
}

impl ParamState {
    fn new(size: usize) -> Self {
        ParamState {
            m: alloc::vec![Q16_ZERO; size],
            v: alloc::vec![Q16_ZERO; size],
            velocity: alloc::vec![Q16_ZERO; size],
        }
    }
}

/// Main optimizer struct
pub struct Optimizer {
    pub opt_type: OptimizerType,
    pub lr_scheduler: LrScheduler,
    pub grad_clip: GradClip,
    pub weight_decay: i32, // Q16 (L2 regularization strength)
    pub momentum: i32,     // Q16 (for SGD momentum)
    pub beta1: i32,        // Q16 (Adam first moment decay)
    pub beta2: i32,        // Q16 (Adam second moment decay)
    pub epsilon: i32,      // Q16 (Adam numerical stability)
    pub step_count: usize,
    /// Per-layer parameter states
    param_states: Vec<(ParamState, ParamState)>, // (weight_state, bias_state)
}

impl Optimizer {
    /// Create SGD optimizer
    pub fn sgd(lr: i32) -> Self {
        Optimizer {
            opt_type: OptimizerType::SGD,
            lr_scheduler: LrScheduler::new(LrSchedule::Constant, lr),
            grad_clip: GradClip::None,
            weight_decay: Q16_ZERO,
            momentum: Q16_ZERO,
            beta1: Q16_ZERO,
            beta2: Q16_ZERO,
            epsilon: 1, // tiny Q16
            step_count: 0,
            param_states: Vec::new(),
        }
    }

    /// Create SGD with momentum optimizer
    pub fn sgd_momentum(lr: i32, momentum: i32) -> Self {
        Optimizer {
            opt_type: OptimizerType::SGDMomentum,
            lr_scheduler: LrScheduler::new(LrSchedule::Constant, lr),
            grad_clip: GradClip::None,
            weight_decay: Q16_ZERO,
            momentum,
            beta1: Q16_ZERO,
            beta2: Q16_ZERO,
            epsilon: 1,
            step_count: 0,
            param_states: Vec::new(),
        }
    }

    /// Create Adam optimizer
    pub fn adam(lr: i32) -> Self {
        Optimizer {
            opt_type: OptimizerType::Adam,
            lr_scheduler: LrScheduler::new(LrSchedule::Constant, lr),
            grad_clip: GradClip::None,
            weight_decay: Q16_ZERO,
            momentum: Q16_ZERO,
            beta1: 58982, // 0.9 in Q16
            beta2: 65209, // 0.995 in Q16 (close to 0.999)
            epsilon: 7,   // ~1e-4 in Q16
            step_count: 0,
            param_states: Vec::new(),
        }
    }

    /// Create AdamW optimizer (Adam with decoupled weight decay)
    pub fn adamw(lr: i32, weight_decay: i32) -> Self {
        Optimizer {
            opt_type: OptimizerType::AdamW,
            lr_scheduler: LrScheduler::new(LrSchedule::Constant, lr),
            grad_clip: GradClip::None,
            weight_decay,
            momentum: Q16_ZERO,
            beta1: 58982, // 0.9 in Q16
            beta2: 65209, // 0.995 in Q16
            epsilon: 7,
            step_count: 0,
            param_states: Vec::new(),
        }
    }

    /// Set learning rate scheduler
    pub fn with_scheduler(mut self, scheduler: LrScheduler) -> Self {
        self.lr_scheduler = scheduler;
        self
    }

    /// Set gradient clipping
    pub fn with_grad_clip(mut self, clip: GradClip) -> Self {
        self.grad_clip = clip;
        self
    }

    /// Set weight decay
    pub fn with_weight_decay(mut self, decay: i32) -> Self {
        self.weight_decay = decay;
        self
    }

    /// Initialize parameter states for a network
    pub fn init_for_net(&mut self, net: &NeuralNet) {
        self.param_states.clear();
        for layer in &net.layers {
            let w_state = ParamState::new(layer.weights.len());
            let b_state = ParamState::new(layer.bias.len());
            self.param_states.push((w_state, b_state));
        }
    }

    /// Perform one optimization step on the network
    pub fn step(&mut self, net: &mut NeuralNet) {
        self.step_count = self.step_count.saturating_add(1);
        let lr = self.lr_scheduler.step();

        // Ensure param states are initialized
        if self.param_states.len() != net.layers.len() {
            self.init_for_net(net);
        }

        for (layer_idx, layer) in net.layers.iter_mut().enumerate() {
            if layer.weights.is_empty() && layer.bias.is_empty() {
                continue;
            }

            // Apply gradient clipping
            apply_grad_clip(&mut layer.weight_grad, self.grad_clip);
            apply_grad_clip(&mut layer.bias_grad, self.grad_clip);

            let (ref mut w_state, ref mut b_state) = self.param_states[layer_idx];

            match self.opt_type {
                OptimizerType::SGD => {
                    sgd_update(
                        &mut layer.weights,
                        &layer.weight_grad,
                        lr,
                        self.weight_decay,
                    );
                    sgd_update(&mut layer.bias, &layer.bias_grad, lr, Q16_ZERO);
                }
                OptimizerType::SGDMomentum => {
                    sgd_momentum_update(
                        &mut layer.weights,
                        &layer.weight_grad,
                        &mut w_state.velocity,
                        lr,
                        self.momentum,
                        self.weight_decay,
                    );
                    sgd_momentum_update(
                        &mut layer.bias,
                        &layer.bias_grad,
                        &mut b_state.velocity,
                        lr,
                        self.momentum,
                        Q16_ZERO,
                    );
                }
                OptimizerType::Adam => {
                    adam_update(
                        &mut layer.weights,
                        &layer.weight_grad,
                        &mut w_state.m,
                        &mut w_state.v,
                        lr,
                        self.beta1,
                        self.beta2,
                        self.epsilon,
                        self.step_count,
                        self.weight_decay,
                        false,
                    );
                    adam_update(
                        &mut layer.bias,
                        &layer.bias_grad,
                        &mut b_state.m,
                        &mut b_state.v,
                        lr,
                        self.beta1,
                        self.beta2,
                        self.epsilon,
                        self.step_count,
                        Q16_ZERO,
                        false,
                    );
                }
                OptimizerType::AdamW => {
                    adam_update(
                        &mut layer.weights,
                        &layer.weight_grad,
                        &mut w_state.m,
                        &mut w_state.v,
                        lr,
                        self.beta1,
                        self.beta2,
                        self.epsilon,
                        self.step_count,
                        self.weight_decay,
                        true,
                    );
                    adam_update(
                        &mut layer.bias,
                        &layer.bias_grad,
                        &mut b_state.m,
                        &mut b_state.v,
                        lr,
                        self.beta1,
                        self.beta2,
                        self.epsilon,
                        self.step_count,
                        Q16_ZERO,
                        true,
                    );
                }
            }
        }
    }

    /// Get current learning rate
    pub fn current_lr(&self) -> i32 {
        self.lr_scheduler.current_lr
    }

    /// Get total step count
    pub fn total_steps(&self) -> usize {
        self.step_count
    }
}

// ---------------------------------------------------------------------------
// Update rules
// ---------------------------------------------------------------------------

/// Vanilla SGD: w = w - lr * grad - lr * wd * w
fn sgd_update(weights: &mut [i32], grads: &[i32], lr: i32, weight_decay: i32) {
    for i in 0..weights.len().min(grads.len()) {
        let mut grad = grads[i];
        if weight_decay > 0 {
            grad += q16_mul(weight_decay, weights[i]);
        }
        weights[i] -= q16_mul(lr, grad);
    }
}

/// SGD with momentum: v = mu*v + grad; w = w - lr * v
fn sgd_momentum_update(
    weights: &mut [i32],
    grads: &[i32],
    velocity: &mut [i32],
    lr: i32,
    momentum: i32,
    weight_decay: i32,
) {
    for i in 0..weights.len().min(grads.len()) {
        let mut grad = grads[i];
        if weight_decay > 0 {
            grad += q16_mul(weight_decay, weights[i]);
        }
        velocity[i] = q16_mul(momentum, velocity[i]) + grad;
        weights[i] -= q16_mul(lr, velocity[i]);
    }
}

/// Adam / AdamW update
fn adam_update(
    weights: &mut [i32],
    grads: &[i32],
    m: &mut [i32],
    v: &mut [i32],
    lr: i32,
    beta1: i32,
    beta2: i32,
    epsilon: i32,
    step: usize,
    weight_decay: i32,
    decoupled_wd: bool,
) {
    // Bias correction factors
    // beta1^t and beta2^t via repeated multiplication
    let mut beta1_t = Q16_ONE;
    let mut beta2_t = Q16_ONE;
    let effective_steps = if step > 50 { 50 } else { step };
    for _ in 0..effective_steps {
        beta1_t = q16_mul(beta1_t, beta1);
        beta2_t = q16_mul(beta2_t, beta2);
    }
    let bc1 = Q16_ONE - beta1_t; // 1 - beta1^t
    let bc2 = Q16_ONE - beta2_t; // 1 - beta2^t
    if bc1 == 0 || bc2 == 0 {
        return;
    }

    for i in 0..weights.len().min(grads.len()) {
        let grad = grads[i];

        // Update biased first moment: m = beta1 * m + (1-beta1) * grad
        m[i] = q16_mul(beta1, m[i]) + q16_mul(Q16_ONE - beta1, grad);
        // Update biased second moment: v = beta2 * v + (1-beta2) * grad^2
        let grad_sq = q16_mul(grad, grad);
        v[i] = q16_mul(beta2, v[i]) + q16_mul(Q16_ONE - beta2, grad_sq);

        // Bias-corrected estimates
        let m_hat = q16_div(m[i], bc1);
        let v_hat = q16_div(v[i], bc2);

        // Update: w = w - lr * m_hat / (sqrt(v_hat) + eps)
        let denom = q16_sqrt(v_hat) + epsilon;
        let update = q16_div(q16_mul(lr, m_hat), denom);
        weights[i] -= update;

        // Weight decay
        if weight_decay > 0 {
            if decoupled_wd {
                // AdamW: decoupled weight decay
                weights[i] -= q16_mul(lr, q16_mul(weight_decay, weights[i]));
            }
            // For standard Adam, weight decay is already in the gradient
        }
    }
}

// ---------------------------------------------------------------------------
// Loss functions
// ---------------------------------------------------------------------------

/// Compute mean squared error loss (returns Q16 scalar)
pub fn mse_loss(predicted: &[i32], target: &[i32]) -> i32 {
    if predicted.is_empty() {
        return Q16_ZERO;
    }
    let n = predicted.len().min(target.len());
    let mut sum: i64 = 0;
    for i in 0..n {
        let diff = predicted[i] - target[i];
        sum += ((diff as i64) * (diff as i64)) >> 16;
    }
    (sum / (n as i64)) as i32
}

/// Compute MSE loss gradient
pub fn mse_loss_grad(predicted: &[i32], target: &[i32]) -> Vec<i32> {
    let n = predicted.len().min(target.len());
    let mut grad = Vec::with_capacity(n);
    let scale = if n > 0 {
        q16_div(q16_from_int(2), q16_from_int(n as i32))
    } else {
        Q16_ZERO
    };
    for i in 0..n {
        let diff = predicted[i] - target[i];
        grad.push(q16_mul(scale, diff));
    }
    grad
}

/// Cross-entropy loss (predicted should be softmax output, target is one-hot)
pub fn cross_entropy_loss(predicted: &[i32], target: &[i32]) -> i32 {
    let n = predicted.len().min(target.len());
    let mut loss: i64 = 0;
    for i in 0..n {
        if target[i] > 0 {
            // -log(predicted[i]) approximated
            let p = if predicted[i] < 1 { 1 } else { predicted[i] }; // clamp
            let log_p = q16_log_approx(p);
            loss -= (log_p as i64) * (target[i] as i64) >> 16;
        }
    }
    (loss / (n as i64).max(1)) as i32
}

/// Cross-entropy loss gradient (predicted - target for softmax+CE)
pub fn cross_entropy_grad(predicted: &[i32], target: &[i32]) -> Vec<i32> {
    let n = predicted.len().min(target.len());
    let mut grad = Vec::with_capacity(n);
    for i in 0..n {
        grad.push(predicted[i] - target[i]);
    }
    grad
}

/// Approximate natural log in Q16: ln(x/Q16_ONE) * Q16_ONE
fn q16_log_approx(x: i32) -> i32 {
    if x <= 0 {
        return q16_from_int(-10);
    } // large negative
      // Use the identity: ln(x) ~= 2*(x-1)/(x+1) for x near 1
      // For Q16: x is in Q16, so x/Q16_ONE is the real value
    let num = x - Q16_ONE;
    let den = x + Q16_ONE;
    if den == 0 {
        return Q16_ZERO;
    }
    let ratio = q16_div(num, den);
    ratio << 1 // 2 * ratio
}

// ---------------------------------------------------------------------------
// Training loop helper
// ---------------------------------------------------------------------------

/// Training statistics for one epoch
pub struct EpochStats {
    pub epoch: usize,
    pub avg_loss: i32, // Q16
    pub lr: i32,       // Q16
    pub num_batches: usize,
}

/// Run one training epoch
pub fn train_epoch(
    net: &mut NeuralNet,
    optimizer: &mut Optimizer,
    inputs: &[Vec<i32>],
    targets: &[Vec<i32>],
    epoch: usize,
) -> EpochStats {
    net.is_training = true;
    let mut total_loss: i64 = 0;
    let num_batches = inputs.len().min(targets.len());

    for i in 0..num_batches {
        // Zero gradients
        net.zero_grad();

        // Forward pass
        let output = net.forward(&inputs[i]);

        // Compute loss and gradient
        let loss = mse_loss(&output, &targets[i]);
        total_loss += loss as i64;

        let loss_grad = mse_loss_grad(&output, &targets[i]);

        // Backward pass
        net.backward(&loss_grad);

        // Optimizer step
        optimizer.step(net);
    }

    net.is_training = false;

    let avg_loss = if num_batches > 0 {
        (total_loss / (num_batches as i64)) as i32
    } else {
        Q16_ZERO
    };

    EpochStats {
        epoch,
        avg_loss,
        lr: optimizer.current_lr(),
        num_batches,
    }
}

// ---------------------------------------------------------------------------
// Global state and init
// ---------------------------------------------------------------------------

/// Global training state tracker
pub struct TrainingState {
    pub total_epochs: usize,
    pub total_steps: usize,
    pub best_loss: i32, // Q16
    pub is_training: bool,
}

impl TrainingState {
    const fn new() -> Self {
        TrainingState {
            total_epochs: 0,
            total_steps: 0,
            best_loss: 0x7FFF_FFFF, // max i32 as "infinity"
            is_training: false,
        }
    }

    pub fn update(&mut self, stats: &EpochStats) {
        self.total_epochs = self.total_epochs.saturating_add(1);
        self.total_steps += stats.num_batches;
        if stats.avg_loss < self.best_loss {
            self.best_loss = stats.avg_loss;
        }
    }
}

static TRAINING_STATE: Mutex<TrainingState> = Mutex::new(TrainingState::new());

/// Get a copy of training statistics
pub fn get_training_stats() -> (usize, usize, i32) {
    let state = TRAINING_STATE.lock();
    (state.total_epochs, state.total_steps, state.best_loss)
}

/// Record epoch stats into global state
pub fn record_epoch(stats: &EpochStats) {
    TRAINING_STATE.lock().update(stats);
}

pub fn init() {
    serial_println!("    [optimizer] Training optimizers initialized (Q16 fixed-point)");
    serial_println!("    [optimizer] Algorithms: SGD, SGD+Momentum, Adam, AdamW");
    serial_println!("    [optimizer] Schedulers: Constant, Linear, Step, Cosine, Warmup, OneCycle");
    serial_println!("    [optimizer] Gradient clipping: value, norm");
}
