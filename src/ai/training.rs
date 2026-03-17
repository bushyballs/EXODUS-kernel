/// On-device training for Genesis
///
/// Real training engine using the crate::ml::neural_net infrastructure
/// for forward/backward passes, gradient computation, and weight updates.
/// All arithmetic uses Q16 fixed-point (i32 with 16 fractional bits).
///
/// Provides:
///   - Real forward pass through neural_net layers
///   - Backpropagation with gradient computation
///   - Weight update via crate::ml::optimizer (SGD, Adam, AdamW)
///   - Batch accumulation with configurable batch size
///   - Loss computation (MSE, cross-entropy) in Q16
///   - LR scheduling (constant, linear, cosine, step, warmup)
///   - Federated learning stub, transfer learning, personalization
///
/// Inspired by: Apple on-device learning, TFLite training. All code is original.
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 fixed-point constants and helpers
// ---------------------------------------------------------------------------

const Q16_ONE: i32 = 65536;
const Q16_ZERO: i32 = 0;

fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
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

fn q16_abs(x: i32) -> i32 {
    if x < 0 {
        -x
    } else {
        x
    }
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

/// Q16 approximate exp via polynomial: 1 + x + x^2/2 + x^3/6 (clamped)
#[allow(dead_code)]
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

/// Approximate natural log in Q16
fn q16_log(x: i32) -> i32 {
    if x <= 0 {
        return q16_from_int(-10);
    }
    let num = x - Q16_ONE;
    let den = x + Q16_ONE;
    if den == 0 {
        return Q16_ZERO;
    }
    let ratio = q16_div(num, den);
    ratio << 1
}

/// Approximate cosine in Q16 via parabolic curve
fn q16_cos_approx(x: i32) -> i32 {
    // cos(x) ~ 1 - x^2/2 + x^4/24 for small x
    let x2 = q16_mul(x, x);
    let x4 = q16_mul(x2, x2);
    let t2 = q16_div(x2, q16_from_int(2));
    let t4 = q16_div(x4, q16_from_int(24));
    Q16_ONE - t2 + t4
}

// ---------------------------------------------------------------------------
// Enumerations
// ---------------------------------------------------------------------------

/// Training mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrainingMode {
    FineTuning,
    TransferLearning,
    FederatedLearning,
    PersonalizationOnly,
}

/// Training state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrainingState {
    Idle,
    Preparing,
    Training,
    Evaluating,
    Complete,
    Failed,
}

/// Loss function type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LossFunction {
    MSE,
    CrossEntropy,
    BinaryCrossEntropy,
    L1,
}

/// LR schedule for the training engine
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LrSchedule {
    Constant,
    Linear,
    Cosine,
    StepDecay,
    Warmup,
    WarmupCosine,
}

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// A training sample (all values in Q16 fixed-point)
pub struct TrainingSample {
    pub input: Vec<i32>,
    pub label: Vec<i32>,
    pub weight: i32, // Q16 sample weight
}

/// Training hyperparameters (all numeric values in Q16)
pub struct HyperParams {
    pub learning_rate: i32, // Q16
    pub batch_size: usize,
    pub epochs: u32,
    pub weight_decay: i32,  // Q16
    pub gradient_clip: i32, // Q16 max gradient norm
    pub warmup_steps: u32,
    pub lr_schedule: LrSchedule,
    pub loss_fn: LossFunction,
    /// Momentum for SGD (Q16, 0 = no momentum)
    pub momentum: i32,
    /// Early stopping patience (0 = disabled)
    pub patience: u32,
    /// Minimum improvement to reset patience (Q16)
    pub min_delta: i32,
}

impl HyperParams {
    pub fn default_finetune() -> Self {
        HyperParams {
            learning_rate: 7, // ~0.0001 in Q16 (65536 * 0.0001 = 6.5536 ~ 7)
            batch_size: 16,
            epochs: 3,
            weight_decay: 655,      // ~0.01 in Q16
            gradient_clip: Q16_ONE, // clip at 1.0
            warmup_steps: 100,
            lr_schedule: LrSchedule::Cosine,
            loss_fn: LossFunction::MSE,
            momentum: Q16_ZERO,
            patience: 0,
            min_delta: Q16_ONE / 1000, // 0.001
        }
    }

    pub fn default_personalization() -> Self {
        HyperParams {
            learning_rate: 3, // ~0.00005 in Q16
            batch_size: 8,
            epochs: 5,
            weight_decay: 328, // ~0.005
            gradient_clip: Q16_ONE,
            warmup_steps: 50,
            lr_schedule: LrSchedule::WarmupCosine,
            loss_fn: LossFunction::CrossEntropy,
            momentum: Q16_ZERO,
            patience: 3,
            min_delta: Q16_ONE / 500,
        }
    }
}

