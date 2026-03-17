/// AES-128-CBC (Cipher Block Chaining mode)
///
/// Pure Rust, no-heap implementation of AES-128 in CBC mode.
/// Used for: secure file encryption, disk sectors, backwards compatibility.
///
/// Key size: 128 bits (16 bytes)
/// Block size: 128 bits (16 bytes)
/// Padding: PKCS#7 (per RFC 5652)
///
/// Rules: no heap, no Vec/Box/String, no float casts, no panic.
use crate::serial_println;

/// AES-128 S-Box (SubBytes substitution table)
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
    let mut i = 0;
    while i < 8 {
        if b & 1 != 0 {
            result ^= a;
        }
        a = xtime(a);
        b >>= 1;
        i += 1;
    }
    result
}

/// AES-128 expanded key schedule (11 round keys = 176 bytes for 10 rounds + initial)
pub struct Aes128 {
    round_keys: [[u8; 16]; 11],
}

impl Aes128 {
    /// Create AES-128 cipher from a 16-byte key
    pub fn new(key: &[u8; 16]) -> Self {
        let round_keys = Self::key_expansion(key);
        Aes128 { round_keys }
    }

    /// AES-128 key expansion: 16-byte key -> 11 round keys (10 rounds + initial)
    fn key_expansion(key: &[u8; 16]) -> [[u8; 16]; 11] {
        let mut w = [0u8; 176]; // 44 words * 4 bytes
                                // Copy original key
        w[..16].copy_from_slice(key);

        let nk = 4; // key words for AES-128
        let nb = 4; // block words
        let nr = 10; // rounds for AES-128

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
            }

            let mut j = 0;
            while j < 4 {
                w[4 * i + j] = w[4 * (i - nk) + j] ^ temp[j];
                j += 1;
            }
            i += 1;
        }

        let mut round_keys = [[0u8; 16]; 11];
        let mut r = 0;
        while r < 11 {
            round_keys[r].copy_from_slice(&w[r * 16..(r + 1) * 16]);
            r += 1;
        }
        round_keys
    }

    /// SubBytes: substitute each byte using S-Box
    #[inline(always)]
    fn sub_bytes(state: &mut [u8; 16]) {
        let mut i = 0;
        while i < 16 {
            state[i] = SBOX[state[i] as usize];
            i += 1;
        }
    }

    /// InvSubBytes: inverse substitution
    #[inline(always)]
    fn inv_sub_bytes(state: &mut [u8; 16]) {
        let mut i = 0;
        while i < 16 {
            state[i] = INV_SBOX[state[i] as usize];
            i += 1;
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
        let mut col = 0;
        while col < 4 {
            let i = col * 4;
            let s0 = state[i];
            let s1 = state[i + 1];
            let s2 = state[i + 2];
            let s3 = state[i + 3];

            state[i] = gf_mul(2, s0) ^ gf_mul(3, s1) ^ s2 ^ s3;
            state[i + 1] = s0 ^ gf_mul(2, s1) ^ gf_mul(3, s2) ^ s3;
            state[i + 2] = s0 ^ s1 ^ gf_mul(2, s2) ^ gf_mul(3, s3);
            state[i + 3] = gf_mul(3, s0) ^ s1 ^ s2 ^ gf_mul(2, s3);
            col += 1;
        }
    }

    /// InvMixColumns: inverse column mixing
    #[inline(always)]
    fn inv_mix_columns(state: &mut [u8; 16]) {
        let mut col = 0;
        while col < 4 {
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
            col += 1;
        }
    }

    /// AddRoundKey: XOR state with round key
    #[inline(always)]
    fn add_round_key(state: &mut [u8; 16], round_key: &[u8; 16]) {
        let mut i = 0;
        while i < 16 {
            state[i] ^= round_key[i];
            i += 1;
        }
    }

    /// Encrypt a single 16-byte block
    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        Self::add_round_key(block, &self.round_keys[0]);

        let mut round = 1;
        while round < 10 {
            Self::sub_bytes(block);
            Self::shift_rows(block);
            Self::mix_columns(block);
            Self::add_round_key(block, &self.round_keys[round]);
            round += 1;
        }

        // Final round (no MixColumns)
        Self::sub_bytes(block);
        Self::shift_rows(block);
        Self::add_round_key(block, &self.round_keys[10]);
    }

    /// Decrypt a single 16-byte block
    pub fn decrypt_block(&self, block: &mut [u8; 16]) {
        Self::add_round_key(block, &self.round_keys[10]);

        let mut round = 9;
        loop {
            Self::inv_shift_rows(block);
            Self::inv_sub_bytes(block);
            Self::add_round_key(block, &self.round_keys[round + 1]);
            Self::inv_mix_columns(block);
            if round == 0 {
                break;
            }
            round -= 1;
        }

        // Final round (no InvMixColumns)
        Self::inv_shift_rows(block);
        Self::inv_sub_bytes(block);
        Self::add_round_key(block, &self.round_keys[0]);
    }
}

pub const AES_CBC_BLOCK_SIZE: usize = 16;
pub const AES_CBC_MAX_PLAINTEXT: usize = 4096;

/// Result type for AES-CBC operations
#[derive(Copy, Clone, PartialEq)]
pub enum AesCbcResult {
    Ok,
    InvalidLength,
    InvalidPadding,
}

