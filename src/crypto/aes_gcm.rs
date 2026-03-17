/// AES-128-GCM Authenticated Encryption with Associated Data (AEAD)
///
/// Pure Rust, no-alloc, no-float implementation of AES-128-GCM per NIST SP 800-38D.
///
/// Algorithm references:
///   - NIST FIPS 197 — Advanced Encryption Standard (AES)
///   - NIST SP 800-38D — Galois/Counter Mode (GCM) and GMAC
///   - RFC 5116 — An Interface and Algorithms for Authenticated Encryption
///
/// Key properties:
///   - 128-bit (16-byte) key
///   - 96-bit (12-byte) IV/nonce — must be unique per (key, message) pair
///   - 128-bit (16-byte) authentication tag
///   - No alloc: all buffers are caller-provided or fixed-size statics
///   - No floats: pure integer arithmetic throughout
///   - No panics: all fallible paths return Option/Result/enum
///   - Constant-time tag comparison (prevents oracle attacks)
///
/// WARNING: Nonce reuse with the same key completely breaks GCM security.
/// Never encrypt two different plaintexts under the same (key, nonce) pair.

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

/// AES-128 key size in bytes.
pub const AES_GCM_KEY_SIZE: usize = 16;
/// GCM standard 96-bit nonce/IV size in bytes.
pub const AES_GCM_IV_SIZE: usize = 12;
/// GCM authentication tag size in bytes.
pub const AES_GCM_TAG_SIZE: usize = 16;
/// AES block size in bytes (fixed for all AES variants).
pub const AES_GCM_BLOCK_SIZE: usize = 16;
/// Maximum plaintext/ciphertext size supported (kernel limitation).
pub const AES_GCM_MAX_PLAINTEXT: usize = 1024;
/// Maximum AAD size supported.
pub const AES_GCM_MAX_AAD: usize = 256;

// ---------------------------------------------------------------------------
// AES S-Box (forward, 256 bytes)
// ---------------------------------------------------------------------------

/// AES forward S-Box (SubBytes substitution table).
/// Source: NIST FIPS 197, Figure 7.
pub const AES_SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

/// AES-128 round constants (Rcon) for key schedule.
/// Rcon[i] = 2^i in GF(2^8) with the AES irreducible polynomial.
const RCON: [u8; 10] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36];

// ---------------------------------------------------------------------------
// GF(2^8) helper for AES MixColumns
// ---------------------------------------------------------------------------

/// Multiply by 2 in GF(2^8) with AES irreducible polynomial x^8+x^4+x^3+x+1.
#[inline(always)]
fn xtime(a: u8) -> u8 {
    let hi = (a >> 7) & 1;
    let shifted = a << 1;
    shifted ^ (hi.wrapping_neg() & 0x1b)
}

/// Multiply by 3 in GF(2^8): mul3(a) = xtime(a) ^ a.
#[inline(always)]
fn mul3(a: u8) -> u8 {
    xtime(a) ^ a
}

// ---------------------------------------------------------------------------
// AES-128 key schedule
// ---------------------------------------------------------------------------

