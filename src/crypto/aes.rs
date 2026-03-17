/// AES-256 block cipher implementation
///
/// Pure Rust AES-256 with ECB, CBC, CTR, and GCM modes.
/// Used for: disk encryption, secure storage, TLS data encryption.
///
/// Key size: 256 bits (32 bytes)
/// Block size: 128 bits (16 bytes)
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

/// AES S-Box (SubBytes substitution table)
const SBOX: [u8; 256] = [
    0x63, 0x7C, 0x77, 0x7B, 0xF2, 0x6B, 0x6F, 0xC5, 0x30, 0x01, 0x67, 0x2B, 0xFE, 0xD7, 0xAB, 0x76,
    0xCA, 0x82, 0xC9, 0x7D, 0xFA, 0x59, 0x47, 0xF0, 0xAD, 0xD4, 0xA2, 0xAF, 0x9C, 0xA4, 0x72, 0xC0,
    0xB7, 0xFD, 0x93, 0x26, 0x36, 0x3F, 0xF7, 0xCC, 0x34, 0xA5, 0xE5, 0xF1, 0x71, 0xD8, 0x31, 0x15,
    0x04, 0xC7, 0x23, 0xC3, 0x18, 0x96, 0x05, 0x9A, 0x07, 0x12, 0x80, 0xE2, 0xEB, 0x27, 0xB2, 0x75,
    0x09, 0x83, 0x2C, 0x1A, 0x1B, 0x6E, 0x5A, 0xA0, 0x52, 0x3B, 0xD6, 0xB3, 0x29, 0xE3, 0x2F, 0x84,
    0x53, 0xD1, 0x00, 0xED, 0x20, 0xFC, 0xB1, 0x5B, 0x6A, 0xCB, 0xBE, 0x39, 0x4A, 0x4C, 0x58, 0xCF,
    0xD0, 0xEF, 0xAA, 0xFB, 0x43, 0x4D, 0x33, 0x85, 0x45, 0xF9, 0x02, 0x7F, 0x50, 0x3C, 0x9F, 0xA8,
    0x51, 0xA3, 0x40, 0x8F, 0x92, 0x9D, 0x38, 0xF5, 0xBC, 0xB6, 0xDA, 0x21, 0x10, 0xFF, 0xF3, 0xD2,
    0xCD, 0x0C, 0x13, 0xEC, 0x5F, 0x97, 0x44, 0x17, 0xC4, 0xA7, 0x7E, 0x3D, 0x64, 0x5D, 0x19, 0x73,
    0x60, 0x81, 0x4F, 0xDC, 0x22, 0x2A, 0x90, 0x88, 0x46, 0xEE, 0xB8, 0x14, 0xDE, 0x5E, 0x0B, 0xDB,
    0xE0, 0x32, 0x3A, 0x0A, 0x49, 0x06, 0x24, 0x5C, 0xC2, 0xD3, 0xAC, 0x62, 0x91, 0x95, 0xE4, 0x79,
    0xE7, 0xC8, 0x37, 0x6D, 0x8D, 0xD5, 0x4E, 0xA9, 0x6C, 0x56, 0xF4, 0xEA, 0x65, 0x7A, 0xAE, 0x08,
    0xBA, 0x78, 0x25, 0x2E, 0x1C, 0xA6, 0xB4, 0xC6, 0xE8, 0xDD, 0x74, 0x1F, 0x4B, 0xBD, 0x8B, 0x8A,
    0x70, 0x3E, 0xB5, 0x66, 0x48, 0x03, 0xF6, 0x0E, 0x61, 0x35, 0x57, 0xB9, 0x86, 0xC1, 0x1D, 0x9E,
    0xE1, 0xF8, 0x98, 0x11, 0x69, 0xD9, 0x8E, 0x94, 0x9B, 0x1E, 0x87, 0xE9, 0xCE, 0x55, 0x28, 0xDF,
    0x8C, 0xA1, 0x89, 0x0D, 0xBF, 0xE6, 0x42, 0x68, 0x41, 0x99, 0x2D, 0x0F, 0xB0, 0x54, 0xBB, 0x16,
];

