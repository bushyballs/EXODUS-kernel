use crate::sync::Mutex;
use alloc::vec;
/// Quantization engine — INT8/INT4 for embedded deployment
///
/// Run large models on constrained hardware by compressing weights:
///   - INT8: 4x smaller, ~99% accuracy (per-channel scales)
///   - INT4: 8x smaller, ~95% accuracy (two values packed per byte)
///   - Mixed precision: critical layers in INT8, rest in INT4
///   - Per-channel and per-group quantization with configurable group size
///   - Dequantization routines for inference
///   - Quantization error measurement (MSE, max absolute error)
///   - Dynamic range calibration from activation samples
///   - Quantized matrix-vector multiplication
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

use super::transformer::Q16;

// Q16 constants
const Q16_ONE: Q16 = 65536;

// =============================================================================
// Data structures
// =============================================================================

#[derive(Clone, Copy, PartialEq)]
pub enum QuantMode {
    None,  // Full precision (Q16 fixed-point)
    Int8,  // 8-bit quantized
    Int4,  // 4-bit quantized
    Mixed, // Attention in INT8, FFN in INT4
}

/// Quantization parameters for one group of values
#[derive(Clone, Copy)]
struct QuantParams {
    /// Scale factor: dequantized_value = quantized_value * scale
    scale: Q16,
    /// Zero point offset (for asymmetric quantization)
    zero_point: i8,
    /// Number of values in this quantization group
    group_size: u16,
}

/// Per-channel calibration statistics
#[derive(Clone, Copy)]
struct ChannelCalibration {
    min_val: Q16,
    max_val: Q16,
    mean_val: Q16,
    /// Running sum of absolute values (for scale estimation)
    abs_sum: i64,
    sample_count: u32,
}

/// Quantization error metrics
#[derive(Clone, Copy)]
pub struct QuantError {
    /// Mean squared error (Q16)
    pub mse: Q16,
    /// Maximum absolute error (Q16)
    pub max_abs_error: Q16,
    /// Signal-to-quantization-noise ratio (Q16, in "dB" * 65536)
    pub sqnr: Q16,
    /// Number of values measured
    pub n_values: u32,
}

/// Layer precision assignment for mixed-precision quantization
#[derive(Clone, Copy)]
pub struct LayerPrecision {
    pub layer_idx: u32,
    /// Which mode to use for attention weights in this layer
    pub attention_mode: QuantMode,
    /// Which mode to use for FFN weights in this layer
    pub ffn_mode: QuantMode,
}

/// Quantized weight block — stores compressed weights for one tensor
pub struct QuantBlock {
    data_int8: Vec<i8>,
    data_int4: Vec<u8>, // Packed: 2 values per byte
    params: Vec<QuantParams>,
    mode: QuantMode,
    original_rows: u32,
    original_cols: u32,
    /// Number of elements in the original tensor
    n_elements: u32,
}

/// The quantization engine: manages quantization state and configuration
struct QuantEngine {
    mode: QuantMode,
    group_size: u16,
    /// Per-layer precision assignments (for mixed mode)
    layer_precisions: Vec<LayerPrecision>,
    /// Calibration data collected from activations
    calibrations: Vec<ChannelCalibration>,
    /// Statistics
    total_quantized_layers: u32,
    total_quantized_params: u64,
    compression_ratio: u32, // x100 (e.g., 400 = 4.0x)
    memory_saved_kb: u64,
    total_dequantizations: u64,
}

static QUANTIZER: Mutex<Option<QuantEngine>> = Mutex::new(None);

// =============================================================================
// INT8 quantization
// =============================================================================

