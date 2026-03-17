use crate::crypto::chacha20;
use crate::crypto::sha256;
/// Crypto subsystem tests
///
/// Part of the AIOS. Tests ChaCha20, SHA-256, and Poly1305 key derivation
/// using known-answer test vectors from RFC 8439, NIST FIPS 180-4, and
/// RFC 4231. All tests are deterministic and hardware-independent.
///
/// No std, no heap (no Vec/Box/String/alloc), no floats, no panics.
use crate::test_framework::runner::TestResult;

// ---------------------------------------------------------------------------
// Local assertion helpers
// ---------------------------------------------------------------------------

macro_rules! req {
    ($cond:expr, $msg:expr) => {
        if !$cond {
            crate::serial_println!("    [crypto-test] ASSERT FAILED: {}", $msg);
            return TestResult::Failed;
        }
    };
}

macro_rules! req_eq_u8 {
    ($a:expr, $b:expr, $ctx:expr) => {
        if $a != $b {
            crate::serial_println!(
                "    [crypto-test] ASSERT {}: expected {:#04x}, got {:#04x}",
                $ctx,
                $b,
                $a
            );
            return TestResult::Failed;
        }
    };
}

// ---------------------------------------------------------------------------
// ChaCha20 tests
// ---------------------------------------------------------------------------

/// ChaCha20 block function with all-zero key/nonce/counter=0.
///
/// The first 4 bytes of the keystream with (key=0, nonce=0, counter=0) are
/// known from the ChaCha20 spec/test suite:
///   block[0..4] = 0x76 0xb8 0xe0 0xad  (little-endian 0xade0b876)
///
/// Reference: https://datatracker.ietf.org/doc/html/rfc8439#appendix-A.1
pub fn test_chacha20_all_zero_key() -> TestResult {
    crate::serial_println!("    [crypto-test] running test_chacha20_all_zero_key...");

    let key = [0u8; 32];
    let nonce = [0u8; 12];
    let mut block = [0u8; 64];
    chacha20::chacha20_block(&key, &nonce, 0, &mut block);

    // RFC 8439 Appendix A.1 -- first 4 bytes of the block with zero inputs
    req_eq_u8!(block[0], 0x76, "block[0]");
    req_eq_u8!(block[1], 0xb8, "block[1]");
    req_eq_u8!(block[2], 0xe0, "block[2]");
    req_eq_u8!(block[3], 0xad, "block[3]");

    // Block must be exactly 64 bytes
    req!(block.len() == chacha20::BLOCK_SIZE, "block is 64 bytes");

    crate::serial_println!("    [crypto-test] PASS: test_chacha20_all_zero_key");
    TestResult::Passed
}

/// RFC 8439 Section 2.3.2 -- ChaCha20 block test vector.
///
/// Key  = 00 01 02 .. 1f (32 bytes ascending)
/// Nonce= 00 00 00 09 00 00 00 4a 00 00 00 00
/// Counter = 1
/// Expected first 16 bytes of keystream:
///   10 f1 e7 e4 d1 3b 59 15 50 0f dd 1f a3 20 71 c4
pub fn test_chacha20_block_rfc8439_vec1() -> TestResult {
    crate::serial_println!("    [crypto-test] running test_chacha20_block_rfc8439_vec1...");

    let key: [u8; 32] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d,
        0x1e, 0x1f,
    ];
    let nonce: [u8; 12] = [
        0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x4a, 0x00, 0x00, 0x00, 0x00,
    ];

    let mut block = [0u8; 64];
    chacha20::chacha20_block(&key, &nonce, 1, &mut block);

    let expected: [u8; 16] = [
        0x10, 0xf1, 0xe7, 0xe4, 0xd1, 0x3b, 0x59, 0x15, 0x50, 0x0f, 0xdd, 0x1f, 0xa3, 0x20, 0x71,
        0xc4,
    ];
    for i in 0..16 {
        req_eq_u8!(block[i], expected[i], "block byte");
    }

    crate::serial_println!("    [crypto-test] PASS: test_chacha20_block_rfc8439_vec1");
    TestResult::Passed
}