/// Inverse S-Box (InvSubBytes substitution table)
const INV_SBOX: [u8; 256] = [
    0x52, 0x09, 0x6A, 0xD5, 0x30, 0x36, 0xA5, 0x38, 0xBF, 0x40, 0xA3, 0x9E, 0x81, 0xF3, 0xD7, 0xFB,
    0x7C, 0xE3, 0x39, 0x82, 0x9B, 0x2F, 0xFF, 0x87, 0x34, 0x8E, 0x43, 0x44, 0xC4, 0xDE, 0xE9, 0xCB,
    0x54, 0x7B, 0x94, 0x32, 0xA6, 0xC2, 0x23, 0x3D, 0xEE, 0x4C, 0x95, 0x0B, 0x42, 0xFA, 0xC3, 0x4E,
    0x08, 0x2E, 0xA1, 0x66, 0x28, 0xD9, 0x24, 0xB2, 0x76, 0x5B, 0xA2, 0x49, 0x6D, 0x8B, 0xD1, 0x25,
    0x72, 0xF8, 0xF6, 0x64, 0x86, 0x68, 0x98, 0x16, 0xD4, 0xA4, 0x5C, 0xCC, 0x5D, 0x65, 0xB6, 0x92,
    0x6C, 0x70, 0x48, 0x50, 0xFD, 0xED, 0xB9, 0xDA, 0x5E, 0x15, 0x46, 0x57, 0xA7, 0x8D, 0x9D, 0x84,
    0x90, 0xD8, 0xAB, 0x00, 0x8C, 0xBC, 0xD3, 0x0A, 0xF7, 0xE4, 0x58, 0x05, 0xB8, 0xB3, 0x45, 0x06,
    0xD0, 0x2C, 0x1E, 0x8F, 0xCA, 0x3F, 0x0F, 0x02, 0xC1, 0xAF, 0xBD, 0x03, 0x01, 0x13, 0x8A, 0x6B,
    0x3A, 0x91, 0x11, 0x41, 0x4F, 0x67, 0xDC, 0xEA, 0x97, 0xF2, 0xCF, 0xCE, 0xF0, 0xB4, 0xE6, 0x73,
    0x96, 0xAC, 0x74, 0x22, 0xE7, 0xAD, 0x35, 0x85, 0xE2, 0xF9, 0x37, 0xE8, 0x1C, 0x75, 0xDF, 0x6E,
    0x47, 0xF1, 0x1A, 0x71, 0x1D, 0x29, 0xC5, 0x89, 0x6F, 0xB7, 0x62, 0x0E, 0xAA, 0x18, 0xBE, 0x1B,
    0xFC, 0x56, 0x3E, 0x4B, 0xC6, 0xD2, 0x79, 0x20, 0x9A, 0xDB, 0xC0, 0xFE, 0x78, 0xCD, 0x5A, 0xF4,
    0x1F, 0xDD, 0xA8, 0x33, 0x88, 0x07, 0xC7, 0x31, 0xB1, 0x12, 0x10, 0x59, 0x27, 0x80, 0xEC, 0x5F,
    0x60, 0x51, 0x7F, 0xA9, 0x19, 0xB5, 0x4A, 0x0D, 0x2D, 0xE5, 0x7A, 0x9F, 0x93, 0xC9, 0x9C, 0xEF,
    0xA0, 0xE0, 0x3B, 0x4D, 0xAE, 0x2A, 0xF5, 0xB0, 0xC8, 0xEB, 0xBB, 0x3C, 0x83, 0x53, 0x99, 0x61,
    0x17, 0x2B, 0x04, 0x7E, 0xBA, 0x77, 0xD6, 0x26, 0xE1, 0x69, 0x14, 0x63, 0x55, 0x21, 0x0C, 0x7D,
];

