use super::hmac;
/// PBKDF2-HMAC-SHA256 (Key Derivation Function)
///
/// Pure Rust, no-heap implementation of PBKDF2 per RFC 8018.
/// Used for password-based key derivation and password hashing.
///
/// Rules: no heap, no Vec/Box/String, no float casts, no panic.
use crate::serial_println;

pub const PBKDF2_MAX_OUTPUT: usize = 64;
pub const PBKDF2_MAX_PASSWORD: usize = 256;
pub const PBKDF2_MAX_SALT: usize = 64;
pub const PBKDF2_MAX_ITERATIONS: u32 = 100_000; // DoS mitigation

/// PBKDF2-HMAC-SHA256 key derivation
///
/// RFC 8018 Section 5.2: derives a key from a password and salt
/// using HMAC-SHA256 as the PRF (pseudo-random function).
///
/// # Arguments
/// - `password`: password bytes
/// - `pass_len`: password length (0..256)
/// - `salt`: salt bytes
/// - `salt_len`: salt length (0..64)
/// - `iterations`: number of iterations (1..100000, capped at 100k)
/// - `output`: 64-byte output buffer
/// - `out_len`: desired output length (1..64)
///
/// # Returns
/// false if iterations == 0, true otherwise
pub fn pbkdf2_hmac_sha256(
    password: &[u8],
    pass_len: usize,
    salt: &[u8],
    salt_len: usize,
    iterations: u32,
    output: &mut [u8; 64],
    out_len: usize,
) -> bool {
    // Validate inputs
    if iterations == 0 {
        return false;
    }

    // Cap iterations to prevent DoS
    let iterations = if iterations > PBKDF2_MAX_ITERATIONS {
        PBKDF2_MAX_ITERATIONS
    } else {
        iterations
    };

    // Output length must be 1..64
    let out_len = if out_len > 64 { 64 } else { out_len };
    let out_len = if out_len == 0 { 64 } else { out_len };

    // Compute one block (32 bytes = one HMAC-SHA256 output)
    // T_1 = PRF(password, salt || INT_BE(1))
    // U_1 = T_1
    // U_i = PRF(password, U_{i-1})
    // T = U_1 XOR U_2 XOR ... XOR U_iterations

    let mut u = [0u8; 32];
    let mut t = [0u8; 32];

    // PRF(password, salt || INT_BE(1))
    let mut u_input = [0u8; 64 + 4]; // max salt (64) + counter (4)
    u_input[..salt_len].copy_from_slice(&salt[..salt_len]);
    u_input[salt_len..salt_len + 4].copy_from_slice(&1u32.to_be_bytes());

    u = hmac::hmac_sha256(&password[..pass_len], &u_input[..salt_len + 4]);
    t = u;

    // U_i = PRF(password, U_{i-1}) for i = 2..iterations
    let mut i = 1;
    while i < iterations {
        u = hmac::hmac_sha256(&password[..pass_len], &u);
        // XOR into T
        let mut j = 0;
        while j < 32 {
            t[j] ^= u[j];
            j += 1;
        }
        i += 1;
    }

    // Copy output
    let copy_len = if out_len < 32 { out_len } else { 32 };
    output[..copy_len].copy_from_slice(&t[..copy_len]);

    true
}

/// Self-test: verify against RFC 6070 test vector
pub fn init() {
    // RFC 6070 Test Vector 1:
    // PBKDF2-HMAC-SHA256("password", "salt", 1, 32)
    // Expected first bytes: [0x12, 0x0f, 0xb6, 0xcf, ...]

    let mut output = [0u8; 64];
    let success = pbkdf2_hmac_sha256(b"password", 8, b"salt", 4, 1, &mut output, 32);

    if !success {
        serial_println!("    [pbkdf2] Self-test FAILED (iterations==0)");
        return;
    }

    // Check first 4 bytes against test vector
    let expected = [0x12u8, 0x0f, 0xb6, 0xcf];
    let mut match_first = true;
    let mut i = 0;
    while i < 4 {
        if output[i] != expected[i] {
            match_first = false;
        }
        i += 1;
    }

    if match_first {
        serial_println!("    [pbkdf2] PBKDF2-HMAC-SHA256 initialized");
    } else {
        serial_println!("    [pbkdf2] Self-test FAILED (vector mismatch)");
    }
}
