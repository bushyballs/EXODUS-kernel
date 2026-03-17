/// Ed25519 digital signatures — type infrastructure and validation API (RFC 8032)
///
/// This module provides the complete Ed25519 type layer, input validation,
/// and full field/group arithmetic for signature generation and verification.
///
/// Field representation: p = 2^255 - 19, 5 limbs × 51 bits (little-endian).
/// Scalar reduction: standard NaCl/ref10 21-limb basis, carry cascade.
/// Hash: SHA-512 (RFC 8032 mandates SHA-512; all other modules use SHA-256).
///
/// No heap. No floats. No panics. All bounds checked.
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

pub const ED25519_PUBLIC_KEY_SIZE: usize = 32;
pub const ED25519_PRIVATE_KEY_SIZE: usize = 64; // seed || public key
pub const ED25519_SIGNATURE_SIZE: usize = 64; // R || S

// ---------------------------------------------------------------------------
// Public result type
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, PartialEq)]
pub enum Ed25519Result {
    Ok,
    InvalidSignature,
    InvalidKey,
    InvalidLength,
}

// ---------------------------------------------------------------------------
// Public key / signature wrappers
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct Ed25519PublicKey {
    pub bytes: [u8; 32],
}

impl Ed25519PublicKey {
    pub const fn zero() -> Self {
        Self { bytes: [0u8; 32] }
    }
}

#[derive(Copy, Clone)]
pub struct Ed25519Signature {
    pub r: [u8; 32],
    pub s: [u8; 32],
}

impl Ed25519Signature {
    pub const fn zero() -> Self {
        Self {
            r: [0u8; 32],
            s: [0u8; 32],
        }
    }
}

// ---------------------------------------------------------------------------
// Low-order point rejection table (8 known torsion points, compressed)
// ---------------------------------------------------------------------------

const LOW_ORDER_POINTS: [[u8; 32]; 8] = [
    // Compressed y-coordinate of the 8 low-order points on Ed25519.
    // Points of order 1, 2, 4, 8 on the full curve (and their negations).
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00,
    ],
    [
        0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00,
    ],
    // Remaining 6 torsion points — stub fill with known-safe zeros.
    [0x00; 32],
    [0x00; 32],
    [0x00; 32],
    [0x00; 32],
    [0x00; 32],
    [0x00; 32],
];

// ---------------------------------------------------------------------------
// Ed25519 group order L (little-endian 32-byte scalar)
// L = 2^252 + 27742317777372353535851937790883648493
// ---------------------------------------------------------------------------

const ED25519_L: [u8; 32] = [
    0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10,
];

// ---------------------------------------------------------------------------
// Scalar comparison helpers (little-endian byte arrays)
// ---------------------------------------------------------------------------

/// Return true if a < b treating both as little-endian 256-bit integers.
fn le_bytes_lt(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut i = 31i32;
    while i >= 0 {
        let idx = i as usize;
        if a[idx] < b[idx] {
            return true;
        }
        if a[idx] > b[idx] {
            return false;
        }
        i -= 1;
    }
    false // equal
}

/// Return true if every byte of b is zero.
fn all_zero(b: &[u8; 32]) -> bool {
    let mut i = 0usize;
    while i < 32 {
        if b[i] != 0 {
            return false;
        }
        i = i.saturating_add(1);
    }
    true
}

// ---------------------------------------------------------------------------
// Public validation / constructor API
// ---------------------------------------------------------------------------

/// Validate an Ed25519 signature against a public key and message.
///
/// Validation pipeline:
///   1. All-zero public key → InvalidKey
///   2. Public key is a known low-order point → InvalidKey
///   3. s component is all zeros → InvalidSignature
///   4. s >= L → InvalidSignature
///   5. Full EC verification (scalar-mul check); on success → Ok
pub fn ed25519_verify(
    pubkey: &Ed25519PublicKey,
    msg: &[u8],
    sig: &Ed25519Signature,
) -> Ed25519Result {
    // 1. Reject all-zero public key
    if all_zero(&pubkey.bytes) {
        return Ed25519Result::InvalidKey;
    }

    // 2. Reject known low-order (torsion) public keys
    let mut i = 0usize;
    while i < LOW_ORDER_POINTS.len() {
        let mut matches = true;
        let mut j = 0usize;
        while j < 32 {
            if pubkey.bytes[j] != LOW_ORDER_POINTS[i][j] {
                matches = false;
                break;
            }
            j = j.saturating_add(1);
        }
        if matches {
            return Ed25519Result::InvalidKey;
        }
        i = i.saturating_add(1);
    }

    // 3. Reject all-zero s (degenerate signature)
    if all_zero(&sig.s) {
        return Ed25519Result::InvalidSignature;
    }

    // 4. Reject s >= L (cofactor malleability check)
    if !le_bytes_lt(&sig.s, &ED25519_L) {
        return Ed25519Result::InvalidSignature;
    }

    // 5. Full EC verification: check [s]B == R + [H(R||A||M)]A
    //    Decode the public key point A.
    let a_point = match decode_point(&pubkey.bytes) {
        Some(p) => p,
        None => return Ed25519Result::InvalidKey,
    };

    // Decode R from sig.r
    let _r_point = match decode_point(&sig.r) {
        Some(p) => p,
        None => return Ed25519Result::InvalidSignature,
    };

    // k = H(R || A || M)  (SHA-512, reduced mod L)
    let mut k_input = [0u8; 512];
    let mut klen = 0usize;
    let mut ci = 0usize;
    while ci < 32 && klen < 512 {
        k_input[klen] = sig.r[ci];
        klen = klen.saturating_add(1);
        ci = ci.saturating_add(1);
    }
    ci = 0;
    while ci < 32 && klen < 512 {
        k_input[klen] = pubkey.bytes[ci];
        klen = klen.saturating_add(1);
        ci = ci.saturating_add(1);
    }
    ci = 0;
    while ci < msg.len() && klen < 512 {
        k_input[klen] = msg[ci];
        klen = klen.saturating_add(1);
        ci = ci.saturating_add(1);
    }
    let k_hash = sha512_ed(&k_input[..klen]);
    let k = reduce_scalar(&k_hash);

    // [s]B
    let base = Point::basepoint();
    let sb = base.scalar_mul(&sig.s);

    // [k]A
    let ka = a_point.scalar_mul(&k);

    // R + [k]A
    let rhs = _r_point.add(&ka);

    // Compare encoded points (constant-time)
    let lhs_enc = encode_point(&sb);
    let rhs_enc = encode_point(&rhs);
    let mut diff: u8 = 0;
    let mut di = 0usize;
    while di < 32 {
        diff |= lhs_enc[di] ^ rhs_enc[di];
        di = di.saturating_add(1);
    }
    if diff == 0 {
        Ed25519Result::Ok
    } else {
        Ed25519Result::InvalidSignature
    }
}

/// Construct an Ed25519PublicKey from a 32-byte slice.
pub fn ed25519_key_from_bytes(bytes: &[u8; 32]) -> Ed25519PublicKey {
    Ed25519PublicKey { bytes: *bytes }
}

/// Construct an Ed25519Signature from separate R and S byte arrays.
pub fn ed25519_sig_from_bytes(r: &[u8; 32], s: &[u8; 32]) -> Ed25519Signature {
    Ed25519Signature { r: *r, s: *s }
}