/// Round constants for key expansion
const RCON: [u8; 10] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1B, 0x36];

/// GF(2^8) multiplication by 2 (xtime)
#[inline(always)]
fn xtime(a: u8) -> u8 {
    let shifted = (a as u16) << 1;
    if a & 0x80 != 0 {
        (shifted ^ 0x11B) as u8
    } else {
        shifted as u8
    }
}

/// GF(2^8) multiplication
#[inline(always)]
fn gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut result: u8 = 0;
    for _ in 0..8 {
        if b & 1 != 0 {
            result ^= a;
        }
        a = xtime(a);
        b >>= 1;
    }
    result
}

/// AES-256 expanded key schedule (15 round keys = 240 bytes)
pub struct Aes256 {
    round_keys: [[u8; 16]; 15],
}

impl Aes256 {
    /// Create AES-256 cipher from a 32-byte key
    pub fn new(key: &[u8; 32]) -> Self {
        let round_keys = Self::key_expansion(key);
        Aes256 { round_keys }
    }

    /// AES-256 key expansion: 32-byte key -> 15 round keys (14 rounds + initial)
    fn key_expansion(key: &[u8; 32]) -> [[u8; 16]; 15] {
        let mut w = [0u8; 240]; // 60 words * 4 bytes
                                // Copy original key
        w[..32].copy_from_slice(key);

        let nk = 8; // key words for AES-256
        let nb = 4; // block words
        let nr = 14; // rounds for AES-256

        let mut i = nk;
        while i < nb * (nr + 1) {
            let mut temp = [
                w[4 * (i - 1)],
                w[4 * (i - 1) + 1],
                w[4 * (i - 1) + 2],
                w[4 * (i - 1) + 3],
            ];

            if i % nk == 0 {
                // RotWord + SubWord + Rcon
                let t = temp[0];
                temp[0] = SBOX[temp[1] as usize] ^ RCON[i / nk - 1];
                temp[1] = SBOX[temp[2] as usize];
                temp[2] = SBOX[temp[3] as usize];
                temp[3] = SBOX[t as usize];
            } else if i % nk == 4 {
                // SubWord only (AES-256 extra step)
                for byte in temp.iter_mut() {
                    *byte = SBOX[*byte as usize];
                }
            }

            for j in 0..4 {
                w[4 * i + j] = w[4 * (i - nk) + j] ^ temp[j];
            }
            i += 1;
        }

        let mut round_keys = [[0u8; 16]; 15];
        for r in 0..15 {
            round_keys[r].copy_from_slice(&w[r * 16..(r + 1) * 16]);
        }
        round_keys
    }

    /// SubBytes: substitute each byte using S-Box
    #[inline(always)]
    fn sub_bytes(state: &mut [u8; 16]) {
        for byte in state.iter_mut() {
            *byte = SBOX[*byte as usize];
        }
    }

    /// InvSubBytes: inverse substitution
    #[inline(always)]
    fn inv_sub_bytes(state: &mut [u8; 16]) {
        for byte in state.iter_mut() {
            *byte = INV_SBOX[*byte as usize];
        }
    }

    /// ShiftRows: cyclically shift rows of the state matrix
    #[inline(always)]
    fn shift_rows(state: &mut [u8; 16]) {
        // Row 1: shift left by 1
        let t = state[1];
        state[1] = state[5];
        state[5] = state[9];
        state[9] = state[13];
        state[13] = t;
        // Row 2: shift left by 2
        let t0 = state[2];
        let t1 = state[6];
        state[2] = state[10];
        state[6] = state[14];
        state[10] = t0;
        state[14] = t1;
        // Row 3: shift left by 3 (= right by 1)
        let t = state[15];
        state[15] = state[11];
        state[11] = state[7];
        state[7] = state[3];
        state[3] = t;
    }

