use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec;
/// LoRA adapter loading and merging
///
/// Part of the AIOS LLM layer. Implements Low-Rank Adaptation (LoRA)
/// which decomposes weight updates into two small matrices A and B
/// such that delta_W = alpha/rank * B @ A. This allows efficient
/// fine-tuning by only training a fraction of the parameters.
///
/// Each adapter stores:
///   - weight_a: shape [rank, in_dim]  (the "down" projection)
///   - weight_b: shape [out_dim, rank] (the "up" projection)
///   - The merged delta is: (alpha / rank) * weight_b @ weight_a
///
/// Adapters can be applied additively at inference time or permanently
/// merged into the base weight matrix.
use alloc::vec::Vec;

/// A low-rank adaptation matrix pair (A, B)
pub struct LoraAdapter {
    /// Human-readable name of this adapter
    pub name: String,
    /// Rank of the decomposition (typically 4, 8, 16, 32)
    pub rank: usize,
    /// Scaling factor (merged scale = alpha / rank)
    pub alpha: f32,
    /// Down-projection weights, shape [rank * in_dim], row-major
    pub weight_a: Vec<f32>,
    /// Up-projection weights, shape [out_dim * rank], row-major
    pub weight_b: Vec<f32>,
    /// Input dimension (columns of the base weight matrix)
    pub in_dim: usize,
    /// Output dimension (rows of the base weight matrix)
    pub out_dim: usize,
    /// Target layer indices this adapter applies to
    pub target_layers: Vec<usize>,
    /// Whether this adapter is currently active
    pub active: bool,
}

impl LoraAdapter {
    /// Create a new LoRA adapter with zero-initialised A and small-random B.
    ///
    /// In practice, A is initialised to small random values and B to zero
    /// (so the adapter starts as identity), but here we initialise A with
    /// a simple deterministic pattern and B to zero.
    pub fn new(name: &str, rank: usize) -> Self {
        Self::with_dims(name, rank, 8.0, 0, 0)
    }

    /// Create a LoRA adapter with explicit dimensions.
    pub fn with_dims(name: &str, rank: usize, alpha: f32, in_dim: usize, out_dim: usize) -> Self {
        let a_size = rank * in_dim;
        let b_size = out_dim * rank;

        // Initialise A with Kaiming-style small values: 1/sqrt(in_dim)
        let scale = if in_dim > 0 {
            fast_inv_sqrt(in_dim as f32)
        } else {
            0.01
        };
        let mut weight_a = Vec::with_capacity(a_size);
        let mut rng = SimpleRng::new(name.len() as u64 ^ rank as u64);
        for _ in 0..a_size {
            weight_a.push(rng.next_f32_range(-scale, scale));
        }

        // B starts at zero so delta_W starts as zero
        let weight_b = vec![0.0_f32; b_size];

        serial_println!(
            "    [lora] Created adapter '{}': rank={}, alpha={}, in={}, out={}",
            name,
            rank,
            alpha as i32,
            in_dim,
            out_dim
        );

        LoraAdapter {
            name: String::from(name),
            rank,
            alpha,
            weight_a,
            weight_b,
            in_dim,
            out_dim,
            target_layers: Vec::new(),
            active: true,
        }
    }

    /// Compute the merged scaling factor: alpha / rank
    fn scaling(&self) -> f32 {
        if self.rank == 0 {
            return 0.0;
        }
        self.alpha / (self.rank as f32)
    }

    /// Apply the LoRA delta additively to a base weight matrix.
    ///
    /// `base_weight` has shape [out_dim * in_dim], row-major.
    /// The delta is: scaling * weight_b @ weight_a
    /// This modifies the base weight **temporarily** for one forward pass.
    pub fn apply(&self, base_weight: &mut [f32]) {
        if !self.active || self.in_dim == 0 || self.out_dim == 0 || self.rank == 0 {
            return;
        }
        let expected_size = self.out_dim * self.in_dim;
        if base_weight.len() < expected_size {
            return;
        }
        let scale = self.scaling();

        // Compute delta_W[i][j] = scale * sum_r(weight_b[i][r] * weight_a[r][j])
        for i in 0..self.out_dim {
            for j in 0..self.in_dim {
                let mut sum = 0.0_f32;
                for r in 0..self.rank {
                    let b_val = self.get_b(i, r);
                    let a_val = self.get_a(r, j);
                    sum += b_val * a_val;
                }
                base_weight[i * self.in_dim + j] += scale * sum;
            }
        }
    }

