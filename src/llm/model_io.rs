use crate::sync::Mutex;
/// Model I/O — save/load .hoags model format
///
/// Custom binary format for Hoags LLM models.
/// Features:
///   - Header with magic number, version, model configuration
///   - Weight tensor serialization (i8 quantized with Q16 scales)
///   - Layer-by-layer loading from byte slices
///   - CRC32 checksum verification
///   - Model registry for managing multiple loaded models
///   - Vocabulary embedding serialization
///   - Support for quantized (INT8/INT4) and full-precision weights
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

use super::transformer::Q16;

// =============================================================================
// File format constants
// =============================================================================

const HOAGS_MAGIC: u32 = 0x484F4147; // "HOAG" in ASCII
const HOAGS_VERSION: u16 = 2; // Format version
const HEADER_SIZE: usize = 128; // Fixed header size in bytes
const TENSOR_HEADER_SIZE: usize = 24; // Per-tensor metadata
const LAYER_MARKER: u32 = 0x4C415952; // "LAYR" marker between layers
const VOCAB_MARKER: u32 = 0x564F4342; // "VOCB" marker for vocab section
const END_MARKER: u32 = 0x454E4421; // "END!" end-of-file marker

// =============================================================================
// Data structures
// =============================================================================

/// .hoags file format header — first 128 bytes of every model file
#[derive(Clone, Copy)]
pub struct HoagsModelHeader {
    pub magic: u32, // 0x484F4147 = "HOAG"
    pub version: u16,
    pub model_type: u8, // 0=base, 1=fine-tuned, 2=lora-only
    pub quant_mode: u8, // 0=none, 1=int8, 2=int4, 3=mixed
    pub vocab_size: u32,
    pub dim: u32,
    pub n_layers: u32,
    pub n_heads: u32,
    pub n_kv_heads: u32,
    pub ffn_dim: u32,
    pub max_seq_len: u32,
    pub rope_theta: u32,
    pub total_params: u64,
    pub file_size: u64,
    pub checksum: u64,
    pub train_tokens: u64, // How many tokens was this model trained on
    pub created_timestamp: u64,
}

/// Metadata for a serialized tensor within the file
#[derive(Clone, Copy)]
struct TensorMeta {
    /// Tensor identifier (0=wq, 1=wk, 2=wv, 3=wo, 4=w_gate, 5=w_up, 6=w_down,
    /// 7=attn_norm, 8=ffn_norm, 9=token_emb, 10=output_proj, 11=final_norm)
    tensor_id: u8,
    /// Quantization mode for this tensor (0=Q16, 1=INT8, 2=INT4)
    quant_mode: u8,
    /// Number of rows
    rows: u32,
    /// Number of columns
    cols: u32,
    /// Q16 scale factor for dequantization
    scale: Q16,
    /// Data size in bytes
    data_bytes: u32,
}

/// Result of a model load operation
#[derive(Clone, Copy, PartialEq)]
pub enum ModelLoadState {
    NotLoaded,
    Loading,
    Ready,
    Error,
}

/// Error types during model I/O
#[derive(Clone, Copy, PartialEq)]
pub enum ModelIoError {
    InvalidMagic,
    UnsupportedVersion,
    ChecksumMismatch,
    TruncatedData,
    InvalidTensorMeta,
    InvalidLayerMarker,
    OutOfMemory,
}

/// A loaded layer's weights (references into the model data)
struct LoadedLayer {
    /// Quantized weight data for each tensor (wq, wk, wv, wo, w_gate, w_up, w_down)
    weights: [Vec<i8>; 7],
    /// Scale factors for each weight tensor
    scales: [Q16; 7],
    /// Dimensions [rows, cols] for each weight tensor
    dims: [(u32, u32); 7],
    /// Norm weights (attn_norm, ffn_norm) stored as Q16
    attn_norm: Vec<Q16>,
    ffn_norm: Vec<Q16>,
}

/// Model manager: handles serialization, loading, and registry
struct ModelManager {
    header: Option<HoagsModelHeader>,
    load_state: ModelLoadState,
    load_progress: u8, // 0-100
    last_error: Option<ModelIoError>,
    /// Registry of available models
    available_models: Vec<HoagsModelHeader>,
    active_model_idx: u32,
    total_models_loaded: u32,
    /// CRC32 lookup table
    crc_table: [u32; 256],
}

static MODEL_IO: Mutex<Option<ModelManager>> = Mutex::new(None);

// =============================================================================
// CRC32 implementation (no external deps)
// =============================================================================

/// Build the CRC32 lookup table (IEEE polynomial 0xEDB88320)
fn build_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    for i in 0..256u32 {
        let mut crc = i;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB88320;
            } else {
                crc >>= 1;
            }
        }
        table[i as usize] = crc;
    }
    table
}

/// Compute CRC32 checksum of a byte slice using the lookup table
fn crc32(table: &[u32; 256], data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFFFFFF;
    for &byte in data {
        let index = ((crc ^ byte as u32) & 0xFF) as usize;
        crc = (crc >> 8) ^ table[index];
    }
    crc ^ 0xFFFFFFFF
}

/// Compute a 64-bit checksum by combining two CRC32 passes.
/// First pass is normal CRC32 over data.
/// Second pass is CRC32 over data with each byte XORed by 0xA5.
fn checksum64(table: &[u32; 256], data: &[u8]) -> u64 {
    let lo = crc32(table, data) as u64;
    // Second pass with XOR
    let mut crc: u32 = 0xFFFFFFFF;
    for &byte in data {
        let xored = byte ^ 0xA5;
        let index = ((crc ^ xored as u32) & 0xFF) as usize;
        crc = (crc >> 8) ^ table[index];
    }
    let hi = (crc ^ 0xFFFFFFFF) as u64;
    (hi << 32) | lo
}

