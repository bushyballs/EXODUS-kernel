use alloc::vec;
/// scrypt — memory-hard key derivation function (RFC 7914)
///
/// Pure Rust implementation of the scrypt password-based KDF.
/// Designed to be both CPU-hard and memory-hard, making brute-force
/// attacks expensive even with specialized hardware (ASICs/FPGAs).
///
/// Used for:
///   - Password hashing
///   - Key derivation from passwords
///   - Proof-of-work schemes
///
/// Algorithm overview:
///   1. Derive initial data from password+salt using PBKDF2-HMAC-SHA256
///   2. Apply ROMix to each parallel lane:
///      a. Build a large memory table using BlockMix (Salsa20/8)
///      b. Randomly access the table N times (memory-hard step)
///   3. Derive the final key using PBKDF2-HMAC-SHA256 again
///
/// Parameters:
///   - N: CPU/memory cost (must be power of 2, e.g., 2^14 = 16384)
///   - r: Block size parameter (typically 8)
///   - p: Parallelism factor (typically 1)
///
/// Memory usage: 128 * r * N bytes per lane
///
/// Part of the AIOS crypto layer.
use alloc::vec::Vec;

/// scrypt parameters
pub struct ScryptParams {
    pub n: u32,      // CPU/memory cost (must be power of 2)
    pub r: u32,      // block size parameter
    pub p: u32,      // parallelism
    pub dk_len: u32, // derived key length
}

/// Salsa20/8 core function.
///
/// The heart of scrypt's mixing function. Applies 8 rounds (4 double-rounds)
/// of the Salsa20 stream cipher to a 64-byte (16-word) block in-place.
///
/// This is Salsa20 reduced to 8 rounds (not the full 20), which is sufficient
/// for scrypt's mixing purposes since we don't need stream cipher security.
///
/// RFC 7914, Section 3
fn salsa20_8(block: &mut [u32; 16]) {
    let mut x = *block;

    // 8 rounds = 4 double rounds
    for _ in 0..4 {
        // Column round
        x[4] ^= x[0].wrapping_add(x[12]).rotate_left(7);
        x[8] ^= x[4].wrapping_add(x[0]).rotate_left(9);
        x[12] ^= x[8].wrapping_add(x[4]).rotate_left(13);
        x[0] ^= x[12].wrapping_add(x[8]).rotate_left(18);

        x[9] ^= x[5].wrapping_add(x[1]).rotate_left(7);
        x[13] ^= x[9].wrapping_add(x[5]).rotate_left(9);
        x[1] ^= x[13].wrapping_add(x[9]).rotate_left(13);
        x[5] ^= x[1].wrapping_add(x[13]).rotate_left(18);

        x[14] ^= x[10].wrapping_add(x[6]).rotate_left(7);
        x[2] ^= x[14].wrapping_add(x[10]).rotate_left(9);
        x[6] ^= x[2].wrapping_add(x[14]).rotate_left(13);
        x[10] ^= x[6].wrapping_add(x[2]).rotate_left(18);

        x[3] ^= x[15].wrapping_add(x[11]).rotate_left(7);
        x[7] ^= x[3].wrapping_add(x[15]).rotate_left(9);
        x[11] ^= x[7].wrapping_add(x[3]).rotate_left(13);
        x[15] ^= x[11].wrapping_add(x[7]).rotate_left(18);

        // Row round
        x[1] ^= x[0].wrapping_add(x[3]).rotate_left(7);
        x[2] ^= x[1].wrapping_add(x[0]).rotate_left(9);
        x[3] ^= x[2].wrapping_add(x[1]).rotate_left(13);
        x[0] ^= x[3].wrapping_add(x[2]).rotate_left(18);

        x[6] ^= x[5].wrapping_add(x[4]).rotate_left(7);
        x[7] ^= x[6].wrapping_add(x[5]).rotate_left(9);
        x[4] ^= x[7].wrapping_add(x[6]).rotate_left(13);
        x[5] ^= x[4].wrapping_add(x[7]).rotate_left(18);

        x[11] ^= x[10].wrapping_add(x[9]).rotate_left(7);
        x[8] ^= x[11].wrapping_add(x[10]).rotate_left(9);
        x[9] ^= x[8].wrapping_add(x[11]).rotate_left(13);
        x[10] ^= x[9].wrapping_add(x[8]).rotate_left(18);

        x[12] ^= x[15].wrapping_add(x[14]).rotate_left(7);
        x[13] ^= x[12].wrapping_add(x[15]).rotate_left(9);
        x[14] ^= x[13].wrapping_add(x[12]).rotate_left(13);
        x[15] ^= x[14].wrapping_add(x[13]).rotate_left(18);
    }

    // Add the input to the output (feedforward)
    for i in 0..16 {
        block[i] = block[i].wrapping_add(x[i]);
    }
}