/// ChaCha20 encryption then decryption roundtrip on a known plaintext.
/// RFC 8439 Section 2.4.2 -- first 8 bytes of ciphertext: 6e 2e 35 9a 25 68 f9 80
///
/// Uses a stack-allocated copy (no heap / no Vec).
pub fn test_chacha20_encrypt_decrypt_roundtrip() -> TestResult {
    crate::serial_println!("    [crypto-test] running test_chacha20_encrypt_decrypt_roundtrip...");

    let key: [u8; 32] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d,
        0x1e, 0x1f,
    ];
    let nonce: [u8; 12] = [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x4a, 0x00, 0x00, 0x00, 0x00,
    ];

    // RFC 8439 Section 2.4.2 plaintext (114 bytes)
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

    // Stack copy for in-place encryption
    let mut data = plaintext;
    chacha20::chacha20_encrypt(&key, &nonce, 1, &mut data);

    // First 8 bytes of ciphertext (RFC 8439 Section 2.4.2)
    let expected_ct_start: [u8; 8] = [0x6e, 0x2e, 0x35, 0x9a, 0x25, 0x68, 0xf9, 0x80];
    for i in 0..8 {
        req_eq_u8!(data[i], expected_ct_start[i], "ciphertext byte");
    }

    // Decrypt and verify restoration
    chacha20::chacha20_decrypt(&key, &nonce, 1, &mut data);
    let mut match_ok = true;
    for i in 0..114 {
        if data[i] != plaintext[i] {
            match_ok = false;
            break;
        }
    }
    req!(match_ok, "decrypt restores plaintext");

    crate::serial_println!("    [crypto-test] PASS: test_chacha20_encrypt_decrypt_roundtrip");
    TestResult::Passed
}

/// Encrypting empty data must not crash and must return immediately.
pub fn test_chacha20_empty_input() -> TestResult {
    crate::serial_println!("    [crypto-test] running test_chacha20_empty_input...");

    let key = [0u8; 32];
    let nonce = [0u8; 12];
    let mut empty: [u8; 0] = [];
    chacha20::chacha20_encrypt(&key, &nonce, 0, &mut empty);
    // No assertion needed -- reaching here means no panic.

    crate::serial_println!("    [crypto-test] PASS: test_chacha20_empty_input");
    TestResult::Passed
}

/// Single-byte encrypt / decrypt roundtrip -- exercises fractional block path.
pub fn test_chacha20_single_byte() -> TestResult {
    crate::serial_println!("    [crypto-test] running test_chacha20_single_byte...");

    let key = [0xFFu8; 32];
    let nonce = [0xAAu8; 12];
    let original: u8 = 0x42;
    let mut buf = [original];
    chacha20::chacha20_encrypt(&key, &nonce, 0, &mut buf);
    chacha20::chacha20_decrypt(&key, &nonce, 0, &mut buf);
    req_eq_u8!(buf[0], original, "single byte roundtrip");

    crate::serial_println!("    [crypto-test] PASS: test_chacha20_single_byte");
    TestResult::Passed
}

/// Poly1305 key generation (RFC 8439 Section 2.6.2).
/// Key  = 80 81 82 .. 9f
/// Nonce= 00 00 00 00 00 01 02 03 04 05 06 07
/// Expected poly key first 8 bytes: 8a d5 a0 8b 90 5f 81 cc
pub fn test_poly1305_key_gen() -> TestResult {
    crate::serial_println!("    [crypto-test] running test_poly1305_key_gen...");

    let key: [u8; 32] = [
        0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x8b, 0x8c, 0x8d, 0x8e,
        0x8f, 0x90, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0x9b, 0x9c, 0x9d,
        0x9e, 0x9f,
    ];
    let nonce: [u8; 12] = [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
    ];
    let expected: [u8; 8] = [0x8a, 0xd5, 0xa0, 0x8b, 0x90, 0x5f, 0x81, 0xcc];

    let poly_key = chacha20::poly1305_key_gen(&key, &nonce);
    for i in 0..8 {
        req_eq_u8!(poly_key[i], expected[i], "poly1305 key byte");
    }

    crate::serial_println!("    [crypto-test] PASS: test_poly1305_key_gen");
    TestResult::Passed
}