// =============================================================================
// Little-endian byte reading helpers
// =============================================================================

fn read_u8(data: &[u8], offset: usize) -> Option<u8> {
    data.get(offset).copied()
}

fn read_u16_le(data: &[u8], offset: usize) -> Option<u16> {
    if offset + 2 > data.len() {
        return None;
    }
    Some(u16::from_le_bytes([data[offset], data[offset + 1]]))
}

fn read_u32_le(data: &[u8], offset: usize) -> Option<u32> {
    if offset + 4 > data.len() {
        return None;
    }
    Some(u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ]))
}

fn read_u64_le(data: &[u8], offset: usize) -> Option<u64> {
    if offset + 8 > data.len() {
        return None;
    }
    Some(u64::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ]))
}

fn read_i32_le(data: &[u8], offset: usize) -> Option<i32> {
    if offset + 4 > data.len() {
        return None;
    }
    Some(i32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ]))
}

// =============================================================================
// Little-endian byte writing helpers
// =============================================================================

fn write_u8(buf: &mut Vec<u8>, val: u8) {
    buf.push(val);
}

fn write_u16_le(buf: &mut Vec<u8>, val: u16) {
    buf.extend_from_slice(&val.to_le_bytes());
}

fn write_u32_le(buf: &mut Vec<u8>, val: u32) {
    buf.extend_from_slice(&val.to_le_bytes());
}

fn write_u64_le(buf: &mut Vec<u8>, val: u64) {
    buf.extend_from_slice(&val.to_le_bytes());
}

fn write_i32_le(buf: &mut Vec<u8>, val: i32) {
    buf.extend_from_slice(&val.to_le_bytes());
}

// =============================================================================
// ModelManager implementation
// =============================================================================

impl ModelManager {
    fn new() -> Self {
        ModelManager {
            header: None,
            load_state: ModelLoadState::NotLoaded,
            load_progress: 0,
            last_error: None,
            available_models: Vec::new(),
            active_model_idx: 0,
            total_models_loaded: 0,
            crc_table: build_crc32_table(),
        }
    }

    /// Create a header for a new model being trained
    fn create_header(
        vocab: u32,
        dim: u32,
        layers: u32,
        heads: u32,
        kv_heads: u32,
        ffn: u32,
        max_seq: u32,
    ) -> HoagsModelHeader {
        let params = Self::estimate_params(dim, layers, ffn, vocab);
        HoagsModelHeader {
            magic: HOAGS_MAGIC,
            version: HOAGS_VERSION,
            model_type: 0,
            quant_mode: 0,
            vocab_size: vocab,
            dim,
            n_layers: layers,
            n_heads: heads,
            n_kv_heads: kv_heads,
            ffn_dim: ffn,
            max_seq_len: max_seq,
            rope_theta: 1_000_000,
            total_params: params,
            file_size: 0, // Filled during serialization
            checksum: 0,  // Filled during serialization
            train_tokens: 0,
            created_timestamp: 0,
        }
    }

    fn estimate_params(dim: u32, layers: u32, ffn: u32, vocab: u32) -> u64 {
        let d = dim as u64;
        let ff = ffn as u64;
        let v = vocab as u64;
        let n = layers as u64;
        v * d * 2 + n * (d * d * 4 + d * ff * 3 + d * 2)
    }