impl QuantBlock {
    /// Quantize a Q16 tensor to INT8 with per-group scales.
    ///
    /// For each group of `group_size` values:
    ///   1. Find min and max
    ///   2. Compute scale = (max - min) / 254
    ///   3. Compute zero_point = round(-min / scale) - 127
    ///   4. Quantize: q = clamp(round(value / scale) + zero_point, -127, 127)
    fn quantize_int8(data: &[Q16], rows: u32, cols: u32, group_size: u16) -> Self {
        let n = data.len();
        let gs = group_size as usize;
        let n_groups = (n + gs - 1) / gs;
        let mut int8_data = vec![0i8; n];
        let mut params = Vec::with_capacity(n_groups);

        for g in 0..n_groups {
            let start = g * gs;
            let end = (start + gs).min(n);
            let group = &data[start..end];

            // Find dynamic range
            let mut min_val = i32::MAX;
            let mut max_val = i32::MIN;
            for &v in group {
                if v < min_val {
                    min_val = v;
                }
                if v > max_val {
                    max_val = v;
                }
            }

            // Symmetric quantization: scale maps [-absmax, absmax] to [-127, 127]
            let abs_max = if (-min_val) > max_val {
                -min_val
            } else {
                max_val
            };
            let abs_max = if abs_max == 0 { 1 } else { abs_max };

            // scale = abs_max / 127
            let scale = abs_max / 127;
            let scale = if scale == 0 { 1 } else { scale };

            // Quantize each value
            for i in start..end {
                let q = data[i] / scale;
                int8_data[i] = q.max(-127).min(127) as i8;
            }

            params.push(QuantParams {
                scale,
                zero_point: 0, // Symmetric: zero_point = 0
                group_size,
            });
        }

        QuantBlock {
            data_int8: int8_data,
            data_int4: Vec::new(),
            params,
            mode: QuantMode::Int8,
            original_rows: rows,
            original_cols: cols,
            n_elements: n as u32,
        }
    }

    /// Quantize a Q16 tensor to INT8 with per-channel (per-row) scales.
    /// Each row gets its own scale factor for better accuracy.
    fn quantize_int8_per_channel(data: &[Q16], rows: u32, cols: u32) -> Self {
        let n = data.len();
        let c = cols as usize;
        let r = rows as usize;
        let mut int8_data = vec![0i8; n];
        let mut params = Vec::with_capacity(r);

        for row in 0..r {
            let start = row * c;
            let end = (start + c).min(n);

            // Find max absolute value in this row
            let mut abs_max: i32 = 0;
            for i in start..end {
                let a = if data[i] < 0 { -data[i] } else { data[i] };
                if a > abs_max {
                    abs_max = a;
                }
            }
            if abs_max == 0 {
                abs_max = 1;
            }

            let scale = abs_max / 127;
            let scale = if scale == 0 { 1 } else { scale };

            for i in start..end {
                let q = data[i] / scale;
                int8_data[i] = q.max(-127).min(127) as i8;
            }

            params.push(QuantParams {
                scale,
                zero_point: 0,
                group_size: cols as u16,
            });
        }

        QuantBlock {
            data_int8: int8_data,
            data_int4: Vec::new(),
            params,
            mode: QuantMode::Int8,
            original_rows: rows,
            original_cols: cols,
            n_elements: n as u32,
        }
    }

    // =========================================================================
    // INT4 quantization
    // =========================================================================