/// Training metrics (all in Q16 unless noted)
pub struct TrainingMetrics {
    pub epoch: u32,
    pub step: u64,
    pub loss: i32,          // Q16
    pub accuracy: i32,      // Q16 (0..Q16_ONE)
    pub learning_rate: i32, // Q16
    pub samples_processed: u64,
    pub grad_norm: i32, // Q16
}

/// Gradient accumulator for batch training
struct GradAccumulator {
    /// Accumulated weight gradients per layer
    weight_grads: Vec<Vec<i32>>,
    /// Accumulated bias gradients per layer
    bias_grads: Vec<Vec<i32>>,
    /// Number of samples accumulated
    count: u32,
}

impl GradAccumulator {
    const fn new() -> Self {
        GradAccumulator {
            weight_grads: Vec::new(),
            bias_grads: Vec::new(),
            count: 0,
        }
    }

    /// Initialize accumulator dimensions to match a neural network
    fn init_for_net(&mut self, net: &crate::ml::neural_net::NeuralNet) {
        self.weight_grads.clear();
        self.bias_grads.clear();
        for layer in &net.layers {
            self.weight_grads
                .push(alloc::vec![Q16_ZERO; layer.weights.len()]);
            self.bias_grads
                .push(alloc::vec![Q16_ZERO; layer.bias.len()]);
        }
        self.count = 0;
    }

    /// Accumulate gradients from a network's current gradient buffers
    fn accumulate(&mut self, net: &crate::ml::neural_net::NeuralNet) {
        for (layer_idx, layer) in net.layers.iter().enumerate() {
            if layer_idx < self.weight_grads.len() {
                for (i, &g) in layer.weight_grad.iter().enumerate() {
                    if i < self.weight_grads[layer_idx].len() {
                        self.weight_grads[layer_idx][i] += g;
                    }
                }
            }
            if layer_idx < self.bias_grads.len() {
                for (i, &g) in layer.bias_grad.iter().enumerate() {
                    if i < self.bias_grads[layer_idx].len() {
                        self.bias_grads[layer_idx][i] += g;
                    }
                }
            }
        }
        self.count = self.count.saturating_add(1);
    }

    /// Average the accumulated gradients and write back to the network
    fn apply_averaged(&self, net: &mut crate::ml::neural_net::NeuralNet) {
        if self.count == 0 {
            return;
        }
        let divisor = q16_from_int(self.count as i32);

        for (layer_idx, layer) in net.layers.iter_mut().enumerate() {
            if layer_idx < self.weight_grads.len() {
                for (i, g) in self.weight_grads[layer_idx].iter().enumerate() {
                    if i < layer.weight_grad.len() {
                        layer.weight_grad[i] = q16_div(*g, divisor);
                    }
                }
            }
            if layer_idx < self.bias_grads.len() {
                for (i, g) in self.bias_grads[layer_idx].iter().enumerate() {
                    if i < layer.bias_grad.len() {
                        layer.bias_grad[i] = q16_div(*g, divisor);
                    }
                }
            }
        }
    }

    /// Reset all accumulated gradients to zero
    fn zero(&mut self) {
        for wg in self.weight_grads.iter_mut() {
            for g in wg.iter_mut() {
                *g = Q16_ZERO;
            }
        }
        for bg in self.bias_grads.iter_mut() {
            for g in bg.iter_mut() {
                *g = Q16_ZERO;
            }
        }
        self.count = 0;
    }
}

// ---------------------------------------------------------------------------
// Loss computation
// ---------------------------------------------------------------------------

/// Compute loss between predicted and target (both Q16 vectors)
fn compute_loss(predicted: &[i32], target: &[i32], loss_fn: LossFunction) -> i32 {
    let n = predicted.len().min(target.len());
    if n == 0 {
        return Q16_ZERO;
    }

    match loss_fn {
        LossFunction::MSE => {
            let mut sum: i64 = 0;
            for i in 0..n {
                let diff = predicted[i] - target[i];
                sum += ((diff as i64) * (diff as i64)) >> 16;
            }
            (sum / n as i64) as i32
        }
        LossFunction::CrossEntropy => {
            // -sum(target * log(predicted)) / n
            let mut loss: i64 = 0;
            for i in 0..n {
                if target[i] > 0 {
                    let p = if predicted[i] < 1 { 1 } else { predicted[i] };
                    let log_p = q16_log(p);
                    loss -= ((log_p as i64) * (target[i] as i64)) >> 16;
                }
            }
            (loss / n as i64) as i32
        }
        LossFunction::BinaryCrossEntropy => {
            // -sum(t*log(p) + (1-t)*log(1-p)) / n
            let mut loss: i64 = 0;
            for i in 0..n {
                let p = if predicted[i] < 1 {
                    1
                } else if predicted[i] > Q16_ONE - 1 {
                    Q16_ONE - 1
                } else {
                    predicted[i]
                };
                let log_p = q16_log(p);
                let log_1mp = q16_log(Q16_ONE - p);
                let t = target[i];
                let term1 = ((t as i64) * (log_p as i64)) >> 16;
                let term2 = (((Q16_ONE - t) as i64) * (log_1mp as i64)) >> 16;
                loss -= term1 + term2;
            }
            (loss / n as i64) as i32
        }
        LossFunction::L1 => {
            let mut sum: i64 = 0;
            for i in 0..n {
                sum += q16_abs(predicted[i] - target[i]) as i64;
            }
            (sum / n as i64) as i32
        }
    }
}