    /// Serialize model header to exactly HEADER_SIZE bytes (128 bytes)
    fn serialize_header(header: &HoagsModelHeader) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER_SIZE);
        write_u32_le(&mut bytes, header.magic);
        write_u16_le(&mut bytes, header.version);
        write_u8(&mut bytes, header.model_type);
        write_u8(&mut bytes, header.quant_mode);
        write_u32_le(&mut bytes, header.vocab_size);
        write_u32_le(&mut bytes, header.dim);
        write_u32_le(&mut bytes, header.n_layers);
        write_u32_le(&mut bytes, header.n_heads);
        write_u32_le(&mut bytes, header.n_kv_heads);
        write_u32_le(&mut bytes, header.ffn_dim);
        write_u32_le(&mut bytes, header.max_seq_len);
        write_u32_le(&mut bytes, header.rope_theta);
        write_u64_le(&mut bytes, header.total_params);
        write_u64_le(&mut bytes, header.file_size);
        write_u64_le(&mut bytes, header.checksum);
        write_u64_le(&mut bytes, header.train_tokens);
        write_u64_le(&mut bytes, header.created_timestamp);
        // Pad to HEADER_SIZE
        while bytes.len() < HEADER_SIZE {
            bytes.push(0);
        }
        bytes
    }

    /// Parse header from bytes
    fn parse_header(data: &[u8]) -> Result<HoagsModelHeader, ModelIoError> {
        if data.len() < HEADER_SIZE {
            return Err(ModelIoError::TruncatedData);
        }
        let magic = read_u32_le(data, 0).ok_or(ModelIoError::TruncatedData)?;
        if magic != HOAGS_MAGIC {
            return Err(ModelIoError::InvalidMagic);
        }
        let version = read_u16_le(data, 4).ok_or(ModelIoError::TruncatedData)?;
        if version > HOAGS_VERSION {
            return Err(ModelIoError::UnsupportedVersion);
        }

        Ok(HoagsModelHeader {
            magic,
            version,
            model_type: read_u8(data, 6).ok_or(ModelIoError::TruncatedData)?,
            quant_mode: read_u8(data, 7).ok_or(ModelIoError::TruncatedData)?,
            vocab_size: read_u32_le(data, 8).ok_or(ModelIoError::TruncatedData)?,
            dim: read_u32_le(data, 12).ok_or(ModelIoError::TruncatedData)?,
            n_layers: read_u32_le(data, 16).ok_or(ModelIoError::TruncatedData)?,
            n_heads: read_u32_le(data, 20).ok_or(ModelIoError::TruncatedData)?,
            n_kv_heads: read_u32_le(data, 24).ok_or(ModelIoError::TruncatedData)?,
            ffn_dim: read_u32_le(data, 28).ok_or(ModelIoError::TruncatedData)?,
            max_seq_len: read_u32_le(data, 32).ok_or(ModelIoError::TruncatedData)?,
            rope_theta: read_u32_le(data, 36).ok_or(ModelIoError::TruncatedData)?,
            total_params: read_u64_le(data, 40).ok_or(ModelIoError::TruncatedData)?,
            file_size: read_u64_le(data, 48).ok_or(ModelIoError::TruncatedData)?,
            checksum: read_u64_le(data, 56).ok_or(ModelIoError::TruncatedData)?,
            train_tokens: read_u64_le(data, 64).ok_or(ModelIoError::TruncatedData)?,
            created_timestamp: read_u64_le(data, 72).ok_or(ModelIoError::TruncatedData)?,
        })
    }

    /// Serialize a tensor's metadata header
    fn serialize_tensor_meta(meta: &TensorMeta) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(TENSOR_HEADER_SIZE);
        write_u8(&mut bytes, meta.tensor_id);
        write_u8(&mut bytes, meta.quant_mode);
        // 2 bytes padding for alignment
        write_u8(&mut bytes, 0);
        write_u8(&mut bytes, 0);
        write_u32_le(&mut bytes, meta.rows);
        write_u32_le(&mut bytes, meta.cols);
        write_i32_le(&mut bytes, meta.scale);
        write_u32_le(&mut bytes, meta.data_bytes);
        // Pad to TENSOR_HEADER_SIZE
        while bytes.len() < TENSOR_HEADER_SIZE {
            bytes.push(0);
        }
        bytes
    }

    /// Parse a tensor metadata header
    fn parse_tensor_meta(data: &[u8], offset: usize) -> Result<TensorMeta, ModelIoError> {
        if offset + TENSOR_HEADER_SIZE > data.len() {
            return Err(ModelIoError::TruncatedData);
        }
        Ok(TensorMeta {
            tensor_id: read_u8(data, offset).ok_or(ModelIoError::TruncatedData)?,
            quant_mode: read_u8(data, offset + 1).ok_or(ModelIoError::TruncatedData)?,
            rows: read_u32_le(data, offset + 4).ok_or(ModelIoError::TruncatedData)?,
            cols: read_u32_le(data, offset + 8).ok_or(ModelIoError::TruncatedData)?,
            scale: read_i32_le(data, offset + 12).ok_or(ModelIoError::TruncatedData)?,
            data_bytes: read_u32_le(data, offset + 16).ok_or(ModelIoError::TruncatedData)?,
        })
    }

    /// Serialize a weight tensor (i8 quantized data with scale)
    fn serialize_weight_tensor(
        data: &[i8],
        scale: Q16,
        rows: u32,
        cols: u32,
        tensor_id: u8,
    ) -> Vec<u8> {
        let mut buf = Vec::new();
        let meta = TensorMeta {
            tensor_id,
            quant_mode: 1, // INT8
            rows,
            cols,
            scale,
            data_bytes: data.len() as u32,
        };
        buf.extend_from_slice(&Self::serialize_tensor_meta(&meta));
        // Write raw i8 data as bytes
        for &val in data {
            buf.push(val as u8);
        }
        buf
    }

    /// Serialize a Q16 vector (used for norms)
    fn serialize_q16_vec(data: &[Q16], tensor_id: u8) -> Vec<u8> {
        let mut buf = Vec::new();
        let meta = TensorMeta {
            tensor_id,
            quant_mode: 0, // Full precision Q16
            rows: 1,
            cols: data.len() as u32,
            scale: 1 << 16, // Q16_ONE
            data_bytes: (data.len() * 4) as u32,
        };
        buf.extend_from_slice(&Self::serialize_tensor_meta(&meta));
        for &val in data {
            write_i32_le(&mut buf, val);
        }
        buf
    }

    /// Deserialize i8 weight data from a byte slice at the given offset.
    /// Returns the weight data and the new offset after reading.
    fn deserialize_weight_data(
        data: &[u8],
        offset: usize,
        n_bytes: u32,
    ) -> Result<(Vec<i8>, usize), ModelIoError> {
        let end = offset + n_bytes as usize;
        if end > data.len() {
            return Err(ModelIoError::TruncatedData);
        }
        let weights: Vec<i8> = data[offset..end].iter().map(|&b| b as i8).collect();
        Ok((weights, end))
    }

    /// Deserialize a Q16 vector from bytes at offset.
    fn deserialize_q16_vec(
        data: &[u8],
        offset: usize,
        n_elements: u32,
    ) -> Result<(Vec<Q16>, usize), ModelIoError> {
        let n = n_elements as usize;
        let end = offset + n * 4;
        if end > data.len() {
            return Err(ModelIoError::TruncatedData);
        }
        let mut result = Vec::with_capacity(n);
        let mut pos = offset;
        for _ in 0..n {
            let val = read_i32_le(data, pos).ok_or(ModelIoError::TruncatedData)?;
            result.push(val);
            pos += 4;
        }
        Ok((result, end))
    }

    /// Serialize a complete model to bytes.
    /// Layout:
    ///   [Header: 128 bytes]
    ///   [VOCB marker: 4 bytes]
    ///   [Token embedding tensor]
    ///   For each layer:
    ///     [LAYR marker: 4 bytes]
    ///     [Layer index: 4 bytes]
    ///     [wq tensor] [wk tensor] [wv tensor] [wo tensor]
    ///     [w_gate tensor] [w_up tensor] [w_down tensor]
    ///     [attn_norm tensor] [ffn_norm tensor]
    ///   [Final norm tensor]
    ///   [Output projection tensor]
    ///   [END! marker: 4 bytes]
    fn serialize_model(
        &self,
        header: &HoagsModelHeader,
        token_emb_data: &[i8],
        token_emb_scale: Q16,
        layers: &[(
            /* wq */ (&[i8], Q16, u32, u32),
            /* wk */ (&[i8], Q16, u32, u32),
            /* wv */ (&[i8], Q16, u32, u32),
            /* wo */ (&[i8], Q16, u32, u32),
            /* w_gate */ (&[i8], Q16, u32, u32),
            /* w_up */ (&[i8], Q16, u32, u32),
            /* w_down */ (&[i8], Q16, u32, u32),
            /* attn_norm */ &[Q16],
            /* ffn_norm */ &[Q16],
        )],
        final_norm: &[Q16],
        output_proj_data: &[i8],
        output_proj_scale: Q16,
        output_proj_rows: u32,
        output_proj_cols: u32,
    ) -> Vec<u8> {
        let mut buf = Vec::new();

        // Placeholder header (we'll fill checksum and file_size at the end)
        let mut hdr = *header;
        buf.extend_from_slice(&Self::serialize_header(&hdr));

        // Vocab marker and token embedding
        write_u32_le(&mut buf, VOCAB_MARKER);
        buf.extend_from_slice(&Self::serialize_weight_tensor(
            token_emb_data,
            token_emb_scale,
            header.vocab_size,
            header.dim,
            9,
        ));

        // Layers
        for (layer_idx, layer) in layers.iter().enumerate() {
            write_u32_le(&mut buf, LAYER_MARKER);
            write_u32_le(&mut buf, layer_idx as u32);

            let (wq, wk, wv, wo, wg, wu, wd, an, fn_) = layer;

            // Weight tensors: id 0-6
            buf.extend_from_slice(&Self::serialize_weight_tensor(wq.0, wq.1, wq.2, wq.3, 0));
            buf.extend_from_slice(&Self::serialize_weight_tensor(wk.0, wk.1, wk.2, wk.3, 1));
            buf.extend_from_slice(&Self::serialize_weight_tensor(wv.0, wv.1, wv.2, wv.3, 2));
            buf.extend_from_slice(&Self::serialize_weight_tensor(wo.0, wo.1, wo.2, wo.3, 3));
            buf.extend_from_slice(&Self::serialize_weight_tensor(wg.0, wg.1, wg.2, wg.3, 4));
            buf.extend_from_slice(&Self::serialize_weight_tensor(wu.0, wu.1, wu.2, wu.3, 5));
            buf.extend_from_slice(&Self::serialize_weight_tensor(wd.0, wd.1, wd.2, wd.3, 6));

            // Norm weights: id 7-8
            buf.extend_from_slice(&Self::serialize_q16_vec(an, 7));
            buf.extend_from_slice(&Self::serialize_q16_vec(fn_, 8));
        }

        // Final norm (id 11)
        buf.extend_from_slice(&Self::serialize_q16_vec(final_norm, 11));

        // Output projection (id 10)
        buf.extend_from_slice(&Self::serialize_weight_tensor(
            output_proj_data,
            output_proj_scale,
            output_proj_rows,
            output_proj_cols,
            10,
        ));

        // End marker
        write_u32_le(&mut buf, END_MARKER);

        // Now compute checksum over everything after the header
        let payload = &buf[HEADER_SIZE..];
        let checksum = checksum64(&self.crc_table, payload);

        // Update header with file size and checksum
        hdr.file_size = buf.len() as u64;
        hdr.checksum = checksum;
        let header_bytes = Self::serialize_header(&hdr);
        buf[..HEADER_SIZE].copy_from_slice(&header_bytes);

        buf
    }

    /// Load and validate a model from a byte slice.
    /// Returns the parsed header and per-layer weight data.
    fn load_model(
        &mut self,
        data: &[u8],
    ) -> Result<(HoagsModelHeader, Vec<LoadedLayer>), ModelIoError> {
        self.load_state = ModelLoadState::Loading;
        self.load_progress = 0;

        // Parse header
        let header = Self::parse_header(data)?;
        self.load_progress = 5;

        // Verify checksum
        if data.len() > HEADER_SIZE && header.checksum != 0 {
            let payload = &data[HEADER_SIZE..];
            let computed = checksum64(&self.crc_table, payload);
            if computed != header.checksum {
                self.load_state = ModelLoadState::Error;
                self.last_error = Some(ModelIoError::ChecksumMismatch);
                return Err(ModelIoError::ChecksumMismatch);
            }
        }
        self.load_progress = 10;

        let mut offset = HEADER_SIZE;

        // Expect VOCB marker
        let marker = read_u32_le(data, offset).ok_or(ModelIoError::TruncatedData)?;
        if marker != VOCAB_MARKER {
            self.load_state = ModelLoadState::Error;
            self.last_error = Some(ModelIoError::InvalidLayerMarker);
            return Err(ModelIoError::InvalidLayerMarker);
        }
        offset += 4;

        // Skip token embedding tensor (we read it but don't store in LoadedLayer)
        let _emb_meta = Self::parse_tensor_meta(data, offset)?;
        offset += TENSOR_HEADER_SIZE;
        offset += _emb_meta.data_bytes as usize;
        self.load_progress = 15;

        // Load layers
        let mut layers = Vec::with_capacity(header.n_layers as usize);
        let progress_per_layer = 70 / header.n_layers.max(1);

        for layer_idx in 0..header.n_layers {
            // Expect LAYR marker
            let marker = read_u32_le(data, offset).ok_or(ModelIoError::TruncatedData)?;
            if marker != LAYER_MARKER {
                self.load_state = ModelLoadState::Error;
                self.last_error = Some(ModelIoError::InvalidLayerMarker);
                return Err(ModelIoError::InvalidLayerMarker);
            }
            offset += 4;

            // Layer index
            let idx = read_u32_le(data, offset).ok_or(ModelIoError::TruncatedData)?;
            if idx != layer_idx {
                self.load_state = ModelLoadState::Error;
                self.last_error = Some(ModelIoError::InvalidLayerMarker);
                return Err(ModelIoError::InvalidLayerMarker);
            }
            offset += 4;

            let mut loaded = LoadedLayer {
                weights: [
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                ],
                scales: [0; 7],
                dims: [(0, 0); 7],
                attn_norm: Vec::new(),
                ffn_norm: Vec::new(),
            };

            // Read 7 weight tensors (wq, wk, wv, wo, w_gate, w_up, w_down)
            for t in 0..7 {
                let meta = Self::parse_tensor_meta(data, offset)?;
                offset += TENSOR_HEADER_SIZE;
                let (w, new_off) = Self::deserialize_weight_data(data, offset, meta.data_bytes)?;
                offset = new_off;
                loaded.weights[t] = w;
                loaded.scales[t] = meta.scale;
                loaded.dims[t] = (meta.rows, meta.cols);
            }

            // Read attn_norm (tensor id 7)
            let meta = Self::parse_tensor_meta(data, offset)?;
            offset += TENSOR_HEADER_SIZE;
            let (norm, new_off) = Self::deserialize_q16_vec(data, offset, meta.cols)?;
            offset = new_off;
            loaded.attn_norm = norm;

            // Read ffn_norm (tensor id 8)
            let meta = Self::parse_tensor_meta(data, offset)?;
            offset += TENSOR_HEADER_SIZE;
            let (norm, new_off) = Self::deserialize_q16_vec(data, offset, meta.cols)?;
            offset = new_off;
            loaded.ffn_norm = norm;

            layers.push(loaded);
            self.load_progress = 15 + (layer_idx * progress_per_layer) as u8;
        }

        // Skip final norm and output projection (they follow the same pattern)
        // Final norm (id 11)
        if offset + TENSOR_HEADER_SIZE <= data.len() {
            let meta = Self::parse_tensor_meta(data, offset)?;
            offset += TENSOR_HEADER_SIZE;
            offset += meta.data_bytes as usize;
        }

        // Output projection (id 10)
        if offset + TENSOR_HEADER_SIZE <= data.len() {
            let meta = Self::parse_tensor_meta(data, offset)?;
            offset += TENSOR_HEADER_SIZE;
            offset += meta.data_bytes as usize;
        }

        // Verify END marker
        if offset + 4 <= data.len() {
            let marker = read_u32_le(data, offset).ok_or(ModelIoError::TruncatedData)?;
            if marker != END_MARKER {
                serial_println!("    model_io: Warning: missing END marker");
            }
        }

        self.header = Some(header);
        self.load_state = ModelLoadState::Ready;
        self.load_progress = 100;
        self.total_models_loaded = self.total_models_loaded.saturating_add(1);

        Ok((header, layers))
    }

    /// Validate a model file without fully loading it (header + checksum only)
    fn validate(&self, data: &[u8]) -> Result<HoagsModelHeader, ModelIoError> {
        let header = Self::parse_header(data)?;

        // Verify checksum if present
        if data.len() > HEADER_SIZE && header.checksum != 0 {
            let payload = &data[HEADER_SIZE..];
            let computed = checksum64(&self.crc_table, payload);
            if computed != header.checksum {
                return Err(ModelIoError::ChecksumMismatch);
            }
        }

        Ok(header)
    }

    fn register_model(&mut self, header: HoagsModelHeader) {
        self.available_models.push(header);
    }

    fn set_active(&mut self, idx: u32) {
        self.active_model_idx = idx;
    }

    fn get_active_header(&self) -> Option<&HoagsModelHeader> {
        self.available_models.get(self.active_model_idx as usize)
    }

    fn model_count(&self) -> u32 {
        self.available_models.len() as u32
    }

    fn load_state(&self) -> ModelLoadState {
        self.load_state
    }

    fn load_progress(&self) -> u8 {
        self.load_progress
    }

    fn last_error(&self) -> Option<ModelIoError> {
        self.last_error
    }
}

