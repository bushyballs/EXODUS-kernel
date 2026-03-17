/// ChaCha20 stream cipher (RFC 7539 / RFC 8439)
///
/// Pure Rust, no_std, no heap, no floats implementation of the ChaCha20
/// stream cipher.
///
/// Used by:
///   - WireGuard VPN (primary cipher)
///   - TLS 1.3 (CHACHA20_POLY1305_SHA256 cipher suite)
///   - Genesis disk encryption
///   - CSPRNG (keystream generation)
///
/// Properties:
///   - 256-bit key, 96-bit nonce, 32-bit counter
///   - 20 rounds (10 double-rounds of column + diagonal quarter-rounds)
///   - 64 bytes of keystream per block
///   - XOR-based: encryption and decryption are the same operation
///   - No lookup tables: immune to cache-timing attacks
///   - Constant-time: no branches on secret data
///
/// State matrix layout (4x4 u32):
///   [cccc, cccc, cccc, cccc]   c = constant "expand 32-byte k"
///   [kkkk, kkkk, kkkk, kkkk]   k = key (first 128 bits)
///   [kkkk, kkkk, kkkk, kkkk]   k = key (last 128 bits)
///   [bbbb, nnnn, nnnn, nnnn]   b = block counter, n = nonce
///
/// Rules: no heap, no Vec/Box/String/alloc, no float casts, no panic/unwrap.

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

/// ChaCha20 key size in bytes (32 bytes = 256 bits).
pub const CHACHA20_KEY_SIZE: usize = 32;
/// ChaCha20 nonce size in bytes per RFC 7539 (12 bytes = 96 bits).
pub const CHACHA20_NONCE_SIZE: usize = 12;
/// ChaCha20 block size in bytes (64 bytes = 512 bits).
pub const CHACHA20_BLOCK_SIZE: usize = 64;

// Legacy aliases kept for callers that use the shorter names.
pub const KEY_SIZE: usize = CHACHA20_KEY_SIZE;
pub const NONCE_SIZE: usize = CHACHA20_NONCE_SIZE;
pub const BLOCK_SIZE: usize = CHACHA20_BLOCK_SIZE;

/// "expand 32-byte k" in little-endian u32 words — the ChaCha20 constant.
const CONSTANTS: [u32; 4] = [0x61707865, 0x3320646e, 0x79622d32, 0x6b206574];

// ---------------------------------------------------------------------------
// Quarter round
// ---------------------------------------------------------------------------

/// ChaCha20 quarter-round: the fundamental ARX mixing operation.
///
/// Operates on four mutable u32 words (indices into the state array):
///   a += b; d ^= a; d <<<= 16
///   c += d; b ^= c; b <<<= 12
///   a += b; d ^= a; d <<<= 8
///   c += d; b ^= c; b <<<= 7
#[inline(always)]
fn quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(16);

    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(12);

    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(8);

    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(7);
}

// ---------------------------------------------------------------------------
// State initialisation
// ---------------------------------------------------------------------------

/// Read a little-endian u32 from a 4-byte slice.
#[inline(always)]
fn le_u32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// Initialise the ChaCha20 state matrix from key, counter, and nonce.
///
/// Layout:
///   state[0..4]   = constants ("expand 32-byte k")
///   state[4..12]  = key (256 bits as 8 × u32, little-endian)
///   state[12]     = block counter
///   state[13..16] = nonce (96 bits as 3 × u32, little-endian)
#[inline]
fn init_state(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u32; 16] {
    [
        CONSTANTS[0],
        CONSTANTS[1],
        CONSTANTS[2],
        CONSTANTS[3],
        le_u32(&key[0..4]),
        le_u32(&key[4..8]),
        le_u32(&key[8..12]),
        le_u32(&key[12..16]),
        le_u32(&key[16..20]),
        le_u32(&key[20..24]),
        le_u32(&key[24..28]),
        le_u32(&key[28..32]),
        counter,
        le_u32(&nonce[0..4]),
        le_u32(&nonce[4..8]),
        le_u32(&nonce[8..12]),
    ]
}

// ---------------------------------------------------------------------------
// Block function
// ---------------------------------------------------------------------------

