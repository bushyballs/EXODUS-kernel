use super::aes::Aes256;
/// AES-GCM authenticated encryption mode (NIST SP 800-38D)
///
/// Standalone AES-256-GCM AEAD implementation with a cleaner API than
/// the raw functions in aes.rs. Wraps AES-256 block cipher with GHASH
/// for authenticated encryption.
///
/// Properties:
///   - 256-bit key, 96-bit nonce
///   - Authenticated encryption with associated data (AEAD)
///   - 128-bit authentication tag
///   - Based on Galois field multiplication in GF(2^128)
///   - Constant-time tag verification
///
/// Part of the AIOS crypto layer.
use alloc::vec::Vec;

/// AES block size in bytes
const AES_BLOCK: usize = 16;

/// GCM tag size in bytes
const TAG_SIZE: usize = 16;

/// GF(2^128) multiplication for GCM (GHASH).
///
/// Multiplies two 128-bit elements in GF(2^128) with the reduction
/// polynomial x^128 + x^7 + x^2 + x + 1 (represented as 0xE1000...).
///
/// Algorithm: standard shift-and-XOR multiplication.
/// Both operands are stored as big-endian [u8; 16].
///
/// NIST SP 800-38D, Section 6.3
fn gf128_mul(x: &[u8; 16], y: &[u8; 16]) -> [u8; 16] {
    let mut z = [0u8; 16];
    let mut v = *y;

    for i in 0..128 {
        let byte_idx = i / 8;
        let bit_idx = 7 - (i % 8);

        // If the i-th bit of X is set, Z ^= V
        if (x[byte_idx] >> bit_idx) & 1 == 1 {
            for j in 0..16 {
                z[j] ^= v[j];
            }
        }

        // Save LSB of V before right-shifting
        let lsb = v[15] & 1;

        // Right-shift V by 1 bit
        for j in (1..16).rev() {
            v[j] = (v[j] >> 1) | (v[j - 1] << 7);
        }
        v[0] >>= 1;

        // If LSB was set, XOR with the reduction polynomial R
        // R = 0xE1 || 0^120 (i.e., R[0] = 0xE1, rest zero)
        if lsb == 1 {
            v[0] ^= 0xE1;
        }
    }
    z
}

/// GHASH: universal hash function for GCM authentication.
///
/// GHASH(H, A, C) processes the AAD and ciphertext through GF(2^128)
/// multiplication, producing a 128-bit authentication tag input.
///
/// NIST SP 800-38D, Section 6.4
fn ghash(h: &[u8; 16], aad: &[u8], ciphertext: &[u8]) -> [u8; 16] {
    let mut tag = [0u8; 16];

    // Process AAD blocks
    let mut offset = 0;
    while offset < aad.len() {
        let mut block = [0u8; 16];
        let remaining = aad.len() - offset;
        let copy_len = remaining.min(16);
        block[..copy_len].copy_from_slice(&aad[offset..offset + copy_len]);

        for j in 0..16 {
            tag[j] ^= block[j];
        }
        tag = gf128_mul(&tag, h);
        offset += 16;
    }

    // Process ciphertext blocks
    offset = 0;
    while offset < ciphertext.len() {
        let mut block = [0u8; 16];
        let remaining = ciphertext.len() - offset;
        let copy_len = remaining.min(16);
        block[..copy_len].copy_from_slice(&ciphertext[offset..offset + copy_len]);

        for j in 0..16 {
            tag[j] ^= block[j];
        }
        tag = gf128_mul(&tag, h);
        offset += 16;
    }

    // Final block: len(A) || len(C) in bits, as big-endian u64
    let mut len_block = [0u8; 16];
    let aad_bits = (aad.len() as u64) * 8;
    let ct_bits = (ciphertext.len() as u64) * 8;
    len_block[..8].copy_from_slice(&aad_bits.to_be_bytes());
    len_block[8..16].copy_from_slice(&ct_bits.to_be_bytes());

    for j in 0..16 {
        tag[j] ^= len_block[j];
    }
    tag = gf128_mul(&tag, h);

    tag
}

/// Increment a 128-bit counter block (big-endian, last 4 bytes only for GCM)
#[inline(always)]
fn inc32(counter: &mut [u8; 16]) {
    // GCM increments only the rightmost 32 bits
    for i in (12..16).rev() {
        counter[i] = counter[i].wrapping_add(1);
        if counter[i] != 0 {
            return;
        }
    }
}