// =============================================================================
// Public API
// =============================================================================

pub fn init() {
    let mut m = MODEL_IO.lock();
    *m = Some(ModelManager::new());
    serial_println!("    Model I/O: .hoags v2 format, CRC64, layer-wise load ready");
}

/// Parse and validate a model header from raw bytes
pub fn parse_header(data: &[u8]) -> Option<HoagsModelHeader> {
    ModelManager::parse_header(data).ok()
}

/// Serialize a model header to bytes
pub fn serialize_header(header: &HoagsModelHeader) -> Vec<u8> {
    ModelManager::serialize_header(header)
}

/// Create a new model header with given configuration
pub fn create_header(
    vocab: u32,
    dim: u32,
    layers: u32,
    heads: u32,
    kv_heads: u32,
    ffn: u32,
    max_seq: u32,
) -> HoagsModelHeader {
    ModelManager::create_header(vocab, dim, layers, heads, kv_heads, ffn, max_seq)
}

/// Register a model in the global registry
pub fn register_model(header: HoagsModelHeader) {
    if let Some(mgr) = MODEL_IO.lock().as_mut() {
        mgr.register_model(header);
    }
}

/// Get the current load state
pub fn load_state() -> ModelLoadState {
    MODEL_IO
        .lock()
        .as_ref()
        .map_or(ModelLoadState::NotLoaded, |m| m.load_state())
}