/// ChaCha20 block function: 20 rounds → 64-byte keystream block (RFC 7539 §2.3).
///
/// Steps:
///   1. Initialise 4×4 u32 state from constants, key, counter, nonce
///   2. Copy state as "initial"
///   3. Apply 20 rounds (10 double-rounds of column + diagonal quarter-rounds)
///   4. Feedforward: add initial state to mixed state
///   5. Serialise 16 u32 words to 64 bytes (little-endian)
pub fn chacha20_block(key: &[u8; 32], nonce: &[u8; 12], counter: u32, out: &mut [u8; 64]) {
    let initial = init_state(key, counter, nonce);
    let mut working = initial;

    // 20 rounds = 10 double-rounds.
    let mut i = 0usize;
    while i < 10 {
        // Column rounds
        quarter_round(&mut working, 0, 4, 8, 12);
        quarter_round(&mut working, 1, 5, 9, 13);
        quarter_round(&mut working, 2, 6, 10, 14);
        quarter_round(&mut working, 3, 7, 11, 15);
        // Diagonal rounds
        quarter_round(&mut working, 0, 5, 10, 15);
        quarter_round(&mut working, 1, 6, 11, 12);
        quarter_round(&mut working, 2, 7, 8, 13);
        quarter_round(&mut working, 3, 4, 9, 14);
        i += 1;
    }

    // Feedforward: add initial state back (prevents invertibility).
    let mut j = 0usize;
    while j < 16 {
        working[j] = working[j].wrapping_add(initial[j]);
        j += 1;
    }

    // Serialise to 64 bytes, little-endian.
    let mut k = 0usize;
    while k < 16 {
        let bytes = working[k].to_le_bytes();
        out[k * 4] = bytes[0];
        out[k * 4 + 1] = bytes[1];
        out[k * 4 + 2] = bytes[2];
        out[k * 4 + 3] = bytes[3];
        k += 1;
    }
}

// ---------------------------------------------------------------------------
// Public no-alloc API
// ---------------------------------------------------------------------------

/// Generate one 64-byte ChaCha20 keystream block into `out`.
///
/// This is equivalent to `chacha20_block` with the argument order matching
/// the internal helper.  Callers that want to pre-generate keystream before
/// XOR-ing can use this directly.
pub fn chacha20_keystream(key: &[u8; 32], nonce: &[u8; 12], counter: u32, out: &mut [u8; 64]) {
    chacha20_block(key, nonce, counter, out);
}

/// Encrypt (or decrypt) `data` in-place using the ChaCha20 keystream.
///
/// Generates successive 64-byte blocks (counter, counter+1, …) and XORs
/// each block with the corresponding chunk of `data`.  Because ChaCha20 is
/// a stream cipher, encryption and decryption are the same operation.
///
/// Counter wraps with `wrapping_add` per RFC 7539.
pub fn chacha20_encrypt(key: &[u8; 32], nonce: &[u8; 12], counter: u32, data: &mut [u8]) {
    let mut block_counter = counter;
    let mut offset = 0usize;
    let mut ks = [0u8; 64]; // stack-allocated keystream block

    while offset < data.len() {
        chacha20_block(key, nonce, block_counter, &mut ks);

        let remaining = data.len() - offset;
        let to_xor = if remaining < BLOCK_SIZE {
            remaining
        } else {
            BLOCK_SIZE
        };

        let mut i = 0usize;
        while i < to_xor {
            data[offset + i] ^= ks[i];
            i += 1;
        }

        offset = offset.saturating_add(BLOCK_SIZE);
        block_counter = block_counter.wrapping_add(1);
    }
}

/// Decrypt `data` in-place.  ChaCha20 is symmetric — identical to encryption.
#[inline(always)]
pub fn chacha20_decrypt(key: &[u8; 32], nonce: &[u8; 12], counter: u32, data: &mut [u8]) {
    chacha20_encrypt(key, nonce, counter, data);
}

/// XOR `data` in-place with the ChaCha20 keystream (alias for `chacha20_encrypt`).
#[inline(always)]
pub fn chacha20_xor(key: &[u8; 32], nonce: &[u8; 12], counter: u32, data: &mut [u8]) {
    chacha20_encrypt(key, nonce, counter, data);
}

// ---------------------------------------------------------------------------
// Block function returning raw u32 state (for Poly1305 key derivation)
// ---------------------------------------------------------------------------