    /// Quantize to INT4 (packed, 2 values per byte) with per-group scales.
    ///
    /// Each INT4 value is in [-7, 7] (signed 4-bit).
    /// Packing: low nibble = even index, high nibble = odd index.
    fn quantize_int4(data: &[Q16], rows: u32, cols: u32, group_size: u16) -> Self {
        let n = data.len();
        let gs = group_size as usize;
        let packed_len = (n + 1) / 2;
        let n_groups = (n + gs - 1) / gs;
        let mut int4_data = vec![0u8; packed_len];
        let mut params = Vec::with_capacity(n_groups);

        for g in 0..n_groups {
            let start = g * gs;
            let end = (start + gs).min(n);
            let group = &data[start..end];

            // Find dynamic range (symmetric)
            let mut abs_max: i32 = 0;
            for &v in group {
                let a = if v < 0 { -v } else { v };
                if a > abs_max {
                    abs_max = a;
                }
            }
            if abs_max == 0 {
                abs_max = 1;
            }

            // Scale maps [-abs_max, abs_max] to [-7, 7]
            let scale = abs_max / 7;
            let scale = if scale == 0 { 1 } else { scale };

            for i in start..end {
                let q = (data[i] / scale).max(-7).min(7) as i8;
                // Pack into nibbles
                let byte_idx = i / 2;
                let q_unsigned = (q & 0x0F) as u8; // Low 4 bits (preserves sign via two's complement)
                if i % 2 == 0 {
                    int4_data[byte_idx] = q_unsigned;
                } else {
                    int4_data[byte_idx] |= q_unsigned << 4;
                }
            }

            params.push(QuantParams {
                scale,
                zero_point: 0,
                group_size,
            });
        }

        QuantBlock {
            data_int8: Vec::new(),
            data_int4: int4_data,
            params,
            mode: QuantMode::Int4,
            original_rows: rows,
            original_cols: cols,
            n_elements: n as u32,
        }
    }

    // =========================================================================
    // Dequantization
    // =========================================================================

    /// Dequantize INT8 data back to Q16
    fn dequantize_int8(&self) -> Vec<Q16> {
        let n = self.data_int8.len();
        let gs = self.params.first().map_or(n, |p| p.group_size as usize);
        let mut result = Vec::with_capacity(n);

        for i in 0..n {
            let group_idx = i / gs;
            let scale = self.params.get(group_idx).map_or(1, |p| p.scale);
            let zp = self.params.get(group_idx).map_or(0, |p| p.zero_point);
            let val = (self.data_int8[i] as i32 - zp as i32) * scale;
            result.push(val);
        }
        result
    }

    /// Dequantize INT4 data back to Q16
    fn dequantize_int4(&self) -> Vec<Q16> {
        let n = self.n_elements as usize;
        let gs = self.params.first().map_or(n, |p| p.group_size as usize);
        let mut result = Vec::with_capacity(n);

        for i in 0..n {
            let byte_idx = i / 2;
            if byte_idx >= self.data_int4.len() {
                break;
            }

            let packed = self.data_int4[byte_idx];
            let nibble = if i % 2 == 0 {
                packed & 0x0F
            } else {
                packed >> 4
            };

            // Sign-extend 4-bit to i8: if bit 3 is set, it's negative
            let signed_val = if nibble & 0x08 != 0 {
                nibble as i8 | (!0x0F_u8 as i8) // Sign extend: set upper 4 bits
            } else {
                nibble as i8
            };

            let group_idx = i / gs;
            let scale = self.params.get(group_idx).map_or(1, |p| p.scale);
            result.push(signed_val as i32 * scale);
        }
        result
    }

    /// Dequantize back to Q16 (dispatches based on mode)
    fn dequantize(&self) -> Vec<Q16> {
        match self.mode {
            QuantMode::Int8 => self.dequantize_int8(),
            QuantMode::Int4 => self.dequantize_int4(),
            _ => Vec::new(),
        }
    }

    /// Dequantize a single row (for row-by-row inference)
    fn dequantize_row(&self, row: u32) -> Vec<Q16> {
        let cols = self.original_cols as usize;
        let start = row as usize * cols;
        let end = start + cols;

        match self.mode {
            QuantMode::Int8 => {
                if end > self.data_int8.len() {
                    return vec![0; cols];
                }
                let mut result = Vec::with_capacity(cols);
                for i in start..end {
                    let group_idx = i / self.params.first().map_or(cols, |p| p.group_size as usize);
                    let scale = self.params.get(group_idx).map_or(1, |p| p.scale);
                    result.push(self.data_int8[i] as i32 * scale);
                }
                result
            }
            QuantMode::Int4 => {
                let mut result = Vec::with_capacity(cols);
                for i in start..end {
                    let byte_idx = i / 2;
                    if byte_idx >= self.data_int4.len() {
                        break;
                    }
                    let packed = self.data_int4[byte_idx];
                    let nibble = if i % 2 == 0 {
                        packed & 0x0F
                    } else {
                        packed >> 4
                    };
                    let signed_val = if nibble & 0x08 != 0 {
                        nibble as i8 | (!0x0F_u8 as i8)
                    } else {
                        nibble as i8
                    };
                    let group_idx = i / self.params.first().map_or(cols, |p| p.group_size as usize);
                    let scale = self.params.get(group_idx).map_or(1, |p| p.scale);
                    result.push(signed_val as i32 * scale);
                }
                result
            }
            _ => vec![0; cols],
        }
    }