/// Get the current load progress (0-100)
pub fn load_progress() -> u8 {
    MODEL_IO.lock().as_ref().map_or(0, |m| m.load_progress())
}

/// Get the number of registered models
pub fn model_count() -> u32 {
    MODEL_IO.lock().as_ref().map_or(0, |m| m.model_count())
}

/// Validate a model file (header + checksum only, no full load)
pub fn validate(data: &[u8]) -> bool {
    MODEL_IO
        .lock()
        .as_ref()
        .map_or(false, |m| m.validate(data).is_ok())
}

/// Compute CRC32 checksum for arbitrary data
pub fn compute_crc32(data: &[u8]) -> u32 {
    let table = build_crc32_table();
    crc32(&table, data)
}

// =============================================================================
// GGUF format loading
// =============================================================================

/// GGUF magic bytes: "GGUF" (0x47 0x47 0x55 0x46)
const GGUF_MAGIC: [u8; 4] = [0x47, 0x47, 0x55, 0x46];

/// GGUF metadata value types
#[derive(Clone, Copy, PartialEq)]
pub enum GgufValueType {
    Uint8 = 0,
    Int8 = 1,
    Uint16 = 2,
    Int16 = 3,
    Uint32 = 4,
    Int32 = 5,
    Float32 = 6,
    Bool = 7,
    String = 8,
    Array = 9,
    Uint64 = 10,
    Int64 = 11,
    Float64 = 12,
    Unknown = 255,
}

