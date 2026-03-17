use super::sha256;
/// HMAC-SHA256 — keyed hash message authentication code (RFC 2104)
///
/// Pure Rust implementation of HMAC using SHA-256 as the underlying hash.
///
/// Also provides:
///   - HKDF-SHA256 (RFC 5869): key derivation from shared secrets
///   - PBKDF2-SHA256 (RFC 8018): password-based key derivation
///   - Streaming HMAC: incremental message feeding
///   - Constant-time verification: timing-safe tag comparison
///
/// Used for:
///   - Message authentication in WireGuard
///   - TLS record authentication
///   - Session cookie signing
///   - API key derivation
///   - Password hashing (PBKDF2)
///   - Key expansion from shared secrets (HKDF)
use alloc::vec::Vec;

/// HMAC block size (same as SHA-256 block size)
const BLOCK_SIZE: usize = 64;

/// HMAC output size (same as SHA-256 digest size)
pub const MAC_SIZE: usize = 32;

// --- One-shot HMAC-SHA256 ---

/// Compute HMAC-SHA256(key, message) in one call.
///
/// RFC 2104: HMAC(K, m) = H((K' XOR opad) || H((K' XOR ipad) || m))
///
/// Key handling:
///   - If key.len() > 64: key is hashed with SHA-256 first (becomes 32 bytes)
///   - If key.len() <= 64: key is zero-padded to 64 bytes
///
/// Returns a 32-byte authentication tag.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    // Normalize key to exactly BLOCK_SIZE bytes
    let key_block = normalize_key(key);

    // Inner padding: key XOR 0x36 (repeated)
    let mut ipad = [0x36u8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        ipad[i] ^= key_block[i];
    }

    // Outer padding: key XOR 0x5c (repeated)
    let mut opad = [0x5cu8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        opad[i] ^= key_block[i];
    }

    // Inner hash: SHA-256(ipad || message)
    let mut inner = sha256::Sha256::new();
    inner.update(&ipad);
    inner.update(message);
    let inner_hash = inner.finalize();

    // Outer hash: SHA-256(opad || inner_hash)
    let mut outer = sha256::Sha256::new();
    outer.update(&opad);
    outer.update(&inner_hash);
    outer.finalize()
}

/// Compute HMAC-SHA256 and write the result into an output buffer.
///
/// This is the no-alloc, out-parameter variant of `hmac_sha256()`.
/// Identical algorithm; differs only in that the tag is written into
/// `out` rather than returned by value.
///
/// RFC 2104: HMAC(K, m) = H((K' XOR opad) || H((K' XOR ipad) || m))
///
/// Used by WireGuard (handshake MAC), TLS 1.3 (record MAC), and
/// the HKDF construction in `hkdf.rs`.
///
/// # Arguments
/// - `key`:  authentication key (any length; keys longer than 64 bytes
///           are pre-hashed with SHA-256 per RFC 2104)
/// - `data`: message to authenticate
/// - `out`:  32-byte output buffer for the HMAC tag
pub fn hmac_sha256_into(key: &[u8], data: &[u8], out: &mut [u8; 32]) {
    let result = hmac_sha256(key, data);
    out.copy_from_slice(&result);
}

/// Normalize a key to exactly BLOCK_SIZE bytes.
/// If too long, hash it. If too short, zero-pad.
fn normalize_key(key: &[u8]) -> [u8; BLOCK_SIZE] {
    let mut block = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let h = sha256::hash(key);
        block[..32].copy_from_slice(&h);
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    block
}

// --- Streaming HMAC-SHA256 ---

/// Streaming HMAC-SHA256 hasher.
///
/// Allows incremental feeding of message data via `update()`, then
/// produces the final 32-byte tag via `finalize()`.
///
/// # Usage
/// ```
/// let mut mac = HmacSha256::new(b"secret key");
/// mac.update(b"message part 1");
/// mac.update(b"message part 2");
/// let tag = mac.finalize();
/// ```
pub struct HmacSha256 {
    /// Inner SHA-256 hasher (already fed the ipad-XORed key)
    inner: sha256::Sha256,
    /// Outer key block (key XOR opad), saved for finalization
    opad_key: [u8; BLOCK_SIZE],
}