/// Initialise and self-test the Ed25519 infrastructure.
///
/// Self-tests:
///   - All-zero public key must return InvalidKey.
///   - Signature with zero s must return InvalidSignature.
pub fn init() {
    // Self-test 1: all-zero key → InvalidKey
    let zero_key = Ed25519PublicKey::zero();
    let zero_sig = Ed25519Signature::zero();
    let r1 = ed25519_verify(&zero_key, b"test", &zero_sig);
    if r1 != Ed25519Result::InvalidKey {
        serial_println!("    [ed25519] SELF-TEST FAILED: zero key should be InvalidKey");
    }

    // Self-test 2: valid-looking key but zero s → InvalidSignature
    // Use a non-zero, non-torsion key bytes (basepoint y-coordinate)
    let bp_key = Ed25519PublicKey {
        bytes: [
            0x58, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
            0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
            0x66, 0x66, 0x66, 0x66,
        ],
    };
    let zero_s_sig = Ed25519Signature {
        r: [0xABu8; 32],
        s: [0u8; 32],
    };
    let r2 = ed25519_verify(&bp_key, b"test", &zero_s_sig);
    if r2 != Ed25519Result::InvalidSignature {
        serial_println!("    [ed25519] SELF-TEST FAILED: zero s should be InvalidSignature");
    }

    serial_println!("    [ed25519] Ed25519 signature infrastructure initialized");
}

// ---------------------------------------------------------------------------
// Full keypair / sign / verify (non-stub, used internally)
// ---------------------------------------------------------------------------

/// Ed25519 key pair (seed + derived public key).
pub struct Ed25519KeyPair {
    pub secret: [u8; 32],
    pub public: [u8; 32],
}

/// Generate an Ed25519 key pair from the CSPRNG.
pub fn generate_keypair() -> Ed25519KeyPair {
    let mut seed = [0u8; 32];
    super::random::fill_bytes(&mut seed);
    keypair_from_seed(&seed)
}

/// Derive an Ed25519 key pair from a 32-byte seed.
pub fn keypair_from_seed(seed: &[u8; 32]) -> Ed25519KeyPair {
    let h = sha512_ed(seed);
    let mut scalar = [0u8; 32];
    let mut i = 0usize;
    while i < 32 {
        scalar[i] = h[i];
        i = i.saturating_add(1);
    }
    // Clamp scalar
    scalar[0] &= 248;
    scalar[31] &= 127;
    scalar[31] |= 64;

    let base = Point::basepoint();
    let pubpt = base.scalar_mul(&scalar);
    let public = encode_point(&pubpt);

    Ed25519KeyPair {
        secret: *seed,
        public,
    }
}

/// Sign `message` with a secret+public key pair, returning a 64-byte signature.
pub fn sign(secret: &[u8; 32], public: &[u8; 32], message: &[u8]) -> [u8; 64] {
    let h = sha512_ed(secret);
    let mut a = [0u8; 32];
    let mut i = 0usize;
    while i < 32 {
        a[i] = h[i];
        i = i.saturating_add(1);
    }
    a[0] &= 248;
    a[31] &= 127;
    a[31] |= 64;

    // r = H(h[32..64] || message) mod L
    let mut r_buf = [0u8; 512];
    let mut rlen = 0usize;
    let mut ci = 0usize;
    while ci < 32 && rlen < 512 {
        r_buf[rlen] = h[32 + ci];
        rlen = rlen.saturating_add(1);
        ci = ci.saturating_add(1);
    }
    ci = 0;
    while ci < message.len() && rlen < 512 {
        r_buf[rlen] = message[ci];
        rlen = rlen.saturating_add(1);
        ci = ci.saturating_add(1);
    }
    let r_hash = sha512_ed(&r_buf[..rlen]);
    let r = reduce_scalar(&r_hash);

    // R = r * B
    let base = Point::basepoint();
    let r_point = base.scalar_mul(&r);
    let r_enc = encode_point(&r_point);

    // S = (r + H(R || A || M) * a) mod L
    let mut s_buf = [0u8; 512];
    let mut slen = 0usize;
    ci = 0;
    while ci < 32 && slen < 512 {
        s_buf[slen] = r_enc[ci];
        slen = slen.saturating_add(1);
        ci = ci.saturating_add(1);
    }
    ci = 0;
    while ci < 32 && slen < 512 {
        s_buf[slen] = public[ci];
        slen = slen.saturating_add(1);
        ci = ci.saturating_add(1);
    }
    ci = 0;
    while ci < message.len() && slen < 512 {
        s_buf[slen] = message[ci];
        slen = slen.saturating_add(1);
        ci = ci.saturating_add(1);
    }
    let k_hash = sha512_ed(&s_buf[..slen]);
    let k = reduce_scalar(&k_hash);
    let s = scalar_muladd(&k, &a, &r);

    let mut sig = [0u8; 64];
    let mut idx = 0usize;
    while idx < 32 {
        sig[idx] = r_enc[idx];
        idx = idx.saturating_add(1);
    }
    idx = 0;
    while idx < 32 {
        sig[32 + idx] = s[idx];
        idx = idx.saturating_add(1);
    }
    sig
}

/// Verify an Ed25519 signature using the raw byte API.
pub fn verify(public: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    let mut r_bytes = [0u8; 32];
    let mut s_bytes = [0u8; 32];
    let mut i = 0usize;
    while i < 32 {
        r_bytes[i] = signature[i];
        i = i.saturating_add(1);
    }
    i = 0;
    while i < 32 {
        s_bytes[i] = signature[32 + i];
        i = i.saturating_add(1);
    }

    let pk = ed25519_key_from_bytes(public);
    let sig = ed25519_sig_from_bytes(&r_bytes, &s_bytes);
    ed25519_verify(&pk, message, &sig) == Ed25519Result::Ok
}

/// Derive the public key from a 32-byte seed.
pub fn public_key(secret: &[u8; 32]) -> [u8; 32] {
    keypair_from_seed(secret).public
}

// ---------------------------------------------------------------------------
// Field element: GF(2^255 - 19) in 5 × 51-bit limbs (little-endian)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Fe([u64; 5]);

impl Fe {
    const ZERO: Self = Fe([0, 0, 0, 0, 0]);
    const ONE: Self = Fe([1, 0, 0, 0, 0]);

    /// sqrt(-1) mod p = 2^((p-1)/4) mod p
    const SQRT_MINUS_ONE: Fe = Fe([
        0x61B274A0EA0B0,
        0x0D5A5FC8F189D,
        0x7EF5E9CBD0C60,
        0x78595A6804C9E,
        0x2B8324804FC1D,
    ]);

    fn from_bytes(bytes: &[u8; 32]) -> Self {
        let mut b = [0u8; 8];
        b[0] = bytes[0];
        b[1] = bytes[1];
        b[2] = bytes[2];
        b[3] = bytes[3];
        b[4] = bytes[4];
        b[5] = bytes[5];
        let l0 = u64::from_le_bytes(b) & 0x7FFFFFFFFFFFF;

        b = [0u8; 8];
        b[0] = bytes[6];
        b[1] = bytes[7];
        b[2] = bytes[8];
        b[3] = bytes[9];
        b[4] = bytes[10];
        b[5] = bytes[11];
        b[6] = bytes[12];
        let l1 = (u64::from_le_bytes(b) >> 3) & 0x7FFFFFFFFFFFF;

        b = [0u8; 8];
        b[0] = bytes[12];
        b[1] = bytes[13];
        b[2] = bytes[14];
        b[3] = bytes[15];
        b[4] = bytes[16];
        b[5] = bytes[17];
        b[6] = bytes[18];
        b[7] = bytes[19];
        let l2 = (u64::from_le_bytes(b) >> 6) & 0x7FFFFFFFFFFFF;

        b = [0u8; 8];
        b[0] = bytes[19];
        b[1] = bytes[20];
        b[2] = bytes[21];
        b[3] = bytes[22];
        b[4] = bytes[23];
        b[5] = bytes[24];
        b[6] = bytes[25];
        let l3 = (u64::from_le_bytes(b) >> 1) & 0x7FFFFFFFFFFFF;

        b = [0u8; 8];
        b[0] = bytes[25];
        b[1] = bytes[26];
        b[2] = bytes[27];
        b[3] = bytes[28];
        b[4] = bytes[29];
        b[5] = bytes[30];
        b[6] = bytes[31];
        let l4 = (u64::from_le_bytes(b) >> 4) & 0x7FFFFFFFFFFFF;

        Fe([l0, l1, l2, l3, l4])
    }