impl GgufValueType {
    fn from_u32(v: u32) -> Self {
        match v {
            0 => GgufValueType::Uint8,
            1 => GgufValueType::Int8,
            2 => GgufValueType::Uint16,
            3 => GgufValueType::Int16,
            4 => GgufValueType::Uint32,
            5 => GgufValueType::Int32,
            6 => GgufValueType::Float32,
            7 => GgufValueType::Bool,
            8 => GgufValueType::String,
            9 => GgufValueType::Array,
            10 => GgufValueType::Uint64,
            11 => GgufValueType::Int64,
            12 => GgufValueType::Float64,
            _ => GgufValueType::Unknown,
        }
    }
}

/// A parsed GGUF metadata key-value entry
pub struct GgufMetaEntry {
    /// Key string (as raw bytes)
    pub key: Vec<u8>,
    /// Value type
    pub value_type: GgufValueType,
    /// Raw value bytes (interpretation depends on value_type)
    pub value_bytes: Vec<u8>,
}

/// A parsed GGUF tensor descriptor
pub struct GgufTensorInfo {
    /// Tensor name (as raw bytes)
    pub name: Vec<u8>,
    /// Number of dimensions
    pub n_dims: u32,
    /// Dimension sizes (up to 4)
    pub dims: [u64; 4],
    /// Tensor type (matches llama.cpp ggml type enum)
    pub tensor_type: u32,
    /// Byte offset of tensor data within the tensor data block
    pub offset: u64,
}

/// Result of parsing a GGUF file
pub struct GgufModel {
    /// GGUF format version
    pub version: u32,
    /// Metadata key-value pairs
    pub metadata: Vec<GgufMetaEntry>,
    /// Tensor descriptors
    pub tensors: Vec<GgufTensorInfo>,
}

/// Parse a GGUF string: u64 length (LE) followed by UTF-8 bytes (no NUL).
/// Returns (bytes, new_offset) on success.
fn parse_gguf_string(data: &[u8], offset: usize) -> Option<(Vec<u8>, usize)> {
    if offset + 8 > data.len() {
        return None;
    }
    let len = u64::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ]) as usize;
    let start = offset + 8;
    let end = start.checked_add(len)?;
    if end > data.len() {
        return None;
    }
    Some((data[start..end].to_vec(), end))
}