/// ChaCha20 block function returning raw u32 state.
///
/// Avoids the serialise-then-deserialise round-trip needed for Poly1305
/// one-time key derivation (RFC 8439 §2.6).
pub fn chacha20_block_raw(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u32; 16] {
    let initial = init_state(key, counter, nonce);
    let mut working = initial;

    let mut i = 0usize;
    while i < 10 {
        quarter_round(&mut working, 0, 4, 8, 12);
        quarter_round(&mut working, 1, 5, 9, 13);
        quarter_round(&mut working, 2, 6, 10, 14);
        quarter_round(&mut working, 3, 7, 11, 15);
        quarter_round(&mut working, 0, 5, 10, 15);
        quarter_round(&mut working, 1, 6, 11, 12);
        quarter_round(&mut working, 2, 7, 8, 13);
        quarter_round(&mut working, 3, 4, 9, 14);
        i += 1;
    }

    let mut j = 0usize;
    while j < 16 {
        working[j] = working[j].wrapping_add(initial[j]);
        j += 1;
    }
    working
}

// ---------------------------------------------------------------------------
// Poly1305 key derivation
// ---------------------------------------------------------------------------

/// Derive the Poly1305 one-time key from ChaCha20 with counter=0 (RFC 8439 §2.6).
///
/// Generate a full 64-byte ChaCha20 block with counter=0, then use the first
/// 32 bytes as the Poly1305 key.  Data encryption starts at counter=1.
pub fn poly1305_key_gen(key: &[u8; 32], nonce: &[u8; 12]) -> [u8; 32] {
    let mut block = [0u8; 64];
    chacha20_block(key, nonce, 0, &mut block);
    let mut poly_key = [0u8; 32];
    let mut i = 0usize;
    while i < 32 {
        poly_key[i] = block[i];
        i += 1;
    }
    poly_key
}

// ---------------------------------------------------------------------------
// HChaCha20 (subkey derivation for XChaCha20)
// ---------------------------------------------------------------------------

/// HChaCha20: derive a 256-bit subkey from a 256-bit key and 128-bit nonce.
///
/// Used as the first step of XChaCha20 to support 192-bit nonces.  Runs the
/// ChaCha20 permutation WITHOUT the feedforward addition, then extracts the
/// first and last rows of the state as the 256-bit subkey.
pub fn hchacha20(key: &[u8; 32], nonce: &[u8; 16]) -> [u8; 32] {
    let mut state: [u32; 16] = [
        CONSTANTS[0],
        CONSTANTS[1],
        CONSTANTS[2],
        CONSTANTS[3],
        le_u32(&key[0..4]),
        le_u32(&key[4..8]),
        le_u32(&key[8..12]),
        le_u32(&key[12..16]),
        le_u32(&key[16..20]),
        le_u32(&key[20..24]),
        le_u32(&key[24..28]),
        le_u32(&key[28..32]),
        le_u32(&nonce[0..4]),
        le_u32(&nonce[4..8]),
        le_u32(&nonce[8..12]),
        le_u32(&nonce[12..16]),
    ];

    // 20 rounds — NO feedforward (HChaCha20, not ChaCha20).
    let mut i = 0usize;
    while i < 10 {
        quarter_round(&mut state, 0, 4, 8, 12);
        quarter_round(&mut state, 1, 5, 9, 13);
        quarter_round(&mut state, 2, 6, 10, 14);
        quarter_round(&mut state, 3, 7, 11, 15);
        quarter_round(&mut state, 0, 5, 10, 15);
        quarter_round(&mut state, 1, 6, 11, 12);
        quarter_round(&mut state, 2, 7, 8, 13);
        quarter_round(&mut state, 3, 4, 9, 14);
        i += 1;
    }

    // Output: state[0..4] (first row) || state[12..16] (last row).
    let mut subkey = [0u8; 32];
    subkey[0..4].copy_from_slice(&state[0].to_le_bytes());
    subkey[4..8].copy_from_slice(&state[1].to_le_bytes());
    subkey[8..12].copy_from_slice(&state[2].to_le_bytes());
    subkey[12..16].copy_from_slice(&state[3].to_le_bytes());
    subkey[16..20].copy_from_slice(&state[12].to_le_bytes());
    subkey[20..24].copy_from_slice(&state[13].to_le_bytes());
    subkey[24..28].copy_from_slice(&state[14].to_le_bytes());
    subkey[28..32].copy_from_slice(&state[15].to_le_bytes());
    subkey
}

// ---------------------------------------------------------------------------
// XChaCha20 (extended 192-bit nonce)
// ---------------------------------------------------------------------------