/// ChaCha20 encrypt/decrypt roundtrip with a short message (stack buffer).
/// Ciphertext must differ from plaintext; decryption must restore plaintext.
pub fn test_chacha20_short_roundtrip() -> TestResult {
    crate::serial_println!("    [crypto-test] running test_chacha20_short_roundtrip...");

    let key: [u8; 32] = [
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e,
        0x1f, 0x20,
    ];
    let nonce: [u8; 12] = [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
    ];
    let plaintext = *b"Hello, bare-metal crypto!";
    let mut data = plaintext;

    chacha20::chacha20_encrypt(&key, &nonce, 1, &mut data);

    // Ciphertext must differ from plaintext (with overwhelming probability)
    let mut differs = false;
    for i in 0..plaintext.len() {
        if data[i] != plaintext[i] {
            differs = true;
            break;
        }
    }
    req!(differs, "ciphertext should differ from plaintext");

    // Decrypt and verify
    chacha20::chacha20_decrypt(&key, &nonce, 1, &mut data);
    let mut match_ok = true;
    for i in 0..plaintext.len() {
        if data[i] != plaintext[i] {
            match_ok = false;
            break;
        }
    }
    req!(match_ok, "decrypted data matches plaintext");

    crate::serial_println!("    [crypto-test] PASS: test_chacha20_short_roundtrip");
    TestResult::Passed
}

/// XChaCha20 encrypt/decrypt roundtrip (24-byte nonce variant).
pub fn test_xchacha20_roundtrip() -> TestResult {
    crate::serial_println!("    [crypto-test] running test_xchacha20_roundtrip...");

    let key = [0x55u8; 32];
    let nonce = [0xAAu8; 24];
    let plaintext = *b"xchacha20 extended nonce test";
    let mut data = plaintext;

    chacha20::xchacha20_encrypt(&key, &nonce, 0, &mut data);

    // Ciphertext differs
    let mut differs = false;
    for i in 0..plaintext.len() {
        if data[i] != plaintext[i] {
            differs = true;
            break;
        }
    }
    req!(differs, "xchacha20 ciphertext differs");

    // Decrypt
    chacha20::xchacha20_decrypt(&key, &nonce, 0, &mut data);
    let mut match_ok = true;
    for i in 0..plaintext.len() {
        if data[i] != plaintext[i] {
            match_ok = false;
            break;
        }
    }
    req!(match_ok, "xchacha20 roundtrip matches");

    crate::serial_println!("    [crypto-test] PASS: test_xchacha20_roundtrip");
    TestResult::Passed
}

// ---------------------------------------------------------------------------
// SHA-256 tests
// ---------------------------------------------------------------------------

/// SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
/// NIST FIPS 180-4 example / universally verified constant.
pub fn test_sha256_empty() -> TestResult {
    crate::serial_println!("    [crypto-test] running test_sha256_empty...");

    let hash = sha256::hash(b"");
    let expected: [u8; 32] = [
        0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f, 0xb9,
        0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b, 0x78, 0x52,
        0xb8, 0x55,
    ];
    req!(
        sha256::ct_eq(&hash, &expected),
        "SHA-256 empty hash mismatch"
    );

    crate::serial_println!("    [crypto-test] PASS: test_sha256_empty");
    TestResult::Passed
}

/// SHA-256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
pub fn test_sha256_abc() -> TestResult {
    crate::serial_println!("    [crypto-test] running test_sha256_abc...");

    let hash = sha256::hash(b"abc");
    let expected: [u8; 32] = [
        0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22,
        0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00,
        0x15, 0xad,
    ];
    req!(
        sha256::ct_eq(&hash, &expected),
        "SHA-256 'abc' hash mismatch"
    );

    crate::serial_println!("    [crypto-test] PASS: test_sha256_abc");
    TestResult::Passed
}

