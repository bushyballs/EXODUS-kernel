use super::aes::Aes256;
/// AES-CTR counter mode encryption
///
/// Standalone AES-256-CTR stream cipher implementation.
/// Uses the AES block cipher from the sibling `aes` module for keystream generation.
///
/// Properties:
///   - Symmetric: encryption and decryption are the same XOR operation
///   - Seekable: can jump to any block offset
///   - Stream cipher: produces arbitrary-length keystream
///   - No padding needed: processes partial final blocks naturally
///
/// Counter format: nonce (variable) || counter (big-endian, fills remainder of 16 bytes)
///
/// Part of the AIOS crypto layer.
use alloc::vec::Vec;

/// AES block size
const AES_BLOCK: usize = 16;

/// AES-CTR cipher context
///
/// Holds the expanded AES round keys and the current counter block.
/// The counter block is nonce || big-endian counter, where the nonce
/// occupies the first bytes and the counter fills the rest.
pub struct AesCtr {
    /// AES-256 cipher with expanded key schedule
    cipher: Aes256,
    /// Current 128-bit counter block: nonce || counter
    counter_block: [u8; AES_BLOCK],
    /// Length of the nonce portion (to know which bytes are counter)
    nonce_len: usize,
    /// Buffered keystream for partial-block processing
    keystream_buf: [u8; AES_BLOCK],
    /// How many bytes of keystream_buf have been consumed
    keystream_used: usize,
}

impl AesCtr {
    /// Create a new AES-CTR cipher.
    ///
    /// Supports 8-byte or 12-byte nonces:
    ///   - 12-byte nonce: 4-byte counter (NIST recommendation, ~64GB limit)
    ///   - 8-byte nonce: 8-byte counter (larger streams)
    ///
    /// The counter starts at 0 and increments per block.
    pub fn new(key: &[u8], nonce: &[u8]) -> Self {
        assert!(key.len() == 32, "AES-CTR requires a 256-bit (32-byte) key");
        assert!(
            nonce.len() == 12 || nonce.len() == 8 || nonce.len() == 16,
            "AES-CTR nonce must be 8, 12, or 16 bytes"
        );

        let mut key_arr = [0u8; 32];
        key_arr.copy_from_slice(key);
        let cipher = Aes256::new(&key_arr);

        let mut counter_block = [0u8; AES_BLOCK];
        let nonce_len = nonce.len().min(AES_BLOCK);
        counter_block[..nonce_len].copy_from_slice(&nonce[..nonce_len]);
        // Remaining bytes are zero (initial counter = 0)

        AesCtr {
            cipher,
            counter_block,
            nonce_len,
            keystream_buf: [0u8; AES_BLOCK],
            keystream_used: AES_BLOCK, // Force generation on first use
        }
    }

    /// Increment the counter portion of the counter block (big-endian).
    ///
    /// Only increments the bytes after the nonce.
    fn increment_counter(&mut self) {
        // Increment the counter bytes in big-endian order
        for i in (self.nonce_len..AES_BLOCK).rev() {
            self.counter_block[i] = self.counter_block[i].wrapping_add(1);
            if self.counter_block[i] != 0 {
                return; // No carry
            }
        }
        // Counter wrapped around (extremely unlikely in practice)
    }

    /// Generate the next block of keystream.
    fn generate_keystream_block(&mut self) {
        self.keystream_buf = self.counter_block;
        self.cipher.encrypt_block(&mut self.keystream_buf);
        self.increment_counter();
        self.keystream_used = 0;
    }

    /// Encrypt or decrypt data (CTR mode is symmetric).
    ///
    /// Processes input by XORing with the AES-CTR keystream.
    /// Can be called multiple times for streaming operation.
    pub fn process(&mut self, data: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity(data.len());
        let mut offset = 0;

        while offset < data.len() {
            // Generate a new keystream block if needed
            if self.keystream_used >= AES_BLOCK {
                self.generate_keystream_block();
            }

            // XOR as many bytes as possible from the current keystream block
            let available = AES_BLOCK - self.keystream_used;
            let remaining = data.len() - offset;
            let to_process = available.min(remaining);

            for i in 0..to_process {
                output.push(data[offset + i] ^ self.keystream_buf[self.keystream_used + i]);
            }

            self.keystream_used += to_process;
            offset += to_process;
        }

        output
    }

    /// Process data in-place (encrypt or decrypt).
    pub fn process_in_place(&mut self, data: &mut [u8]) {
        let mut offset = 0;

        while offset < data.len() {
            if self.keystream_used >= AES_BLOCK {
                self.generate_keystream_block();
            }

            let available = AES_BLOCK - self.keystream_used;
            let remaining = data.len() - offset;
            let to_process = available.min(remaining);

            for i in 0..to_process {
                data[offset + i] ^= self.keystream_buf[self.keystream_used + i];
            }

            self.keystream_used += to_process;
            offset += to_process;
        }
    }

    /// Seek to a specific block offset.
    ///
    /// Resets the counter to the specified block number and clears
    /// any buffered keystream. The next `process()` call will start
    /// generating keystream from this block.
    pub fn seek(&mut self, block_offset: u64) {
        // Reset counter portion to zero
        for i in self.nonce_len..AES_BLOCK {
            self.counter_block[i] = 0;
        }

        // Set counter to block_offset in big-endian
        let counter_len = AES_BLOCK - self.nonce_len;
        let bytes = block_offset.to_be_bytes();
        // Place the counter bytes right-aligned in the counter portion
        if counter_len >= 8 {
            let start = AES_BLOCK - 8;
            self.counter_block[start..AES_BLOCK].copy_from_slice(&bytes);
        } else {
            // For short counter fields, take only the low bytes
            let skip = 8 - counter_len;
            self.counter_block[self.nonce_len..AES_BLOCK].copy_from_slice(&bytes[skip..]);
        }

        // Invalidate buffered keystream
        self.keystream_used = AES_BLOCK;
    }

    /// Generate raw keystream bytes without any input data.
    ///
    /// Useful for key generation or CSPRNG applications.
    pub fn keystream(&mut self, len: usize) -> Vec<u8> {
        let zeros = alloc::vec![0u8; len];
        self.process(&zeros)
    }
}

/// One-shot AES-256-CTR encrypt/decrypt
pub fn aes_ctr_encrypt(key: &[u8; 32], nonce: &[u8], data: &[u8]) -> Vec<u8> {
    let mut ctr = AesCtr::new(key, nonce);
    ctr.process(data)
}

/// One-shot AES-256-CTR decrypt (same as encrypt)
pub fn aes_ctr_decrypt(key: &[u8; 32], nonce: &[u8], data: &[u8]) -> Vec<u8> {
    aes_ctr_encrypt(key, nonce, data)
}

pub fn init() {
    crate::serial_println!("    [ctr] AES-256-CTR stream cipher ready");
}