    // =========================================================================
    // Quantized matrix-vector multiply
    // =========================================================================

    /// Compute matrix-vector product directly on INT8 quantized weights.
    /// output[r] = sum(weight[r][c] * input[c]) * scale[group(r)]
    /// This avoids full dequantization for faster inference.
    fn matvec_int8(&self, input: &[Q16], output: &mut [Q16]) {
        let rows = self.original_rows as usize;
        let cols = self.original_cols as usize;

        for r in 0..rows.min(output.len()) {
            let row_start = r * cols;
            let mut sum: i64 = 0;

            for c in 0..cols.min(input.len()) {
                let idx = row_start + c;
                if idx >= self.data_int8.len() {
                    break;
                }
                sum += self.data_int8[idx] as i64 * input[c] as i64;
            }

            // Apply scale (per-row or per-group)
            let gs = self.params.first().map_or(cols, |p| p.group_size as usize);
            // For per-channel (group_size == cols), group_idx == r
            let group_idx = if gs >= cols { r } else { (r * cols) / gs };
            let scale = self.params.get(group_idx).map_or(1, |p| p.scale);

            // sum is in units of (int8 * Q16) = ~24-bit * Q16
            // We need: (sum * scale) >> 16 to get Q16 result
            output[r] = ((sum * scale as i64) >> 16) as Q16;
        }
    }

    /// Compute matrix-vector product on INT4 quantized weights.
    fn matvec_int4(&self, input: &[Q16], output: &mut [Q16]) {
        let rows = self.original_rows as usize;
        let cols = self.original_cols as usize;

        for r in 0..rows.min(output.len()) {
            let row_start = r * cols;
            let mut sum: i64 = 0;

            for c in 0..cols.min(input.len()) {
                let idx = row_start + c;
                let byte_idx = idx / 2;
                if byte_idx >= self.data_int4.len() {
                    break;
                }

                let packed = self.data_int4[byte_idx];
                let nibble = if idx % 2 == 0 {
                    packed & 0x0F
                } else {
                    packed >> 4
                };
                let signed_val = if nibble & 0x08 != 0 {
                    (nibble as i8 | (!0x0F_u8 as i8)) as i64
                } else {
                    nibble as i64
                };

                sum += signed_val * input[c] as i64;
            }

            let gs = self.params.first().map_or(cols, |p| p.group_size as usize);
            let group_idx = if gs >= cols { r } else { (r * cols) / gs };
            let scale = self.params.get(group_idx).map_or(1, |p| p.scale);
            output[r] = ((sum * scale as i64) >> 16) as Q16;
        }
    }

    // =========================================================================
    // Storage metrics
    // =========================================================================

    /// Size of quantized data in bytes
    fn compressed_size(&self) -> usize {
        match self.mode {
            QuantMode::Int8 => self.data_int8.len() + self.params.len() * 8,
            QuantMode::Int4 => self.data_int4.len() + self.params.len() * 8,
            _ => 0,
        }
    }

    /// Size of original uncompressed data in bytes (Q16 = 4 bytes each)
    fn uncompressed_size(&self) -> usize {
        self.n_elements as usize * 4
    }

    /// Compression ratio (x100, e.g., 400 = 4.0x)
    fn compression_ratio(&self) -> u32 {
        let compressed = self.compressed_size();
        if compressed == 0 {
            return 100;
        }
        ((self.uncompressed_size() * 100) / compressed) as u32
    }
}