/// SHA-256 of the 448-bit NIST message (crosses a block boundary).
/// Input: "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
/// Expected: 248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1
pub fn test_sha256_multiblock() -> TestResult {
    crate::serial_println!("    [crypto-test] running test_sha256_multiblock...");

    let msg = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
    let hash = sha256::hash(msg);
    let expected: [u8; 32] = [
        0x24, 0x8d, 0x6a, 0x61, 0xd2, 0x06, 0x38, 0xb8, 0xe5, 0xc0, 0x26, 0x93, 0x0c, 0x3e, 0x60,
        0x39, 0xa3, 0x3c, 0xe4, 0x59, 0x64, 0xff, 0x21, 0x67, 0xf6, 0xec, 0xed, 0xd4, 0x19, 0xdb,
        0x06, 0xc1,
    ];
    req!(
        sha256::ct_eq(&hash, &expected),
        "SHA-256 448-bit message mismatch"
    );

    crate::serial_println!("    [crypto-test] PASS: test_sha256_multiblock");
    TestResult::Passed
}

/// Streaming SHA-256: "abc" fed as three one-byte updates must equal one-shot hash.
pub fn test_sha256_streaming() -> TestResult {
    crate::serial_println!("    [crypto-test] running test_sha256_streaming...");

    let mut hasher = sha256::Sha256::new();
    hasher.update(b"a");
    hasher.update(b"b");
    hasher.update(b"c");
    let streaming = hasher.finalize();

    let oneshot = sha256::hash(b"abc");
    req!(
        sha256::ct_eq(&streaming, &oneshot),
        "streaming != one-shot hash"
    );

    crate::serial_println!("    [crypto-test] PASS: test_sha256_streaming");
    TestResult::Passed
}

/// hash_multi must equal the one-shot hash of concatenation.
pub fn test_sha256_hash_multi() -> TestResult {
    crate::serial_println!("    [crypto-test] running test_sha256_hash_multi...");

    let part1 = b"hello ";
    let part2 = b"world";
    let multi = sha256::hash_multi(&[part1, part2]);
    let concat = sha256::hash(b"hello world");
    req!(sha256::ct_eq(&multi, &concat), "hash_multi != hash_concat");

    crate::serial_println!("    [crypto-test] PASS: test_sha256_hash_multi");
    TestResult::Passed
}

/// double_hash must equal SHA256(SHA256(data)).
pub fn test_sha256_double_hash() -> TestResult {
    crate::serial_println!("    [crypto-test] running test_sha256_double_hash...");

    let data = b"double hash kernel test";
    let dh = sha256::double_hash(data);
    let manual = sha256::hash(&sha256::hash(data));
    req!(sha256::ct_eq(&dh, &manual), "double_hash != manual double");

    crate::serial_println!("    [crypto-test] PASS: test_sha256_double_hash");
    TestResult::Passed
}

/// HMAC-SHA256 with RFC 4231 Test Case 2.
/// Key  = "Jefe"
/// Data = "what do ya want for nothing?"
/// Expected: 5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843
pub fn test_hmac_sha256_rfc4231() -> TestResult {
    crate::serial_println!("    [crypto-test] running test_hmac_sha256_rfc4231...");

    let result = sha256::hmac_sha256(b"Jefe", b"what do ya want for nothing?");
    let expected: [u8; 32] = [
        0x5b, 0xdc, 0xc1, 0x46, 0xbf, 0x60, 0x75, 0x4e, 0x6a, 0x04, 0x24, 0x26, 0x08, 0x95, 0x75,
        0xc7, 0x5a, 0x00, 0x3f, 0x08, 0x9d, 0x27, 0x39, 0x83, 0x9d, 0xec, 0x58, 0xb9, 0x64, 0xec,
        0x38, 0x43,
    ];
    req!(
        sha256::ct_eq(&result, &expected),
        "HMAC-SHA256 RFC 4231 TC2 mismatch"
    );

    crate::serial_println!("    [crypto-test] PASS: test_hmac_sha256_rfc4231");
    TestResult::Passed
}