    /// InvShiftRows: inverse row shift
    #[inline(always)]
    fn inv_shift_rows(state: &mut [u8; 16]) {
        // Row 1: shift right by 1
        let t = state[13];
        state[13] = state[9];
        state[9] = state[5];
        state[5] = state[1];
        state[1] = t;
        // Row 2: shift right by 2
        let t0 = state[2];
        let t1 = state[6];
        state[2] = state[10];
        state[6] = state[14];
        state[10] = t0;
        state[14] = t1;
        // Row 3: shift right by 3 (= left by 1)
        let t = state[3];
        state[3] = state[7];
        state[7] = state[11];
        state[11] = state[15];
        state[15] = t;
    }

    /// MixColumns: mix columns of the state matrix in GF(2^8)
    #[inline(always)]
    fn mix_columns(state: &mut [u8; 16]) {
        for col in 0..4 {
            let i = col * 4;
            let s0 = state[i];
            let s1 = state[i + 1];
            let s2 = state[i + 2];
            let s3 = state[i + 3];

            state[i] = gf_mul(2, s0) ^ gf_mul(3, s1) ^ s2 ^ s3;
            state[i + 1] = s0 ^ gf_mul(2, s1) ^ gf_mul(3, s2) ^ s3;
            state[i + 2] = s0 ^ s1 ^ gf_mul(2, s2) ^ gf_mul(3, s3);
            state[i + 3] = gf_mul(3, s0) ^ s1 ^ s2 ^ gf_mul(2, s3);
        }
    }

    /// InvMixColumns: inverse column mixing
    #[inline(always)]
    fn inv_mix_columns(state: &mut [u8; 16]) {
        for col in 0..4 {
            let i = col * 4;
            let s0 = state[i];
            let s1 = state[i + 1];
            let s2 = state[i + 2];
            let s3 = state[i + 3];

            state[i] = gf_mul(0x0E, s0) ^ gf_mul(0x0B, s1) ^ gf_mul(0x0D, s2) ^ gf_mul(0x09, s3);
            state[i + 1] =
                gf_mul(0x09, s0) ^ gf_mul(0x0E, s1) ^ gf_mul(0x0B, s2) ^ gf_mul(0x0D, s3);
            state[i + 2] =
                gf_mul(0x0D, s0) ^ gf_mul(0x09, s1) ^ gf_mul(0x0E, s2) ^ gf_mul(0x0B, s3);
            state[i + 3] =
                gf_mul(0x0B, s0) ^ gf_mul(0x0D, s1) ^ gf_mul(0x09, s2) ^ gf_mul(0x0E, s3);
        }
    }

    /// AddRoundKey: XOR state with round key
    #[inline(always)]
    fn add_round_key(state: &mut [u8; 16], round_key: &[u8; 16]) {
        for i in 0..16 {
            state[i] ^= round_key[i];
        }
    }

    /// Encrypt a single 16-byte block (ECB mode, single block)
    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        Self::add_round_key(block, &self.round_keys[0]);

        for round in 1..14 {
            Self::sub_bytes(block);
            Self::shift_rows(block);
            Self::mix_columns(block);
            Self::add_round_key(block, &self.round_keys[round]);
        }

        // Final round (no MixColumns)
        Self::sub_bytes(block);
        Self::shift_rows(block);
        Self::add_round_key(block, &self.round_keys[14]);
    }

    /// Decrypt a single 16-byte block
    pub fn decrypt_block(&self, block: &mut [u8; 16]) {
        Self::add_round_key(block, &self.round_keys[14]);

        for round in (1..14).rev() {
            Self::inv_shift_rows(block);
            Self::inv_sub_bytes(block);
            Self::add_round_key(block, &self.round_keys[round]);
            Self::inv_mix_columns(block);
        }

        // Final round (no InvMixColumns)
        Self::inv_shift_rows(block);
        Self::inv_sub_bytes(block);
        Self::add_round_key(block, &self.round_keys[0]);
    }
}