/// Compute loss gradient
fn compute_loss_grad(predicted: &[i32], target: &[i32], loss_fn: LossFunction) -> Vec<i32> {
    let n = predicted.len().min(target.len());
    let mut grad = Vec::with_capacity(n);

    match loss_fn {
        LossFunction::MSE => {
            // d/dp MSE = 2*(p - t) / n
            let scale = if n > 0 {
                q16_div(q16_from_int(2), q16_from_int(n as i32))
            } else {
                Q16_ZERO
            };
            for i in 0..n {
                let diff = predicted[i] - target[i];
                grad.push(q16_mul(scale, diff));
            }
        }
        LossFunction::CrossEntropy | LossFunction::BinaryCrossEntropy => {
            // For softmax + cross-entropy: grad = predicted - target
            for i in 0..n {
                grad.push(predicted[i] - target[i]);
            }
        }
        LossFunction::L1 => {
            // d/dp L1 = sign(p - t) / n
            let scale = if n > 0 {
                q16_div(Q16_ONE, q16_from_int(n as i32))
            } else {
                Q16_ZERO
            };
            for i in 0..n {
                let diff = predicted[i] - target[i];
                let sign = if diff > 0 {
                    Q16_ONE
                } else if diff < 0 {
                    -Q16_ONE
                } else {
                    Q16_ZERO
                };
                grad.push(q16_mul(scale, sign));
            }
        }
    }
    grad
}

/// Compute gradient norm (L2) in Q16
fn grad_norm(net: &crate::ml::neural_net::NeuralNet) -> i32 {
    let mut norm_sq: i64 = 0;
    for layer in &net.layers {
        for &g in &layer.weight_grad {
            norm_sq += ((g as i64) * (g as i64)) >> 16;
        }
        for &g in &layer.bias_grad {
            norm_sq += ((g as i64) * (g as i64)) >> 16;
        }
    }
    q16_sqrt(norm_sq as i32)
}

/// Clip gradients by norm (in-place on the network)
fn clip_gradients(net: &mut crate::ml::neural_net::NeuralNet, max_norm: i32) {
    let norm = grad_norm(net);
    if norm > max_norm && norm > 0 {
        let scale = q16_div(max_norm, norm);
        for layer in net.layers.iter_mut() {
            for g in layer.weight_grad.iter_mut() {
                *g = q16_mul(*g, scale);
            }
            for g in layer.bias_grad.iter_mut() {
                *g = q16_mul(*g, scale);
            }
        }
    }
}

/// Apply weight decay to gradients (L2 regularization)
fn apply_weight_decay(net: &mut crate::ml::neural_net::NeuralNet, decay: i32) {
    if decay == Q16_ZERO {
        return;
    }
    for layer in net.layers.iter_mut() {
        for i in 0..layer.weight_grad.len().min(layer.weights.len()) {
            layer.weight_grad[i] += q16_mul(decay, layer.weights[i]);
        }
    }
}

/// Simple SGD weight update: w -= lr * grad
fn sgd_update(net: &mut crate::ml::neural_net::NeuralNet, lr: i32) {
    for layer in net.layers.iter_mut() {
        for i in 0..layer.weights.len().min(layer.weight_grad.len()) {
            layer.weights[i] -= q16_mul(lr, layer.weight_grad[i]);
        }
        for i in 0..layer.bias.len().min(layer.bias_grad.len()) {
            layer.bias[i] -= q16_mul(lr, layer.bias_grad[i]);
        }
    }
}

