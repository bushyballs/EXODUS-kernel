/// BLAKE2b hash function — faster than SHA-256, RFC 7693
///
/// Pure Rust implementation of the BLAKE2b cryptographic hash function.
/// Produces up to 64 bytes of output. Default output is 64 bytes.
///
/// Used for:
///   - Fast file integrity checking
///   - Key derivation
///   - Merkle tree hashing
///
/// Implementation details:
///   - 12 rounds of the BLAKE2b compression function
///   - 128-byte (1024-bit) message blocks
///   - 8-word (512-bit) chaining value
///   - Configurable output length (1-64 bytes)
///   - Optional keyed hashing (MAC mode)
///
/// Security: Up to 256-bit collision resistance (64-byte output).

/// BLAKE2b block size in bytes (128 bytes = 1024 bits)
const BLOCK_SIZE: usize = 128;

/// Number of rounds in BLAKE2b
const ROUNDS: usize = 12;

/// BLAKE2b IV: first 8 fractional digits of sqrt(2..9) as u64
/// Same as SHA-512 IV values
const IV: [u64; 8] = [
    0x6a09e667f3bcc908,
    0xbb67ae8584caa73b,
    0x3c6ef372fe94f82b,
    0xa54ff53a5f1d36f1,
    0x510e527fade682d1,
    0x9b05688c2b3e6c1f,
    0x1f83d9abfb41bd6b,
    0x5be0cd19137e2179,
];