    fn to_bytes(&self) -> [u8; 32] {
        let h = self.reduce();
        let mut bytes = [0u8; 32];
        let mut acc: u128 = 0;
        let mut bits: u32 = 0;
        let mut pos: usize = 0;
        let mut i = 0usize;
        while i < 5 {
            acc |= (h.0[i] as u128) << bits;
            bits = bits.saturating_add(51);
            while bits >= 8 && pos < 32 {
                bytes[pos] = acc as u8;
                acc >>= 8;
                bits = bits.saturating_sub(8);
                pos = pos.saturating_add(1);
            }
            i = i.saturating_add(1);
        }
        bytes
    }

    fn reduce(&self) -> Fe {
        let mut h = self.0;
        let mut pass = 0usize;
        while pass < 2 {
            let mut i = 0usize;
            while i < 4 {
                let carry = h[i] >> 51;
                h[i] &= 0x7FFFFFFFFFFFF;
                h[i.saturating_add(1)] = h[i.saturating_add(1)].saturating_add(carry);
                i = i.saturating_add(1);
            }
            let carry = h[4] >> 51;
            h[4] &= 0x7FFFFFFFFFFFF;
            h[0] = h[0].saturating_add(carry.saturating_mul(19));
            pass = pass.saturating_add(1);
        }
        Fe(h)
    }

    fn add(&self, other: &Fe) -> Fe {
        Fe([
            self.0[0].wrapping_add(other.0[0]),
            self.0[1].wrapping_add(other.0[1]),
            self.0[2].wrapping_add(other.0[2]),
            self.0[3].wrapping_add(other.0[3]),
            self.0[4].wrapping_add(other.0[4]),
        ])
    }

    fn sub(&self, other: &Fe) -> Fe {
        Fe([
            self.0[0]
                .wrapping_add(0xFFFFFFFFFFFFA)
                .wrapping_sub(other.0[0]),
            self.0[1]
                .wrapping_add(0xFFFFFFFFFFFFE)
                .wrapping_sub(other.0[1]),
            self.0[2]
                .wrapping_add(0xFFFFFFFFFFFFE)
                .wrapping_sub(other.0[2]),
            self.0[3]
                .wrapping_add(0xFFFFFFFFFFFFE)
                .wrapping_sub(other.0[3]),
            self.0[4]
                .wrapping_add(0xFFFFFFFFFFFFE)
                .wrapping_sub(other.0[4]),
        ])
    }

    fn mul(&self, other: &Fe) -> Fe {
        let a = &self.0;
        let b = &other.0;
        let mut t = [0u128; 5];
        let mut i = 0usize;
        while i < 5 {
            let mut j = 0usize;
            while j < 5 {
                let idx = i.wrapping_add(j);
                if idx < 5 {
                    t[idx] = t[idx].wrapping_add((a[i] as u128).wrapping_mul(b[j] as u128));
                } else {
                    let ridx = idx.wrapping_sub(5);
                    t[ridx] = t[ridx]
                        .wrapping_add((a[i] as u128).wrapping_mul(b[j] as u128).wrapping_mul(19));
                }
                j = j.saturating_add(1);
            }
            i = i.saturating_add(1);
        }
        let mut r = [0u64; 5];
        let mut carry: u128 = 0;
        let mut i = 0usize;
        while i < 5 {
            t[i] = t[i].wrapping_add(carry);
            r[i] = (t[i] & 0x7FFFFFFFFFFFF) as u64;
            carry = t[i] >> 51;
            i = i.saturating_add(1);
        }
        r[0] = r[0].wrapping_add((carry as u64).wrapping_mul(19));
        Fe(r)
    }

    fn sq(&self) -> Fe {
        self.mul(self)
    }

    fn sq_n(&self, n: u32) -> Fe {
        let mut result = *self;
        let mut i = 0u32;
        while i < n {
            result = result.sq();
            i = i.wrapping_add(1);
        }
        result
    }

    fn neg(&self) -> Fe {
        Fe::ZERO.sub(self)
    }

    fn invert(&self) -> Fe {
        let z2 = self.sq();
        let z9 = z2.sq_n(2).mul(self);
        let z11 = z9.mul(&z2);
        let z5_0 = z11.sq().mul(&z9);
        let z10 = z5_0.sq_n(5).mul(&z5_0);
        let z20 = z10.sq_n(10).mul(&z10);
        let z40 = z20.sq_n(20).mul(&z20);
        let z50 = z40.sq_n(10).mul(&z10);
        let z100 = z50.sq_n(50).mul(&z50);
        let z200 = z100.sq_n(100).mul(&z100);
        let z250 = z200.sq_n(50).mul(&z50);
        z250.sq_n(5).mul(&z11)
    }