    /// Permanently merge the LoRA delta into the base weight matrix.
    /// After merging, the adapter is deactivated.
    pub fn merge_into(&self, base_weight: &mut [f32]) {
        self.apply(base_weight);
        // Mark as merged (caller should set active=false on the adapter)
    }

    /// Compute the output of the LoRA branch for an input vector.
    ///
    /// `input` has length `in_dim`.
    /// Returns a vector of length `out_dim` representing the LoRA delta.
    pub fn forward(&self, input: &[f32]) -> Vec<f32> {
        if self.in_dim == 0 || self.out_dim == 0 || self.rank == 0 {
            return vec![0.0_f32; self.out_dim];
        }
        let scale = self.scaling();

        // Step 1: hidden = A @ input  (shape: [rank])
        let mut hidden = vec![0.0_f32; self.rank];
        for r in 0..self.rank {
            let mut sum = 0.0_f32;
            for j in 0..self.in_dim.min(input.len()) {
                sum += self.get_a(r, j) * input[j];
            }
            hidden[r] = sum;
        }

        // Step 2: output = scale * B @ hidden  (shape: [out_dim])
        let mut output = vec![0.0_f32; self.out_dim];
        for i in 0..self.out_dim {
            let mut sum = 0.0_f32;
            for r in 0..self.rank {
                sum += self.get_b(i, r) * hidden[r];
            }
            output[i] = scale * sum;
        }

        output
    }

    /// Train the adapter using a simple gradient update.
    ///
    /// `input` is the forward-pass input [in_dim].
    /// `grad_output` is the gradient from the loss [out_dim].
    /// `lr` is the learning rate.
    pub fn backward_update(&mut self, input: &[f32], grad_output: &[f32], lr: f32) {
        if self.in_dim == 0 || self.out_dim == 0 || self.rank == 0 {
            return;
        }
        let scale = self.scaling();

        // Recompute hidden = A @ input
        let mut hidden = vec![0.0_f32; self.rank];
        for r in 0..self.rank {
            let mut sum = 0.0_f32;
            for j in 0..self.in_dim.min(input.len()) {
                sum += self.get_a(r, j) * input[j];
            }
            hidden[r] = sum;
        }

        // grad_B[i][r] = scale * grad_output[i] * hidden[r]
        for i in 0..self.out_dim.min(grad_output.len()) {
            for r in 0..self.rank {
                let grad = scale * grad_output[i] * hidden[r];
                let idx = i * self.rank + r;
                if idx < self.weight_b.len() {
                    self.weight_b[idx] -= lr * grad;
                }
            }
        }

        // grad_hidden[r] = scale * sum_i(grad_output[i] * B[i][r])
        let mut grad_hidden = vec![0.0_f32; self.rank];
        for r in 0..self.rank {
            let mut sum = 0.0_f32;
            for i in 0..self.out_dim.min(grad_output.len()) {
                sum += grad_output[i] * self.get_b(i, r);
            }
            grad_hidden[r] = scale * sum;
        }

        // grad_A[r][j] = grad_hidden[r] * input[j]
        for r in 0..self.rank {
            for j in 0..self.in_dim.min(input.len()) {
                let grad = grad_hidden[r] * input[j];
                let idx = r * self.in_dim + j;
                if idx < self.weight_a.len() {
                    self.weight_a[idx] -= lr * grad;
                }
            }
        }
    }

