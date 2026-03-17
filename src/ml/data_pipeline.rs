use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
/// Data pipeline for Genesis ML runtime
///
/// Provides data loading, batching, shuffling, augmentation, normalization,
/// and train/validation splitting for on-device training. All operations use
/// Q16 fixed-point arithmetic to avoid floating-point dependencies.
///
/// Inspired by: PyTorch DataLoader, TensorFlow tf.data. All code is original.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 fixed-point constants and helpers
// ---------------------------------------------------------------------------

const Q16_ONE: i32 = 65536;
const Q16_ZERO: i32 = 0;
const Q16_HALF: i32 = 32768;

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

// ---------------------------------------------------------------------------
// Data sample and dataset
// ---------------------------------------------------------------------------

/// A single data sample with features and label
#[derive(Clone)]
pub struct Sample {
    pub features: Vec<i32>, // Q16 feature values
    pub label: Vec<i32>,    // Q16 label/target values
    pub weight: i32,        // Q16 sample weight (default Q16_ONE)
}

impl Sample {
    pub fn new(features: Vec<i32>, label: Vec<i32>) -> Self {
        Sample {
            features,
            label,
            weight: Q16_ONE,
        }
    }

    pub fn with_weight(mut self, weight: i32) -> Self {
        self.weight = weight;
        self
    }

    /// Feature dimensionality
    pub fn feature_dim(&self) -> usize {
        self.features.len()
    }

    /// Label dimensionality
    pub fn label_dim(&self) -> usize {
        self.label.len()
    }
}

/// A dataset containing multiple samples
pub struct Dataset {
    pub name: String,
    pub samples: Vec<Sample>,
    pub feature_dim: usize,
    pub label_dim: usize,
}

impl Dataset {
    pub fn new(name: &str) -> Self {
        Dataset {
            name: String::from(name),
            samples: Vec::new(),
            feature_dim: 0,
            label_dim: 0,
        }
    }

    /// Add a sample to the dataset
    pub fn add_sample(&mut self, sample: Sample) {
        if self.samples.is_empty() {
            self.feature_dim = sample.feature_dim();
            self.label_dim = sample.label_dim();
        }
        self.samples.push(sample);
    }

    /// Add raw feature/label pair
    pub fn add(&mut self, features: Vec<i32>, label: Vec<i32>) {
        self.add_sample(Sample::new(features, label));
    }

    /// Number of samples
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Get a sample by index
    pub fn get(&self, idx: usize) -> Option<&Sample> {
        self.samples.get(idx)
    }

    /// Create dataset from raw arrays
    pub fn from_arrays(name: &str, features: &[Vec<i32>], labels: &[Vec<i32>]) -> Self {
        let mut ds = Dataset::new(name);
        let n = features.len().min(labels.len());
        for i in 0..n {
            ds.add(features[i].clone(), labels[i].clone());
        }
        ds
    }
}

// ---------------------------------------------------------------------------
// PRNG for shuffling and augmentation
// ---------------------------------------------------------------------------

struct PipelineRng {
    state: u32,
}