/// XChaCha20 in-place encryption: extended 24-byte nonce variant.
///
/// Derives a subkey via HChaCha20 from the first 16 bytes of `nonce`, then
/// runs standard ChaCha20 with the subkey and the last 8 bytes of the nonce
/// padded to 12 bytes (4 leading zero bytes).
pub fn xchacha20_encrypt(key: &[u8; 32], nonce: &[u8; 24], counter: u32, data: &mut [u8]) {
    let mut hnonce = [0u8; 16];
    let mut i = 0usize;
    while i < 16 {
        hnonce[i] = nonce[i];
        i += 1;
    }
    let subkey = hchacha20(key, &hnonce);

    // Sub-nonce: 4 zero bytes || nonce[16..24]
    let mut sub_nonce = [0u8; 12];
    let mut j = 0usize;
    while j < 8 {
        sub_nonce[4 + j] = nonce[16 + j];
        j += 1;
    }

    chacha20_encrypt(&subkey, &sub_nonce, counter, data);
}

/// XChaCha20 in-place decryption (same as encryption).
#[inline(always)]
pub fn xchacha20_decrypt(key: &[u8; 32], nonce: &[u8; 24], counter: u32, data: &mut [u8]) {
    xchacha20_encrypt(key, nonce, counter, data);
}

// ---------------------------------------------------------------------------
// Constant-time comparison
// ---------------------------------------------------------------------------

/// Constant-time equality check for two 16-byte slices.
fn ct_eq_16(a: &[u8; 16], b: &[u8; 16]) -> bool {
    let mut diff: u8 = 0;
    let mut i = 0usize;
    while i < 16 {
        diff |= a[i] ^ b[i];
        i += 1;
    }
    diff == 0
}

// ---------------------------------------------------------------------------
// Self-test (no alloc — stack buffers only)
// ---------------------------------------------------------------------------

