/// ecdsa — ECDSA P-256 (secp256r1) signature verification infrastructure
///
/// Implements the public API and validation types for ECDSA over the NIST
/// P-256 curve.  The 256-bit integer type used here is `[u32; 8]` in
/// big-endian word order (word[0] is the most significant).
///
/// Rules: no_std, no heap (no Vec/Box/String/alloc::*), no float casts,
///        no panic/unwrap/expect, saturating counters, wrapping sequence
///        numbers, MMIO via read_volatile/write_volatile only.
///
/// This module provides:
///   - Validation of r and s values (must be in [1, n-1])
///   - Type-safe wrappers: `EcdsaPublicKey`, `EcdsaSignature`, `EcdsaResult`
///   - Helper functions: `bytes_to_u256`, `u256_lt`, `u256_is_zero`
///   - Constructor helpers: `ecdsa_key_from_bytes`, `ecdsa_sig_from_bytes`
///   - Top-level `ecdsa_verify` function (validates r/s range; EC point
///     multiplication stub returns Ok for structurally valid inputs)
///   - `init()` runs a self-test and prints an initialization message
///
/// Full EC point multiplication is not implemented here — the goal is the
/// correct API surface and input-validation infrastructure.
use crate::serial_println;

// ---------------------------------------------------------------------------
// P-256 curve order n (big-endian u32 words, word[0] = most significant)
// n = 0xffffffff00000000ffffffffffffffff bce6faada7179e84f3b9cac2fc632551
// ---------------------------------------------------------------------------

pub const P256_N: [u32; 8] = [
    0xffffffff, 0x00000000, 0xffffffff, 0xffffffff, 0xbce6faad, 0xa7179e84, 0xf3b9cac2, 0xfc632551,
];

// Constant zero — used in comparisons.
const U256_ZERO: [u32; 8] = [0u32; 8];

// Constant one — lower bound for valid r and s (must be >= 1).
const U256_ONE: [u32; 8] = [0, 0, 0, 0, 0, 0, 0, 1];

// ---------------------------------------------------------------------------
// Result type
// ---------------------------------------------------------------------------

/// Outcome of an ECDSA verify operation.
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum EcdsaResult {
    /// Signature is structurally valid (r, s in [1, n-1]).
    /// (Full EC-point verify is stubbed; this indicates the format is correct.)
    Ok,
    /// r or s is out of range or zero.
    BadSignature,
    /// The public key is structurally invalid (not yet validated in stub).
    InvalidKey,
    /// Input buffers have the wrong length (future-proofing for slice-based API).
    InvalidLength,
}

// ---------------------------------------------------------------------------
// Key and signature types
// ---------------------------------------------------------------------------

/// P-256 public key: affine coordinates (x, y), each 32 bytes big-endian.
#[derive(Copy, Clone)]
pub struct EcdsaPublicKey {
    pub x: [u8; 32],
    pub y: [u8; 32],
}

impl EcdsaPublicKey {
    /// Returns a zeroed public key (not a valid key — use only as a placeholder).
    pub const fn zero() -> Self {
        EcdsaPublicKey {
            x: [0u8; 32],
            y: [0u8; 32],
        }
    }
}

/// ECDSA signature: (r, s), each 32 bytes big-endian.
#[derive(Copy, Clone)]
pub struct EcdsaSignature {
    pub r: [u8; 32],
    pub s: [u8; 32],
}

impl EcdsaSignature {
    /// Returns a zeroed signature (always invalid — for testing and placeholders).
    pub const fn zero() -> Self {
        EcdsaSignature {
            r: [0u8; 32],
            s: [0u8; 32],
        }
    }
}

// ---------------------------------------------------------------------------
// 256-bit integer helpers  ([u32; 8], big-endian words)
// ---------------------------------------------------------------------------

/// Parse a 32-byte big-endian buffer into a `[u32; 8]` big-endian word array.
///
/// `b[0..4]`  → `result[0]` (most significant word)
/// `b[28..32]` → `result[7]` (least significant word)
fn bytes_to_u256(b: &[u8; 32]) -> [u32; 8] {
    let mut out = [0u32; 8];
    let mut i = 0usize;
    while i < 8 {
        let off = i.saturating_mul(4);
        out[i] = u32::from_be_bytes([
            b[off],
            b[off.saturating_add(1)],
            b[off.saturating_add(2)],
            b[off.saturating_add(3)],
        ]);
        i = i.saturating_add(1);
    }
    out
}

/// Returns `true` if `a < b` (both in big-endian word order).
fn u256_lt(a: &[u32; 8], b: &[u32; 8]) -> bool {
    let mut i = 0usize;
    while i < 8 {
        if a[i] < b[i] {
            return true;
        }
        if a[i] > b[i] {
            return false;
        }
        i = i.saturating_add(1);
    }
    // Equal — not strictly less than.
    false
}

/// Returns `true` if all words of `a` are zero.
fn u256_is_zero(a: &[u32; 8]) -> bool {
    let mut i = 0usize;
    while i < 8 {
        if a[i] != 0 {
            return false;
        }
        i = i.saturating_add(1);
    }
    true
}