/// Skip over a GGUF metadata value of the given type without storing it.
/// Returns new offset, or None on parse error.
fn skip_gguf_value(data: &[u8], offset: usize, vtype: GgufValueType) -> Option<usize> {
    match vtype {
        GgufValueType::Uint8 | GgufValueType::Int8 | GgufValueType::Bool => Some(offset + 1),
        GgufValueType::Uint16 | GgufValueType::Int16 => Some(offset + 2),
        GgufValueType::Uint32 | GgufValueType::Int32 | GgufValueType::Float32 => Some(offset + 4),
        GgufValueType::Uint64 | GgufValueType::Int64 | GgufValueType::Float64 => Some(offset + 8),
        GgufValueType::String => {
            let (_, new_off) = parse_gguf_string(data, offset)?;
            Some(new_off)
        }
        GgufValueType::Array => {
            // Array: element_type (u32) + count (u64) + elements
            if offset + 12 > data.len() {
                return None;
            }
            let elem_type = GgufValueType::from_u32(read_u32_le(data, offset)?);
            let count = u64::from_le_bytes([
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
                data[offset + 8],
                data[offset + 9],
                data[offset + 10],
                data[offset + 11],
            ]) as usize;
            let mut cur = offset + 12;
            for _ in 0..count {
                cur = skip_gguf_value(data, cur, elem_type)?;
            }
            Some(cur)
        }
        GgufValueType::Unknown => None,
    }
}

/// Read a GGUF metadata value into raw bytes (small scalar types only).
/// For strings returns the string bytes. Arrays and unknowns are skipped (empty vec).
fn read_gguf_value(data: &[u8], offset: usize, vtype: GgufValueType) -> Option<(Vec<u8>, usize)> {
    match vtype {
        GgufValueType::Uint8 | GgufValueType::Int8 | GgufValueType::Bool => {
            if offset >= data.len() {
                return None;
            }
            Some((alloc::vec![data[offset]], offset + 1))
        }
        GgufValueType::Uint16 | GgufValueType::Int16 => {
            if offset + 2 > data.len() {
                return None;
            }
            Some((data[offset..offset + 2].to_vec(), offset + 2))
        }
        GgufValueType::Uint32 | GgufValueType::Int32 | GgufValueType::Float32 => {
            if offset + 4 > data.len() {
                return None;
            }
            Some((data[offset..offset + 4].to_vec(), offset + 4))
        }
        GgufValueType::Uint64 | GgufValueType::Int64 | GgufValueType::Float64 => {
            if offset + 8 > data.len() {
                return None;
            }
            Some((data[offset..offset + 8].to_vec(), offset + 8))
        }
        GgufValueType::String => {
            let (s, new_off) = parse_gguf_string(data, offset)?;
            Some((s, new_off))
        }
        GgufValueType::Array | GgufValueType::Unknown => {
            // Skip and return empty — array contents not materialised
            let new_off = skip_gguf_value(data, offset, vtype)?;
            Some((Vec::new(), new_off))
        }
    }
}

/// Load and parse a GGUF format model file.
///
/// GGUF binary layout:
///   [0..3]  magic "GGUF"
///   [4..7]  version (u32 LE)
///   [8..15] tensor_count (u64 LE)
///   [16..23] metadata_kv_count (u64 LE)
///   Followed by metadata_kv_count KV pairs:
///     key: gguf_string
///     value_type: u32 LE
///     value: (variable, depends on value_type)
///   Then tensor_count tensor descriptors:
///     name: gguf_string
///     n_dims: u32 LE
///     dims: n_dims * u64 LE (up to 4)
///     tensor_type: u32 LE
///     offset: u64 LE (byte offset within tensor data block)
///
/// Returns `Err` with a static message if the data is malformed.
pub fn load_model_from_gguf(data: &[u8]) -> Result<GgufModel, &'static str> {
    if data.len() < 24 {
        return Err("GGUF: data too short for header");
    }

    // Check magic
    if &data[0..4] != &GGUF_MAGIC {
        return Err("GGUF: invalid magic bytes");
    }

    let version = read_u32_le(data, 4).ok_or("GGUF: truncated version")?;

    // tensor_count and kv_count are u64 in GGUF v3, u32 in v1/v2.
    // We handle v1+ by reading as u64 (v1/v2 treat upper 4 bytes as next field
    // but they are always zero for sane files).
    let tensor_count = u64::from_le_bytes([
        data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
    ]) as usize;

    let kv_count = u64::from_le_bytes([
        data[16], data[17], data[18], data[19], data[20], data[21], data[22], data[23],
    ]) as usize;

    let mut offset = 24;
    let mut metadata: Vec<GgufMetaEntry> = Vec::with_capacity(kv_count.min(256));

    // Parse metadata KV pairs
    for _ in 0..kv_count {
        // Key
        let (key, new_off) =
            parse_gguf_string(data, offset).ok_or("GGUF: truncated metadata key")?;
        offset = new_off;

        // Value type
        if offset + 4 > data.len() {
            return Err("GGUF: truncated metadata value type");
        }
        let vtype_raw = read_u32_le(data, offset).ok_or("GGUF: truncated value type")?;
        offset += 4;
        let vtype = GgufValueType::from_u32(vtype_raw);

        // Value
        let (value_bytes, new_off) =
            read_gguf_value(data, offset, vtype).ok_or("GGUF: failed to parse metadata value")?;
        offset = new_off;

        metadata.push(GgufMetaEntry {
            key,
            value_type: vtype,
            value_bytes,
        });
    }

    let mut tensors: Vec<GgufTensorInfo> = Vec::with_capacity(tensor_count.min(4096));

    // Parse tensor descriptors
    for _ in 0..tensor_count {
        // Tensor name
        let (name, new_off) =
            parse_gguf_string(data, offset).ok_or("GGUF: truncated tensor name")?;
        offset = new_off;

        // n_dims
        if offset + 4 > data.len() {
            return Err("GGUF: truncated tensor n_dims");
        }
        let n_dims = read_u32_le(data, offset).ok_or("GGUF: truncated n_dims")?;
        offset += 4;

        if n_dims > 4 {
            return Err("GGUF: tensor has more than 4 dimensions");
        }

        // dims
        let mut dims = [1u64; 4];
        for d in 0..(n_dims as usize) {
            if offset + 8 > data.len() {
                return Err("GGUF: truncated tensor dim");
            }
            dims[d] = u64::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            ]);
            offset += 8;
        }

        // tensor_type
        if offset + 4 > data.len() {
            return Err("GGUF: truncated tensor type");
        }
        let tensor_type = read_u32_le(data, offset).ok_or("GGUF: truncated tensor type")?;
        offset += 4;

        // offset
        if offset + 8 > data.len() {
            return Err("GGUF: truncated tensor offset");
        }
        let toffset = u64::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
            data[offset + 4],
            data[offset + 5],
            data[offset + 6],
            data[offset + 7],
        ]);
        offset += 8;

        tensors.push(GgufTensorInfo {
            name,
            n_dims,
            dims,
            tensor_type,
            offset: toffset,
        });
    }

    Ok(GgufModel {
        version,
        metadata,
        tensors,
    })
}