/// Constant-time comparison of two 16-byte values
fn ct_eq_16(a: &[u8; 16], b: &[u8; 16]) -> bool {
    let mut diff: u8 = 0;
    for i in 0..16 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// AES-GCM cipher context
///
/// Pre-computes the hash subkey H = AES(K, 0^128) at construction time
/// to avoid recomputing it for every encrypt/decrypt operation.
pub struct AesGcm {
    /// AES-256 cipher with expanded key schedule
    cipher: Aes256,
    /// GHASH hash subkey H = AES_K(0^128)
    h: [u8; 16],
}

impl AesGcm {
    /// Create a new AES-GCM context from a 128-bit or 256-bit key.
    ///
    /// Accepts 16-byte (AES-128) or 32-byte (AES-256) keys.
    /// For this kernel implementation, 256-bit keys are expected.
    pub fn new(key: &[u8]) -> Self {
        assert!(key.len() == 32, "AES-GCM requires a 256-bit (32-byte) key");

        let mut key_arr = [0u8; 32];
        key_arr.copy_from_slice(key);
        let cipher = Aes256::new(&key_arr);

        // Compute hash subkey H = AES(K, 0^128)
        let mut h = [0u8; 16];
        cipher.encrypt_block(&mut h);

        AesGcm { cipher, h }
    }

    /// Encrypt plaintext with AES-GCM.
    ///
    /// Returns (ciphertext, 16-byte authentication tag).
    ///
    /// The nonce MUST be unique for each encryption with the same key.
    /// Nonce reuse completely breaks GCM security.
    ///
    /// # Arguments
    /// - `nonce`: 96-bit (12-byte) nonce
    /// - `aad`: Additional authenticated data (authenticated but not encrypted)
    /// - `plaintext`: Data to encrypt
    ///
    /// # Returns
    /// Tuple of (ciphertext, 16-byte tag)
    pub fn encrypt(&self, nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> (Vec<u8>, [u8; 16]) {
        // Build initial counter J0 = nonce || 0x00000001
        let mut j0 = [0u8; AES_BLOCK];
        j0[..12].copy_from_slice(nonce);
        j0[15] = 1;

        // Start CTR encryption at J0 + 1
        let mut counter = j0;
        inc32(&mut counter);

        // Encrypt plaintext using CTR mode
        let mut ciphertext = Vec::with_capacity(plaintext.len());
        let mut offset = 0;
        while offset < plaintext.len() {
            let mut keystream = counter;
            self.cipher.encrypt_block(&mut keystream);

            let remaining = plaintext.len() - offset;
            let to_process = remaining.min(AES_BLOCK);
            for i in 0..to_process {
                ciphertext.push(plaintext[offset + i] ^ keystream[i]);
            }

            inc32(&mut counter);
            offset += AES_BLOCK;
        }

        // Compute GHASH over AAD and ciphertext
        let mut tag = ghash(&self.h, aad, &ciphertext);

        // Encrypt the GHASH output with J0 to produce the final tag
        let mut j0_enc = j0;
        self.cipher.encrypt_block(&mut j0_enc);
        for i in 0..TAG_SIZE {
            tag[i] ^= j0_enc[i];
        }

        (ciphertext, tag)
    }

    /// Decrypt ciphertext with AES-GCM.
    ///
    /// Returns `Some(plaintext)` if the tag is valid, `None` if authentication fails.
    ///
    /// IMPORTANT: Tag verification happens BEFORE decryption. If the tag
    /// is invalid, no plaintext is returned, preventing processing of
    /// tampered data.
    ///
    /// # Arguments
    /// - `nonce`: 96-bit (12-byte) nonce (same as used for encryption)
    /// - `aad`: Additional authenticated data (must match encryption)
    /// - `ciphertext`: Data to decrypt
    /// - `tag`: 16-byte authentication tag to verify
    ///
    /// # Returns
    /// `Some(plaintext)` on success, `None` on tag mismatch
    pub fn decrypt(
        &self,
        nonce: &[u8; 12],
        aad: &[u8],
        ciphertext: &[u8],
        tag: &[u8; 16],
    ) -> Option<Vec<u8>> {
        // Build J0
        let mut j0 = [0u8; AES_BLOCK];
        j0[..12].copy_from_slice(nonce);
        j0[15] = 1;

        // Compute expected GHASH tag BEFORE decryption
        let mut expected_tag = ghash(&self.h, aad, ciphertext);
        let mut j0_enc = j0;
        self.cipher.encrypt_block(&mut j0_enc);
        for i in 0..TAG_SIZE {
            expected_tag[i] ^= j0_enc[i];
        }

        // Constant-time tag verification
        if !ct_eq_16(&expected_tag, tag) {
            return None;
        }

        // Tag is valid; decrypt using CTR mode starting at J0 + 1
        let mut counter = j0;
        inc32(&mut counter);

        let mut plaintext = Vec::with_capacity(ciphertext.len());
        let mut offset = 0;
        while offset < ciphertext.len() {
            let mut keystream = counter;
            self.cipher.encrypt_block(&mut keystream);

            let remaining = ciphertext.len() - offset;
            let to_process = remaining.min(AES_BLOCK);
            for i in 0..to_process {
                plaintext.push(ciphertext[offset + i] ^ keystream[i]);
            }

            inc32(&mut counter);
            offset += AES_BLOCK;
        }

        Some(plaintext)
    }

    /// Encrypt in-place and return the tag.
    pub fn encrypt_in_place(&self, nonce: &[u8; 12], aad: &[u8], data: &mut [u8]) -> [u8; 16] {
        let mut j0 = [0u8; AES_BLOCK];
        j0[..12].copy_from_slice(nonce);
        j0[15] = 1;

        let mut counter = j0;
        inc32(&mut counter);

        // Encrypt in-place
        let mut offset = 0;
        while offset < data.len() {
            let mut keystream = counter;
            self.cipher.encrypt_block(&mut keystream);

            let remaining = data.len() - offset;
            let to_process = remaining.min(AES_BLOCK);
            for i in 0..to_process {
                data[offset + i] ^= keystream[i];
            }

            inc32(&mut counter);
            offset += AES_BLOCK;
        }

        // Compute GHASH over AAD and ciphertext (data is now ciphertext)
        let mut tag = ghash(&self.h, aad, data);
        let mut j0_enc = j0;
        self.cipher.encrypt_block(&mut j0_enc);
        for i in 0..TAG_SIZE {
            tag[i] ^= j0_enc[i];
        }

        tag
    }
}

pub fn init() {
    crate::serial_println!("    [gcm] AES-256-GCM authenticated encryption ready");
}