/// Returns `true` if `a >= b` (big-endian word order).
/// Equivalent to `!u256_lt(a, b)`.
#[inline]
fn u256_ge(a: &[u32; 8], b: &[u32; 8]) -> bool {
    !u256_lt(a, b)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Verify an ECDSA P-256 signature.
///
/// Validation performed:
/// 1. Parse `r` and `s` from the signature as big-endian 256-bit integers.
/// 2. Reject if `r == 0` or `s == 0` → `BadSignature`.
/// 3. Reject if `r >= n` or `s >= n` → `BadSignature`.
/// 4. (Stub) For structurally valid (r, s), return `Ok`.
///    A full implementation would perform EC point multiplication here.
///
/// `hash` is the 32-byte message digest (e.g., SHA-256 output).
pub fn ecdsa_verify(_key: &EcdsaPublicKey, _hash: &[u8; 32], sig: &EcdsaSignature) -> EcdsaResult {
    // --- Step 1: parse r and s ---
    let r = bytes_to_u256(&sig.r);
    let s = bytes_to_u256(&sig.s);

    // --- Step 2: reject zero ---
    if u256_is_zero(&r) || u256_is_zero(&s) {
        return EcdsaResult::BadSignature;
    }

    // --- Step 3: reject r >= n or s >= n ---
    // r must be in [1, n-1], i.e., r < n.
    if u256_ge(&r, &P256_N) {
        return EcdsaResult::BadSignature;
    }
    if u256_ge(&s, &P256_N) {
        return EcdsaResult::BadSignature;
    }

    // --- Step 4: EC point multiplication stub ---
    // Inputs are structurally valid.  A complete implementation would:
    //   w  = s^-1 mod n
    //   u1 = hash * w mod n
    //   u2 = r    * w mod n
    //   R  = u1*G + u2*Q   (EC point addition)
    //   return Ok iff R.x mod n == r
    EcdsaResult::Ok
}

/// Construct an `EcdsaPublicKey` from two 32-byte big-endian coordinate arrays.
pub fn ecdsa_key_from_bytes(x: &[u8; 32], y: &[u8; 32]) -> EcdsaPublicKey {
    let mut key = EcdsaPublicKey::zero();
    let mut i = 0usize;
    while i < 32 {
        key.x[i] = x[i];
        key.y[i] = y[i];
        i = i.saturating_add(1);
    }
    key
}

/// Construct an `EcdsaSignature` from two 32-byte big-endian (r, s) arrays.
pub fn ecdsa_sig_from_bytes(r: &[u8; 32], s: &[u8; 32]) -> EcdsaSignature {
    let mut sig = EcdsaSignature::zero();
    let mut i = 0usize;
    while i < 32 {
        sig.r[i] = r[i];
        sig.s[i] = s[i];
        i = i.saturating_add(1);
    }
    sig
}

// ---------------------------------------------------------------------------
// Self-test
// ---------------------------------------------------------------------------

/// Run a compile-time-verifiable self-test:
///   - A zero signature must produce `BadSignature`.
///   - A signature with r=1, s=1 (both < n, both nonzero) must produce `Ok`.
fn self_test() -> bool {
    let key = EcdsaPublicKey::zero();
    let hash = [0u8; 32];

    // Test 1: zero signature — must be rejected.
    let zero_sig = EcdsaSignature::zero();
    if ecdsa_verify(&key, &hash, &zero_sig) != EcdsaResult::BadSignature {
        return false;
    }

    // Test 2: r = 1, s = 1 — both in [1, n-1] — must be accepted (stub Ok).
    let mut r_bytes = [0u8; 32];
    let mut s_bytes = [0u8; 32];
    r_bytes[31] = 1; // big-endian 1
    s_bytes[31] = 1;
    let small_sig = ecdsa_sig_from_bytes(&r_bytes, &s_bytes);
    if ecdsa_verify(&key, &hash, &small_sig) != EcdsaResult::Ok {
        return false;
    }

    // Test 3: r = n (out of range) — must be rejected.
    // Build r = P256_N as bytes (big-endian words → bytes).
    let mut n_bytes = [0u8; 32];
    let mut wi = 0usize;
    while wi < 8 {
        let be = P256_N[wi].to_be_bytes();
        let off = wi.saturating_mul(4);
        n_bytes[off] = be[0];
        n_bytes[off.saturating_add(1)] = be[1];
        n_bytes[off.saturating_add(2)] = be[2];
        n_bytes[off.saturating_add(3)] = be[3];
        wi = wi.saturating_add(1);
    }
    let out_of_range_sig = ecdsa_sig_from_bytes(&n_bytes, &s_bytes);
    if ecdsa_verify(&key, &hash, &out_of_range_sig) != EcdsaResult::BadSignature {
        return false;
    }

    true
}

// ---------------------------------------------------------------------------
// Module init
// ---------------------------------------------------------------------------

/// Initialize the ECDSA infrastructure.
///
/// Runs the self-test suite (zero signature must produce `BadSignature`) and
/// prints an initialization message to the serial console.
pub fn init() {
    if self_test() {
        serial_println!("[ecdsa] P-256 ECDSA infrastructure initialized");
    } else {
        serial_println!("[ecdsa] WARNING: P-256 ECDSA self-test FAILED");
    }
}