    /// Get element B[i][r]  (out_dim x rank, row-major)
    #[inline]
    fn get_b(&self, i: usize, r: usize) -> f32 {
        let idx = i * self.rank + r;
        if idx < self.weight_b.len() {
            self.weight_b[idx]
        } else {
            0.0
        }
    }

    /// Get element A[r][j]  (rank x in_dim, row-major)
    #[inline]
    fn get_a(&self, r: usize, j: usize) -> f32 {
        let idx = r * self.in_dim + j;
        if idx < self.weight_a.len() {
            self.weight_a[idx]
        } else {
            0.0
        }
    }

    /// Compute the L2 norm of the adapter weights for regularisation.
    pub fn weight_norm(&self) -> f32 {
        let mut sum = 0.0_f32;
        for &v in self.weight_a.iter() {
            sum += v * v;
        }
        for &v in self.weight_b.iter() {
            sum += v * v;
        }
        fast_sqrt(sum)
    }
}

// ── Adapter registry ────────────────────────────────────────────────

/// Manages multiple LoRA adapters that can be stacked
pub struct LoraRegistry {
    pub adapters: Vec<LoraAdapter>,
    pub max_adapters: usize,
}

impl LoraRegistry {
    pub fn new(max_adapters: usize) -> Self {
        LoraRegistry {
            adapters: Vec::new(),
            max_adapters,
        }
    }

    /// Register a new adapter. Returns its index.
    pub fn add(&mut self, adapter: LoraAdapter) -> Option<usize> {
        if self.adapters.len() >= self.max_adapters {
            serial_println!("    [lora] Registry full, cannot add '{}'", adapter.name);
            return None;
        }
        let idx = self.adapters.len();
        serial_println!(
            "    [lora] Registered adapter '{}' at index {}",
            adapter.name,
            idx
        );
        self.adapters.push(adapter);
        Some(idx)
    }

    /// Apply all active adapters to a base weight matrix.
    pub fn apply_all(&self, base_weight: &mut [f32]) {
        for adapter in &self.adapters {
            if adapter.active {
                adapter.apply(base_weight);
            }
        }
    }

    /// Deactivate an adapter by name.
    pub fn deactivate(&mut self, name: &str) -> bool {
        for adapter in self.adapters.iter_mut() {
            if adapter.name == name {
                adapter.active = false;
                serial_println!("    [lora] Deactivated adapter '{}'", name);
                return true;
            }
        }
        false
    }

    /// Activate an adapter by name.
    pub fn activate(&mut self, name: &str) -> bool {
        for adapter in self.adapters.iter_mut() {
            if adapter.name == name {
                adapter.active = true;
                serial_println!("    [lora] Activated adapter '{}'", name);
                return true;
            }
        }
        false
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

fn fast_inv_sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let half = 0.5 * x;
    let mut i = x.to_bits();
    i = 0x5f37_59df - (i >> 1);
    let y = f32::from_bits(i);
    let y = y * (1.5 - half * y * y);
    y * (1.5 - half * y * y)
}

fn fast_sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    x * fast_inv_sqrt(x)
}

struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    fn new(seed: u64) -> Self {
        SimpleRng {
            state: if seed == 0 { 0xCAFE_BABE } else { seed },
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

    fn next_f32_range(&mut self, lo: f32, hi: f32) -> f32 {
        let t = (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32;
        lo + t * (hi - lo)
    }
}

// ── Global Singleton ────────────────────────────────────────────────

struct LoraState {
    registry: LoraRegistry,
}

static LORA: Mutex<Option<LoraState>> = Mutex::new(None);

pub fn init() {
    let state = LoraState {
        registry: LoraRegistry::new(16),
    };
    let mut guard = LORA.lock();
    *guard = Some(state);
    serial_println!("    [lora] LoRA subsystem initialised (max_adapters=16)");
}

/// Register an adapter in the global registry.
pub fn register_adapter(adapter: LoraAdapter) -> Option<usize> {
    let mut guard = LORA.lock();
    if let Some(state) = guard.as_mut() {
        state.registry.add(adapter)
    } else {
        None
    }
}
