/// Transparent filesystem compression
///
/// Part of the AIOS storage layer.
///
/// Implements a simplified LZ4-style compression using run-length encoding
/// with match finding. No external crates are used.
use crate::sync::Mutex;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

pub enum CompressionAlgo {
    Lz4,
    Zstd,
    None,
}

pub struct Compressor {
    algo: CompressionAlgo,
    /// Minimum match length before encoding as a reference.
    min_match_len: usize,
    /// Maximum lookback window size.
    window_size: usize,
}

impl Compressor {
    pub fn new(algo: CompressionAlgo) -> Self {
        let (min_match, window) = match algo {
            CompressionAlgo::Lz4 => (4, 4096),
            CompressionAlgo::Zstd => (3, 8192),
            CompressionAlgo::None => (0, 0),
        };
        Compressor {
            algo,
            min_match_len: min_match,
            window_size: window,
        }
    }

    /// Compress input data.
    ///
    /// Format: A stream of tokens. Each token is:
    ///   - Literal byte `0x00` followed by `u16-le length` followed by `length` literal bytes.
    ///   - Match byte `0x01` followed by `u16-le offset` and `u16-le length`.
    ///
    /// For `CompressionAlgo::None`, the output is the input unchanged.
    pub fn compress(&self, input: &[u8]) -> Result<Vec<u8>, ()> {
        match self.algo {
            CompressionAlgo::None => Ok(input.to_vec()),
            CompressionAlgo::Lz4 | CompressionAlgo::Zstd => self.compress_lz(input),
        }
    }

    /// Decompress data previously compressed by `compress`.
    pub fn decompress(&self, input: &[u8]) -> Result<Vec<u8>, ()> {
        match self.algo {
            CompressionAlgo::None => Ok(input.to_vec()),
            CompressionAlgo::Lz4 | CompressionAlgo::Zstd => self.decompress_lz(input),
        }
    }

    /// Simple LZ-style compression with literal and match tokens.
    fn compress_lz(&self, input: &[u8]) -> Result<Vec<u8>, ()> {
        if input.is_empty() {
            return Ok(Vec::new());
        }

        let mut output = Vec::new();
        let mut pos = 0usize;
        let mut literal_start = 0usize;

        while pos < input.len() {
            // Search for a match in the lookback window
            let window_start = if pos > self.window_size {
                pos - self.window_size
            } else {
                0
            };
            let mut best_offset = 0usize;
            let mut best_len = 0usize;

            let mut search = window_start;
            while search < pos {
                let mut match_len = 0usize;
                while pos + match_len < input.len()
                    && search + match_len < pos
                    && input[search + match_len] == input[pos + match_len]
                    && match_len < 65535
                {
                    match_len += 1;
                }
                if match_len >= self.min_match_len && match_len > best_len {
                    best_offset = pos - search;
                    best_len = match_len;
                }
                search += 1;
            }

            if best_len >= self.min_match_len {
                // Emit any pending literals first
                if literal_start < pos {
                    let lit_len = pos - literal_start;
                    self.emit_literal(&input[literal_start..pos], lit_len, &mut output);
                }

                // Emit match token
                output.push(0x01);
                output.push((best_offset & 0xFF) as u8);
                output.push(((best_offset >> 8) & 0xFF) as u8);
                output.push((best_len & 0xFF) as u8);
                output.push(((best_len >> 8) & 0xFF) as u8);

                pos += best_len;
                literal_start = pos;
            } else {
                pos += 1;
            }
        }

        // Emit remaining literals
        if literal_start < input.len() {
            let lit_len = input.len() - literal_start;
            self.emit_literal(&input[literal_start..], lit_len, &mut output);
        }

        Ok(output)
    }

    fn emit_literal(&self, data: &[u8], len: usize, output: &mut Vec<u8>) {
        output.push(0x00);
        output.push((len & 0xFF) as u8);
        output.push(((len >> 8) & 0xFF) as u8);
        output.extend_from_slice(&data[..len]);
    }

    /// Decompress LZ-encoded data.
    fn decompress_lz(&self, input: &[u8]) -> Result<Vec<u8>, ()> {
        let mut output = Vec::new();
        let mut pos = 0usize;

        while pos < input.len() {
            let token = input[pos];
            pos += 1;

            match token {
                0x00 => {
                    // Literal token
                    if pos + 2 > input.len() {
                        return Err(());
                    }
                    let len = (input[pos] as usize) | ((input[pos + 1] as usize) << 8);
                    pos += 2;
                    if pos + len > input.len() {
                        return Err(());
                    }
                    output.extend_from_slice(&input[pos..pos + len]);
                    pos += len;
                }
                0x01 => {
                    // Match token
                    if pos + 4 > input.len() {
                        return Err(());
                    }
                    let offset = (input[pos] as usize) | ((input[pos + 1] as usize) << 8);
                    let length = (input[pos + 2] as usize) | ((input[pos + 3] as usize) << 8);
                    pos += 4;

                    if offset == 0 || offset > output.len() {
                        return Err(());
                    }

                    let start = output.len() - offset;
                    for i in 0..length {
                        let byte = output[start + (i % offset)];
                        output.push(byte);
                    }
                }
                _ => {
                    return Err(());
                }
            }
        }

        Ok(output)
    }

    /// Return the compression ratio as a Q16 fixed-point value.
    /// ratio = compressed_size / original_size * (1 << 16).
    pub fn ratio_q16(original: usize, compressed: usize) -> i32 {
        if original == 0 {
            return 1 << 16;
        }
        ((compressed as u64 * (1u64 << 16)) / original as u64) as i32
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static COMPRESSOR: Mutex<Option<Compressor>> = Mutex::new(None);

pub fn init() {
    let mut guard = COMPRESSOR.lock();
    *guard = Some(Compressor::new(CompressionAlgo::Lz4));
    serial_println!("  [storage] Compression subsystem initialized (LZ4)");
}

/// Access the compressor under lock.
pub fn with_compressor<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut Compressor) -> R,
{
    let mut guard = COMPRESSOR.lock();
    guard.as_mut().map(f)
}