impl HmacSha256 {
    /// Create a new streaming HMAC-SHA256 with the given key.
    ///
    /// The key is normalized and the ipad block is immediately fed
    /// to the inner hasher, so subsequent `update()` calls only
    /// need to provide message data.
    pub fn new(key: &[u8]) -> Self {
        let key_block = normalize_key(key);

        // Compute ipad and opad key blocks
        let mut ipad = [0x36u8; BLOCK_SIZE];
        let mut opad = [0x5cu8; BLOCK_SIZE];
        for i in 0..BLOCK_SIZE {
            ipad[i] ^= key_block[i];
            opad[i] ^= key_block[i];
        }

        // Initialize inner hasher with ipad
        let mut inner = sha256::Sha256::new();
        inner.update(&ipad);

        HmacSha256 {
            inner,
            opad_key: opad,
        }
    }

    /// Feed message data into the HMAC computation.
    /// Can be called multiple times for streaming use.
    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    /// Finalize and return the 32-byte HMAC tag.
    /// Consumes the hasher.
    pub fn finalize(self) -> [u8; 32] {
        // Complete inner hash
        let inner_hash = self.inner.finalize();

        // Outer hash: SHA-256(opad_key || inner_hash)
        let mut outer = sha256::Sha256::new();
        outer.update(&self.opad_key);
        outer.update(&inner_hash);
        outer.finalize()
    }

    /// Finalize and return tag without consuming the hasher.
    /// Allows continued streaming after getting an intermediate tag.
    pub fn finalize_clone(&self) -> [u8; 32] {
        let inner_hash = self.inner.finalize_clone();

        let mut outer = sha256::Sha256::new();
        outer.update(&self.opad_key);
        outer.update(&inner_hash);
        outer.finalize()
    }
}

// --- HMAC verification ---

/// Verify an HMAC tag in constant time.
///
/// Computes HMAC-SHA256(key, message) and compares with the expected tag.
/// Uses constant-time comparison to prevent timing attacks.
///
/// Returns true if the tag is valid.
pub fn verify(key: &[u8], message: &[u8], expected_tag: &[u8; 32]) -> bool {
    let computed = hmac_sha256(key, message);
    constant_time_eq(&computed, expected_tag)
}

/// Verify an HMAC tag using the streaming API, constant-time.
pub fn verify_streaming(hmac: HmacSha256, expected_tag: &[u8; 32]) -> bool {
    let computed = hmac.finalize();
    constant_time_eq(&computed, expected_tag)
}

