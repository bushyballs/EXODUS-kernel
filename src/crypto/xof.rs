/// Extendable Output Functions — SHAKE128/SHAKE256
///
/// Pure Rust Keccak-based XOF implementation (FIPS 202).
/// Provides SHAKE128 (128-bit security) and SHAKE256 (256-bit security).
///
/// Used for:
///   - Key derivation with variable-length output
///   - Domain separation in post-quantum schemes (Kyber, Dilithium)
///   - Randomness expansion
///
/// Implementation details:
///   - Keccak-f[1600] permutation (25 x 64-bit state, 24 rounds)
///   - Sponge construction: absorb then squeeze
///   - SHAKE128: rate = 168 bytes, capacity = 32 bytes
///   - SHAKE256: rate = 136 bytes, capacity = 64 bytes
///
/// Part of the AIOS crypto layer.
use alloc::vec::Vec;

/// Number of Keccak-f rounds
const KECCAK_ROUNDS: usize = 24;

/// State size: 25 u64 words = 200 bytes = 1600 bits
const STATE_WORDS: usize = 25;

/// SHAKE variant
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShakeVariant {
    Shake128,
    Shake256,
}

/// Keccak-f[1600] round constants (iota step)
const RC: [u64; 24] = [
    0x0000000000000001,
    0x0000000000008082,
    0x800000000000808A,
    0x8000000080008000,
    0x000000000000808B,
    0x0000000080000001,
    0x8000000080008081,
    0x8000000000008009,
    0x000000000000008A,
    0x0000000000000088,
    0x0000000080008009,
    0x000000008000000A,
    0x000000008000808B,
    0x800000000000008B,
    0x8000000000008089,
    0x8000000000008003,
    0x8000000000008002,
    0x8000000000000080,
    0x000000000000800A,
    0x800000008000000A,
    0x8000000080008081,
    0x8000000000008080,
    0x0000000080000001,
    0x8000000080008008,
];

/// Rotation offsets for the rho step
const ROT: [u32; 25] = [
    0, 1, 62, 28, 27, 36, 44, 6, 55, 20, 3, 10, 43, 25, 39, 41, 45, 15, 21, 8, 18, 2, 61, 56, 14,
];

/// Pi step permutation indices
/// pi: A[x,y] -> A[y, 2x+3y mod 5]
/// Stored as a flat mapping: state[PI[i]] = old_state[i]
const PI: [usize; 25] = [
    0, 10, 20, 5, 15, 16, 1, 11, 21, 6, 7, 17, 2, 12, 22, 23, 8, 18, 3, 13, 14, 24, 9, 19, 4,
];

/// Keccak-f[1600] permutation.
///
/// The core transformation applied to the 1600-bit state.
/// Consists of 24 rounds, each with 5 steps: theta, rho, pi, chi, iota.
///
/// FIPS 202, Section 3.3
fn keccak_f(state: &mut [u64; STATE_WORDS]) {
    for round in 0..KECCAK_ROUNDS {
        // === Theta step ===
        // Compute column parities
        let mut c = [0u64; 5];
        for x in 0..5 {
            c[x] = state[x] ^ state[x + 5] ^ state[x + 10] ^ state[x + 15] ^ state[x + 20];
        }
        let mut d = [0u64; 5];
        for x in 0..5 {
            d[x] = c[(x + 4) % 5] ^ c[(x + 1) % 5].rotate_left(1);
        }
        for x in 0..5 {
            for y in 0..5 {
                state[x + 5 * y] ^= d[x];
            }
        }

        // === Rho and Pi steps (combined) ===
        // Rho: rotate each lane by a fixed amount
        // Pi: rearrange lane positions
        let mut temp = [0u64; STATE_WORDS];
        for i in 0..STATE_WORDS {
            temp[PI[i]] = state[i].rotate_left(ROT[i]);
        }

        // === Chi step ===
        // Non-linear mixing: for each row, XOR with AND of complement of neighbor
        for y in 0..5 {
            let base = 5 * y;
            let t0 = temp[base];
            let t1 = temp[base + 1];
            let t2 = temp[base + 2];
            let t3 = temp[base + 3];
            let t4 = temp[base + 4];
            state[base] = t0 ^ (!t1 & t2);
            state[base + 1] = t1 ^ (!t2 & t3);
            state[base + 2] = t2 ^ (!t3 & t4);
            state[base + 3] = t3 ^ (!t4 & t0);
            state[base + 4] = t4 ^ (!t0 & t1);
        }

        // === Iota step ===
        // Break symmetry by XORing a round constant into state[0]
        state[0] ^= RC[round];
    }
}