/// PKCS#7 padding: add padding bytes to fill last block
fn pkcs7_pad(data: &[u8]) -> Vec<u8> {
    let pad_len = 16 - (data.len() % 16);
    let mut padded = Vec::with_capacity(data.len() + pad_len);
    padded.extend_from_slice(data);
    for _ in 0..pad_len {
        padded.push(pad_len as u8);
    }
    padded
}

/// Remove PKCS#7 padding; returns None if padding is invalid
fn pkcs7_unpad(data: &[u8]) -> Option<Vec<u8>> {
    if data.is_empty() || data.len() % 16 != 0 {
        return None;
    }
    let pad_len = *data.last()? as usize;
    if pad_len == 0 || pad_len > 16 || pad_len > data.len() {
        return None;
    }
    // Verify all padding bytes
    for &b in &data[data.len() - pad_len..] {
        if b as usize != pad_len {
            return None;
        }
    }
    Some(data[..data.len() - pad_len].to_vec())
}

/// AES-256-ECB encrypt (with PKCS#7 padding)
pub fn ecb_encrypt(key: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
    let cipher = Aes256::new(key);
    let padded = pkcs7_pad(plaintext);
    let mut output = padded;
    let mut i = 0;
    while i + 16 <= output.len() {
        let mut block = [0u8; 16];
        block.copy_from_slice(&output[i..i + 16]);
        cipher.encrypt_block(&mut block);
        output[i..i + 16].copy_from_slice(&block);
        i += 16;
    }
    output
}

/// AES-256-ECB decrypt (with PKCS#7 unpadding)
pub fn ecb_decrypt(key: &[u8; 32], ciphertext: &[u8]) -> Option<Vec<u8>> {
    if ciphertext.len() % 16 != 0 {
        return None;
    }
    let cipher = Aes256::new(key);
    let mut output = ciphertext.to_vec();
    let mut i = 0;
    while i + 16 <= output.len() {
        let mut block = [0u8; 16];
        block.copy_from_slice(&output[i..i + 16]);
        cipher.decrypt_block(&mut block);
        output[i..i + 16].copy_from_slice(&block);
        i += 16;
    }
    pkcs7_unpad(&output)
}

/// AES-256-CBC encrypt (with PKCS#7 padding)
pub fn cbc_encrypt(key: &[u8; 32], iv: &[u8; 16], plaintext: &[u8]) -> Vec<u8> {
    let cipher = Aes256::new(key);
    let padded = pkcs7_pad(plaintext);
    let mut output = vec![0u8; padded.len()];
    let mut prev = *iv;

    let mut i = 0;
    while i + 16 <= padded.len() {
        let mut block = [0u8; 16];
        block.copy_from_slice(&padded[i..i + 16]);
        // XOR with previous ciphertext block (or IV for first block)
        for j in 0..16 {
            block[j] ^= prev[j];
        }
        cipher.encrypt_block(&mut block);
        output[i..i + 16].copy_from_slice(&block);
        prev = block;
        i += 16;
    }
    output
}

/// AES-256-CBC decrypt (with PKCS#7 unpadding)
pub fn cbc_decrypt(key: &[u8; 32], iv: &[u8; 16], ciphertext: &[u8]) -> Option<Vec<u8>> {
    if ciphertext.len() % 16 != 0 {
        return None;
    }
    let cipher = Aes256::new(key);
    let mut output = vec![0u8; ciphertext.len()];
    let mut prev = *iv;

    let mut i = 0;
    while i + 16 <= ciphertext.len() {
        let mut block = [0u8; 16];
        block.copy_from_slice(&ciphertext[i..i + 16]);
        let ct_block = block;
        cipher.decrypt_block(&mut block);
        // XOR with previous ciphertext block (or IV)
        for j in 0..16 {
            block[j] ^= prev[j];
        }
        output[i..i + 16].copy_from_slice(&block);
        prev = ct_block;
        i += 16;
    }
    pkcs7_unpad(&output)
}