// =============================================================================
// Quantization error measurement
// =============================================================================

/// Measure quantization error by comparing original and dequantized values.
fn measure_error(original: &[Q16], dequantized: &[Q16]) -> QuantError {
    let n = original.len().min(dequantized.len());
    if n == 0 {
        return QuantError {
            mse: 0,
            max_abs_error: 0,
            sqnr: 0,
            n_values: 0,
        };
    }

    let mut sum_sq_error: i64 = 0;
    let mut max_abs: i32 = 0;
    let mut sum_sq_signal: i64 = 0;

    for i in 0..n {
        let error = original[i] - dequantized[i];
        let abs_error = if error < 0 { -error } else { error };
        if abs_error > max_abs {
            max_abs = abs_error;
        }

        // Accumulate squared error and squared signal in reduced precision
        // to avoid overflow on large tensors
        sum_sq_error += (error as i64 * error as i64) >> 16;
        sum_sq_signal += (original[i] as i64 * original[i] as i64) >> 16;
    }

    let mse = (sum_sq_error / n as i64) as Q16;

    // SQNR = 10 * log10(signal_power / noise_power) in Q16
    // Approximate: SQNR ~ signal_power / noise_power (linear ratio, not dB)
    let sqnr = if sum_sq_error > 0 {
        ((sum_sq_signal * Q16_ONE as i64) / sum_sq_error) as Q16
    } else {
        Q16_ONE * 100 // Very high SQNR if no error
    };

    QuantError {
        mse,
        max_abs_error: max_abs,
        sqnr,
        n_values: n as u32,
    }
}

// =============================================================================
// Calibration
// =============================================================================

impl ChannelCalibration {
    fn new() -> Self {
        ChannelCalibration {
            min_val: i32::MAX,
            max_val: i32::MIN,
            mean_val: 0,
            abs_sum: 0,
            sample_count: 0,
        }
    }

    /// Update calibration statistics with a new sample
    fn observe(&mut self, value: Q16) {
        if value < self.min_val {
            self.min_val = value;
        }
        if value > self.max_val {
            self.max_val = value;
        }
        let abs_val = if value < 0 {
            -value as i64
        } else {
            value as i64
        };
        self.abs_sum += abs_val;
        self.sample_count += 1;
        // Running mean: mean = abs_sum / count (computed on demand)
    }

    /// Compute the optimal scale for INT8 quantization from calibration data
    fn optimal_int8_scale(&self) -> Q16 {
        // Use max absolute value method
        let abs_max = if (-self.min_val) > self.max_val {
            -self.min_val
        } else {
            self.max_val
        };
        let abs_max = if abs_max <= 0 { 1 } else { abs_max };
        let scale = abs_max / 127;
        if scale == 0 {
            1
        } else {
            scale
        }
    }

    /// Compute optimal scale for INT4 quantization
    fn optimal_int4_scale(&self) -> Q16 {
        let abs_max = if (-self.min_val) > self.max_val {
            -self.min_val
        } else {
            self.max_val
        };
        let abs_max = if abs_max <= 0 { 1 } else { abs_max };
        let scale = abs_max / 7;
        if scale == 0 {
            1
        } else {
            scale
        }
    }

    /// Dynamic range in Q16
    fn dynamic_range(&self) -> Q16 {
        self.max_val - self.min_val
    }
}

// =============================================================================
// QuantEngine implementation
// =============================================================================

impl QuantEngine {
    fn new() -> Self {
        QuantEngine {
            mode: QuantMode::Int8,
            group_size: 128,
            layer_precisions: Vec::new(),
            calibrations: Vec::new(),
            total_quantized_layers: 0,
            total_quantized_params: 0,
            compression_ratio: 400,
            memory_saved_kb: 0,
            total_dequantizations: 0,
        }
    }

    fn set_mode(&mut self, mode: QuantMode) {
        self.mode = mode;
        self.compression_ratio = match mode {
            QuantMode::None => 100,
            QuantMode::Int8 => 400,
            QuantMode::Int4 => 800,
            QuantMode::Mixed => 600,
        };
    }