/// XOF (Extendable Output Function) state
///
/// Implements the Keccak sponge construction for SHAKE128/SHAKE256.
/// Operates in two phases:
///   1. Absorb: feed input data into the sponge
///   2. Squeeze: extract output bytes from the sponge
pub struct Xof {
    /// SHAKE variant (determines rate/capacity)
    pub variant: ShakeVariant,
    /// Keccak state (1600 bits = 200 bytes = 25 u64 words)
    state: [u64; STATE_WORDS],
    /// Rate in bytes (168 for SHAKE128, 136 for SHAKE256)
    rate: usize,
    /// Position within the current rate block (absorption offset)
    offset: usize,
    /// Whether we have finished absorbing (switched to squeeze phase)
    squeezed: bool,
}

impl Xof {
    /// Create a new XOF instance.
    ///
    /// - SHAKE128: 168-byte rate, 128-bit security
    /// - SHAKE256: 136-byte rate, 256-bit security
    pub fn new(variant: ShakeVariant) -> Self {
        let rate = match variant {
            ShakeVariant::Shake128 => 168, // (1600 - 2*128) / 8
            ShakeVariant::Shake256 => 136, // (1600 - 2*256) / 8
        };

        Xof {
            variant,
            state: [0u64; STATE_WORDS],
            rate,
            offset: 0,
            squeezed: false,
        }
    }

    /// XOR a byte into the state at the given byte position.
    #[inline(always)]
    fn xor_byte(&mut self, pos: usize, byte: u8) {
        let word_idx = pos / 8;
        let byte_idx = pos % 8;
        self.state[word_idx] ^= (byte as u64) << (byte_idx * 8);
    }

    /// Read a byte from the state at the given byte position.
    #[inline(always)]
    fn read_byte(&self, pos: usize) -> u8 {
        let word_idx = pos / 8;
        let byte_idx = pos % 8;
        (self.state[word_idx] >> (byte_idx * 8)) as u8
    }

    /// Absorb data into the sponge.
    ///
    /// Can be called multiple times before squeezing.
    /// Panics if called after squeeze() has been called.
    pub fn absorb(&mut self, data: &[u8]) {
        assert!(!self.squeezed, "Cannot absorb after squeezing");

        let mut i = 0;
        while i < data.len() {
            // XOR input bytes into the state at the rate portion
            let space = self.rate - self.offset;
            let to_absorb = space.min(data.len() - i);

            for j in 0..to_absorb {
                self.xor_byte(self.offset + j, data[i + j]);
            }

            self.offset += to_absorb;
            i += to_absorb;

            // When the rate portion is full, apply the permutation
            if self.offset == self.rate {
                keccak_f(&mut self.state);
                self.offset = 0;
            }
        }
    }

    /// Finalize absorption and apply SHAKE domain separation + padding.
    ///
    /// SHAKE uses the suffix 0x1F (domain separation for XOF),
    /// followed by multi-rate padding (pad10*1).
    fn finalize_absorb(&mut self) {
        if self.squeezed {
            return;
        }

        // SHAKE domain separation: append 0x1F
        // 0x1F = 0b00011111 = SHAKE suffix (4 bits: 1111) + padding start (01)
        self.xor_byte(self.offset, 0x1F);

        // Multi-rate padding: set the last bit of the rate block
        self.xor_byte(self.rate - 1, 0x80);

        // Apply the permutation
        keccak_f(&mut self.state);

        self.offset = 0;
        self.squeezed = true;
    }