impl PipelineRng {
    const fn new(seed: u32) -> Self {
        PipelineRng {
            state: if seed == 0 { 0xCAFE_BABE } else { seed },
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

    /// Random index in [0, max)
    fn next_usize(&mut self, max: usize) -> usize {
        if max == 0 {
            return 0;
        }
        (self.next_u32() as usize) % max
    }

    /// Random Q16 value in [-range, +range]
    fn next_q16_range(&mut self, range: i32) -> i32 {
        let r = self.next_u32();
        let normalized = (r % 65536) as i32 - 32768;
        q16_mul(normalized * 2, range) >> 16
    }

    /// Random boolean with given Q16 probability
    fn next_bool(&mut self, prob_q16: i32) -> bool {
        let r = (self.next_u32() % 65536) as i32;
        r < prob_q16
    }
}

static PIPELINE_RNG: Mutex<PipelineRng> = Mutex::new(PipelineRng::new(0xBEEF_1234));

/// Set the pipeline random seed
pub fn set_seed(seed: u32) {
    PIPELINE_RNG.lock().state = if seed == 0 { 0xCAFE_BABE } else { seed };
}

// ---------------------------------------------------------------------------
// Shuffling
// ---------------------------------------------------------------------------

/// Shuffle dataset in-place (Fisher-Yates)
pub fn shuffle(dataset: &mut Dataset) {
    let n = dataset.samples.len();
    if n <= 1 {
        return;
    }
    let mut rng = PIPELINE_RNG.lock();
    for i in (1..n).rev() {
        let j = rng.next_usize(i + 1);
        dataset.samples.swap(i, j);
    }
}

/// Shuffle a vector of indices
pub fn shuffle_indices(indices: &mut Vec<usize>) {
    let n = indices.len();
    if n <= 1 {
        return;
    }
    let mut rng = PIPELINE_RNG.lock();
    for i in (1..n).rev() {
        let j = rng.next_usize(i + 1);
        indices.swap(i, j);
    }
}

// ---------------------------------------------------------------------------
// Batching
// ---------------------------------------------------------------------------

/// A batch of data samples
pub struct Batch {
    pub features: Vec<Vec<i32>>, // batch_size x feature_dim
    pub labels: Vec<Vec<i32>>,   // batch_size x label_dim
    pub weights: Vec<i32>,       // batch_size sample weights
    pub size: usize,
}

impl Batch {
    pub fn new() -> Self {
        Batch {
            features: Vec::new(),
            labels: Vec::new(),
            weights: Vec::new(),
            size: 0,
        }
    }

    /// Get flattened feature matrix (all samples concatenated)
    pub fn flat_features(&self) -> Vec<i32> {
        let mut flat = Vec::new();
        for f in &self.features {
            flat.extend_from_slice(f);
        }
        flat
    }

    /// Get flattened label matrix
    pub fn flat_labels(&self) -> Vec<i32> {
        let mut flat = Vec::new();
        for l in &self.labels {
            flat.extend_from_slice(l);
        }
        flat
    }
}

/// Create batches from a dataset
pub fn create_batches(dataset: &Dataset, batch_size: usize) -> Vec<Batch> {
    if dataset.is_empty() || batch_size == 0 {
        return Vec::new();
    }

    let num_batches = (dataset.len() + batch_size - 1) / batch_size;
    let mut batches = Vec::with_capacity(num_batches);

    let mut idx = 0;
    while idx < dataset.len() {
        let mut batch = Batch::new();
        let end = (idx + batch_size).min(dataset.len());
        for i in idx..end {
            let sample = &dataset.samples[i];
            batch.features.push(sample.features.clone());
            batch.labels.push(sample.label.clone());
            batch.weights.push(sample.weight);
            batch.size += 1;
        }
        batches.push(batch);
        idx = end;
    }
    batches
}

// ---------------------------------------------------------------------------
// Train/validation split
// ---------------------------------------------------------------------------

/// Result of a train/val split
pub struct DataSplit {
    pub train: Dataset,
    pub val: Dataset,
}

/// Split dataset into training and validation sets
/// `val_ratio` is Q16 fraction (e.g., Q16_ONE/5 = 20% validation)
pub fn train_val_split(dataset: &Dataset, val_ratio: i32) -> DataSplit {
    let n = dataset.len();
    let val_count = q16_mul(q16_from_int(n as i32), val_ratio) >> 16;
    let val_count = (val_count as usize).min(n);
    let train_count = n - val_count;

    // Create shuffled indices
    let mut indices: Vec<usize> = (0..n).collect();
    shuffle_indices(&mut indices);

    let mut train = Dataset::new(&format!("{}_train", dataset.name));
    let mut val = Dataset::new(&format!("{}_val", dataset.name));

    for (i, &idx) in indices.iter().enumerate() {
        let sample = dataset.samples[idx].clone();
        if i < train_count {
            train.add_sample(sample);
        } else {
            val.add_sample(sample);
        }
    }

    train.feature_dim = dataset.feature_dim;
    train.label_dim = dataset.label_dim;
    val.feature_dim = dataset.feature_dim;
    val.label_dim = dataset.label_dim;

    DataSplit { train, val }
}

/// K-fold cross-validation split indices
pub fn kfold_indices(n: usize, k: usize) -> Vec<(Vec<usize>, Vec<usize>)> {
    if k == 0 || n == 0 {
        return Vec::new();
    }
    let fold_size = n / k;
    let mut folds = Vec::with_capacity(k);

    for fold in 0..k {
        let val_start = fold * fold_size;
        let val_end = if fold == k - 1 {
            n
        } else {
            val_start + fold_size
        };

        let mut train_idx = Vec::new();
        let mut val_idx = Vec::new();

        for i in 0..n {
            if i >= val_start && i < val_end {
                val_idx.push(i);
            } else {
                train_idx.push(i);
            }
        }
        folds.push((train_idx, val_idx));
    }
    folds
}

// ---------------------------------------------------------------------------
// Normalization
// ---------------------------------------------------------------------------

/// Normalization statistics for a feature column
pub struct NormStats {
    pub mean: i32,    // Q16
    pub std_dev: i32, // Q16
    pub min_val: i32, // Q16
    pub max_val: i32, // Q16
}

/// Compute per-feature normalization statistics
pub fn compute_norm_stats(dataset: &Dataset) -> Vec<NormStats> {
    if dataset.is_empty() {
        return Vec::new();
    }
    let dim = dataset.feature_dim;
    let n = dataset.len();

    let mut stats = Vec::with_capacity(dim);
    for f in 0..dim {
        let mut sum: i64 = 0;
        let mut min_val = i32::MAX;
        let mut max_val = i32::MIN;

        for s in &dataset.samples {
            if f < s.features.len() {
                let v = s.features[f];
                sum += v as i64;
                if v < min_val {
                    min_val = v;
                }
                if v > max_val {
                    max_val = v;
                }
            }
        }

        let mean = if n > 0 {
            (sum / (n as i64)) as i32
        } else {
            Q16_ZERO
        };

        let mut var_sum: i64 = 0;
        for s in &dataset.samples {
            if f < s.features.len() {
                let diff = s.features[f] - mean;
                var_sum += ((diff as i64) * (diff as i64)) >> 16;
            }
        }
        let variance = if n > 0 {
            (var_sum / (n as i64)) as i32
        } else {
            Q16_ONE
        };
        let std_dev = q16_sqrt(variance);

        stats.push(NormStats {
            mean,
            std_dev,
            min_val,
            max_val,
        });
    }
    stats
}

/// Normalize features to zero mean, unit variance (z-score)
pub fn normalize_zscore(dataset: &mut Dataset, stats: &[NormStats]) {
    for sample in dataset.samples.iter_mut() {
        for (f, stat) in stats.iter().enumerate() {
            if f < sample.features.len() && stat.std_dev != 0 {
                sample.features[f] = q16_div(sample.features[f] - stat.mean, stat.std_dev);
            }
        }
    }
}

/// Normalize features to [0, Q16_ONE] range (min-max)
pub fn normalize_minmax(dataset: &mut Dataset, stats: &[NormStats]) {
    for sample in dataset.samples.iter_mut() {
        for (f, stat) in stats.iter().enumerate() {
            if f < sample.features.len() {
                let range = stat.max_val - stat.min_val;
                if range != 0 {
                    sample.features[f] = q16_div(sample.features[f] - stat.min_val, range);
                }
            }
        }
    }
}

/// Denormalize z-score normalized values back to original scale
pub fn denormalize_zscore(values: &mut [i32], stats: &[NormStats]) {
    for (f, stat) in stats.iter().enumerate() {
        if f < values.len() {
            values[f] = q16_mul(values[f], stat.std_dev) + stat.mean;
        }
    }
}

/// Denormalize min-max normalized values back to original scale
pub fn denormalize_minmax(values: &mut [i32], stats: &[NormStats]) {
    for (f, stat) in stats.iter().enumerate() {
        if f < values.len() {
            let range = stat.max_val - stat.min_val;
            values[f] = q16_mul(values[f], range) + stat.min_val;
        }
    }
}

// ---------------------------------------------------------------------------
// Data augmentation
// ---------------------------------------------------------------------------

/// Data augmentation configuration
#[derive(Clone)]
pub struct AugmentConfig {
    /// Add random noise to features (Q16 std dev of noise)
    pub noise_std: i32,
    /// Randomly scale features by [1-scale_range, 1+scale_range]
    pub scale_range: i32, // Q16
    /// Probability of applying augmentation to each sample (Q16)
    pub apply_prob: i32,
    /// Random feature dropout probability (Q16)
    pub feature_dropout: i32,
    /// Mixup alpha (0 = disabled, Q16)
    pub mixup_alpha: i32,
    /// Random horizontal flip for 2D data
    pub random_flip: bool,
}

impl AugmentConfig {
    pub fn none() -> Self {
        AugmentConfig {
            noise_std: Q16_ZERO,
            scale_range: Q16_ZERO,
            apply_prob: Q16_ZERO,
            feature_dropout: Q16_ZERO,
            mixup_alpha: Q16_ZERO,
            random_flip: false,
        }
    }

    pub fn default_config() -> Self {
        AugmentConfig {
            noise_std: Q16_ONE / 100,      // 0.01 std dev
            scale_range: Q16_ONE / 20,     // +/- 5%
            apply_prob: Q16_HALF,          // 50% chance
            feature_dropout: Q16_ONE / 20, // 5% feature dropout
            mixup_alpha: Q16_ZERO,
            random_flip: false,
        }
    }
}

/// Augment a single sample (returns new sample, original unchanged)
pub fn augment_sample(sample: &Sample, config: &AugmentConfig) -> Sample {
    let mut rng = PIPELINE_RNG.lock();
    let mut features = sample.features.clone();

    if !rng.next_bool(config.apply_prob) {
        return sample.clone();
    }

    // Add Gaussian-like noise
    if config.noise_std > 0 {
        for f in features.iter_mut() {
            let noise = rng.next_q16_range(config.noise_std);
            *f += noise;
        }
    }

    // Random scaling
    if config.scale_range > 0 {
        let scale = Q16_ONE + rng.next_q16_range(config.scale_range);
        for f in features.iter_mut() {
            *f = q16_mul(*f, scale);
        }
    }

    // Feature dropout
    if config.feature_dropout > 0 {
        for f in features.iter_mut() {
            if rng.next_bool(config.feature_dropout) {
                *f = Q16_ZERO;
            }
        }
    }

    // Random flip (reverse features, useful for 1D signals / 2D images)
    if config.random_flip && rng.next_bool(Q16_HALF) {
        features.reverse();
    }

    Sample {
        features,
        label: sample.label.clone(),
        weight: sample.weight,
    }
}

/// Augment an entire dataset (creates new samples, appended to the dataset)
pub fn augment_dataset(dataset: &mut Dataset, config: &AugmentConfig, copies: usize) {
    let original_len = dataset.samples.len();
    for _ in 0..copies {
        for i in 0..original_len {
            let augmented = augment_sample(&dataset.samples[i], config);
            dataset.samples.push(augmented);
        }
    }
}

/// Mixup augmentation: blend two samples
pub fn mixup(a: &Sample, b: &Sample, lambda: i32) -> Sample {
    let inv_lambda = Q16_ONE - lambda;
    let dim = a.features.len().min(b.features.len());
    let ldim = a.label.len().min(b.label.len());

    let mut features = Vec::with_capacity(dim);
    for i in 0..dim {
        features.push(q16_mul(lambda, a.features[i]) + q16_mul(inv_lambda, b.features[i]));
    }

    let mut label = Vec::with_capacity(ldim);
    for i in 0..ldim {
        label.push(q16_mul(lambda, a.label[i]) + q16_mul(inv_lambda, b.label[i]));
    }

    Sample {
        features,
        label,
        weight: Q16_ONE,
    }
}

// ---------------------------------------------------------------------------
// Data loader (iterator-like)
// ---------------------------------------------------------------------------

/// Configuration for the data loader
pub struct DataLoaderConfig {
    pub batch_size: usize,
    pub shuffle: bool,
    pub drop_last: bool, // drop last incomplete batch
    pub augment: AugmentConfig,
}

impl DataLoaderConfig {
    pub fn new(batch_size: usize) -> Self {
        DataLoaderConfig {
            batch_size,
            shuffle: true,
            drop_last: false,
            augment: AugmentConfig::none(),
        }
    }

    pub fn with_shuffle(mut self, shuffle: bool) -> Self {
        self.shuffle = shuffle;
        self
    }

    pub fn with_drop_last(mut self, drop_last: bool) -> Self {
        self.drop_last = drop_last;
        self
    }

    pub fn with_augment(mut self, augment: AugmentConfig) -> Self {
        self.augment = augment;
        self
    }
}

/// Data loader: produces batches from a dataset
pub struct DataLoader {
    pub config: DataLoaderConfig,
    indices: Vec<usize>,
    position: usize,
    epoch: usize,
}

impl DataLoader {
    pub fn new(dataset_len: usize, config: DataLoaderConfig) -> Self {
        let indices: Vec<usize> = (0..dataset_len).collect();
        DataLoader {
            config,
            indices,
            position: 0,
            epoch: 0,
        }
    }

    /// Reset for a new epoch
    pub fn reset(&mut self) {
        self.position = 0;
        self.epoch = self.epoch.saturating_add(1);
        if self.config.shuffle {
            shuffle_indices(&mut self.indices);
        }
    }

    /// Get next batch of indices (returns None when epoch is done)
    pub fn next_batch_indices(&mut self) -> Option<Vec<usize>> {
        if self.position >= self.indices.len() {
            return None;
        }

        let end = (self.position + self.config.batch_size).min(self.indices.len());
        let remaining = end - self.position;

        // Drop last incomplete batch if configured
        if self.config.drop_last && remaining < self.config.batch_size {
            return None;
        }

        let batch_indices = self.indices[self.position..end].to_vec();
        self.position = end;
        Some(batch_indices)
    }

    /// Get next batch from dataset
    pub fn next_batch(&mut self, dataset: &Dataset) -> Option<Batch> {
        let indices = self.next_batch_indices()?;
        let mut batch = Batch::new();
        let has_augment = self.config.augment.apply_prob > 0;

        for &idx in &indices {
            if let Some(sample) = dataset.get(idx) {
                let s = if has_augment {
                    augment_sample(sample, &self.config.augment)
                } else {
                    sample.clone()
                };
                batch.features.push(s.features);
                batch.labels.push(s.label);
                batch.weights.push(s.weight);
                batch.size += 1;
            }
        }
        Some(batch)
    }

    /// Current epoch number
    pub fn current_epoch(&self) -> usize {
        self.epoch
    }

    /// Number of batches per epoch
    pub fn num_batches(&self) -> usize {
        let n = self.indices.len();
        if self.config.drop_last {
            n / self.config.batch_size
        } else {
            (n + self.config.batch_size - 1) / self.config.batch_size
        }
    }
}

// ---------------------------------------------------------------------------
// Global state and init
// ---------------------------------------------------------------------------

/// Pipeline statistics
pub struct PipelineStats {
    pub datasets_loaded: usize,
    pub total_samples: usize,
    pub total_batches_served: usize,
}

impl PipelineStats {
    const fn new() -> Self {
        PipelineStats {
            datasets_loaded: 0,
            total_samples: 0,
            total_batches_served: 0,
        }
    }
}

static PIPELINE_STATS: Mutex<PipelineStats> = Mutex::new(PipelineStats::new());

/// Record that a dataset was loaded
pub fn record_dataset_loaded(sample_count: usize) {
    let mut stats = PIPELINE_STATS.lock();
    stats.datasets_loaded = stats.datasets_loaded.saturating_add(1);
    stats.total_samples += sample_count;
}

/// Record that batches were served
pub fn record_batches_served(count: usize) {
    PIPELINE_STATS.lock().total_batches_served += count;
}

/// Get pipeline statistics
pub fn get_stats() -> (usize, usize, usize) {
    let stats = PIPELINE_STATS.lock();
    (
        stats.datasets_loaded,
        stats.total_samples,
        stats.total_batches_served,
    )
}

pub fn init() {
    serial_println!("    [data_pipeline] Data pipeline initialized (Q16 fixed-point)");
    serial_println!(
        "    [data_pipeline] Features: batching, shuffling, augmentation, normalization"
    );
    serial_println!("    [data_pipeline] Splits: train/val, k-fold cross-validation");
}