/// Constant-time comparison of two 32-byte values.
///
/// Examines every byte regardless of where differences occur,
/// preventing timing side-channels that leak information about
/// which byte positions differ.
fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff: u8 = 0;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// Constant-time comparison of arbitrary-length slices.
/// Returns false immediately if lengths differ (length is not secret).
fn constant_time_eq_slices(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

// --- HKDF-SHA256 (RFC 5869) ---

/// HKDF-Extract: derive a pseudorandom key (PRK) from input keying material.
///
/// RFC 5869, Section 2.2:
///   PRK = HMAC-Hash(salt, IKM)
///
/// If no salt is provided, use a string of HashLen zeros.
/// The salt acts as a "domain separator" and source of additional entropy.
///
/// Input: salt (optional, can be empty), IKM (input keying material, e.g., DH shared secret)
/// Output: PRK (32 bytes, suitable for use with hkdf_expand)
pub fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> [u8; 32] {
    // If salt is empty, use a block of zeros (per RFC 5869)
    if salt.is_empty() {
        let zero_salt = [0u8; 32];
        hmac_sha256(&zero_salt, ikm)
    } else {
        hmac_sha256(salt, ikm)
    }
}

/// HKDF-Expand: expand a PRK to the desired output length.
///
/// RFC 5869, Section 2.3:
///   N = ceil(L / HashLen)
///   T(0) = empty string
///   T(i) = HMAC-Hash(PRK, T(i-1) || info || i)  for i = 1..N
///   OKM = first L bytes of T(1) || T(2) || ... || T(N)
///
/// Input: PRK (from hkdf_extract), info (context/application label), length (desired output bytes)
/// Output: OKM (output keying material, up to 255 * 32 = 8160 bytes)
pub fn hkdf_expand(prk: &[u8; 32], info: &[u8], length: usize) -> Vec<u8> {
    // Maximum output: 255 * HashLen = 255 * 32 = 8160 bytes
    let max_length = 255 * 32;
    let length = length.min(max_length);

    let n = (length + 31) / 32; // number of blocks needed
    let mut output = Vec::with_capacity(length);
    let mut t_prev: Vec<u8> = Vec::new(); // T(0) = empty

    for i in 1..=n {
        // T(i) = HMAC-Hash(PRK, T(i-1) || info || i)
        let mut hmac = HmacSha256::new(prk);
        hmac.update(&t_prev);
        hmac.update(info);
        hmac.update(&[i as u8]);
        let t_i = hmac.finalize();

        t_prev = t_i.to_vec();
        output.extend_from_slice(&t_i);
    }

    output.truncate(length);
    output
}

/// HKDF one-shot: extract-then-expand in a single call.
///
/// Convenience function combining hkdf_extract and hkdf_expand.
pub fn hkdf(salt: &[u8], ikm: &[u8], info: &[u8], length: usize) -> Vec<u8> {
    let prk = hkdf_extract(salt, ikm);
    hkdf_expand(&prk, info, length)
}

// --- PBKDF2-SHA256 (RFC 8018) ---

/// PBKDF2 with HMAC-SHA256 as the PRF.
///
/// RFC 8018, Section 5.2:
///   DK = T1 || T2 || ... || Tdklen/hlen
///   Ti = F(Password, Salt, c, i)
///   F(P, S, c, i) = U1 XOR U2 XOR ... XOR Uc
///   U1 = PRF(P, S || INT(i))
///   U2 = PRF(P, U1)
///   ...
///   Uc = PRF(P, Uc-1)
///
/// Input:
///   - password: the user's password
///   - salt: random salt (at least 16 bytes recommended)
///   - iterations: work factor (at least 100,000 recommended for passwords)
///   - dk_len: desired derived key length in bytes
///
/// Output: derived key of dk_len bytes
pub fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32, dk_len: usize) -> Vec<u8> {
    let num_blocks = (dk_len + 31) / 32; // ceil(dk_len / hLen)
    let mut dk = Vec::with_capacity(dk_len);

    for block_idx in 1..=num_blocks {
        // F(Password, Salt, c, i)
        let block = pbkdf2_f(password, salt, iterations, block_idx as u32);
        dk.extend_from_slice(&block);
    }

    dk.truncate(dk_len);
    dk
}

/// PBKDF2 function F(Password, Salt, c, i) — computes one block of derived key.
///
/// U1 = HMAC(Password, Salt || INT_32_BE(i))
/// U2 = HMAC(Password, U1)
/// ...
/// Uc = HMAC(Password, Uc-1)
/// F = U1 XOR U2 XOR ... XOR Uc
fn pbkdf2_f(password: &[u8], salt: &[u8], iterations: u32, block_index: u32) -> [u8; 32] {
    // U1 = PRF(Password, Salt || INT(i))
    let mut hmac = HmacSha256::new(password);
    hmac.update(salt);
    hmac.update(&block_index.to_be_bytes());
    let mut u_prev = hmac.finalize();
    let mut result = u_prev;

    // U2 through Uc
    for _ in 1..iterations {
        let u_next = hmac_sha256(password, &u_prev);
        // XOR into result
        for j in 0..32 {
            result[j] ^= u_next[j];
        }
        u_prev = u_next;
    }

    result
}

/// Convenience: derive a 32-byte key from a password using PBKDF2-SHA256.
pub fn derive_key(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let dk = pbkdf2_sha256(password, salt, iterations, 32);
    let mut key = [0u8; 32];
    key.copy_from_slice(&dk);
    key
}

// --- Self-test vectors ---