/// AES-128-CBC encrypt with PKCS#7 padding
///
/// # Arguments
/// - `key`: 16-byte AES key
/// - `iv`: 16-byte initialization vector
/// - `plaintext`: data to encrypt
/// - `ciphertext`: output buffer (must be 4096 bytes)
///
/// # Returns
/// Length of ciphertext, or 0 if plaintext > 4080 bytes
pub fn aes128_cbc_encrypt(
    key: &[u8; 16],
    iv: &[u8; 16],
    plaintext: &[u8],
    ciphertext: &mut [u8; 4096],
) -> usize {
    // Check length: plaintext + padding <= 4096
    if plaintext.len() > AES_CBC_MAX_PLAINTEXT - 16 {
        return 0;
    }

    let cipher = Aes128::new(key);

    // Calculate padded length (PKCS#7)
    let pad_len = 16 - (plaintext.len() % 16);
    let padded_len = plaintext.len() + pad_len;

    // Build padded plaintext in-memory (bounded by max)
    let mut padded = [0u8; AES_CBC_MAX_PLAINTEXT];
    padded[..plaintext.len()].copy_from_slice(plaintext);
    let mut i = plaintext.len();
    while i < padded_len {
        padded[i] = pad_len as u8;
        i += 1;
    }

    // CBC encrypt
    let mut prev = *iv;
    let mut offset = 0;
    while offset + 16 <= padded_len {
        let mut block = [0u8; 16];
        block.copy_from_slice(&padded[offset..offset + 16]);

        // XOR with previous ciphertext block (or IV for first)
        let mut j = 0;
        while j < 16 {
            block[j] ^= prev[j];
            j += 1;
        }

        cipher.encrypt_block(&mut block);
        ciphertext[offset..offset + 16].copy_from_slice(&block);
        prev = block;
        offset += 16;
    }

    padded_len
}

/// AES-128-CBC decrypt with PKCS#7 unpadding
///
/// # Arguments
/// - `key`: 16-byte AES key
/// - `iv`: 16-byte initialization vector
/// - `ciphertext`: encrypted data (must be multiple of 16 bytes)
/// - `plaintext`: output buffer (must be 4096 bytes)
///
/// # Returns
/// (length of plaintext, result status)
pub fn aes128_cbc_decrypt(
    key: &[u8; 16],
    iv: &[u8; 16],
    ciphertext: &[u8],
    plaintext: &mut [u8; 4096],
) -> (usize, AesCbcResult) {
    // Validate ciphertext length
    if ciphertext.len() == 0
        || ciphertext.len() % 16 != 0
        || ciphertext.len() > AES_CBC_MAX_PLAINTEXT
    {
        return (0, AesCbcResult::InvalidLength);
    }

    let cipher = Aes128::new(key);
    let mut prev = *iv;

    // CBC decrypt
    let mut offset = 0;
    while offset + 16 <= ciphertext.len() {
        let mut block = [0u8; 16];
        block.copy_from_slice(&ciphertext[offset..offset + 16]);
        let ct_block = block;

        cipher.decrypt_block(&mut block);

        // XOR with previous ciphertext block (or IV)
        let mut j = 0;
        while j < 16 {
            block[j] ^= prev[j];
            j += 1;
        }

        plaintext[offset..offset + 16].copy_from_slice(&block);
        prev = ct_block;
        offset += 16;
    }

    // Verify and strip PKCS#7 padding
    let pad_len = plaintext[ciphertext.len() - 1] as usize;
    if pad_len == 0 || pad_len > 16 || pad_len > ciphertext.len() {
        return (0, AesCbcResult::InvalidPadding);
    }

    // Constant-time padding verification
    let mut valid: u8 = 1;
    let mut i = 0;
    while i < pad_len {
        if plaintext[ciphertext.len() - 1 - i] as usize != pad_len {
            valid = 0;
        }
        i += 1;
    }

    if valid == 0 {
        return (0, AesCbcResult::InvalidPadding);
    }

    let plaintext_len = ciphertext.len() - pad_len;
    (plaintext_len, AesCbcResult::Ok)
}

/// Constant-time comparison of two 16-byte blocks
fn ct_eq_16(a: &[u8; 16], b: &[u8; 16]) -> bool {
    let mut diff: u8 = 0;
    let mut i = 0;
    while i < 16 {
        diff |= a[i] ^ b[i];
        i += 1;
    }
    diff == 0
}

/// Self-test: encrypt then decrypt a known plaintext
pub fn init() {
    // Test vector: "Hello, World!!!" (16 bytes)
    let plaintext = b"Hello, World!!!";
    let key = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f,
    ];
    let iv = [
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e,
        0x1f,
    ];

    let mut ciphertext = [0u8; 4096];
    let ct_len = aes128_cbc_encrypt(&key, &iv, plaintext, &mut ciphertext);

    let mut decrypted = [0u8; 4096];
    let (pt_len, result) = aes128_cbc_decrypt(&key, &iv, &ciphertext[..ct_len], &mut decrypted);

    // Verify round-trip
    let mut success = false;
    if result == AesCbcResult::Ok && pt_len == plaintext.len() {
        let mut match_all = true;
        let mut i = 0;
        while i < plaintext.len() {
            if decrypted[i] != plaintext[i] {
                match_all = false;
            }
            i += 1;
        }
        if match_all {
            success = true;
        }
    }

    if success {
        serial_println!("    [aes_cbc] AES-128-CBC initialized");
    } else {
        serial_println!("    [aes_cbc] Self-test FAILED");
    }
}