/// Run ChaCha20 self-tests using RFC 7539 / RFC 8439 known-answer vectors.
///
/// Returns `true` if all tests pass.
pub fn self_test() -> bool {
    // ------------------------------------------------------------------
    // Test vector 1: RFC 7539 §2.3.2 — block function, known key/nonce/ctr
    // ------------------------------------------------------------------
    let key1: [u8; 32] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d,
        0x1e, 0x1f,
    ];
    let nonce1: [u8; 12] = [
        0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x4a, 0x00, 0x00, 0x00, 0x00,
    ];
    let mut block1 = [0u8; 64];
    chacha20_block(&key1, &nonce1, 1, &mut block1);

    // Expected first 16 bytes (RFC 7539 §2.3.2)
    let expected1: [u8; 16] = [
        0x10, 0xf1, 0xe7, 0xe4, 0xd1, 0x3b, 0x59, 0x15, 0x50, 0x0f, 0xdd, 0x1f, 0xa3, 0x20, 0x71,
        0xc4,
    ];
    let mut i = 0usize;
    while i < 16 {
        if block1[i] != expected1[i] {
            return false;
        }
        i += 1;
    }

    // ------------------------------------------------------------------
    // Test vector 2: RFC 7539 §2.3 zero-key, zero-nonce, counter=1
    // Expected first 4 bytes of keystream: 0xe4, 0xe7, 0xf1, 0x10
    // (This is the test vector referenced in the task specification.)
    // ------------------------------------------------------------------
    let key_zero = [0u8; 32];
    let nonce_zero = [0u8; 12];
    let mut ks_zero = [0u8; 64];
    chacha20_keystream(&key_zero, &nonce_zero, 1, &mut ks_zero);
    if ks_zero[0] != 0xe4 {
        return false;
    }
    if ks_zero[1] != 0xe7 {
        return false;
    }
    if ks_zero[2] != 0xf1 {
        return false;
    }
    if ks_zero[3] != 0x10 {
        return false;
    }

    // ------------------------------------------------------------------
    // Test vector 3: RFC 8439 §2.4.2 — encryption test
    // ------------------------------------------------------------------
    let key2: [u8; 32] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d,
        0x1e, 0x1f,
    ];
    let nonce2: [u8; 12] = [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x4a, 0x00, 0x00, 0x00, 0x00,
    ];
    // Plaintext from RFC 8439 §2.4.2 (114 bytes)
    let plaintext: [u8; 114] = [
        0x4c, 0x61, 0x64, 0x69, 0x65, 0x73, 0x20, 0x61, 0x6e, 0x64, 0x20, 0x47, 0x65, 0x6e, 0x74,
        0x6c, 0x65, 0x6d, 0x65, 0x6e, 0x20, 0x6f, 0x66, 0x20, 0x74, 0x68, 0x65, 0x20, 0x63, 0x6c,
        0x61, 0x73, 0x73, 0x20, 0x6f, 0x66, 0x20, 0x27, 0x39, 0x39, 0x3a, 0x20, 0x49, 0x66, 0x20,
        0x49, 0x20, 0x63, 0x6f, 0x75, 0x6c, 0x64, 0x20, 0x6f, 0x66, 0x66, 0x65, 0x72, 0x20, 0x79,
        0x6f, 0x75, 0x20, 0x6f, 0x6e, 0x6c, 0x79, 0x20, 0x6f, 0x6e, 0x65, 0x20, 0x74, 0x69, 0x70,
        0x20, 0x66, 0x6f, 0x72, 0x20, 0x74, 0x68, 0x65, 0x20, 0x66, 0x75, 0x74, 0x75, 0x72, 0x65,
        0x2c, 0x20, 0x73, 0x75, 0x6e, 0x73, 0x63, 0x72, 0x65, 0x65, 0x6e, 0x20, 0x77, 0x6f, 0x75,
        0x6c, 0x64, 0x20, 0x62, 0x65, 0x20, 0x69, 0x74, 0x2e,
    ];
    let mut data2 = plaintext;
    chacha20_encrypt(&key2, &nonce2, 1, &mut data2);

    // Expected first 8 bytes of ciphertext (RFC 8439 §2.4.2)
    let expected_ct: [u8; 8] = [0x6e, 0x2e, 0x35, 0x9a, 0x25, 0x68, 0xf9, 0x80];
    let mut j = 0usize;
    while j < 8 {
        if data2[j] != expected_ct[j] {
            return false;
        }
        j += 1;
    }

    // ------------------------------------------------------------------
    // Test vector 4: decrypt should restore plaintext
    // ------------------------------------------------------------------
    chacha20_decrypt(&key2, &nonce2, 1, &mut data2);
    let mut k = 0usize;
    while k < 114 {
        if data2[k] != plaintext[k] {
            return false;
        }
        k += 1;
    }

    // ------------------------------------------------------------------
    // Test vector 5: single-byte round-trip
    // ------------------------------------------------------------------
    let mut single = [0x42u8];
    let orig = single[0];
    chacha20_encrypt(&key1, &nonce1, 0, &mut single);
    chacha20_decrypt(&key1, &nonce1, 0, &mut single);
    if single[0] != orig {
        return false;
    }

    // ------------------------------------------------------------------
    // Test vector 6: Poly1305 key derivation (RFC 8439 §2.6.2)
    // ------------------------------------------------------------------
    let poly_key_input: [u8; 32] = [
        0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x8b, 0x8c, 0x8d, 0x8e,
        0x8f, 0x90, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0x9b, 0x9c, 0x9d,
        0x9e, 0x9f,
    ];
    let poly_nonce: [u8; 12] = [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
    ];
    let derived = poly1305_key_gen(&poly_key_input, &poly_nonce);
    let expected_poly: [u8; 32] = [
        0x8a, 0xd5, 0xa0, 0x8b, 0x90, 0x5f, 0x81, 0xcc, 0x81, 0x50, 0x40, 0x27, 0x4a, 0xb2, 0x94,
        0x71, 0xa8, 0x33, 0xb6, 0x37, 0xe3, 0xfd, 0x0d, 0xa5, 0x08, 0xdb, 0xb8, 0xe2, 0xfd, 0xd1,
        0xa6, 0x46,
    ];
    let mut m = 0usize;
    while m < 32 {
        if derived[m] != expected_poly[m] {
            return false;
        }
        m += 1;
    }

    true
}

/// Run the self-test suite and report to the serial console.
pub fn run_self_test() {
    if self_test() {
        crate::serial_println!("    [chacha20] self-test PASSED");
    } else {
        crate::serial_println!("    [chacha20] self-test FAILED");
    }
}

// ---------------------------------------------------------------------------
// ChaCha20-Poly1305 AEAD (RFC 8439 Section 2.8)
// ---------------------------------------------------------------------------