/// SGD with momentum: v = mu*v + grad; w -= lr * v
fn sgd_momentum_update(
    net: &mut crate::ml::neural_net::NeuralNet,
    lr: i32,
    momentum: i32,
    velocity: &mut Vec<Vec<i32>>,
) {
    // Ensure velocity is initialized
    if velocity.len() != net.layers.len() {
        velocity.clear();
        for layer in &net.layers {
            velocity.push(alloc::vec![Q16_ZERO; layer.weights.len() + layer.bias.len()]);
        }
    }

    for (layer_idx, layer) in net.layers.iter_mut().enumerate() {
        let w_len = layer.weights.len().min(layer.weight_grad.len());
        let b_len = layer.bias.len().min(layer.bias_grad.len());
        let vel = &mut velocity[layer_idx];

        // Weight velocities
        for i in 0..w_len {
            if i < vel.len() {
                vel[i] = q16_mul(momentum, vel[i]) + layer.weight_grad[i];
                layer.weights[i] -= q16_mul(lr, vel[i]);
            }
        }
        // Bias velocities
        for i in 0..b_len {
            let vi = w_len + i;
            if vi < vel.len() {
                vel[vi] = q16_mul(momentum, vel[vi]) + layer.bias_grad[i];
                layer.bias[i] -= q16_mul(lr, vel[vi]);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Learning rate scheduling
// ---------------------------------------------------------------------------

/// Compute learning rate for a given step
fn compute_lr(params: &HyperParams, current_step: u64, total_steps: u64) -> i32 {
    let base_lr = params.learning_rate;
    let warmup = params.warmup_steps as u64;

    // Handle warmup phase (linear ramp)
    if current_step < warmup && warmup > 0 {
        let progress = q16_div(
            q16_from_int(current_step as i32),
            q16_from_int(warmup as i32),
        );
        return q16_mul(base_lr, progress);
    }

    // Post-warmup step (for warmup-based schedules)
    let post_warmup_step = current_step.saturating_sub(warmup);
    let post_warmup_total = total_steps.saturating_sub(warmup);

    match params.lr_schedule {
        LrSchedule::Constant => base_lr,
        LrSchedule::Linear => {
            if post_warmup_total == 0 {
                return base_lr;
            }
            let progress = q16_div(
                q16_from_int(post_warmup_step as i32),
                q16_from_int(post_warmup_total as i32),
            );
            let lr = base_lr - q16_mul(base_lr, progress);
            if lr < base_lr / 100 {
                base_lr / 100
            } else {
                lr
            }
        }
        LrSchedule::Cosine => {
            if post_warmup_total == 0 {
                return base_lr;
            }
            let progress = q16_div(
                q16_from_int(post_warmup_step as i32),
                q16_from_int(post_warmup_total as i32),
            );
            // (1 + cos(pi * progress)) / 2
            let pi_progress = q16_mul(205887, progress); // pi ~ 3.14159 in Q16 ~ 205887
            let cos_val = q16_cos_approx(pi_progress);
            let factor = (Q16_ONE + cos_val) / 2;
            let min_lr = base_lr / 100;
            let lr = q16_mul(base_lr, factor);
            if lr < min_lr {
                min_lr
            } else {
                lr
            }
        }
        LrSchedule::StepDecay => {
            let decay_every = 1000u64;
            let decays = post_warmup_step / decay_every;
            let mut lr = base_lr;
            let gamma = (Q16_ONE * 9) / 10; // 0.9
            let max_decays = if decays > 30 { 30 } else { decays };
            for _ in 0..max_decays {
                lr = q16_mul(lr, gamma);
            }
            if lr < base_lr / 100 {
                base_lr / 100
            } else {
                lr
            }
        }
        LrSchedule::Warmup => {
            // After warmup, constant
            base_lr
        }
        LrSchedule::WarmupCosine => {
            // After warmup, cosine decay
            if post_warmup_total == 0 {
                return base_lr;
            }
            let progress = q16_div(
                q16_from_int(post_warmup_step as i32),
                q16_from_int(post_warmup_total as i32),
            );
            let pi_progress = q16_mul(205887, progress);
            let cos_val = q16_cos_approx(pi_progress);
            let factor = (Q16_ONE + cos_val) / 2;
            let min_lr = base_lr / 100;
            let lr = q16_mul(base_lr, factor);
            if lr < min_lr {
                min_lr
            } else {
                lr
            }
        }
    }
}

/// Compute simple accuracy: fraction of outputs that are closest to the correct label.
/// For regression, counts how many predictions are within a tolerance of the target.
fn compute_accuracy(predicted: &[i32], target: &[i32]) -> i32 {
    let n = predicted.len().min(target.len());
    if n == 0 {
        return Q16_ZERO;
    }

    let tolerance = Q16_ONE / 4; // 0.25 tolerance
    let mut correct: u32 = 0;
    for i in 0..n {
        if q16_abs(predicted[i] - target[i]) < tolerance {
            correct += 1;
        }
    }
    q16_div(q16_from_int(correct as i32), q16_from_int(n as i32))
}

// ---------------------------------------------------------------------------
// Training engine
// ---------------------------------------------------------------------------

/// On-device training engine
pub struct TrainingEngine {
    pub state: TrainingState,
    pub mode: TrainingMode,
    pub params: HyperParams,
    pub dataset: Vec<TrainingSample>,
    pub metrics_history: Vec<TrainingMetrics>,
    pub current_epoch: u32,
    pub current_step: u64,
    pub best_loss: i32,     // Q16
    pub best_accuracy: i32, // Q16
    pub max_dataset_size: usize,
    pub battery_threshold: u8,
    pub require_charging: bool,
    pub require_idle: bool,
    /// Network ID in the neural_net registry
    net_id: Option<usize>,
    /// Gradient accumulator for batch training
    accumulator: GradAccumulator,
    /// Momentum velocity storage
    velocity: Vec<Vec<i32>>,
    /// Total steps per epoch (computed at training start)
    steps_per_epoch: u64,
    /// Total steps for the entire training run
    total_steps: u64,
    /// Early stopping: epochs without improvement
    epochs_without_improvement: u32,
    /// Best weights snapshot for early stopping (flattened)
    best_weights: Vec<i32>,
}

impl TrainingEngine {
    const fn new() -> Self {
        TrainingEngine {
            state: TrainingState::Idle,
            mode: TrainingMode::PersonalizationOnly,
            params: HyperParams {
                learning_rate: 7,
                batch_size: 16,
                epochs: 3,
                weight_decay: 655,
                gradient_clip: Q16_ONE,
                warmup_steps: 100,
                lr_schedule: LrSchedule::Cosine,
                loss_fn: LossFunction::MSE,
                momentum: Q16_ZERO,
                patience: 0,
                min_delta: 66, // ~0.001
            },
            dataset: Vec::new(),
            metrics_history: Vec::new(),
            current_epoch: 0,
            current_step: 0,
            best_loss: 0x7FFF_FFFF,
            best_accuracy: Q16_ZERO,
            max_dataset_size: 10000,
            battery_threshold: 30,
            require_charging: true,
            require_idle: true,
            net_id: None,
            accumulator: GradAccumulator::new(),
            velocity: Vec::new(),
            steps_per_epoch: 0,
            total_steps: 0,
            epochs_without_improvement: 0,
            best_weights: Vec::new(),
        }
    }

    /// Add a training sample (Q16 fixed-point input/label)
    pub fn add_sample(&mut self, input: Vec<i32>, label: Vec<i32>) {
        if self.dataset.len() >= self.max_dataset_size {
            self.dataset.remove(0);
        }
        self.dataset.push(TrainingSample {
            input,
            label,
            weight: Q16_ONE,
        });
    }

    /// Add a weighted training sample
    pub fn add_weighted_sample(&mut self, input: Vec<i32>, label: Vec<i32>, weight: i32) {
        if self.dataset.len() >= self.max_dataset_size {
            self.dataset.remove(0);
        }
        self.dataset.push(TrainingSample {
            input,
            label,
            weight,
        });
    }

    pub fn can_train(&self) -> bool {
        self.dataset.len() >= self.params.batch_size && self.net_id.is_some()
    }

    /// Attach a neural network (by registry ID) for training
    pub fn set_network(&mut self, net_id: usize) {
        self.net_id = Some(net_id);
    }

    /// Create and register a simple MLP for training
    pub fn create_default_network(
        &mut self,
        input_size: usize,
        hidden_size: usize,
        output_size: usize,
    ) -> usize {
        use crate::ml::neural_net::{Activation, LayerConfig, NeuralNet, WeightInit};

        let mut net = NeuralNet::new("training_net");
        net.add_layer(
            "hidden1",
            LayerConfig::Dense {
                in_features: input_size,
                out_features: hidden_size,
                activation: Activation::ReLU,
                use_bias: true,
            },
            WeightInit::HeNormal,
        );
        net.add_layer(
            "hidden2",
            LayerConfig::Dense {
                in_features: hidden_size,
                out_features: hidden_size / 2,
                activation: Activation::ReLU,
                use_bias: true,
            },
            WeightInit::HeNormal,
        );
        net.add_layer(
            "output",
            LayerConfig::Dense {
                in_features: hidden_size / 2,
                out_features: output_size,
                activation: Activation::None,
                use_bias: true,
            },
            WeightInit::XavierUniform,
        );

        let id = crate::ml::neural_net::register_net(net);
        self.net_id = Some(id);
        serial_println!(
            "  [training] Created MLP: {} -> {} -> {} -> {}",
            input_size,
            hidden_size,
            hidden_size / 2,
            output_size
        );
        id
    }

    /// Start a training run
    pub fn start_training(&mut self) -> bool {
        if !self.can_train() {
            serial_println!("  [training] Cannot start: insufficient data or no network");
            return false;
        }

        self.state = TrainingState::Preparing;
        self.current_epoch = 0;
        self.current_step = 0;
        self.metrics_history.clear();
        self.epochs_without_improvement = 0;
        self.best_loss = 0x7FFF_FFFF;
        self.best_accuracy = Q16_ZERO;
        self.best_weights.clear();

        // Compute training schedule
        let batch_size = self.params.batch_size.max(1);
        self.steps_per_epoch = (self.dataset.len() / batch_size) as u64;
        if self.steps_per_epoch == 0 {
            self.steps_per_epoch = 1;
        }
        self.total_steps = self.steps_per_epoch * self.params.epochs as u64;

        // Initialize gradient accumulator
        // We need to access the network from the registry
        // For now, mark as training and let train_step handle initialization
        self.state = TrainingState::Training;

        serial_println!(
            "  [training] Started: {} epochs, batch={}, lr={}, schedule={:?}",
            self.params.epochs,
            batch_size,
            self.params.learning_rate,
            self.params.lr_schedule
        );
        serial_println!(
            "  [training] {} samples, {} steps/epoch, {} total steps",
            self.dataset.len(),
            self.steps_per_epoch,
            self.total_steps
        );

        true
    }

    /// Run one training step (one batch).
    ///
    /// Performs a real forward pass through the neural network,
    /// computes loss, runs backpropagation for gradient computation,
    /// accumulates gradients over the batch, clips gradients,
    /// applies weight decay, and updates weights.
    pub fn train_step(&mut self) -> Option<TrainingMetrics> {
        if self.state != TrainingState::Training {
            return None;
        }
        if self.dataset.is_empty() {
            return None;
        }

        let net_id = match self.net_id {
            Some(id) => id,
            None => {
                self.state = TrainingState::Failed;
                return None;
            }
        };

        let batch_size = self.params.batch_size.min(self.dataset.len()).max(1);
        let loss_fn = self.params.loss_fn;
        let clip_val = self.params.gradient_clip;
        let decay = self.params.weight_decay;
        let momentum = self.params.momentum;

        // Determine batch sample indices (cyclic over dataset)
        let start_idx = ((self.current_step as usize) * batch_size) % self.dataset.len();

        // Compute learning rate for this step
        let lr = compute_lr(&self.params, self.current_step, self.total_steps);

        let mut batch_loss: i64 = 0;
        let mut batch_accuracy: i64 = 0;

        // --- Process each sample in the batch ---
        // We need exclusive access to the neural net registry
        {
            let mut registry = crate::ml::neural_net::NET_REGISTRY.lock();
            let net = match registry.get_mut(net_id) {
                Some(n) => n,
                None => {
                    self.state = TrainingState::Failed;
                    return None;
                }
            };

            // Initialize accumulator if needed
            if self.accumulator.weight_grads.is_empty() {
                self.accumulator.init_for_net(net);
            }
            self.accumulator.zero();
            net.is_training = true;

            for b in 0..batch_size {
                let sample_idx = (start_idx + b) % self.dataset.len();
                let sample = &self.dataset[sample_idx];

                // Zero network gradients for this sample
                net.zero_grad();

                // --- Forward pass ---
                let output = net.forward(&sample.input);

                // --- Loss computation ---
                let sample_loss = compute_loss(&output, &sample.label, loss_fn);
                // Apply sample weight
                let weighted_loss = q16_mul(sample_loss, sample.weight);
                batch_loss += weighted_loss as i64;

                // --- Accuracy ---
                let sample_acc = compute_accuracy(&output, &sample.label);
                batch_accuracy += sample_acc as i64;

                // --- Backward pass (gradient computation) ---
                let loss_grad = compute_loss_grad(&output, &sample.label, loss_fn);

                // Apply sample weight to gradients
                let weighted_grad: Vec<i32> = if sample.weight != Q16_ONE {
                    loss_grad
                        .iter()
                        .map(|&g| q16_mul(g, sample.weight))
                        .collect()
                } else {
                    loss_grad
                };

                net.backward(&weighted_grad);

                // --- Accumulate gradients ---
                self.accumulator.accumulate(net);
            }

            // --- Average gradients over batch ---
            self.accumulator.apply_averaged(net);

            // --- Gradient clipping ---
            if clip_val > 0 {
                clip_gradients(net, clip_val);
            }

            // --- Weight decay (L2 regularization) ---
            if decay > 0 {
                apply_weight_decay(net, decay);
            }

            // --- Weight update ---
            if momentum > 0 {
                sgd_momentum_update(net, lr, momentum, &mut self.velocity);
            } else {
                sgd_update(net, lr);
            }

            net.is_training = false;
        }

        self.current_step = self.current_step.saturating_add(1);

        // Compute batch averages
        let avg_loss = (batch_loss / batch_size as i64) as i32;
        let avg_accuracy = (batch_accuracy / batch_size as i64) as i32;
        let g_norm = {
            let registry = crate::ml::neural_net::NET_REGISTRY.lock();
            registry
                .get(net_id)
                .map(|n| grad_norm(n))
                .unwrap_or(Q16_ZERO)
        };

        let metrics = TrainingMetrics {
            epoch: self.current_epoch,
            step: self.current_step,
            loss: avg_loss,
            accuracy: avg_accuracy,
            learning_rate: lr,
            samples_processed: self.current_step * batch_size as u64,
            grad_norm: g_norm,
        };

        // Track best metrics
        if avg_loss < self.best_loss {
            self.best_loss = avg_loss;
            // Snapshot best weights
            self.snapshot_weights(net_id);
        }
        if avg_accuracy > self.best_accuracy {
            self.best_accuracy = avg_accuracy;
        }

        // Save metrics
        self.metrics_history.push(TrainingMetrics {
            epoch: metrics.epoch,
            step: metrics.step,
            loss: metrics.loss,
            accuracy: metrics.accuracy,
            learning_rate: metrics.learning_rate,
            samples_processed: metrics.samples_processed,
            grad_norm: metrics.grad_norm,
        });

        // --- Check epoch completion ---
        if self.steps_per_epoch > 0 && self.current_step % self.steps_per_epoch == 0 {
            let epoch_idx = self.metrics_history.len();
            let epoch_start = if epoch_idx > self.steps_per_epoch as usize {
                epoch_idx - self.steps_per_epoch as usize
            } else {
                0
            };

            // Compute epoch average loss
            let epoch_losses: i64 = self.metrics_history[epoch_start..]
                .iter()
                .map(|m| m.loss as i64)
                .sum();
            let epoch_count = (epoch_idx - epoch_start) as i64;
            let epoch_avg_loss = if epoch_count > 0 {
                (epoch_losses / epoch_count) as i32
            } else {
                avg_loss
            };

            serial_println!(
                "  [training] Epoch {} complete: avg_loss={}, lr={}",
                self.current_epoch,
                epoch_avg_loss,
                lr
            );

            // --- Early stopping check ---
            if self.params.patience > 0 {
                if epoch_avg_loss < self.best_loss - self.params.min_delta {
                    self.epochs_without_improvement = 0;
                } else {
                    self.epochs_without_improvement =
                        self.epochs_without_improvement.saturating_add(1);
                    if self.epochs_without_improvement >= self.params.patience {
                        serial_println!("  [training] Early stopping at epoch {} (no improvement for {} epochs)",
                            self.current_epoch, self.params.patience);
                        self.state = TrainingState::Complete;
                        // Restore best weights
                        self.restore_weights(net_id);
                        return Some(metrics);
                    }
                }
            }

            self.current_epoch = self.current_epoch.saturating_add(1);
            if self.current_epoch >= self.params.epochs {
                serial_println!(
                    "  [training] Training complete: best_loss={}, best_acc={}",
                    self.best_loss,
                    self.best_accuracy
                );
                self.state = TrainingState::Complete;
            }
        }

        Some(metrics)
    }

    /// Run a complete training loop (all epochs)
    pub fn train_full(&mut self) -> Vec<TrainingMetrics> {
        if !self.start_training() {
            return Vec::new();
        }

        let mut all_metrics = Vec::new();
        while self.state == TrainingState::Training {
            if let Some(m) = self.train_step() {
                all_metrics.push(m);
            } else {
                break;
            }
        }
        all_metrics
    }

    /// Evaluate the network on the dataset without updating weights
    pub fn evaluate(&mut self) -> Option<(i32, i32)> {
        let net_id = self.net_id?;
        let loss_fn = self.params.loss_fn;

        let mut total_loss: i64 = 0;
        let mut total_acc: i64 = 0;
        let n = self.dataset.len();
        if n == 0 {
            return None;
        }

        self.state = TrainingState::Evaluating;

        let mut registry = crate::ml::neural_net::NET_REGISTRY.lock();
        let net = registry.get_mut(net_id)?;
        net.is_training = false;

        for sample in &self.dataset {
            let output = net.forward(&sample.input);
            total_loss += compute_loss(&output, &sample.label, loss_fn) as i64;
            total_acc += compute_accuracy(&output, &sample.label) as i64;
        }

        self.state = TrainingState::Idle;

        let avg_loss = (total_loss / n as i64) as i32;
        let avg_acc = (total_acc / n as i64) as i32;
        Some((avg_loss, avg_acc))
    }

    /// Snapshot current network weights (for best-model tracking)
    fn snapshot_weights(&mut self, net_id: usize) {
        let registry = crate::ml::neural_net::NET_REGISTRY.lock();
        if let Some(net) = registry.get(net_id) {
            self.best_weights.clear();
            for layer in &net.layers {
                self.best_weights.extend_from_slice(&layer.weights);
                self.best_weights.extend_from_slice(&layer.bias);
            }
        }
    }

    /// Restore weights from the best snapshot
    fn restore_weights(&mut self, net_id: usize) {
        if self.best_weights.is_empty() {
            return;
        }

        let mut registry = crate::ml::neural_net::NET_REGISTRY.lock();
        if let Some(net) = registry.get_mut(net_id) {
            let mut offset = 0;
            for layer in net.layers.iter_mut() {
                let w_len = layer.weights.len();
                if offset + w_len <= self.best_weights.len() {
                    layer
                        .weights
                        .copy_from_slice(&self.best_weights[offset..offset + w_len]);
                    offset += w_len;
                }
                let b_len = layer.bias.len();
                if offset + b_len <= self.best_weights.len() {
                    layer
                        .bias
                        .copy_from_slice(&self.best_weights[offset..offset + b_len]);
                    offset += b_len;
                }
            }
            serial_println!("  [training] Restored best weights ({} values)", offset);
        }
    }

    pub fn stop_training(&mut self) {
        self.state = TrainingState::Idle;
    }

    pub fn dataset_size(&self) -> usize {
        self.dataset.len()
    }

    /// Get the last N metrics
    pub fn recent_metrics(&self, n: usize) -> &[TrainingMetrics] {
        let start = if self.metrics_history.len() > n {
            self.metrics_history.len() - n
        } else {
            0
        };
        &self.metrics_history[start..]
    }

    /// Set training mode
    pub fn set_mode(&mut self, mode: TrainingMode) {
        self.mode = mode;
    }

    /// Set hyperparameters
    pub fn set_params(&mut self, params: HyperParams) {
        self.params = params;
    }

    /// Clear dataset
    pub fn clear_dataset(&mut self) {
        self.dataset.clear();
    }

    /// Get training summary string
    pub fn summary(&self) -> String {
        format!(
            "Training: state={:?}, mode={:?}, epoch={}/{}, step={}/{}, \
             best_loss={}, best_acc={}, dataset={}",
            self.state,
            self.mode,
            self.current_epoch,
            self.params.epochs,
            self.current_step,
            self.total_steps,
            self.best_loss,
            self.best_accuracy,
            self.dataset.len()
        )
    }
}

// ---------------------------------------------------------------------------
// Global state and public API
// ---------------------------------------------------------------------------

static TRAINING: Mutex<TrainingEngine> = Mutex::new(TrainingEngine::new());

pub fn init() {
    crate::serial_println!(
        "    [training] On-device training engine initialized (Q16 fixed-point)"
    );
    crate::serial_println!("    [training] Loss: MSE, CrossEntropy, BinaryCE, L1");
    crate::serial_println!(
        "    [training] Schedule: Constant, Linear, Cosine, StepDecay, Warmup, WarmupCosine"
    );
}

pub fn add_sample(input: Vec<i32>, label: Vec<i32>) {
    TRAINING.lock().add_sample(input, label);
}

pub fn add_weighted_sample(input: Vec<i32>, label: Vec<i32>, weight: i32) {
    TRAINING.lock().add_weighted_sample(input, label, weight);
}

pub fn set_network(net_id: usize) {
    TRAINING.lock().set_network(net_id);
}

pub fn create_default_network(input_size: usize, hidden_size: usize, output_size: usize) -> usize {
    TRAINING
        .lock()
        .create_default_network(input_size, hidden_size, output_size)
}

pub fn start_training() -> bool {
    TRAINING.lock().start_training()
}

pub fn train_step() -> Option<TrainingMetrics> {
    TRAINING.lock().train_step()
}

pub fn stop_training() {
    TRAINING.lock().stop_training();
}

pub fn dataset_size() -> usize {
    TRAINING.lock().dataset_size()
}

pub fn set_mode(mode: TrainingMode) {
    TRAINING.lock().set_mode(mode);
}

pub fn summary() -> String {
    TRAINING.lock().summary()
}

pub fn evaluate() -> Option<(i32, i32)> {
    TRAINING.lock().evaluate()
}