/// AES-128 key expansion.
///
/// Produces 11 round keys (the original key + 10 derived keys) = 176 bytes.
/// NIST FIPS 197, Section 5.2 (KeyExpansion), Nk=4, Nr=10.
///
/// Round key layout in the returned array:
///   rk[0*16 .. 1*16] = round key 0 (= original key)
///   rk[1*16 .. 2*16] = round key 1
///   ...
///   rk[10*16..11*16] = round key 10
fn aes128_key_expand(key: &[u8; 16]) -> [u8; 176] {
    // w[0..44]: 44 key words of 4 bytes each.
    // We work word-by-word as per FIPS 197.
    let mut w = [0u32; 44];

    // Load the 4 key words (big-endian bytes → u32).
    let mut k = 0usize;
    while k < 4 {
        w[k] = u32::from_be_bytes([key[4 * k], key[4 * k + 1], key[4 * k + 2], key[4 * k + 3]]);
        k = k.saturating_add(1);
    }

    // Expand words 4..44.
    let mut i = 4usize;
    while i < 44 {
        let mut temp = w[i - 1];
        if i % 4 == 0 {
            // RotWord: rotate left by 8 bits (byte rotation).
            temp = temp.rotate_left(8);
            // SubWord: apply S-Box to each byte.
            let b = temp.to_be_bytes();
            temp = u32::from_be_bytes([
                AES_SBOX[b[0] as usize],
                AES_SBOX[b[1] as usize],
                AES_SBOX[b[2] as usize],
                AES_SBOX[b[3] as usize],
            ]);
            // XOR with Rcon — divide index guard: i/4 - 1 is always in [0,9].
            let rcon_idx = (i >> 2).wrapping_sub(1);
            if rcon_idx < 10 {
                temp ^= (RCON[rcon_idx] as u32) << 24;
            }
        }
        w[i] = w[i - 4] ^ temp;
        i = i.saturating_add(1);
    }

    // Pack the 44 words into 176 bytes (big-endian per word).
    let mut rk = [0u8; 176];
    let mut idx = 0usize;
    while idx < 44 {
        let bytes = w[idx].to_be_bytes();
        let base = idx * 4;
        rk[base] = bytes[0];
        rk[base + 1] = bytes[1];
        rk[base + 2] = bytes[2];
        rk[base + 3] = bytes[3];
        idx = idx.saturating_add(1);
    }
    rk
}

// ---------------------------------------------------------------------------
// AES-128 block cipher (encrypt only — GCM only needs forward cipher)
// ---------------------------------------------------------------------------