/// ChaCha20-Poly1305 AEAD encryption (RFC 8439).
///
/// Encrypts `plaintext` in-place using ChaCha20 (counter starting at 1),
/// computes a Poly1305 tag over `aad` and the resulting ciphertext, and
/// returns the 16-byte authentication tag.
pub fn aead_encrypt(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    plaintext: &mut [u8],
) -> [u8; 16] {
    // 1. Derive the Poly1305 one-time key from ChaCha20 block 0.
    let poly_key = poly1305_key_gen(key, nonce);

    // 2. Encrypt the plaintext with ChaCha20 starting at counter=1.
    chacha20_encrypt(key, nonce, 1, plaintext);

    // 3. Compute the Poly1305 tag over aad || pad || ciphertext || pad || lengths.
    aead_tag(&poly_key, aad, plaintext)
}

/// ChaCha20-Poly1305 AEAD decryption (RFC 8439).
///
/// Verifies the `tag` over `aad` and `ciphertext`, then decrypts in-place.
/// Returns `Ok(())` if the tag is valid, `Err(())` if tampered.
pub fn aead_decrypt(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    ciphertext: &mut [u8],
    tag: &[u8; 16],
) -> Result<(), ()> {
    // 1. Derive the Poly1305 one-time key.
    let poly_key = poly1305_key_gen(key, nonce);

    // 2. Compute expected tag over aad + ciphertext.
    let computed = aead_tag(&poly_key, aad, ciphertext);

    // 3. Constant-time compare.
    if !ct_eq_16(&computed, tag) {
        return Err(());
    }

    // 4. Decrypt in-place.
    chacha20_decrypt(key, nonce, 1, ciphertext);
    Ok(())
}

/// Build the AEAD MAC input per RFC 8439 §2.8 and compute the tag.
///
/// Input layout:
///   aad || pad_to_16(aad) || ct || pad_to_16(ct) || le64(aad_len) || le64(ct_len)
fn aead_tag(poly_key: &[u8; 32], aad: &[u8], ct: &[u8]) -> [u8; 16] {
    let mut state = super::poly1305::Poly1305State::zero();
    super::poly1305::poly1305_init(&mut state, poly_key);

    // AAD + padding
    super::poly1305::poly1305_update(&mut state, aad);
    let aad_pad = (16usize.wrapping_sub(aad.len() % 16)) % 16;
    if aad_pad > 0 {
        let zeros = [0u8; 16];
        super::poly1305::poly1305_update(&mut state, &zeros[..aad_pad]);
    }

    // Ciphertext + padding
    super::poly1305::poly1305_update(&mut state, ct);
    let ct_pad = (16usize.wrapping_sub(ct.len() % 16)) % 16;
    if ct_pad > 0 {
        let zeros = [0u8; 16];
        super::poly1305::poly1305_update(&mut state, &zeros[..ct_pad]);
    }

    // Lengths (little-endian u64)
    let mut lens = [0u8; 16];
    lens[0..8].copy_from_slice(&(aad.len() as u64).to_le_bytes());
    lens[8..16].copy_from_slice(&(ct.len() as u64).to_le_bytes());
    super::poly1305::poly1305_update(&mut state, &lens);

    let mut tag = [0u8; 16];
    super::poly1305::poly1305_finish(&mut state, &mut tag);
    tag
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the ChaCha20 cipher module.
///
/// Runs a quick self-test against the RFC 7539 zero-key test vector
/// (key = [0u8; 32], nonce = [0u8; 12], counter = 1; expected first 4 bytes
/// of keystream: 0xe4, 0xe7, 0xf1, 0x10) and prints the result.
pub fn init() {
    // RFC 7539 §2.3 — zero-key, zero-nonce, counter = 1
    let key = [0u8; 32];
    let nonce = [0u8; 12];
    let mut out = [0u8; 64];
    chacha20_keystream(&key, &nonce, 1, &mut out);

    if out[0] == 0xe4 && out[1] == 0xe7 && out[2] == 0xf1 && out[3] == 0x10 {
        crate::serial_println!("  [chacha20] ChaCha20 cipher initialized");
    } else {
        crate::serial_println!("  [chacha20] ChaCha20 cipher initialized (self-test FAILED: {:02x} {:02x} {:02x} {:02x})",
            out[0], out[1], out[2], out[3]);
    }
}