/// SHA-256 of 64 zero bytes (exactly one block) -- tests padding edge case.
/// Expected: f5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4b
pub fn test_sha256_one_block_zeros() -> TestResult {
    crate::serial_println!("    [crypto-test] running test_sha256_one_block_zeros...");

    let input = [0u8; 64];
    let hash = sha256::hash(&input);
    let expected: [u8; 32] = [
        0xf5, 0xa5, 0xfd, 0x42, 0xd1, 0x6a, 0x20, 0x30, 0x27, 0x98, 0xef, 0x6e, 0xd3, 0x09, 0x97,
        0x9b, 0x43, 0x00, 0x3d, 0x23, 0x20, 0xd9, 0xf0, 0xe8, 0xea, 0x98, 0x31, 0xa9, 0x27, 0x59,
        0xfb, 0x4b,
    ];
    req!(
        sha256::ct_eq(&hash, &expected),
        "SHA-256 64-zero-byte mismatch"
    );

    crate::serial_println!("    [crypto-test] PASS: test_sha256_one_block_zeros");
    TestResult::Passed
}

/// ct_eq returns true for identical arrays and false for differing arrays.
pub fn test_sha256_ct_eq() -> TestResult {
    crate::serial_println!("    [crypto-test] running test_sha256_ct_eq...");

    let a = sha256::hash(b"same");
    let b = sha256::hash(b"same");
    let c = sha256::hash(b"different");

    req!(sha256::ct_eq(&a, &b), "ct_eq same should be true");
    req!(!sha256::ct_eq(&a, &c), "ct_eq different should be false");

    // ct_eq_slices with different lengths
    req!(
        !sha256::ct_eq_slices(b"abc", b"ab"),
        "different lengths should be false"
    );
    req!(
        sha256::ct_eq_slices(b"abc", b"abc"),
        "same content should be true"
    );

    crate::serial_println!("    [crypto-test] PASS: test_sha256_ct_eq");
    TestResult::Passed
}

// ---------------------------------------------------------------------------
// run_all
// ---------------------------------------------------------------------------

pub fn run_all() {
    crate::serial_println!("    [crypto-test] ==============================");
    crate::serial_println!("    [crypto-test] Running crypto test suite");
    crate::serial_println!("    [crypto-test] ==============================");

    let mut passed = 0u32;
    let mut failed = 0u32;

    macro_rules! run {
        ($f:expr, $name:literal) => {
            match $f() {
                TestResult::Passed => {
                    passed += 1;
                    crate::serial_println!("    [crypto-test] [PASS] {}", $name);
                }
                TestResult::Skipped => {
                    crate::serial_println!("    [crypto-test] [SKIP] {}", $name);
                }
                TestResult::Failed => {
                    failed += 1;
                    crate::serial_println!("    [crypto-test] [FAIL] {}", $name);
                }
            }
        };
    }

    // ChaCha20
    run!(test_chacha20_all_zero_key, "chacha20_all_zero_key");
    run!(
        test_chacha20_block_rfc8439_vec1,
        "chacha20_block_rfc8439_vec1"
    );
    run!(
        test_chacha20_encrypt_decrypt_roundtrip,
        "chacha20_encrypt_decrypt_roundtrip"
    );
    run!(test_chacha20_empty_input, "chacha20_empty_input");
    run!(test_chacha20_single_byte, "chacha20_single_byte");
    run!(test_poly1305_key_gen, "poly1305_key_gen");
    run!(test_chacha20_short_roundtrip, "chacha20_short_roundtrip");
    run!(test_xchacha20_roundtrip, "xchacha20_roundtrip");

    // SHA-256
    run!(test_sha256_empty, "sha256_empty");
    run!(test_sha256_abc, "sha256_abc");
    run!(test_sha256_multiblock, "sha256_multiblock");
    run!(test_sha256_streaming, "sha256_streaming");
    run!(test_sha256_hash_multi, "sha256_hash_multi");
    run!(test_sha256_double_hash, "sha256_double_hash");
    run!(test_hmac_sha256_rfc4231, "hmac_sha256_rfc4231");
    run!(test_sha256_one_block_zeros, "sha256_one_block_zeros");
    run!(test_sha256_ct_eq, "sha256_ct_eq");

    crate::serial_println!(
        "    [crypto-test] Results: {} passed, {} failed",
        passed,
        failed
    );
}