/// scrypt BlockMix function.
///
/// Processes a 2*r 64-byte blocks using Salsa20/8.
/// Input: B[0] || B[1] || ... || B[2r-1] (each B[i] is 64 bytes)
/// Output: B'[0] || B'[2] || ... || B'[2r-2] || B'[1] || B'[3] || ... || B'[2r-1]
///
/// The output interleaves even and odd indexed blocks.
///
/// RFC 7914, Section 4
fn block_mix(block: &mut [u8]) {
    let r2 = block.len() / 64; // 2*r blocks of 64 bytes each

    // X = B[2r-1] (last 64-byte block)
    let mut x = [0u32; 16];
    let last_block_start = (r2 - 1) * 64;
    for i in 0..16 {
        x[i] = u32::from_le_bytes([
            block[last_block_start + i * 4],
            block[last_block_start + i * 4 + 1],
            block[last_block_start + i * 4 + 2],
            block[last_block_start + i * 4 + 3],
        ]);
    }

    // Process each 64-byte block: X = Salsa20/8(X XOR B[i])
    let mut y = vec![0u8; block.len()];
    for i in 0..r2 {
        // X = X XOR B[i]
        let block_start = i * 64;
        for j in 0..16 {
            let b_word = u32::from_le_bytes([
                block[block_start + j * 4],
                block[block_start + j * 4 + 1],
                block[block_start + j * 4 + 2],
                block[block_start + j * 4 + 3],
            ]);
            x[j] ^= b_word;
        }

        salsa20_8(&mut x);

        // Store in Y with interleaved layout:
        // Even-indexed blocks go to first half, odd to second half
        let dest_idx = if i % 2 == 0 { i / 2 } else { r2 / 2 + i / 2 };
        let dest_start = dest_idx * 64;
        for j in 0..16 {
            let bytes = x[j].to_le_bytes();
            y[dest_start + j * 4] = bytes[0];
            y[dest_start + j * 4 + 1] = bytes[1];
            y[dest_start + j * 4 + 2] = bytes[2];
            y[dest_start + j * 4 + 3] = bytes[3];
        }
    }

    block.copy_from_slice(&y);
}

/// Read a little-endian u64 from the last 64 bytes of a block.
///
/// Used by ROMix to compute the index into the memory table.
/// Takes the first 8 bytes of the last 64-byte sub-block as a LE u64.
fn integerify(block: &[u8], r: u32) -> u64 {
    let block_size = 128 * r as usize;
    // The last 64-byte sub-block starts at offset (2*r - 1) * 64
    let offset = block_size - 64;
    u64::from_le_bytes([
        block[offset],
        block[offset + 1],
        block[offset + 2],
        block[offset + 3],
        block[offset + 4],
        block[offset + 5],
        block[offset + 6],
        block[offset + 7],
    ])
}