    /// Squeeze output bytes from the sponge.
    ///
    /// Can be called multiple times to produce arbitrary-length output.
    /// Automatically finalizes absorption on first call.
    pub fn squeeze(&mut self, output_len: usize) -> Vec<u8> {
        // Finalize absorption if not done yet
        if !self.squeezed {
            self.finalize_absorb();
        }

        let mut output = Vec::with_capacity(output_len);
        let mut remaining = output_len;

        while remaining > 0 {
            // Read bytes from the current rate portion
            let available = self.rate - self.offset;
            let to_squeeze = available.min(remaining);

            for j in 0..to_squeeze {
                output.push(self.read_byte(self.offset + j));
            }

            self.offset += to_squeeze;
            remaining -= to_squeeze;

            // When the rate portion is exhausted, apply the permutation
            if self.offset == self.rate && remaining > 0 {
                keccak_f(&mut self.state);
                self.offset = 0;
            }
        }

        output
    }

    /// Squeeze into a fixed-size buffer
    pub fn squeeze_into(&mut self, buf: &mut [u8]) {
        if !self.squeezed {
            self.finalize_absorb();
        }

        let mut offset = 0;
        while offset < buf.len() {
            let available = self.rate - self.offset;
            let to_squeeze = available.min(buf.len() - offset);

            for j in 0..to_squeeze {
                buf[offset + j] = self.read_byte(self.offset + j);
            }

            self.offset += to_squeeze;
            offset += to_squeeze;

            if self.offset == self.rate && offset < buf.len() {
                keccak_f(&mut self.state);
                self.offset = 0;
            }
        }
    }
}

/// One-shot SHAKE128
pub fn shake128(data: &[u8], output_len: usize) -> Vec<u8> {
    let mut xof = Xof::new(ShakeVariant::Shake128);
    xof.absorb(data);
    xof.squeeze(output_len)
}

/// One-shot SHAKE256
pub fn shake256(data: &[u8], output_len: usize) -> Vec<u8> {
    let mut xof = Xof::new(ShakeVariant::Shake256);
    xof.absorb(data);
    xof.squeeze(output_len)
}

/// SHA-3-256 hash (fixed output, not XOF)
///
/// Uses the same Keccak-f[1600] permutation but with SHA-3 domain
/// separation (0x06) instead of SHAKE (0x1F).
pub fn sha3_256(data: &[u8]) -> [u8; 32] {
    let rate = 136; // Same rate as SHAKE256
    let mut state = [0u64; STATE_WORDS];
    let mut offset = 0;

    // Absorb
    for &byte in data {
        let word_idx = offset / 8;
        let byte_idx = offset % 8;
        state[word_idx] ^= (byte as u64) << (byte_idx * 8);
        offset += 1;
        if offset == rate {
            keccak_f(&mut state);
            offset = 0;
        }
    }

    // SHA-3 domain separation: 0x06
    let word_idx = offset / 8;
    let byte_idx = offset % 8;
    state[word_idx] ^= 0x06u64 << (byte_idx * 8);

    // Padding: set last bit of rate
    let last_pos = rate - 1;
    let word_idx = last_pos / 8;
    let byte_idx = last_pos % 8;
    state[word_idx] ^= 0x80u64 << (byte_idx * 8);

    keccak_f(&mut state);

    // Squeeze 32 bytes
    let mut out = [0u8; 32];
    for i in 0..32 {
        let word_idx = i / 8;
        let byte_idx = i % 8;
        out[i] = (state[word_idx] >> (byte_idx * 8)) as u8;
    }
    out
}

pub fn init() {
    crate::serial_println!("    [xof] SHAKE128/SHAKE256/SHA3-256 (Keccak) ready");
}