/// Increment a 128-bit counter (big-endian, last 4 bytes)
#[inline(always)]
fn increment_counter(counter: &mut [u8; 16]) {
    for i in (0..16).rev() {
        counter[i] = counter[i].wrapping_add(1);
        if counter[i] != 0 {
            break;
        }
    }
}

/// AES-256-CTR encrypt/decrypt (symmetric operation)
pub fn ctr_encrypt(key: &[u8; 32], nonce: &[u8; 12], plaintext: &[u8]) -> Vec<u8> {
    let cipher = Aes256::new(key);
    let mut output = vec![0u8; plaintext.len()];

    // Build initial counter block: nonce (12 bytes) || counter (4 bytes, big-endian starting at 1)
    let mut counter_block = [0u8; 16];
    counter_block[..12].copy_from_slice(nonce);
    counter_block[15] = 1; // Start counter at 1

    let mut offset = 0;
    while offset < plaintext.len() {
        let mut keystream = counter_block;
        cipher.encrypt_block(&mut keystream);

        let remaining = plaintext.len() - offset;
        let to_xor = if remaining < 16 { remaining } else { 16 };

        for i in 0..to_xor {
            output[offset + i] = plaintext[offset + i] ^ keystream[i];
        }

        increment_counter(&mut counter_block);
        offset += 16;
    }
    output
}

/// AES-256-CTR decrypt (same as encrypt — CTR mode is symmetric)
pub fn ctr_decrypt(key: &[u8; 32], nonce: &[u8; 12], ciphertext: &[u8]) -> Vec<u8> {
    ctr_encrypt(key, nonce, ciphertext)
}

/// GF(2^128) multiplication for GCM (GHASH)
/// Operands are 128-bit values stored as [u8; 16] in big-endian
fn ghash_mul(x: &[u8; 16], y: &[u8; 16]) -> [u8; 16] {
    let mut z = [0u8; 16];
    let mut v = *y;

    for i in 0..128 {
        let byte_idx = i / 8;
        let bit_idx = 7 - (i % 8);

        if (x[byte_idx] >> bit_idx) & 1 == 1 {
            for j in 0..16 {
                z[j] ^= v[j];
            }
        }

        // Check if LSB of V is set (for reduction)
        let lsb = v[15] & 1;

        // Right-shift V by 1
        for j in (1..16).rev() {
            v[j] = (v[j] >> 1) | (v[j - 1] << 7);
        }
        v[0] >>= 1;

        // If LSB was set, XOR with reduction polynomial R = 0xE1 << 120
        if lsb == 1 {
            v[0] ^= 0xE1;
        }
    }
    z
}

/// GHASH: universal hash function for GCM
fn ghash(h: &[u8; 16], aad: &[u8], ciphertext: &[u8]) -> [u8; 16] {
    let mut tag = [0u8; 16];

    // Process AAD blocks
    let mut offset = 0;
    while offset < aad.len() {
        let mut block = [0u8; 16];
        let remaining = aad.len() - offset;
        let copy_len = if remaining < 16 { remaining } else { 16 };
        block[..copy_len].copy_from_slice(&aad[offset..offset + copy_len]);
        for j in 0..16 {
            tag[j] ^= block[j];
        }
        tag = ghash_mul(&tag, h);
        offset += 16;
    }

    // Process ciphertext blocks
    offset = 0;
    while offset < ciphertext.len() {
        let mut block = [0u8; 16];
        let remaining = ciphertext.len() - offset;
        let copy_len = if remaining < 16 { remaining } else { 16 };
        block[..copy_len].copy_from_slice(&ciphertext[offset..offset + copy_len]);
        for j in 0..16 {
            tag[j] ^= block[j];
        }
        tag = ghash_mul(&tag, h);
        offset += 16;
    }

    // Append lengths block: len(A) || len(C) in bits, big-endian u64
    let mut len_block = [0u8; 16];
    let aad_bits = (aad.len() as u64) * 8;
    let ct_bits = (ciphertext.len() as u64) * 8;
    len_block[..8].copy_from_slice(&aad_bits.to_be_bytes());
    len_block[8..16].copy_from_slice(&ct_bits.to_be_bytes());
    for j in 0..16 {
        tag[j] ^= len_block[j];
    }
    tag = ghash_mul(&tag, h);

    tag
}