    fn set_group_size(&mut self, size: u16) {
        self.group_size = size;
    }

    /// Set mixed-precision configuration for a specific layer
    fn set_layer_precision(&mut self, layer_idx: u32, attn_mode: QuantMode, ffn_mode: QuantMode) {
        // Update existing or add new
        for lp in &mut self.layer_precisions {
            if lp.layer_idx == layer_idx {
                lp.attention_mode = attn_mode;
                lp.ffn_mode = ffn_mode;
                return;
            }
        }
        self.layer_precisions.push(LayerPrecision {
            layer_idx,
            attention_mode: attn_mode,
            ffn_mode: ffn_mode,
        });
    }

    /// Get the quantization mode for a specific layer's component
    fn get_layer_mode(&self, layer_idx: u32, is_ffn: bool) -> QuantMode {
        for lp in &self.layer_precisions {
            if lp.layer_idx == layer_idx {
                return if is_ffn {
                    lp.ffn_mode
                } else {
                    lp.attention_mode
                };
            }
        }
        // Default: use global mode
        self.mode
    }

    /// Quantize a weight tensor using the engine's current settings
    fn quantize(
        &mut self,
        data: &[Q16],
        rows: u32,
        cols: u32,
        layer_idx: u32,
        is_ffn: bool,
    ) -> QuantBlock {
        let mode = self.get_layer_mode(layer_idx, is_ffn);
        let block = match mode {
            QuantMode::Int8 => QuantBlock::quantize_int8(data, rows, cols, self.group_size),
            QuantMode::Int4 => QuantBlock::quantize_int4(data, rows, cols, self.group_size),
            QuantMode::Mixed => {
                // For mixed: attention uses INT8, FFN uses INT4
                if is_ffn {
                    QuantBlock::quantize_int4(data, rows, cols, self.group_size)
                } else {
                    QuantBlock::quantize_int8(data, rows, cols, self.group_size)
                }
            }
            QuantMode::None => {
                // Return a dummy block with no quantization
                QuantBlock {
                    data_int8: Vec::new(),
                    data_int4: Vec::new(),
                    params: Vec::new(),
                    mode: QuantMode::None,
                    original_rows: rows,
                    original_cols: cols,
                    n_elements: data.len() as u32,
                }
            }
        };

        self.total_quantized_layers = self.total_quantized_layers.saturating_add(1);
        self.total_quantized_params += data.len() as u64;

        let original_bytes = data.len() as u64 * 4;
        let compressed_bytes = block.compressed_size() as u64;
        if original_bytes > compressed_bytes {
            self.memory_saved_kb += (original_bytes - compressed_bytes) / 1024;
        }

        block
    }

    /// Add a calibration channel and begin collecting statistics
    fn add_calibration_channel(&mut self) -> usize {
        self.calibrations.push(ChannelCalibration::new());
        self.calibrations.len() - 1
    }

    /// Feed a calibration sample to a specific channel
    fn calibrate(&mut self, channel: usize, value: Q16) {
        if channel < self.calibrations.len() {
            self.calibrations[channel].observe(value);
        }
    }

    /// Feed a batch of calibration samples (one per channel, like a row of activations)
    fn calibrate_batch(&mut self, values: &[Q16]) {
        // Ensure we have enough channels
        while self.calibrations.len() < values.len() {
            self.calibrations.push(ChannelCalibration::new());
        }
        for (i, &v) in values.iter().enumerate() {
            self.calibrations[i].observe(v);
        }
    }

    /// Get calibrated INT8 scales from collected statistics
    fn calibrated_int8_scales(&self) -> Vec<Q16> {
        self.calibrations
            .iter()
            .map(|c| c.optimal_int8_scale())
            .collect()
    }

    /// Get calibrated INT4 scales
    fn calibrated_int4_scales(&self) -> Vec<Q16> {
        self.calibrations
            .iter()
            .map(|c| c.optimal_int4_scale())
            .collect()
    }