/// Run HMAC/HKDF/PBKDF2 self-tests with known test vectors.
/// Returns true if all tests pass.
pub fn self_test() -> bool {
    // === HMAC-SHA256 tests (RFC 4231) ===

    // Test Case 1: HMAC with 20-byte key of 0x0b
    // Key  = 0x0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b (20 bytes)
    // Data = "Hi There"
    // HMAC = b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7
    let key1 = [0x0bu8; 20];
    let data1 = b"Hi There";
    let hmac1 = hmac_sha256(&key1, data1);
    let expected1: [u8; 32] = [
        0xb0, 0x34, 0x4c, 0x61, 0xd8, 0xdb, 0x38, 0x53, 0x5c, 0xa8, 0xaf, 0xce, 0xaf, 0x0b, 0xf1,
        0x2b, 0x88, 0x1d, 0xc2, 0x00, 0xc9, 0x83, 0x3d, 0xa7, 0x26, 0xe9, 0x37, 0x6c, 0x2e, 0x32,
        0xcf, 0xf7,
    ];
    if !constant_time_eq(&hmac1, &expected1) {
        return false;
    }

    // Test Case 2: HMAC with key "Jefe"
    // Key  = "Jefe"
    // Data = "what do ya want for nothing?"
    // HMAC = 5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843
    let hmac2 = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
    let expected2: [u8; 32] = [
        0x5b, 0xdc, 0xc1, 0x46, 0xbf, 0x60, 0x75, 0x4e, 0x6a, 0x04, 0x24, 0x26, 0x08, 0x95, 0x75,
        0xc7, 0x5a, 0x00, 0x3f, 0x08, 0x9d, 0x27, 0x39, 0x83, 0x9d, 0xec, 0x58, 0xb9, 0x64, 0xec,
        0x38, 0x43,
    ];
    if !constant_time_eq(&hmac2, &expected2) {
        return false;
    }

    // Test Case 3: HMAC with 20-byte key of 0xaa, 50-byte data of 0xdd
    // Key  = 0xaaaa...aa (20 bytes)
    // Data = 0xdddd...dd (50 bytes)
    // HMAC = 773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe
    let key3 = [0xaau8; 20];
    let data3 = [0xddu8; 50];
    let hmac3 = hmac_sha256(&key3, &data3);
    let expected3: [u8; 32] = [
        0x77, 0x3e, 0xa9, 0x1e, 0x36, 0x80, 0x0e, 0x46, 0x85, 0x4d, 0xb8, 0xeb, 0xd0, 0x91, 0x81,
        0xa7, 0x29, 0x59, 0x09, 0x8b, 0x3e, 0xf8, 0xc1, 0x22, 0xd9, 0x63, 0x55, 0x14, 0xce, 0xd5,
        0x65, 0xfe,
    ];
    if !constant_time_eq(&hmac3, &expected3) {
        return false;
    }

    // Test Case 4: HMAC with long key (131 bytes of 0xaa)
    // RFC 4231 Test Case 6: key longer than block size
    // Key  = 0xaaaa...aa (131 bytes) — will be hashed to 32 bytes
    // Data = "Test Using Larger Than Block-Size Key - Hash Key First"
    // HMAC = 60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54
    let key4 = [0xaau8; 131];
    let data4 = b"Test Using Larger Than Block-Size Key - Hash Key First";
    let hmac4 = hmac_sha256(&key4, data4);
    let expected4: [u8; 32] = [
        0x60, 0xe4, 0x31, 0x59, 0x1e, 0xe0, 0xb6, 0x7f, 0x0d, 0x8a, 0x26, 0xaa, 0xcb, 0xf5, 0xb7,
        0x7f, 0x8e, 0x0b, 0xc6, 0x21, 0x37, 0x28, 0xc5, 0x14, 0x05, 0x46, 0x04, 0x0f, 0x0e, 0xe3,
        0x7f, 0x54,
    ];
    if !constant_time_eq(&hmac4, &expected4) {
        return false;
    }

    // === Streaming HMAC test ===
    // Verify streaming produces same result as one-shot
    let mut streaming = HmacSha256::new(b"Jefe");
    streaming.update(b"what do ya want ");
    streaming.update(b"for nothing?");
    let streaming_result = streaming.finalize();
    if !constant_time_eq(&streaming_result, &expected2) {
        return false;
    }

    // === HMAC verification test ===
    if !verify(b"Jefe", b"what do ya want for nothing?", &expected2) {
        return false;
    }
    // Verify that a wrong tag fails
    let mut wrong_tag = expected2;
    wrong_tag[0] ^= 0xFF;
    if verify(b"Jefe", b"what do ya want for nothing?", &wrong_tag) {
        return false; // Should have been rejected
    }

    // === HKDF-SHA256 tests (RFC 5869) ===

    // RFC 5869 Test Case 1
    // IKM  = 0x0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b (22 bytes)
    // salt = 0x000102030405060708090a0b0c (13 bytes)
    // info = 0xf0f1f2f3f4f5f6f7f8f9 (10 bytes)
    // L    = 42
    // PRK  = 077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5
    // OKM  = 3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865
    let ikm1 = [0x0bu8; 22];
    let salt1: [u8; 13] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c,
    ];
    let info1: [u8; 10] = [0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9];

    let prk1 = hkdf_extract(&salt1, &ikm1);
    let expected_prk1: [u8; 32] = [
        0x07, 0x77, 0x09, 0x36, 0x2c, 0x2e, 0x32, 0xdf, 0x0d, 0xdc, 0x3f, 0x0d, 0xc4, 0x7b, 0xba,
        0x63, 0x90, 0xb6, 0xc7, 0x3b, 0xb5, 0x0f, 0x9c, 0x31, 0x22, 0xec, 0x84, 0x4a, 0xd7, 0xc2,
        0xb3, 0xe5,
    ];
    if !constant_time_eq(&prk1, &expected_prk1) {
        return false;
    }

    let okm1 = hkdf_expand(&prk1, &info1, 42);
    let expected_okm1: [u8; 42] = [
        0x3c, 0xb2, 0x5f, 0x25, 0xfa, 0xac, 0xd5, 0x7a, 0x90, 0x43, 0x4f, 0x64, 0xd0, 0x36, 0x2f,
        0x2a, 0x2d, 0x2d, 0x0a, 0x90, 0xcf, 0x1a, 0x5a, 0x4c, 0x5d, 0xb0, 0x2d, 0x56, 0xec, 0xc4,
        0xc5, 0xbf, 0x34, 0x00, 0x72, 0x08, 0xd5, 0xb8, 0x87, 0x18, 0x58, 0x65,
    ];
    if !constant_time_eq_slices(&okm1, &expected_okm1) {
        return false;
    }

    // === PBKDF2-SHA256 test (RFC 6070) ===
    // Password = "password", Salt = "salt", c = 1, dkLen = 32
    // DK = 120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b
    let pbkdf2_result = pbkdf2_sha256(b"password", b"salt", 1, 32);
    let expected_pbkdf2: [u8; 32] = [
        0x12, 0x0f, 0xb6, 0xcf, 0xfc, 0xf8, 0xb3, 0x2c, 0x43, 0xe7, 0x22, 0x52, 0x56, 0xc4, 0xf8,
        0x37, 0xa8, 0x65, 0x48, 0xc9, 0x2c, 0xcc, 0x35, 0x48, 0x08, 0x05, 0x98, 0x7c, 0xb7, 0x0b,
        0xe1, 0x7b,
    ];
    if !constant_time_eq_slices(&pbkdf2_result, &expected_pbkdf2) {
        return false;
    }

    // PBKDF2 with 2 iterations
    // Password = "password", Salt = "salt", c = 2, dkLen = 32
    // DK = ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43
    let pbkdf2_result2 = pbkdf2_sha256(b"password", b"salt", 2, 32);
    let expected_pbkdf2_2: [u8; 32] = [
        0xae, 0x4d, 0x0c, 0x95, 0xaf, 0x6b, 0x46, 0xd3, 0x2d, 0x0a, 0xdf, 0xf9, 0x28, 0xf0, 0x6d,
        0xd0, 0x2a, 0x30, 0x3f, 0x8e, 0xf3, 0xc2, 0x51, 0xdf, 0xd6, 0xe2, 0xd8, 0x5a, 0x95, 0x47,
        0x4c, 0x43,
    ];
    if !constant_time_eq_slices(&pbkdf2_result2, &expected_pbkdf2_2) {
        return false;
    }

    // === HKDF one-shot test ===
    let hkdf_result = hkdf(&salt1, &ikm1, &info1, 42);
    if !constant_time_eq_slices(&hkdf_result, &expected_okm1) {
        return false;
    }

    // === Streaming finalize_clone test ===
    let mut stream2 = HmacSha256::new(b"Jefe");
    stream2.update(b"what do ya want ");
    let intermediate = stream2.finalize_clone();
    stream2.update(b"for nothing?");
    let _final_tag = stream2.finalize();
    // Just verify intermediate doesn't crash and gives a deterministic result
    let mut stream3 = HmacSha256::new(b"Jefe");
    stream3.update(b"what do ya want ");
    let intermediate2 = stream3.finalize_clone();
    if !constant_time_eq(&intermediate, &intermediate2) {
        return false;
    }

    true
}

/// Run self-tests and report to serial console.
pub fn run_self_test() {
    if self_test() {
        crate::serial_println!("    [hmac] Self-test PASSED (10 vectors)");
    } else {
        crate::serial_println!("    [hmac] Self-test FAILED!");
    }
}