/// BLAKE2b message schedule permutation (sigma)
/// 12 rounds x 16 indices, per RFC 7693 Section 2.7
const SIGMA: [[usize; 16]; 12] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
    [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
    [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
    [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
    [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
    [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
    [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
    [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
    [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
];

/// BLAKE2b mixing function G
///
/// Operates on four words of the internal state, mixing in two message words.
/// Uses three rotations (32, 24, 16, 63) which differ from BLAKE2s.
///
/// RFC 7693, Section 3.1:
///   a = a + b + x
///   d = (d ^ a) >>> 32
///   c = c + d
///   b = (b ^ c) >>> 24
///   a = a + b + y
///   d = (d ^ a) >>> 16
///   c = c + d
///   b = (b ^ c) >>> 63
#[inline(always)]
fn g(v: &mut [u64; 16], a: usize, b: usize, c: usize, d: usize, x: u64, y: u64) {
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
    v[d] = (v[d] ^ v[a]).rotate_right(32);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(24);
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
    v[d] = (v[d] ^ v[a]).rotate_right(16);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(63);
}

/// BLAKE2b state (configurable output length 1-64 bytes)
pub struct Blake2b {
    /// Current hash state (eight 64-bit words)
    h: [u64; 8],
    /// Internal buffer for accumulating partial blocks
    buf: [u8; BLOCK_SIZE],
    /// Number of bytes currently in the buffer
    buf_len: usize,
    /// Total number of bytes compressed (128-bit counter, low word)
    t0: u64,
    /// Total counter high word
    t1: u64,
    /// Desired output length in bytes
    out_len: usize,
}

impl Blake2b {
    /// Create a new BLAKE2b hasher with the specified output length.
    ///
    /// output_len must be 1..=64. Default is 64 for BLAKE2b-512.
    pub fn new(output_len: usize) -> Self {
        assert!(
            output_len >= 1 && output_len <= 64,
            "BLAKE2b output_len must be 1..64"
        );

        // Initialize state from IV
        let mut h = IV;

        // XOR parameter block into h[0]
        // Parameter block (RFC 7693 Section 2.5):
        //   Byte 0: digest length
        //   Byte 1: key length (0 for unkeyed)
        //   Byte 2: fanout (1 for sequential)
        //   Byte 3: depth (1 for sequential)
        // Remaining bytes: 0
        h[0] ^= 0x01010000 ^ (output_len as u64);

        Blake2b {
            h,
            buf: [0u8; BLOCK_SIZE],
            buf_len: 0,
            t0: 0,
            t1: 0,
            out_len: output_len,
        }
    }

    /// Create a new BLAKE2b hasher in keyed mode (MAC).
    ///
    /// key must be 1..=64 bytes. output_len must be 1..=64 bytes.
    pub fn new_keyed(key: &[u8], output_len: usize) -> Self {
        assert!(output_len >= 1 && output_len <= 64);
        assert!(!key.is_empty() && key.len() <= 64);

        let mut h = IV;
        // Parameter block with key length
        h[0] ^= 0x01010000 ^ ((key.len() as u64) << 8) ^ (output_len as u64);

        let mut state = Blake2b {
            h,
            buf: [0u8; BLOCK_SIZE],
            buf_len: 0,
            t0: 0,
            t1: 0,
            out_len: output_len,
        };

        // If keyed, the first block is the key padded to BLOCK_SIZE
        let mut key_block = [0u8; BLOCK_SIZE];
        key_block[..key.len()].copy_from_slice(key);
        state.update(&key_block);

        state
    }

    /// BLAKE2b compression function.
    ///
    /// Processes a single 128-byte block. The `last` flag indicates whether
    /// this is the final block (inverts the finalization flag).
    ///
    /// RFC 7693, Section 3.2
    fn compress(&mut self, block: &[u8; BLOCK_SIZE], last: bool) {
        // Parse message block as 16 little-endian u64 words
        let mut m = [0u64; 16];
        for i in 0..16 {
            m[i] = u64::from_le_bytes([
                block[i * 8],
                block[i * 8 + 1],
                block[i * 8 + 2],
                block[i * 8 + 3],
                block[i * 8 + 4],
                block[i * 8 + 5],
                block[i * 8 + 6],
                block[i * 8 + 7],
            ]);
        }

        // Initialize local working vector v[0..15]
        //   v[0..8] = h[0..8]  (current hash state)
        //   v[8..12] = IV[0..4]
        //   v[12] = IV[4] ^ t0  (counter low)
        //   v[13] = IV[5] ^ t1  (counter high)
        //   v[14] = IV[6] ^ f0  (finalization flag)
        //   v[15] = IV[7]
        let mut v = [0u64; 16];
        v[..8].copy_from_slice(&self.h);
        v[8] = IV[0];
        v[9] = IV[1];
        v[10] = IV[2];
        v[11] = IV[3];
        v[12] = IV[4] ^ self.t0;
        v[13] = IV[5] ^ self.t1;
        v[14] = if last {
            IV[6] ^ 0xFFFFFFFF_FFFFFFFF
        } else {
            IV[6]
        };
        v[15] = IV[7];

        // 12 rounds of mixing
        for round in 0..ROUNDS {
            let s = &SIGMA[round];

            // Column step
            g(&mut v, 0, 4, 8, 12, m[s[0]], m[s[1]]);
            g(&mut v, 1, 5, 9, 13, m[s[2]], m[s[3]]);
            g(&mut v, 2, 6, 10, 14, m[s[4]], m[s[5]]);
            g(&mut v, 3, 7, 11, 15, m[s[6]], m[s[7]]);

            // Diagonal step
            g(&mut v, 0, 5, 10, 15, m[s[8]], m[s[9]]);
            g(&mut v, 1, 6, 11, 12, m[s[10]], m[s[11]]);
            g(&mut v, 2, 7, 8, 13, m[s[12]], m[s[13]]);
            g(&mut v, 3, 4, 9, 14, m[s[14]], m[s[15]]);
        }

        // Feedforward: h[i] = h[i] ^ v[i] ^ v[i+8]
        for i in 0..8 {
            self.h[i] ^= v[i] ^ v[i + 8];
        }
    }

    /// Increment the 128-bit counter by `inc` bytes.
    fn increment_counter(&mut self, inc: u64) {
        self.t0 = self.t0.wrapping_add(inc);
        if self.t0 < inc {
            self.t1 = self.t1.wrapping_add(1);
        }
    }

    /// Feed data into the hasher. Can be called multiple times.
    pub fn update(&mut self, data: &[u8]) {
        let mut offset = 0;

        // If we have buffered data, try to complete a block
        if self.buf_len > 0 {
            let fill = BLOCK_SIZE - self.buf_len;
            let copy = data.len().min(fill);
            self.buf[self.buf_len..self.buf_len + copy].copy_from_slice(&data[..copy]);
            self.buf_len += copy;
            offset = copy;

            if self.buf_len == BLOCK_SIZE && offset < data.len() {
                // We have a full block AND more data coming, so this is not the last block
                self.increment_counter(BLOCK_SIZE as u64);
                let block = self.buf;
                self.compress(&block, false);
                self.buf_len = 0;
            }
        }

        // Process full blocks, but always keep the last block in the buffer
        // (because we need to know if it's the final block for the finalization flag)
        while offset + BLOCK_SIZE < data.len() {
            let mut block = [0u8; BLOCK_SIZE];
            block.copy_from_slice(&data[offset..offset + BLOCK_SIZE]);
            self.increment_counter(BLOCK_SIZE as u64);
            self.compress(&block, false);
            offset += BLOCK_SIZE;
        }

        // Buffer remaining bytes
        if offset < data.len() {
            let remaining = data.len() - offset;
            // If buffer already has data and we didn't flush above
            if self.buf_len > 0 && self.buf_len + remaining <= BLOCK_SIZE {
                self.buf[self.buf_len..self.buf_len + remaining].copy_from_slice(&data[offset..]);
                self.buf_len += remaining;
            } else {
                self.buf[..remaining].copy_from_slice(&data[offset..]);
                self.buf_len = remaining;
            }
        }
    }

    /// Finalize the hash and return the digest.
    ///
    /// Always returns a 64-byte array. Only the first `out_len` bytes are meaningful.
    pub fn finalize(mut self) -> [u8; 64] {
        // Increment counter by the remaining bytes in the buffer
        self.increment_counter(self.buf_len as u64);

        // Zero-pad the buffer to a full block
        for i in self.buf_len..BLOCK_SIZE {
            self.buf[i] = 0;
        }

        // Compress the final block with the `last` flag set
        let block = self.buf;
        self.compress(&block, true);

        // Serialize h[0..8] as little-endian bytes
        let mut out = [0u8; 64];
        for i in 0..8 {
            let bytes = self.h[i].to_le_bytes();
            out[i * 8..i * 8 + 8].copy_from_slice(&bytes);
        }
        out
    }

    /// Finalize and return only the requested number of bytes.
    pub fn finalize_truncated(self) -> alloc::vec::Vec<u8> {
        let out_len = self.out_len;
        let full = self.finalize();
        let mut result = alloc::vec::Vec::with_capacity(out_len);
        result.extend_from_slice(&full[..out_len]);
        result
    }
}

/// One-shot BLAKE2b-256 hash (32-byte output)
pub fn blake2b_256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Blake2b::new(32);
    hasher.update(data);
    let full = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&full[..32]);
    out
}

/// One-shot BLAKE2b-512 hash (64-byte output)
pub fn blake2b_512(data: &[u8]) -> [u8; 64] {
    let mut hasher = Blake2b::new(64);
    hasher.update(data);
    hasher.finalize()
}

/// One-shot keyed BLAKE2b MAC (variable output length)
pub fn blake2b_mac(key: &[u8], data: &[u8], out_len: usize) -> alloc::vec::Vec<u8> {
    let mut hasher = Blake2b::new_keyed(key, out_len);
    hasher.update(data);
    hasher.finalize_truncated()
}

pub fn init() {
    crate::serial_println!("    [blake2] BLAKE2b (256/512, keyed MAC) ready");
}