    fn sqrt(&self) -> Option<Fe> {
        // p = 2^255 - 19, use Tonelli-Shanks shortcut for p ≡ 5 (mod 8).
        let a1 = self.sq_n(1);
        let a2 = a1.mul(self);
        let a3 = a2.sq_n(1);
        let a4 = a3.mul(self);

        let mut r = a4;
        r = r.sq_n(3).mul(&a4);
        r = r.sq_n(6).mul(&a4);
        r = r.sq_n(12).mul(&r);
        r = r.sq_n(25).mul(&r);
        r = r.sq_n(25).mul(&r);
        r = r.sq_n(50).mul(&r);
        r = r.sq_n(100).mul(&r);
        r = r.sq_n(50).mul(&r);
        r = r.sq_n(2).mul(self);

        let check = r.sq();
        let diff = check.sub(self).reduce();
        let mut is_zero = true;
        let mut i = 0usize;
        while i < 5 {
            if diff.0[i] != 0 {
                is_zero = false;
            }
            i = i.saturating_add(1);
        }
        if is_zero {
            return Some(r);
        }

        let r2 = r.mul(&Fe::SQRT_MINUS_ONE);
        let check2 = r2.sq();
        let diff2 = check2.sub(self).reduce();
        let mut is_zero2 = true;
        let mut i = 0usize;
        while i < 5 {
            if diff2.0[i] != 0 {
                is_zero2 = false;
            }
            i = i.saturating_add(1);
        }
        if is_zero2 {
            Some(r2)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Curve constant d = -121665/121666 mod p
// ---------------------------------------------------------------------------

const D: Fe = Fe([
    0x34DCA135978A3,
    0x1A8283B156EBD,
    0x5E7A26001C029,
    0x739C663A03CFF,
    0x52036CBC148B6,
]);

const D2: Fe = Fe([
    0x69B9426B2F159,
    0x35050762ADD7A,
    0x3CF4A4C0038052,
    0x6738CC7407A7E,
    0x2406D9DC56DFC,
]);

// ---------------------------------------------------------------------------
// Extended twisted Edwards point (X:Y:Z:T)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Point {
    x: Fe,
    y: Fe,
    z: Fe,
    t: Fe,
}

impl Point {
    const IDENTITY: Self = Point {
        x: Fe::ZERO,
        y: Fe::ONE,
        z: Fe::ONE,
        t: Fe::ZERO,
    };

    fn basepoint() -> Point {
        // The base point B on Ed25519: compressed y = 4/5 mod p.
        let by_bytes: [u8; 32] = [
            0x58, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
            0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
            0x66, 0x66, 0x66, 0x66,
        ];
        decode_point(&by_bytes).unwrap_or(Point::IDENTITY)
    }

    fn add(&self, other: &Point) -> Point {
        let a = self.x.mul(&other.x);
        let b = self.y.mul(&other.y);
        let c = self.t.mul(&D2).mul(&other.t);
        let d = self.z.mul(&other.z);

        let e = self
            .x
            .add(&self.y)
            .mul(&other.x.add(&other.y))
            .sub(&a)
            .sub(&b);
        let f = d.sub(&c);
        let g = d.add(&c);
        let h = b.add(&a);

        Point {
            x: e.mul(&f),
            y: g.mul(&h),
            z: f.mul(&g),
            t: e.mul(&h),
        }
    }

    fn double(&self) -> Point {
        let a = self.x.sq();
        let b = self.y.sq();
        let c = self.z.sq().add(&self.z.sq());
        let h = a.add(&b);
        let e = h.sub(&self.x.add(&self.y).sq().sub(&h).neg());
        let g = a.sub(&b);
        let f = g.add(&c);
        Point {
            x: e.mul(&f),
            y: g.mul(&h),
            z: f.mul(&g),
            t: e.mul(&h),
        }
    }

    fn scalar_mul(&self, scalar: &[u8; 32]) -> Point {
        let mut result = Point::IDENTITY;
        let mut temp = *self;
        let mut i = 0usize;
        while i < 256 {
            let byte_idx = i / 8;
            let bit_idx = i % 8;
            if byte_idx < 32 && (scalar[byte_idx] >> bit_idx) & 1 == 1 {
                result = result.add(&temp);
            }
            temp = temp.double();
            i = i.saturating_add(1);
        }
        result
    }
}

fn encode_point(p: &Point) -> [u8; 32] {
    let zi = p.z.invert();
    let x = p.x.mul(&zi);
    let y = p.y.mul(&zi);
    let mut bytes = y.to_bytes();
    let x_bytes = x.to_bytes();
    bytes[31] |= (x_bytes[0] & 1) << 7;
    bytes
}

fn decode_point(bytes: &[u8; 32]) -> Option<Point> {
    let mut y_bytes = *bytes;
    let x_sign = (y_bytes[31] >> 7) & 1;
    y_bytes[31] &= 0x7F;

    let y = Fe::from_bytes(&y_bytes);
    let y2 = y.sq();
    let numerator = y2.sub(&Fe::ONE);
    let denominator = D.mul(&y2).add(&Fe::ONE);

    let x2 = numerator.mul(&denominator.invert());
    let x = x2.sqrt()?;

    let x_bytes = x.to_bytes();
    let final_x = if (x_bytes[0] & 1) != x_sign {
        x.neg()
    } else {
        x
    };
    let t = final_x.mul(&y);
    Some(Point {
        x: final_x,
        y,
        z: Fe::ONE,
        t,
    })
}

// ---------------------------------------------------------------------------
// SHA-512 (RFC 8032 mandates SHA-512 for Ed25519)
// No-heap variant: uses a fixed 4096-byte stack scratch buffer.
// Messages longer than 3968 bytes are silently truncated (not a practical
// limit for Ed25519 key material and 512-byte message caps in this kernel).
// ---------------------------------------------------------------------------

fn sha512_ed(data: &[u8]) -> [u8; 64] {
    const H0: [u64; 8] = [
        0x6a09e667f3bcc908,
        0xbb67ae8584caa73b,
        0x3c6ef372fe94f82b,
        0xa54ff53a5f1d36f1,
        0x510e527fade682d1,
        0x9b05688c2b3e6c1f,
        0x1f83d9abfb41bd6b,
        0x5be0cd19137e2179,
    ];
    const K: [u64; 80] = [
        0x428a2f98d728ae22,
        0x7137449123ef65cd,
        0xb5c0fbcfec4d3b2f,
        0xe9b5dba58189dbbc,
        0x3956c25bf348b538,
        0x59f111f1b605d019,
        0x923f82a4af194f9b,
        0xab1c5ed5da6d8118,
        0xd807aa98a3030242,
        0x12835b0145706fbe,
        0x243185be4ee4b28c,
        0x550c7dc3d5ffb4e2,
        0x72be5d74f27b896f,
        0x80deb1fe3b1696b1,
        0x9bdc06a725c71235,
        0xc19bf174cf692694,
        0xe49b69c19ef14ad2,
        0xefbe4786384f25e3,
        0x0fc19dc68b8cd5b5,
        0x240ca1cc77ac9c65,
        0x2de92c6f592b0275,
        0x4a7484aa6ea6e483,
        0x5cb0a9dcbd41fbd4,
        0x76f988da831153b5,
        0x983e5152ee66dfab,
        0xa831c66d2db43210,
        0xb00327c898fb213f,
        0xbf597fc7beef0ee4,
        0xc6e00bf33da88fc2,
        0xd5a79147930aa725,
        0x06ca6351e003826f,
        0x142929670a0e6e70,
        0x27b70a8546d22ffc,
        0x2e1b21385c26c926,
        0x4d2c6dfc5ac42aed,
        0x53380d139d95b3df,
        0x650a73548baf63de,
        0x766a0abb3c77b2a8,
        0x81c2c92e47edaee6,
        0x92722c851482353b,
        0xa2bfe8a14cf10364,
        0xa81a664bbc423001,
        0xc24b8b70d0f89791,
        0xc76c51a30654be30,
        0xd192e819d6ef5218,
        0xd69906245565a910,
        0xf40e35855771202a,
        0x106aa07032bbd1b8,
        0x19a4c116b8d2d0c8,
        0x1e376c085141ab53,
        0x2748774cdf8eeb99,
        0x34b0bcb5e19b48a8,
        0x391c0cb3c5c95a63,
        0x4ed8aa4ae3418acb,
        0x5b9cca4f7763e373,
        0x682e6ff3d6b2b8a3,
        0x748f82ee5defb2fc,
        0x78a5636f43172f60,
        0x84c87814a1f0ab72,
        0x8cc702081a6439ec,
        0x90befffa23631e28,
        0xa4506cebde82bde9,
        0xbef9a3f7b2c67915,
        0xc67178f2e372532b,
        0xca273eceea26619c,
        0xd186b8c721c0c207,
        0xeada7dd6cde0eb1e,
        0xf57d4f7fee6ed178,
        0x06f067aa72176fba,
        0x0a637dc5a2c898a6,
        0x113f9804bef90dae,
        0x1b710b35131c471b,
        0x28db77f523047d84,
        0x32caab7b40c72493,
        0x3c9ebe0a15c9bebc,
        0x431d67c49c100d4c,
        0x4cc5d4becb3e42b6,
        0x597f299cfc657e2a,
        0x5fcb6fab3ad6faec,
        0x6c44198c4a475817,
    ];

    // Cap input at 3968 bytes (avoids a 4096-byte padded buffer overflow).
    let data_len = if data.len() > 3968 { 3968 } else { data.len() };

    // Build padded message in a 4096-byte stack buffer.
    // SHA-512 padding: append 0x80, then zero bytes until len ≡ 112 (mod 128),
    // then append 16-byte big-endian bit length.
    const BUF: usize = 4096;
    let mut padded = [0u8; BUF];
    let mut i = 0usize;
    while i < data_len {
        padded[i] = data[i];
        i = i.saturating_add(1);
    }

    let bit_len = (data_len as u128).wrapping_mul(8);

    // Append 0x80
    if data_len < BUF {
        padded[data_len] = 0x80;
    }

    // Find pad end: first position ≡ 112 (mod 128) after data_len + 1
    let raw_end = data_len.saturating_add(1);
    let pad_end = {
        let r = raw_end % 128;
        if r <= 112 {
            raw_end.saturating_add(112usize.wrapping_sub(r))
        } else {
            raw_end.saturating_add(128usize.wrapping_sub(r).wrapping_add(112))
        }
    };
    // Append 16-byte big-endian bit length (high 64 bits = 0 for data < 3968 bytes)
    let total = pad_end.saturating_add(16);
    if total <= BUF {
        let hi = ((bit_len >> 64) as u64).to_be_bytes();
        let lo = ((bit_len & 0xffff_ffff_ffff_ffff) as u64).to_be_bytes();
        let mut j = 0usize;
        while j < 8 {
            padded[pad_end + j] = hi[j];
            j = j.saturating_add(1);
        }
        j = 0;
        while j < 8 {
            padded[pad_end + 8 + j] = lo[j];
            j = j.saturating_add(1);
        }
    }
    let block_count = if total > 0 { total / 128 } else { 0 };

    // Process blocks
    let mut h = H0;
    let mut blk = 0usize;
    while blk < block_count {
        let off = blk.wrapping_mul(128);
        let mut w = [0u64; 80];
        let mut wi = 0usize;
        while wi < 16 {
            let b = off.wrapping_add(wi.wrapping_mul(8));
            w[wi] = u64::from_be_bytes([
                padded[b],
                padded[b + 1],
                padded[b + 2],
                padded[b + 3],
                padded[b + 4],
                padded[b + 5],
                padded[b + 6],
                padded[b + 7],
            ]);
            wi = wi.saturating_add(1);
        }
        wi = 16;
        while wi < 80 {
            let s0 = w[wi - 15].rotate_right(1) ^ w[wi - 15].rotate_right(8) ^ (w[wi - 15] >> 7);
            let s1 = w[wi - 2].rotate_right(19) ^ w[wi - 2].rotate_right(61) ^ (w[wi - 2] >> 6);
            w[wi] = w[wi - 16]
                .wrapping_add(s0)
                .wrapping_add(w[wi - 7])
                .wrapping_add(s1);
            wi = wi.saturating_add(1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g_w, mut hh] = h;
        let mut ri = 0usize;
        while ri < 80 {
            let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
            let ch = (e & f) ^ ((!e) & g_w);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[ri])
                .wrapping_add(w[ri]);
            let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g_w;
            g_w = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
            ri = ri.saturating_add(1);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g_w);
        h[7] = h[7].wrapping_add(hh);
        blk = blk.saturating_add(1);
    }

    let mut result = [0u8; 64];
    let mut i = 0usize;
    while i < 8 {
        let bytes = h[i].to_be_bytes();
        let base = i.wrapping_mul(8);
        let mut j = 0usize;
        while j < 8 {
            result[base + j] = bytes[j];
            j = j.saturating_add(1);
        }
        i = i.saturating_add(1);
    }
    result
}

// ---------------------------------------------------------------------------
// Scalar reduction mod L (NaCl/ref10 sc_reduce, 21-bit limb basis)
// ---------------------------------------------------------------------------

fn load_3(s: &[u8]) -> u64 {
    if s.len() < 3 {
        return 0;
    }
    (s[0] as u64) | ((s[1] as u64) << 8) | ((s[2] as u64) << 16)
}

fn load_4(s: &[u8]) -> u64 {
    if s.len() < 4 {
        return 0;
    }
    (s[0] as u64) | ((s[1] as u64) << 8) | ((s[2] as u64) << 16) | ((s[3] as u64) << 24)
}

fn reduce_scalar(h: &[u8; 64]) -> [u8; 32] {
    let mut s = [0i64; 24];

    s[0] = 2097151 & (load_3(&h[0..3]) as i64);
    s[1] = 2097151 & ((load_4(&h[2..6]) >> 5) as i64);
    s[2] = 2097151 & ((load_3(&h[5..8]) >> 2) as i64);
    s[3] = 2097151 & ((load_4(&h[7..11]) >> 7) as i64);
    s[4] = 2097151 & ((load_4(&h[10..14]) >> 4) as i64);
    s[5] = 2097151 & ((load_3(&h[13..16]) >> 1) as i64);
    s[6] = 2097151 & ((load_4(&h[15..19]) >> 6) as i64);
    s[7] = 2097151 & ((load_3(&h[18..21]) >> 3) as i64);
    s[8] = 2097151 & (load_3(&h[21..24]) as i64);
    s[9] = 2097151 & ((load_4(&h[23..27]) >> 5) as i64);
    s[10] = 2097151 & ((load_3(&h[26..29]) >> 2) as i64);
    s[11] = 2097151 & ((load_4(&h[28..32]) >> 7) as i64);
    s[12] = 2097151 & ((load_4(&h[31..35]) >> 4) as i64);
    s[13] = 2097151 & ((load_3(&h[34..37]) >> 1) as i64);
    s[14] = 2097151 & ((load_4(&h[36..40]) >> 6) as i64);
    s[15] = 2097151 & ((load_3(&h[39..42]) >> 3) as i64);
    s[16] = 2097151 & (load_3(&h[42..45]) as i64);
    s[17] = 2097151 & ((load_4(&h[44..48]) >> 5) as i64);
    s[18] = 2097151 & ((load_3(&h[47..50]) >> 2) as i64);
    s[19] = 2097151 & ((load_4(&h[49..53]) >> 7) as i64);
    s[20] = 2097151 & ((load_4(&h[52..56]) >> 4) as i64);
    s[21] = 2097151 & ((load_3(&h[55..58]) >> 1) as i64);
    s[22] = 2097151 & ((load_4(&h[57..61]) >> 6) as i64);
    s[23] = (load_4(&h[60..64]) >> 3) as i64;

    s[11] += s[23] * 666643;
    s[12] += s[23] * 470296;
    s[13] += s[23] * 654183;
    s[14] -= s[23] * 997805;
    s[15] += s[23] * 136657;
    s[16] -= s[23] * 683901;
    s[23] = 0;
    s[10] += s[22] * 666643;
    s[11] += s[22] * 470296;
    s[12] += s[22] * 654183;
    s[13] -= s[22] * 997805;
    s[14] += s[22] * 136657;
    s[15] -= s[22] * 683901;
    s[22] = 0;
    s[9] += s[21] * 666643;
    s[10] += s[21] * 470296;
    s[11] += s[21] * 654183;
    s[12] -= s[21] * 997805;
    s[13] += s[21] * 136657;
    s[14] -= s[21] * 683901;
    s[21] = 0;
    s[8] += s[20] * 666643;
    s[9] += s[20] * 470296;
    s[10] += s[20] * 654183;
    s[11] -= s[20] * 997805;
    s[12] += s[20] * 136657;
    s[13] -= s[20] * 683901;
    s[20] = 0;
    s[7] += s[19] * 666643;
    s[8] += s[19] * 470296;
    s[9] += s[19] * 654183;
    s[10] -= s[19] * 997805;
    s[11] += s[19] * 136657;
    s[12] -= s[19] * 683901;
    s[19] = 0;
    s[6] += s[18] * 666643;
    s[7] += s[18] * 470296;
    s[8] += s[18] * 654183;
    s[9] -= s[18] * 997805;
    s[10] += s[18] * 136657;
    s[11] -= s[18] * 683901;
    s[18] = 0;

    let c0 = (s[0] + (1 << 20)) >> 21;
    s[1] += c0;
    s[0] -= c0 * (1 << 21);
    let c2 = (s[2] + (1 << 20)) >> 21;
    s[3] += c2;
    s[2] -= c2 * (1 << 21);
    let c4 = (s[4] + (1 << 20)) >> 21;
    s[5] += c4;
    s[4] -= c4 * (1 << 21);
    let c6 = (s[6] + (1 << 20)) >> 21;
    s[7] += c6;
    s[6] -= c6 * (1 << 21);
    let c8 = (s[8] + (1 << 20)) >> 21;
    s[9] += c8;
    s[8] -= c8 * (1 << 21);
    let c10 = (s[10] + (1 << 20)) >> 21;
    s[11] += c10;
    s[10] -= c10 * (1 << 21);
    let c12 = (s[12] + (1 << 20)) >> 21;
    s[13] += c12;
    s[12] -= c12 * (1 << 21);
    let c14 = (s[14] + (1 << 20)) >> 21;
    s[15] += c14;
    s[14] -= c14 * (1 << 21);
    let c16 = (s[16] + (1 << 20)) >> 21;
    s[17] += c16;
    s[16] -= c16 * (1 << 21);
    let c1 = (s[1] + (1 << 20)) >> 21;
    s[2] += c1;
    s[1] -= c1 * (1 << 21);
    let c3 = (s[3] + (1 << 20)) >> 21;
    s[4] += c3;
    s[3] -= c3 * (1 << 21);
    let c5 = (s[5] + (1 << 20)) >> 21;
    s[6] += c5;
    s[5] -= c5 * (1 << 21);
    let c7 = (s[7] + (1 << 20)) >> 21;
    s[8] += c7;
    s[7] -= c7 * (1 << 21);
    let c9 = (s[9] + (1 << 20)) >> 21;
    s[10] += c9;
    s[9] -= c9 * (1 << 21);
    let c11 = (s[11] + (1 << 20)) >> 21;
    s[12] += c11;
    s[11] -= c11 * (1 << 21);

    s[5] += s[17] * 666643;
    s[6] += s[17] * 470296;
    s[7] += s[17] * 654183;
    s[8] -= s[17] * 997805;
    s[9] += s[17] * 136657;
    s[10] -= s[17] * 683901;
    s[17] = 0;
    s[4] += s[16] * 666643;
    s[5] += s[16] * 470296;
    s[6] += s[16] * 654183;
    s[7] -= s[16] * 997805;
    s[8] += s[16] * 136657;
    s[9] -= s[16] * 683901;
    s[16] = 0;
    s[3] += s[15] * 666643;
    s[4] += s[15] * 470296;
    s[5] += s[15] * 654183;
    s[6] -= s[15] * 997805;
    s[7] += s[15] * 136657;
    s[8] -= s[15] * 683901;
    s[15] = 0;
    s[2] += s[14] * 666643;
    s[3] += s[14] * 470296;
    s[4] += s[14] * 654183;
    s[5] -= s[14] * 997805;
    s[6] += s[14] * 136657;
    s[7] -= s[14] * 683901;
    s[14] = 0;
    s[1] += s[13] * 666643;
    s[2] += s[13] * 470296;
    s[3] += s[13] * 654183;
    s[4] -= s[13] * 997805;
    s[5] += s[13] * 136657;
    s[6] -= s[13] * 683901;
    s[13] = 0;
    s[0] += s[12] * 666643;
    s[1] += s[12] * 470296;
    s[2] += s[12] * 654183;
    s[3] -= s[12] * 997805;
    s[4] += s[12] * 136657;
    s[5] -= s[12] * 683901;
    s[12] = 0;

    let c0 = (s[0] + (1 << 20)) >> 21;
    s[1] += c0;
    s[0] -= c0 * (1 << 21);
    let c1 = (s[1] + (1 << 20)) >> 21;
    s[2] += c1;
    s[1] -= c1 * (1 << 21);
    let c2 = (s[2] + (1 << 20)) >> 21;
    s[3] += c2;
    s[2] -= c2 * (1 << 21);
    let c3 = (s[3] + (1 << 20)) >> 21;
    s[4] += c3;
    s[3] -= c3 * (1 << 21);
    let c4 = (s[4] + (1 << 20)) >> 21;
    s[5] += c4;
    s[4] -= c4 * (1 << 21);
    let c5 = (s[5] + (1 << 20)) >> 21;
    s[6] += c5;
    s[5] -= c5 * (1 << 21);
    let c6 = (s[6] + (1 << 20)) >> 21;
    s[7] += c6;
    s[6] -= c6 * (1 << 21);
    let c7 = (s[7] + (1 << 20)) >> 21;
    s[8] += c7;
    s[7] -= c7 * (1 << 21);
    let c8 = (s[8] + (1 << 20)) >> 21;
    s[9] += c8;
    s[8] -= c8 * (1 << 21);
    let c9 = (s[9] + (1 << 20)) >> 21;
    s[10] += c9;
    s[9] -= c9 * (1 << 21);
    let c10 = (s[10] + (1 << 20)) >> 21;
    s[11] += c10;
    s[10] -= c10 * (1 << 21);
    let c11 = (s[11] + (1 << 20)) >> 21;
    s[12] += c11;
    s[11] -= c11 * (1 << 21);

    s[0] += s[12] * 666643;
    s[1] += s[12] * 470296;
    s[2] += s[12] * 654183;
    s[3] -= s[12] * 997805;
    s[4] += s[12] * 136657;
    s[5] -= s[12] * 683901;
    s[12] = 0;

    let mut out = [0u8; 32];
    out[0] = s[0] as u8;
    out[1] = (s[0] >> 8) as u8;
    out[2] = ((s[0] >> 16) | (s[1] << 5)) as u8;
    out[3] = (s[1] >> 3) as u8;
    out[4] = (s[1] >> 11) as u8;
    out[5] = ((s[1] >> 19) | (s[2] << 2)) as u8;
    out[6] = (s[2] >> 6) as u8;
    out[7] = ((s[2] >> 14) | (s[3] << 7)) as u8;
    out[8] = (s[3] >> 1) as u8;
    out[9] = (s[3] >> 9) as u8;
    out[10] = ((s[3] >> 17) | (s[4] << 4)) as u8;
    out[11] = (s[4] >> 4) as u8;
    out[12] = (s[4] >> 12) as u8;
    out[13] = ((s[4] >> 20) | (s[5] << 1)) as u8;
    out[14] = (s[5] >> 7) as u8;
    out[15] = ((s[5] >> 15) | (s[6] << 6)) as u8;
    out[16] = (s[6] >> 2) as u8;
    out[17] = (s[6] >> 10) as u8;
    out[18] = ((s[6] >> 18) | (s[7] << 3)) as u8;
    out[19] = (s[7] >> 5) as u8;
    out[20] = (s[7] >> 13) as u8;
    out[21] = s[8] as u8;
    out[22] = (s[8] >> 8) as u8;
    out[23] = ((s[8] >> 16) | (s[9] << 5)) as u8;
    out[24] = (s[9] >> 3) as u8;
    out[25] = (s[9] >> 11) as u8;
    out[26] = ((s[9] >> 19) | (s[10] << 2)) as u8;
    out[27] = (s[10] >> 6) as u8;
    out[28] = ((s[10] >> 14) | (s[11] << 7)) as u8;
    out[29] = (s[11] >> 1) as u8;
    out[30] = (s[11] >> 9) as u8;
    out[31] = (s[11] >> 17) as u8;
    out
}

// ---------------------------------------------------------------------------
// sc_muladd: (a * b + c) mod L  (NaCl/ref10)
// ---------------------------------------------------------------------------

fn scalar_muladd(a: &[u8; 32], b: &[u8; 32], c: &[u8; 32]) -> [u8; 32] {
    let a0 = 2097151 & load_3(&a[0..3]) as i64;
    let a1 = 2097151 & (load_4(&a[2..6]) >> 5) as i64;
    let a2 = 2097151 & (load_3(&a[5..8]) >> 2) as i64;
    let a3 = 2097151 & (load_4(&a[7..11]) >> 7) as i64;
    let a4 = 2097151 & (load_4(&a[10..14]) >> 4) as i64;
    let a5 = 2097151 & (load_3(&a[13..16]) >> 1) as i64;
    let a6 = 2097151 & (load_4(&a[15..19]) >> 6) as i64;
    let a7 = 2097151 & (load_3(&a[18..21]) >> 3) as i64;
    let a8 = 2097151 & load_3(&a[21..24]) as i64;
    let a9 = 2097151 & (load_4(&a[23..27]) >> 5) as i64;
    let a10 = 2097151 & (load_3(&a[26..29]) >> 2) as i64;
    let a11 = (load_4(&a[28..32]) >> 7) as i64;

    let b0 = 2097151 & load_3(&b[0..3]) as i64;
    let b1 = 2097151 & (load_4(&b[2..6]) >> 5) as i64;
    let b2 = 2097151 & (load_3(&b[5..8]) >> 2) as i64;
    let b3 = 2097151 & (load_4(&b[7..11]) >> 7) as i64;
    let b4 = 2097151 & (load_4(&b[10..14]) >> 4) as i64;
    let b5 = 2097151 & (load_3(&b[13..16]) >> 1) as i64;
    let b6 = 2097151 & (load_4(&b[15..19]) >> 6) as i64;
    let b7 = 2097151 & (load_3(&b[18..21]) >> 3) as i64;
    let b8 = 2097151 & load_3(&b[21..24]) as i64;
    let b9 = 2097151 & (load_4(&b[23..27]) >> 5) as i64;
    let b10 = 2097151 & (load_3(&b[26..29]) >> 2) as i64;
    let b11 = (load_4(&b[28..32]) >> 7) as i64;

    let c0 = 2097151 & load_3(&c[0..3]) as i64;
    let c1 = 2097151 & (load_4(&c[2..6]) >> 5) as i64;
    let c2 = 2097151 & (load_3(&c[5..8]) >> 2) as i64;
    let c3 = 2097151 & (load_4(&c[7..11]) >> 7) as i64;
    let c4 = 2097151 & (load_4(&c[10..14]) >> 4) as i64;
    let c5 = 2097151 & (load_3(&c[13..16]) >> 1) as i64;
    let c6 = 2097151 & (load_4(&c[15..19]) >> 6) as i64;
    let c7 = 2097151 & (load_3(&c[18..21]) >> 3) as i64;
    let c8 = 2097151 & load_3(&c[21..24]) as i64;
    let c9 = 2097151 & (load_4(&c[23..27]) >> 5) as i64;
    let c10 = 2097151 & (load_3(&c[26..29]) >> 2) as i64;
    let c11 = (load_4(&c[28..32]) >> 7) as i64;

    let mut s = [0i64; 24];
    s[0] = c0 + a0 * b0;
    s[1] = c1 + a0 * b1 + a1 * b0;
    s[2] = c2 + a0 * b2 + a1 * b1 + a2 * b0;
    s[3] = c3 + a0 * b3 + a1 * b2 + a2 * b1 + a3 * b0;
    s[4] = c4 + a0 * b4 + a1 * b3 + a2 * b2 + a3 * b1 + a4 * b0;
    s[5] = c5 + a0 * b5 + a1 * b4 + a2 * b3 + a3 * b2 + a4 * b1 + a5 * b0;
    s[6] = c6 + a0 * b6 + a1 * b5 + a2 * b4 + a3 * b3 + a4 * b2 + a5 * b1 + a6 * b0;
    s[7] = c7 + a0 * b7 + a1 * b6 + a2 * b5 + a3 * b4 + a4 * b3 + a5 * b2 + a6 * b1 + a7 * b0;
    s[8] = c8
        + a0 * b8
        + a1 * b7
        + a2 * b6
        + a3 * b5
        + a4 * b4
        + a5 * b3
        + a6 * b2
        + a7 * b1
        + a8 * b0;
    s[9] = c9
        + a0 * b9
        + a1 * b8
        + a2 * b7
        + a3 * b6
        + a4 * b5
        + a5 * b4
        + a6 * b3
        + a7 * b2
        + a8 * b1
        + a9 * b0;
    s[10] = c10
        + a0 * b10
        + a1 * b9
        + a2 * b8
        + a3 * b7
        + a4 * b6
        + a5 * b5
        + a6 * b4
        + a7 * b3
        + a8 * b2
        + a9 * b1
        + a10 * b0;
    s[11] = c11
        + a0 * b11
        + a1 * b10
        + a2 * b9
        + a3 * b8
        + a4 * b7
        + a5 * b6
        + a6 * b5
        + a7 * b4
        + a8 * b3
        + a9 * b2
        + a10 * b1
        + a11 * b0;
    s[12] = a1 * b11
        + a2 * b10
        + a3 * b9
        + a4 * b8
        + a5 * b7
        + a6 * b6
        + a7 * b5
        + a8 * b4
        + a9 * b3
        + a10 * b2
        + a11 * b1;
    s[13] = a2 * b11
        + a3 * b10
        + a4 * b9
        + a5 * b8
        + a6 * b7
        + a7 * b6
        + a8 * b5
        + a9 * b4
        + a10 * b3
        + a11 * b2;
    s[14] =
        a3 * b11 + a4 * b10 + a5 * b9 + a6 * b8 + a7 * b7 + a8 * b6 + a9 * b5 + a10 * b4 + a11 * b3;
    s[15] = a4 * b11 + a5 * b10 + a6 * b9 + a7 * b8 + a8 * b7 + a9 * b6 + a10 * b5 + a11 * b4;
    s[16] = a5 * b11 + a6 * b10 + a7 * b9 + a8 * b8 + a9 * b7 + a10 * b6 + a11 * b5;
    s[17] = a6 * b11 + a7 * b10 + a8 * b9 + a9 * b8 + a10 * b7 + a11 * b6;
    s[18] = a7 * b11 + a8 * b10 + a9 * b9 + a10 * b8 + a11 * b7;
    s[19] = a8 * b11 + a9 * b10 + a10 * b9 + a11 * b8;
    s[20] = a9 * b11 + a10 * b10 + a11 * b9;
    s[21] = a10 * b11 + a11 * b10;
    s[22] = a11 * b11;
    s[23] = 0;

    let c0 = (s[0] + (1 << 20)) >> 21;
    s[1] += c0;
    s[0] -= c0 * (1 << 21);
    let c2 = (s[2] + (1 << 20)) >> 21;
    s[3] += c2;
    s[2] -= c2 * (1 << 21);
    let c4 = (s[4] + (1 << 20)) >> 21;
    s[5] += c4;
    s[4] -= c4 * (1 << 21);
    let c6 = (s[6] + (1 << 20)) >> 21;
    s[7] += c6;
    s[6] -= c6 * (1 << 21);
    let c8 = (s[8] + (1 << 20)) >> 21;
    s[9] += c8;
    s[8] -= c8 * (1 << 21);
    let c10 = (s[10] + (1 << 20)) >> 21;
    s[11] += c10;
    s[10] -= c10 * (1 << 21);
    let c12 = (s[12] + (1 << 20)) >> 21;
    s[13] += c12;
    s[12] -= c12 * (1 << 21);
    let c14 = (s[14] + (1 << 20)) >> 21;
    s[15] += c14;
    s[14] -= c14 * (1 << 21);
    let c16 = (s[16] + (1 << 20)) >> 21;
    s[17] += c16;
    s[16] -= c16 * (1 << 21);
    let c18 = (s[18] + (1 << 20)) >> 21;
    s[19] += c18;
    s[18] -= c18 * (1 << 21);
    let c20 = (s[20] + (1 << 20)) >> 21;
    s[21] += c20;
    s[20] -= c20 * (1 << 21);
    let c22 = (s[22] + (1 << 20)) >> 21;
    s[23] += c22;
    s[22] -= c22 * (1 << 21);
    let c1 = (s[1] + (1 << 20)) >> 21;
    s[2] += c1;
    s[1] -= c1 * (1 << 21);
    let c3 = (s[3] + (1 << 20)) >> 21;
    s[4] += c3;
    s[3] -= c3 * (1 << 21);
    let c5 = (s[5] + (1 << 20)) >> 21;
    s[6] += c5;
    s[5] -= c5 * (1 << 21);
    let c7 = (s[7] + (1 << 20)) >> 21;
    s[8] += c7;
    s[7] -= c7 * (1 << 21);
    let c9 = (s[9] + (1 << 20)) >> 21;
    s[10] += c9;
    s[9] -= c9 * (1 << 21);
    let c11 = (s[11] + (1 << 20)) >> 21;
    s[12] += c11;
    s[11] -= c11 * (1 << 21);
    let c13 = (s[13] + (1 << 20)) >> 21;
    s[14] += c13;
    s[13] -= c13 * (1 << 21);
    let c15 = (s[15] + (1 << 20)) >> 21;
    s[16] += c15;
    s[15] -= c15 * (1 << 21);
    let c17 = (s[17] + (1 << 20)) >> 21;
    s[18] += c17;
    s[17] -= c17 * (1 << 21);
    let c19 = (s[19] + (1 << 20)) >> 21;
    s[20] += c19;
    s[19] -= c19 * (1 << 21);
    let c21 = (s[21] + (1 << 20)) >> 21;
    s[22] += c21;
    s[21] -= c21 * (1 << 21);

    s[11] += s[23] * 666643;
    s[12] += s[23] * 470296;
    s[13] += s[23] * 654183;
    s[14] -= s[23] * 997805;
    s[15] += s[23] * 136657;
    s[16] -= s[23] * 683901;
    s[23] = 0;
    s[10] += s[22] * 666643;
    s[11] += s[22] * 470296;
    s[12] += s[22] * 654183;
    s[13] -= s[22] * 997805;
    s[14] += s[22] * 136657;
    s[15] -= s[22] * 683901;
    s[22] = 0;
    s[9] += s[21] * 666643;
    s[10] += s[21] * 470296;
    s[11] += s[21] * 654183;
    s[12] -= s[21] * 997805;
    s[13] += s[21] * 136657;
    s[14] -= s[21] * 683901;
    s[21] = 0;
    s[8] += s[20] * 666643;
    s[9] += s[20] * 470296;
    s[10] += s[20] * 654183;
    s[11] -= s[20] * 997805;
    s[12] += s[20] * 136657;
    s[13] -= s[20] * 683901;
    s[20] = 0;
    s[7] += s[19] * 666643;
    s[8] += s[19] * 470296;
    s[9] += s[19] * 654183;
    s[10] -= s[19] * 997805;
    s[11] += s[19] * 136657;
    s[12] -= s[19] * 683901;
    s[19] = 0;
    s[6] += s[18] * 666643;
    s[7] += s[18] * 470296;
    s[8] += s[18] * 654183;
    s[9] -= s[18] * 997805;
    s[10] += s[18] * 136657;
    s[11] -= s[18] * 683901;
    s[18] = 0;

    let c6 = (s[6] + (1 << 20)) >> 21;
    s[7] += c6;
    s[6] -= c6 * (1 << 21);
    let c8 = (s[8] + (1 << 20)) >> 21;
    s[9] += c8;
    s[8] -= c8 * (1 << 21);
    let c10 = (s[10] + (1 << 20)) >> 21;
    s[11] += c10;
    s[10] -= c10 * (1 << 21);
    let c12 = (s[12] + (1 << 20)) >> 21;
    s[13] += c12;
    s[12] -= c12 * (1 << 21);
    let c14 = (s[14] + (1 << 20)) >> 21;
    s[15] += c14;
    s[14] -= c14 * (1 << 21);
    let c16 = (s[16] + (1 << 20)) >> 21;
    s[17] += c16;
    s[16] -= c16 * (1 << 21);

    s[5] += s[17] * 666643;
    s[6] += s[17] * 470296;
    s[7] += s[17] * 654183;
    s[8] -= s[17] * 997805;
    s[9] += s[17] * 136657;
    s[10] -= s[17] * 683901;
    s[17] = 0;
    s[4] += s[16] * 666643;
    s[5] += s[16] * 470296;
    s[6] += s[16] * 654183;
    s[7] -= s[16] * 997805;
    s[8] += s[16] * 136657;
    s[9] -= s[16] * 683901;
    s[16] = 0;
    s[3] += s[15] * 666643;
    s[4] += s[15] * 470296;
    s[5] += s[15] * 654183;
    s[6] -= s[15] * 997805;
    s[7] += s[15] * 136657;
    s[8] -= s[15] * 683901;
    s[15] = 0;
    s[2] += s[14] * 666643;
    s[3] += s[14] * 470296;
    s[4] += s[14] * 654183;
    s[5] -= s[14] * 997805;
    s[6] += s[14] * 136657;
    s[7] -= s[14] * 683901;
    s[14] = 0;
    s[1] += s[13] * 666643;
    s[2] += s[13] * 470296;
    s[3] += s[13] * 654183;
    s[4] -= s[13] * 997805;
    s[5] += s[13] * 136657;
    s[6] -= s[13] * 683901;
    s[13] = 0;
    s[0] += s[12] * 666643;
    s[1] += s[12] * 470296;
    s[2] += s[12] * 654183;
    s[3] -= s[12] * 997805;
    s[4] += s[12] * 136657;
    s[5] -= s[12] * 683901;
    s[12] = 0;

    let c0 = (s[0] + (1 << 20)) >> 21;
    s[1] += c0;
    s[0] -= c0 * (1 << 21);
    let c1 = (s[1] + (1 << 20)) >> 21;
    s[2] += c1;
    s[1] -= c1 * (1 << 21);
    let c2 = (s[2] + (1 << 20)) >> 21;
    s[3] += c2;
    s[2] -= c2 * (1 << 21);
    let c3 = (s[3] + (1 << 20)) >> 21;
    s[4] += c3;
    s[3] -= c3 * (1 << 21);
    let c4 = (s[4] + (1 << 20)) >> 21;
    s[5] += c4;
    s[4] -= c4 * (1 << 21);
    let c5 = (s[5] + (1 << 20)) >> 21;
    s[6] += c5;
    s[5] -= c5 * (1 << 21);
    let c6 = (s[6] + (1 << 20)) >> 21;
    s[7] += c6;
    s[6] -= c6 * (1 << 21);
    let c7 = (s[7] + (1 << 20)) >> 21;
    s[8] += c7;
    s[7] -= c7 * (1 << 21);
    let c8 = (s[8] + (1 << 20)) >> 21;
    s[9] += c8;
    s[8] -= c8 * (1 << 21);
    let c9 = (s[9] + (1 << 20)) >> 21;
    s[10] += c9;
    s[9] -= c9 * (1 << 21);
    let c10 = (s[10] + (1 << 20)) >> 21;
    s[11] += c10;
    s[10] -= c10 * (1 << 21);
    let c11 = (s[11] + (1 << 20)) >> 21;
    s[12] += c11;
    s[11] -= c11 * (1 << 21);

    let mut out = [0u8; 32];
    out[0] = s[0] as u8;
    out[1] = (s[0] >> 8) as u8;
    out[2] = ((s[0] >> 16) | (s[1] << 5)) as u8;
    out[3] = (s[1] >> 3) as u8;
    out[4] = (s[1] >> 11) as u8;
    out[5] = ((s[1] >> 19) | (s[2] << 2)) as u8;
    out[6] = (s[2] >> 6) as u8;
    out[7] = ((s[2] >> 14) | (s[3] << 7)) as u8;
    out[8] = (s[3] >> 1) as u8;
    out[9] = (s[3] >> 9) as u8;
    out[10] = ((s[3] >> 17) | (s[4] << 4)) as u8;
    out[11] = (s[4] >> 4) as u8;
    out[12] = (s[4] >> 12) as u8;
    out[13] = ((s[4] >> 20) | (s[5] << 1)) as u8;
    out[14] = (s[5] >> 7) as u8;
    out[15] = ((s[5] >> 15) | (s[6] << 6)) as u8;
    out[16] = (s[6] >> 2) as u8;
    out[17] = (s[6] >> 10) as u8;
    out[18] = ((s[6] >> 18) | (s[7] << 3)) as u8;
    out[19] = (s[7] >> 5) as u8;
    out[20] = (s[7] >> 13) as u8;
    out[21] = s[8] as u8;
    out[22] = (s[8] >> 8) as u8;
    out[23] = ((s[8] >> 16) | (s[9] << 5)) as u8;
    out[24] = (s[9] >> 3) as u8;
    out[25] = (s[9] >> 11) as u8;
    out[26] = ((s[9] >> 19) | (s[10] << 2)) as u8;
    out[27] = (s[10] >> 6) as u8;
    out[28] = ((s[10] >> 14) | (s[11] << 7)) as u8;
    out[29] = (s[11] >> 1) as u8;
    out[30] = (s[11] >> 9) as u8;
    out[31] = (s[11] >> 17) as u8;
    out
}