/// scrypt ROMix function — the memory-hard core.
///
/// 1. Fill a large table V[0..N-1] with successively mixed blocks
/// 2. Randomly access table entries N times, mixing them into the block
///
/// This is where the memory-hardness comes from: the table must be
/// stored in memory (ROM in the original paper, hence "ROMix"), and
/// random access prevents time-memory trade-offs.
///
/// RFC 7914, Section 5
fn romix(block: &mut [u8], n: u32, r: u32) {
    let block_size = 128 * r as usize;
    let n = n as usize;

    // Step 1: Build the lookup table V[0..N-1]
    let mut v = vec![0u8; block_size * n];
    // V[0] = block
    v[..block_size].copy_from_slice(block);
    for i in 1..n {
        // V[i] = BlockMix(V[i-1])
        let (prev, curr) = v.split_at_mut(i * block_size);
        curr[..block_size].copy_from_slice(&prev[(i - 1) * block_size..i * block_size]);
        block_mix(&mut curr[..block_size]);
    }
    // X = BlockMix(V[N-1])
    let last_start = (n - 1) * block_size;
    block.copy_from_slice(&v[last_start..last_start + block_size]);
    block_mix(block);

    // Step 2: Randomly access the table N times
    for _ in 0..n {
        let j = (integerify(block, r) as usize) % n;
        let v_j_start = j * block_size;

        // X = BlockMix(X XOR V[j])
        for k in 0..block_size {
            block[k] ^= v[v_j_start + k];
        }
        block_mix(block);
    }
}

/// PBKDF2-HMAC-SHA256 for scrypt (RFC 2898 / RFC 7914 Section 6).
///
/// Standard PBKDF2 with exactly 1 iteration (as used by scrypt).
/// Produces `dk_len` bytes of derived key material.
fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32, dk_len: usize) -> Vec<u8> {
    let mut result = Vec::with_capacity(dk_len);
    let blocks_needed = (dk_len + 31) / 32; // Each HMAC-SHA256 produces 32 bytes

    for block_idx in 1..=(blocks_needed as u32) {
        // U_1 = HMAC(password, salt || INT_32_BE(block_idx))
        let mut salt_with_idx = Vec::with_capacity(salt.len() + 4);
        salt_with_idx.extend_from_slice(salt);
        salt_with_idx.extend_from_slice(&block_idx.to_be_bytes());

        let mut u = super::sha256::hmac_sha256(password, &salt_with_idx);
        let mut t = u;

        // For iterations > 1: T = U_1 XOR U_2 XOR ... XOR U_c
        for _ in 1..iterations {
            u = super::sha256::hmac_sha256(password, &u);
            for j in 0..32 {
                t[j] ^= u[j];
            }
        }

        result.extend_from_slice(&t);
    }

    result.truncate(dk_len);
    result
}

/// Derive a key from a password using scrypt.
///
/// This is the main entry point for scrypt key derivation.
///
/// # Arguments
/// - `password`: User password bytes
/// - `salt`: Unique salt (should be random, at least 16 bytes)
/// - `params`: scrypt parameters (N, r, p, dk_len)
///
/// # Returns
/// Derived key of length `params.dk_len` bytes
///
/// # Panics
/// - If N is not a power of 2
/// - If N, r, or p are 0
///
/// RFC 7914, Section 6
pub fn derive(password: &[u8], salt: &[u8], params: &ScryptParams) -> Vec<u8> {
    assert!(
        params.n > 0 && (params.n & (params.n - 1)) == 0,
        "scrypt N must be a power of 2"
    );
    assert!(params.r > 0, "scrypt r must be > 0");
    assert!(params.p > 0, "scrypt p must be > 0");

    let block_size = 128 * params.r as usize;
    let total_blocks = params.p as usize;

    // Step 1: Derive initial blocks using PBKDF2-SHA256(password, salt, 1, p*128*r)
    let mut b = pbkdf2_sha256(password, salt, 1, block_size * total_blocks);

    // Step 2: Apply ROMix to each parallel lane independently
    for i in 0..total_blocks {
        let start = i * block_size;
        let end = start + block_size;
        romix(&mut b[start..end], params.n, params.r);
    }

    // Step 3: Derive the final key using PBKDF2-SHA256(password, B, 1, dk_len)
    pbkdf2_sha256(password, &b, 1, params.dk_len as usize)
}

/// Convenience function with common default parameters.
///
/// Uses N=16384, r=8, p=1, which provides moderate security
/// (~16MB memory, ~100ms on modern hardware).
pub fn derive_default(password: &[u8], salt: &[u8], dk_len: u32) -> Vec<u8> {
    derive(
        password,
        salt,
        &ScryptParams {
            n: 16384,
            r: 8,
            p: 1,
            dk_len,
        },
    )
}

pub fn init() {
    crate::serial_println!("    [scrypt] Memory-hard KDF (Salsa20/8, ROMix) ready");
}