/// AES-128 encrypt a single 16-byte block in-place.
///
/// FIPS 197 Section 5.1 — Cipher:
///   Round 0:      AddRoundKey(state, rk[0])
///   Rounds 1-9:   SubBytes → ShiftRows → MixColumns → AddRoundKey(state, rk[r])
///   Round 10:     SubBytes → ShiftRows → AddRoundKey(state, rk[10])
///
/// `rk` is the 176-byte key schedule from `aes128_key_expand`.
fn aes128_encrypt_block(block: &mut [u8; 16], rk: &[u8; 176]) {
    // Round 0: AddRoundKey with rk[0].
    let mut i = 0usize;
    while i < 16 {
        block[i] ^= rk[i];
        i = i.saturating_add(1);
    }

    // Rounds 1–9 (full rounds including MixColumns).
    let mut round = 1usize;
    while round < 10 {
        // SubBytes.
        let mut b = 0usize;
        while b < 16 {
            block[b] = AES_SBOX[block[b] as usize];
            b = b.saturating_add(1);
        }

        // ShiftRows.
        // Row 1 (bytes at indices 1,5,9,13): rotate left by 1.
        {
            let t = block[1];
            block[1] = block[5];
            block[5] = block[9];
            block[9] = block[13];
            block[13] = t;
        }
        // Row 2 (bytes 2,6,10,14): rotate left by 2.
        {
            let t0 = block[2];
            let t1 = block[6];
            block[2] = block[10];
            block[6] = block[14];
            block[10] = t0;
            block[14] = t1;
        }
        // Row 3 (bytes 3,7,11,15): rotate left by 3 (= right by 1).
        {
            let t = block[15];
            block[15] = block[11];
            block[11] = block[7];
            block[7] = block[3];
            block[3] = t;
        }

        // MixColumns: each column [s0,s1,s2,s3] → [2s0^3s1^s2^s3, s0^2s1^3s2^s3, ...].
        let mut col = 0usize;
        while col < 4 {
            let ci = col * 4;
            let s0 = block[ci];
            let s1 = block[ci + 1];
            let s2 = block[ci + 2];
            let s3 = block[ci + 3];
            block[ci] = xtime(s0) ^ mul3(s1) ^ s2 ^ s3;
            block[ci + 1] = s0 ^ xtime(s1) ^ mul3(s2) ^ s3;
            block[ci + 2] = s0 ^ s1 ^ xtime(s2) ^ mul3(s3);
            block[ci + 3] = mul3(s0) ^ s1 ^ s2 ^ xtime(s3);
            col = col.saturating_add(1);
        }

        // AddRoundKey: XOR with round key `round`.
        let rk_base = round * 16;
        let mut j = 0usize;
        while j < 16 {
            block[j] ^= rk[rk_base + j];
            j = j.saturating_add(1);
        }

        round = round.saturating_add(1);
    }

    // Round 10 (final: SubBytes + ShiftRows + AddRoundKey, no MixColumns).
    let mut b = 0usize;
    while b < 16 {
        block[b] = AES_SBOX[block[b] as usize];
        b = b.saturating_add(1);
    }
    {
        let t = block[1];
        block[1] = block[5];
        block[5] = block[9];
        block[9] = block[13];
        block[13] = t;
    }
    {
        let t0 = block[2];
        let t1 = block[6];
        block[2] = block[10];
        block[6] = block[14];
        block[10] = t0;
        block[14] = t1;
    }
    {
        let t = block[15];
        block[15] = block[11];
        block[11] = block[7];
        block[7] = block[3];
        block[3] = t;
    }
    // AddRoundKey with rk[10].
    let rk_base = 160usize;
    let mut j = 0usize;
    while j < 16 {
        block[j] ^= rk[rk_base + j];
        j = j.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// GF(2^128) multiplication for GHASH
// ---------------------------------------------------------------------------

/// Multiply two 128-bit values in GF(2^128) with the GCM reduction polynomial
/// p(x) = x^128 + x^7 + x^2 + x + 1 (represented as R = 0xE1 at byte 0).
///
/// Both `x` and `y` are 16-byte big-endian values (MSB of the field element
/// is in byte 0, bit 7).
///
/// Algorithm: right-to-left binary method (NIST SP 800-38D §6.3).
///   Z = 0; V = Y
///   for i in 0..128:
///     if bit i of X is 1: Z ^= V
///     if LSB of V is 1:   V = (V >> 1) ^ R
///     else:               V = V >> 1
#[inline]
fn gf128_mul(x: &[u8; 16], y: &[u8; 16]) -> [u8; 16] {
    let mut z = [0u8; 16];
    let mut v = *y;

    let mut i = 0usize;
    while i < 128 {
        let byte_idx = i >> 3;
        let bit_pos = 7u8.wrapping_sub((i & 7) as u8);
        if (x[byte_idx] >> bit_pos) & 1 == 1 {
            let mut k = 0usize;
            while k < 16 {
                z[k] ^= v[k];
                k = k.saturating_add(1);
            }
        }

        // Save LSB of V (the rightmost bit in big-endian = bit 0 of byte 15).
        let lsb = v[15] & 1;

        // Right-shift V by 1 bit (big-endian: shift byte 15 first).
        let mut k = 15i32;
        while k >= 1 {
            v[k as usize] = (v[k as usize] >> 1) | (v[(k - 1) as usize] << 7);
            k -= 1;
        }
        v[0] >>= 1;

        // If LSB was 1, XOR with R = 0xE1 || 0x00...0x00.
        if lsb != 0 {
            v[0] ^= 0xe1;
        }

        i = i.saturating_add(1);
    }

    z
}

// ---------------------------------------------------------------------------
// GHASH universal hash function (NIST SP 800-38D §6.4)
// ---------------------------------------------------------------------------

/// GHASH(H, A, C) — produces a 128-bit hash of AAD `A` and ciphertext `C`
/// using the hash subkey H = AES_K(0^128).
///
/// Processing order: A || 0* || C || 0* || [len(A)]_64 || [len(C)]_64.
/// Each block is 16 bytes; partial blocks are zero-padded.
fn ghash(h: &[u8; 16], aad: &[u8], ciphertext: &[u8]) -> [u8; 16] {
    let mut y = [0u8; 16];

    // Process AAD blocks.
    let mut offset = 0usize;
    while offset < aad.len() {
        let mut block = [0u8; 16];
        let remaining = aad.len().saturating_sub(offset);
        let copy_len = if remaining < 16 { remaining } else { 16 };
        let mut k = 0usize;
        while k < copy_len {
            block[k] = aad[offset + k];
            k = k.saturating_add(1);
        }
        let mut j = 0usize;
        while j < 16 {
            y[j] ^= block[j];
            j = j.saturating_add(1);
        }
        y = gf128_mul(&y, h);
        offset = offset.saturating_add(16);
    }

    // Process ciphertext blocks.
    offset = 0;
    while offset < ciphertext.len() {
        let mut block = [0u8; 16];
        let remaining = ciphertext.len().saturating_sub(offset);
        let copy_len = if remaining < 16 { remaining } else { 16 };
        let mut k = 0usize;
        while k < copy_len {
            block[k] = ciphertext[offset + k];
            k = k.saturating_add(1);
        }
        let mut j = 0usize;
        while j < 16 {
            y[j] ^= block[j];
            j = j.saturating_add(1);
        }
        y = gf128_mul(&y, h);
        offset = offset.saturating_add(16);
    }

    // Final length block: [len(A)]_64 || [len(C)]_64 in bits, big-endian.
    let aad_bits: u64 = (aad.len() as u64).wrapping_mul(8);
    let ct_bits: u64 = (ciphertext.len() as u64).wrapping_mul(8);
    let mut len_block = [0u8; 16];
    len_block[0] = (aad_bits >> 56) as u8;
    len_block[1] = (aad_bits >> 48) as u8;
    len_block[2] = (aad_bits >> 40) as u8;
    len_block[3] = (aad_bits >> 32) as u8;
    len_block[4] = (aad_bits >> 24) as u8;
    len_block[5] = (aad_bits >> 16) as u8;
    len_block[6] = (aad_bits >> 8) as u8;
    len_block[7] = aad_bits as u8;
    len_block[8] = (ct_bits >> 56) as u8;
    len_block[9] = (ct_bits >> 48) as u8;
    len_block[10] = (ct_bits >> 40) as u8;
    len_block[11] = (ct_bits >> 32) as u8;
    len_block[12] = (ct_bits >> 24) as u8;
    len_block[13] = (ct_bits >> 16) as u8;
    len_block[14] = (ct_bits >> 8) as u8;
    len_block[15] = ct_bits as u8;
    let mut j = 0usize;
    while j < 16 {
        y[j] ^= len_block[j];
        j = j.saturating_add(1);
    }
    y = gf128_mul(&y, h);

    y
}

// ---------------------------------------------------------------------------
// GCM counter increment (inc32 — only the rightmost 32 bits wrap)
// ---------------------------------------------------------------------------

/// Increment the rightmost 32 bits of a 128-bit GCM counter block (big-endian).
///
/// NIST SP 800-38D: inc_s(X) adds 1 to the rightmost s bits (s=32) modulo 2^32.
/// Only bytes 12–15 are modified; bytes 0–11 (the nonce) are unchanged.
#[inline(always)]
fn gcm_inc32(counter: &mut [u8; 16]) {
    let mut carry = 1u32;
    let mut i = 15usize;
    loop {
        let sum = counter[i] as u32 + carry;
        counter[i] = sum as u8;
        carry = sum >> 8;
        if carry == 0 || i == 12 {
            return;
        }
        i -= 1;
    }
}

// ---------------------------------------------------------------------------
// Constant-time tag comparison
// ---------------------------------------------------------------------------

/// Constant-time comparison of two 16-byte authentication tags.
///
/// Always examines all 16 bytes regardless of where mismatches occur.
/// Prevents timing side-channels that would reveal which bytes are correct.
#[inline]
fn ct_eq_16(a: &[u8; 16], b: &[u8; 16]) -> bool {
    let mut diff: u8 = 0;
    let mut i = 0usize;
    while i < 16 {
        diff |= a[i] ^ b[i];
        i = i.saturating_add(1);
    }
    diff == 0
}

// ---------------------------------------------------------------------------
// Public result type
// ---------------------------------------------------------------------------

/// Result of an AES-GCM decryption operation.
#[derive(Copy, Clone, PartialEq)]
pub enum AesGcmResult {
    /// Decryption succeeded and tag verified.
    Ok,
    /// Authentication tag did not match — ciphertext or AAD was tampered.
    AuthFailed,
    /// Input length exceeded the kernel maximum (1024 bytes plaintext / 256 bytes AAD).
    InvalidLength,
}

// ---------------------------------------------------------------------------
// Public AES-128-GCM AEAD interface
// ---------------------------------------------------------------------------

/// AES-128-GCM encryption.
///
/// Protocol (NIST SP 800-38D §7.1):
///   1. Derive hash subkey: H = AES_K(0^128)
///   2. Build initial counter: J0 = IV || 0x00000001
///   3. CTR-encrypt plaintext starting at inc32(J0)
///   4. Compute tag: T = GHASH(H, AAD, CT) ^ AES_K(J0)
///
/// # Arguments
/// - `key`       — 16-byte AES-128 key
/// - `iv`        — 12-byte nonce (must be unique per (key, message))
/// - `aad`       — additional authenticated data, max `AES_GCM_MAX_AAD` bytes
/// - `plaintext` — data to encrypt, max `AES_GCM_MAX_PLAINTEXT` bytes
/// - `ciphertext`— output buffer, exactly 1024 bytes
/// - `tag`       — 16-byte output authentication tag
///
/// # Returns
/// Number of ciphertext bytes actually written (= `plaintext.len()` on success, 0 on overflow).
pub fn aes128_gcm_encrypt(
    key: &[u8; AES_GCM_KEY_SIZE],
    iv: &[u8; AES_GCM_IV_SIZE],
    aad: &[u8],
    plaintext: &[u8],
    ciphertext: &mut [u8; AES_GCM_MAX_PLAINTEXT],
    tag: &mut [u8; AES_GCM_TAG_SIZE],
) -> usize {
    // Guard: reject oversized inputs.
    if plaintext.len() > AES_GCM_MAX_PLAINTEXT || aad.len() > AES_GCM_MAX_AAD {
        return 0;
    }

    // 1. Key expansion.
    let rk = aes128_key_expand(key);

    // 2. Hash subkey H = AES_K(0^128).
    let mut h = [0u8; 16];
    aes128_encrypt_block(&mut h, &rk);

    // 3. Build J0 = IV || 0x00000001.
    let mut j0 = [0u8; 16];
    let mut k = 0usize;
    while k < 12 {
        j0[k] = iv[k];
        k = k.saturating_add(1);
    }
    j0[12] = 0;
    j0[13] = 0;
    j0[14] = 0;
    j0[15] = 1;

    // 4. CTR encryption starting at counter = inc32(J0).
    let mut counter = j0;
    gcm_inc32(&mut counter);

    let pt_len = plaintext.len();
    let mut offset = 0usize;
    while offset < pt_len {
        let mut ks = counter;
        aes128_encrypt_block(&mut ks, &rk);
        let remaining = pt_len.saturating_sub(offset);
        let to_xor = if remaining < 16 { remaining } else { 16 };
        let mut b = 0usize;
        while b < to_xor {
            ciphertext[offset + b] = plaintext[offset + b] ^ ks[b];
            b = b.saturating_add(1);
        }
        gcm_inc32(&mut counter);
        offset = offset.saturating_add(16);
    }

    // 5. Compute authentication tag: S = GHASH(H, AAD, CT); T = S ^ AES_K(J0).
    let ct_slice: &[u8] = &ciphertext[..pt_len];
    let s = ghash(&h, aad, ct_slice);

    let mut j0_enc = j0;
    aes128_encrypt_block(&mut j0_enc, &rk);

    let mut i = 0usize;
    while i < 16 {
        tag[i] = s[i] ^ j0_enc[i];
        i = i.saturating_add(1);
    }

    pt_len
}

/// AES-128-GCM decryption and tag verification.
///
/// Protocol (NIST SP 800-38D §7.2):
///   1. H = AES_K(0^128)
///   2. J0 = IV || 0x00000001
///   3. S_expected = GHASH(H, AAD, CT) ^ AES_K(J0)
///   4. Constant-time compare S_expected with provided tag
///   5. Only if equal: CTR-decrypt CT → PT
///
/// IMPORTANT: Tag verification runs BEFORE decryption to prevent oracle attacks.
///
/// # Arguments
/// - `key`       — 16-byte AES-128 key
/// - `iv`        — 12-byte nonce (same as used during encryption)
/// - `aad`       — additional authenticated data (must match encryption AAD)
/// - `ciphertext`— input ciphertext bytes, max `AES_GCM_MAX_PLAINTEXT`
/// - `tag`       — 16-byte authentication tag to verify
/// - `plaintext` — output buffer, exactly 1024 bytes
///
/// # Returns
/// `(bytes_written, AesGcmResult)` — bytes_written is 0 on any failure.
pub fn aes128_gcm_decrypt(
    key: &[u8; AES_GCM_KEY_SIZE],
    iv: &[u8; AES_GCM_IV_SIZE],
    aad: &[u8],
    ciphertext: &[u8],
    tag: &[u8; AES_GCM_TAG_SIZE],
    plaintext: &mut [u8; AES_GCM_MAX_PLAINTEXT],
) -> (usize, AesGcmResult) {
    // Guard: reject oversized inputs.
    if ciphertext.len() > AES_GCM_MAX_PLAINTEXT || aad.len() > AES_GCM_MAX_AAD {
        return (0, AesGcmResult::InvalidLength);
    }

    // 1. Key expansion.
    let rk = aes128_key_expand(key);

    // 2. Hash subkey H = AES_K(0^128).
    let mut h = [0u8; 16];
    aes128_encrypt_block(&mut h, &rk);

    // 3. Build J0 = IV || 0x00000001.
    let mut j0 = [0u8; 16];
    let mut k = 0usize;
    while k < 12 {
        j0[k] = iv[k];
        k = k.saturating_add(1);
    }
    j0[12] = 0;
    j0[13] = 0;
    j0[14] = 0;
    j0[15] = 1;

    // 4. Compute expected tag: S = GHASH(H, AAD, CT); expected = S ^ AES_K(J0).
    let s = ghash(&h, aad, ciphertext);
    let mut j0_enc = j0;
    aes128_encrypt_block(&mut j0_enc, &rk);
    let mut expected_tag = [0u8; 16];
    let mut i = 0usize;
    while i < 16 {
        expected_tag[i] = s[i] ^ j0_enc[i];
        i = i.saturating_add(1);
    }

    // 5. Constant-time tag comparison — fail fast if mismatch.
    if !ct_eq_16(&expected_tag, tag) {
        return (0, AesGcmResult::AuthFailed);
    }

    // 6. Tag valid — CTR-decrypt starting at counter = inc32(J0).
    let mut counter = j0;
    gcm_inc32(&mut counter);

    let ct_len = ciphertext.len();
    let mut offset = 0usize;
    while offset < ct_len {
        let mut ks = counter;
        aes128_encrypt_block(&mut ks, &rk);
        let remaining = ct_len.saturating_sub(offset);
        let to_xor = if remaining < 16 { remaining } else { 16 };
        let mut b = 0usize;
        while b < to_xor {
            plaintext[offset + b] = ciphertext[offset + b] ^ ks[b];
            b = b.saturating_add(1);
        }
        gcm_inc32(&mut counter);
        offset = offset.saturating_add(16);
    }

    (ct_len, AesGcmResult::Ok)
}

// ---------------------------------------------------------------------------
// Self-test and init
// ---------------------------------------------------------------------------

/// AES-128-GCM self-test.
///
/// Test 1: Zero key/IV, empty plaintext — verifies tag is non-zero (GHASH
///         with H=AES(0,0) and empty inputs produces a predictable non-zero result).
/// Test 2: Encrypt-then-decrypt round-trip with non-trivial key/IV/AAD.
/// Test 3: Tampered tag must be rejected.
/// Test 4: Tampered AAD must be rejected.
///
/// Returns `true` if all checks pass.
fn self_test() -> bool {
    // ---- Test 1: Empty plaintext, zero key/IV — tag must be non-zero ----
    let key0 = [0u8; 16];
    let iv0 = [0u8; 12];
    let aad0: [u8; 0] = [];
    let pt0: [u8; 0] = [];
    let mut ct0 = [0u8; AES_GCM_MAX_PLAINTEXT];
    let mut tag0 = [0u8; 16];
    let n = aes128_gcm_encrypt(&key0, &iv0, &aad0, &pt0, &mut ct0, &mut tag0);
    if n != 0 {
        return false;
    }
    // Tag should not be all zeros (verifies GHASH + AES block cipher fired).
    let mut any_nonzero = false;
    let mut i = 0usize;
    while i < 16 {
        if tag0[i] != 0 {
            any_nonzero = true;
        }
        i = i.saturating_add(1);
    }
    if !any_nonzero {
        return false;
    }

    // ---- Test 2: Round-trip with non-trivial parameters ----
    let key1: [u8; 16] = [
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
        0x10,
    ];
    let iv1: [u8; 12] = [
        0xca, 0xfe, 0xba, 0xbe, 0xfa, 0xce, 0xdb, 0xad, 0xde, 0xca, 0xf8, 0x88,
    ];
    let aad1 = b"genesis kernel";
    let pt1 = b"Hello, AES-128-GCM!";

    let mut ct1 = [0u8; AES_GCM_MAX_PLAINTEXT];
    let mut tag1 = [0u8; 16];
    let n1 = aes128_gcm_encrypt(&key1, &iv1, aad1, pt1, &mut ct1, &mut tag1);
    if n1 != pt1.len() {
        return false;
    }

    // Ciphertext must differ from plaintext.
    let mut differs = false;
    let mut i = 0usize;
    while i < pt1.len() {
        if ct1[i] != pt1[i] {
            differs = true;
        }
        i = i.saturating_add(1);
    }
    if !differs {
        return false;
    }

    // Decrypt must recover the original plaintext.
    let mut pt1_out = [0u8; AES_GCM_MAX_PLAINTEXT];
    let (n_dec, res) = aes128_gcm_decrypt(&key1, &iv1, aad1, &ct1[..n1], &tag1, &mut pt1_out);
    if res != AesGcmResult::Ok {
        return false;
    }
    if n_dec != pt1.len() {
        return false;
    }
    let mut i = 0usize;
    while i < pt1.len() {
        if pt1_out[i] != pt1[i] {
            return false;
        }
        i = i.saturating_add(1);
    }

    // ---- Test 3: Tampered tag must be rejected ----
    let mut bad_tag = tag1;
    bad_tag[0] ^= 0xff;
    let mut dummy = [0u8; AES_GCM_MAX_PLAINTEXT];
    let (_, res3) = aes128_gcm_decrypt(&key1, &iv1, aad1, &ct1[..n1], &bad_tag, &mut dummy);
    if res3 != AesGcmResult::AuthFailed {
        return false;
    }

    // ---- Test 4: Tampered AAD must be rejected ----
    let bad_aad = b"wrong kernel aad!";
    let (_, res4) = aes128_gcm_decrypt(&key1, &iv1, bad_aad, &ct1[..n1], &tag1, &mut dummy);
    if res4 != AesGcmResult::AuthFailed {
        return false;
    }

    true
}

/// Initialise AES-128-GCM module — run self-test and report.
pub fn init() {
    if self_test() {
        crate::serial_println!("    [aes_gcm] AES-128-GCM AEAD initialized (self-test PASSED)");
    } else {
        crate::serial_println!("    [aes_gcm] AES-128-GCM AEAD initialized (self-test FAILED!)");
    }
}