// =============================================================================
// Flat "GENESISQ" format loading
// =============================================================================

/// Magic bytes for the flat Genesis-native format: "GENESISQ" (8 bytes)
const GENESISQ_MAGIC: &[u8; 8] = b"GENESISQ";

/// A single layer's flat weight data (F16-encoded as pairs of bytes, big-endian)
pub struct FlatLayer {
    /// Packed weight data for this layer (F16 big-endian or Q4_0 blocks)
    pub weight_data: Vec<u8>,
    /// Number of rows
    pub rows: u32,
    /// Number of columns
    pub cols: u32,
}

/// Parsed flat model
pub struct FlatModel {
    /// Format version
    pub version: u32,
    /// Number of transformer layers
    pub layer_count: u32,
    /// Vocabulary size
    pub vocab_size: u32,
    /// Embedding dimension
    pub embed_dim: u32,
    /// Per-layer weight arrays (layer_count entries)
    pub layers: Vec<FlatLayer>,
    /// Whether the weight data uses Q4_0 quantization (false = flat F16)
    pub is_q4: bool,
}

/// Load a flat "GENESISQ" model from a byte slice.
///
/// Binary layout:
///   [0..7]   magic "GENESISQ"
///   [8..11]  version       (u32 LE)
///   [12..15] layer_count   (u32 LE)
///   [16..19] vocab_size    (u32 LE)
///   [20..23] embed_dim     (u32 LE)
///   [24]     quant_mode    (u8: 0=F16, 1=Q4_0)
///   [25..27] reserved      (3 bytes, must be 0)
///   Followed by layer_count layer blocks, each:
///     [0..3]  rows    (u32 LE)
///     [4..7]  cols    (u32 LE)
///     [8..11] n_bytes (u32 LE) — byte length of weight data following
///     [12..12+n_bytes] raw weight bytes (F16 big-endian or Q4_0 blocks)
///
/// Returns `Err` with a static message if the data is malformed.
pub fn load_model_from_flat(data: &[u8]) -> Result<FlatModel, &'static str> {
    if data.len() < 28 {
        return Err("GENESISQ: data too short for header");
    }

    // Check magic
    if &data[0..8] != GENESISQ_MAGIC {
        return Err("GENESISQ: invalid magic bytes");
    }

    let version = read_u32_le(data, 8).ok_or("GENESISQ: truncated version")?;
    let layer_count = read_u32_le(data, 12).ok_or("GENESISQ: truncated layer_count")?;
    let vocab_size = read_u32_le(data, 16).ok_or("GENESISQ: truncated vocab_size")?;
    let embed_dim = read_u32_le(data, 20).ok_or("GENESISQ: truncated embed_dim")?;
    let quant_mode = data[24];
    // bytes 25-27 reserved

    let is_q4 = quant_mode == 1;
    let mut offset = 28usize;
    let mut layers: Vec<FlatLayer> = Vec::with_capacity(layer_count as usize);

    for _ in 0..layer_count {
        if offset + 12 > data.len() {
            return Err("GENESISQ: truncated layer header");
        }
        let rows = read_u32_le(data, offset).ok_or("GENESISQ: truncated rows")?;
        let cols = read_u32_le(data, offset + 4).ok_or("GENESISQ: truncated cols")?;
        let n_bytes = read_u32_le(data, offset + 8).ok_or("GENESISQ: truncated n_bytes")?;
        offset += 12;

        let end = offset
            .checked_add(n_bytes as usize)
            .ok_or("GENESISQ: layer data length overflow")?;
        if end > data.len() {
            return Err("GENESISQ: layer data extends beyond end of file");
        }

        let weight_data = data[offset..end].to_vec();
        offset = end;

        layers.push(FlatLayer {
            weight_data,
            rows,
            cols,
        });
    }

    Ok(FlatModel {
        version,
        layer_count,
        vocab_size,
        embed_dim,
        layers,
        is_q4,
    })
}