/// AES-256-GCM encrypt: returns (ciphertext, 16-byte tag)
pub fn gcm_encrypt(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    plaintext: &[u8],
) -> (Vec<u8>, [u8; 16]) {
    let cipher = Aes256::new(key);

    // Derive H = AES(K, 0^128)
    let mut h = [0u8; 16];
    cipher.encrypt_block(&mut h);

    // J0 = nonce || 0x00000001
    let mut j0 = [0u8; 16];
    j0[..12].copy_from_slice(nonce);
    j0[15] = 1;

    // Encrypt plaintext using CTR mode starting at J0 + 1
    let mut counter = j0;
    increment_counter(&mut counter);

    let mut ciphertext = vec![0u8; plaintext.len()];
    let mut offset = 0;
    while offset < plaintext.len() {
        let mut keystream = counter;
        cipher.encrypt_block(&mut keystream);

        let remaining = plaintext.len() - offset;
        let to_xor = if remaining < 16 { remaining } else { 16 };
        for i in 0..to_xor {
            ciphertext[offset + i] = plaintext[offset + i] ^ keystream[i];
        }

        increment_counter(&mut counter);
        offset += 16;
    }

    // Compute GHASH tag
    let mut tag = ghash(&h, aad, &ciphertext);

    // Encrypt tag with J0 (counter = initial value)
    let mut j0_keystream = j0;
    cipher.encrypt_block(&mut j0_keystream);
    for i in 0..16 {
        tag[i] ^= j0_keystream[i];
    }

    (ciphertext, tag)
}

/// AES-256-GCM decrypt: returns plaintext or None if tag verification fails
pub fn gcm_decrypt(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    ciphertext: &[u8],
    tag: &[u8; 16],
) -> Option<Vec<u8>> {
    let cipher = Aes256::new(key);

    // Derive H = AES(K, 0^128)
    let mut h = [0u8; 16];
    cipher.encrypt_block(&mut h);

    // J0 = nonce || 0x00000001
    let mut j0 = [0u8; 16];
    j0[..12].copy_from_slice(nonce);
    j0[15] = 1;

    // Verify GHASH tag
    let mut expected_tag = ghash(&h, aad, ciphertext);
    let mut j0_keystream = j0;
    cipher.encrypt_block(&mut j0_keystream);
    for i in 0..16 {
        expected_tag[i] ^= j0_keystream[i];
    }

    // Constant-time tag comparison
    let mut diff: u8 = 0;
    for i in 0..16 {
        diff |= expected_tag[i] ^ tag[i];
    }
    if diff != 0 {
        return None;
    }

    // Decrypt ciphertext using CTR mode
    let mut counter = j0;
    increment_counter(&mut counter);

    let mut plaintext = vec![0u8; ciphertext.len()];
    let mut offset = 0;
    while offset < ciphertext.len() {
        let mut keystream = counter;
        cipher.encrypt_block(&mut keystream);

        let remaining = ciphertext.len() - offset;
        let to_xor = if remaining < 16 { remaining } else { 16 };
        for i in 0..to_xor {
            plaintext[offset + i] = ciphertext[offset + i] ^ keystream[i];
        }

        increment_counter(&mut counter);
        offset += 16;
    }

    Some(plaintext)
}

pub fn init() {
    serial_println!("    [aes] AES-256 (ECB/CBC/CTR/GCM) ready");
}