    /// Reset all calibration data
    fn reset_calibration(&mut self) {
        self.calibrations.clear();
    }

    fn estimate_model_size(&self, param_count: u64) -> u64 {
        // Q16 = 4 bytes per param
        let full_size = param_count * 4;
        full_size * 100 / self.compression_ratio as u64
    }

    /// Estimate memory for a model under different quantization modes
    fn compare_sizes(&self, param_count: u64) -> (u64, u64, u64, u64) {
        let full = param_count * 4;
        let int8 = param_count; // 1 byte per param + overhead
        let int4 = param_count / 2; // 0.5 bytes per param + overhead
        let mixed = (param_count * 3) / 4; // ~75% of INT8
        (full, int8, int4, mixed)
    }
}

// =============================================================================
// Public API
// =============================================================================

pub fn init() {
    let mut q = QUANTIZER.lock();
    *q = Some(QuantEngine::new());
    serial_println!("    Quantization: INT8/INT4, per-channel, calibration, mixed precision ready");
}

/// Set the global quantization mode
pub fn set_mode(mode: QuantMode) {
    if let Some(engine) = QUANTIZER.lock().as_mut() {
        engine.set_mode(mode);
    }
}

/// Set the quantization group size
pub fn set_group_size(size: u16) {
    if let Some(engine) = QUANTIZER.lock().as_mut() {
        engine.set_group_size(size);
    }
}

/// Set per-layer mixed-precision configuration
pub fn set_layer_precision(layer: u32, attn_mode: QuantMode, ffn_mode: QuantMode) {
    if let Some(engine) = QUANTIZER.lock().as_mut() {
        engine.set_layer_precision(layer, attn_mode, ffn_mode);
    }
}

/// Feed calibration data (e.g., from a forward pass) to improve quantization accuracy
pub fn calibrate(values: &[Q16]) {
    if let Some(engine) = QUANTIZER.lock().as_mut() {
        engine.calibrate_batch(values);
    }
}

/// Reset calibration data
pub fn reset_calibration() {
    if let Some(engine) = QUANTIZER.lock().as_mut() {
        engine.reset_calibration();
    }
}

/// Quantize a Q16 weight tensor to INT8
pub fn quantize_int8(data: &[Q16], rows: u32, cols: u32, group_size: u16) -> Vec<i8> {
    let block = QuantBlock::quantize_int8(data, rows, cols, group_size);
    block.data_int8
}

/// Quantize a Q16 weight tensor to INT4 (packed bytes)
pub fn quantize_int4(data: &[Q16], rows: u32, cols: u32, group_size: u16) -> Vec<u8> {
    let block = QuantBlock::quantize_int4(data, rows, cols, group_size);
    block.data_int4
}

/// Measure quantization error for INT8
pub fn measure_int8_error(data: &[Q16], rows: u32, cols: u32, group_size: u16) -> QuantError {
    let block = QuantBlock::quantize_int8(data, rows, cols, group_size);
    let deq = block.dequantize();
    measure_error(data, &deq)
}

/// Measure quantization error for INT4
pub fn measure_int4_error(data: &[Q16], rows: u32, cols: u32, group_size: u16) -> QuantError {
    let block = QuantBlock::quantize_int4(data, rows, cols, group_size);
    let deq = block.dequantize();
    measure_error(data, &deq)
}

/// Estimate compressed model size in bytes
pub fn estimate_size(param_count: u64) -> u64 {
    QUANTIZER
        .lock()
        .as_ref()
        .map_or(param_count * 4, |e| e.estimate_model_size(param_count))
}

/// Get memory saved so far in KB
pub fn memory_saved_kb() -> u64 {
    QUANTIZER.lock().as_ref().map_or(0, |e| e.memory_saved_kb)
}

/// Get total quantized parameters
pub fn total_quantized_params() -> u64 {
    QUANTIZER
        .lock()
        .as_ref()
        .map_or(0, |e| e.total_quantized_params)
}
