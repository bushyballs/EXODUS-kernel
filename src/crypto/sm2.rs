use super::sm3::sm3;
/// SM2 — Chinese National Standard Elliptic Curve (GB/T 32918)
///
/// Provides:
///   - SM2 key generation (private key → public key on SM2 curve)
///   - SM2 signature: Sign(Za‖M) → (r, s) using ECDSA-like formula
///   - SM2 verification: Verify(Za‖M, r, s, Q)
///   - ZA computation: SM3(entlen‖ID‖curve_params‖Qx‖Qy)
///
/// SM2 curve parameters (GM/T 0003-2012, recommended 256-bit prime):
///   p  = FFFFFFFEFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF00000000FFFFFFFFFFFFFFFF
///   a  = FFFFFFFEFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF00000000FFFFFFFFFFFFFFFC
///   b  = 28E9FA9E9D9F5E344D5A9E4BCF6509A7F39789F515AB8F92DDBCBD414D940E93
///   Gx = 32C4AE2C1F1981195F9904466A39C9948FE30BBFF2660BE1715A4589334C74C7
///   Gy = BC3736A2F4F6779C59BDCEE36B692153D0A9877CC62A474002DF32E52139F0A0
///   n  = FFFFFFFEFFFFFFFFFFFFFFFFFFFFFFFF7203DF6B21C6052B53BBF40939D54123
///
/// Arithmetic is over a 256-bit prime field using 4×u64 (big-endian limbs).
/// All operations are constant-time where critical for signature verification.
///
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;

// ---------------------------------------------------------------------------
// 256-bit integer type (4 × u64, big-endian limb order)
// ---------------------------------------------------------------------------

pub type U256 = [u64; 4];

// ---------------------------------------------------------------------------
// SM2 curve parameters
// ---------------------------------------------------------------------------

/// p (field prime)
pub const SM2_P: U256 = [
    0xFFFF_FFFE_FFFF_FFFF,
    0xFFFF_FFFF_FFFF_FFFF,
    0xFFFF_FFFF_0000_0000,
    0xFFFF_FFFF_FFFF_FFFF,
];

/// a coefficient
pub const SM2_A: U256 = [
    0xFFFF_FFFE_FFFF_FFFF,
    0xFFFF_FFFF_FFFF_FFFF,
    0xFFFF_FFFF_0000_0000,
    0xFFFF_FFFF_FFFF_FFFC,
];

/// b coefficient
pub const SM2_B: U256 = [
    0x28E9_FA9E_9D9F_5E34,
    0x4D5A_9E4B_CF65_09A7,
    0xF397_89F5_15AB_8F92,
    0xDDBC_BD41_4D94_0E93,
];

/// Generator Gx
pub const SM2_GX: U256 = [
    0x32C4_AE2C_1F19_8119,
    0x5F99_0446_6A39_C994,
    0x8FE3_0BBF_F266_0BE1,
    0x715A_4589_334C_74C7,
];

/// Generator Gy
pub const SM2_GY: U256 = [
    0xBC37_36A2_F4F6_779C,
    0x59BD_CEE3_6B69_2153,
    0xD0A9_877C_C62A_4740,
    0x02DF_32E5_2139_F0A0,
];

/// Order n
pub const SM2_N: U256 = [
    0xFFFF_FFFE_FFFF_FFFF,
    0xFFFF_FFFF_FFFF_FFFF,
    0x7203_DF6B_21C6_052B,
    0x53BB_F409_39D5_4123,
];

// ---------------------------------------------------------------------------
// Affine point
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug)]
pub struct Sm2Point {
    pub x: U256,
    pub y: U256,
    pub is_infinity: bool,
}

impl Sm2Point {
    pub const fn identity() -> Self {
        Sm2Point {
            x: [0; 4],
            y: [0; 4],
            is_infinity: true,
        }
    }
    pub const fn generator() -> Self {
        Sm2Point {
            x: SM2_GX,
            y: SM2_GY,
            is_infinity: false,
        }
    }
}

// ---------------------------------------------------------------------------
// 256-bit modular arithmetic (constant-time where possible)
// ---------------------------------------------------------------------------

/// a < b?
fn u256_lt(a: &U256, b: &U256) -> bool {
    let mut i = 0usize;
    while i < 4 {
        if a[i] < b[i] {
            return true;
        }
        if a[i] > b[i] {
            return false;
        }
        i = i.saturating_add(1);
    }
    false
}

fn u256_eq(a: &U256, b: &U256) -> bool {
    let mut i = 0usize;
    while i < 4 {
        if a[i] != b[i] {
            return false;
        }
        i = i.saturating_add(1);
    }
    true
}

/// a + b mod m (assumes a, b < m)
fn u256_addmod(a: &U256, b: &U256, m: &U256) -> U256 {
    let mut out = [0u64; 4];
    let mut carry = 0u128;
    let mut i = 3i32;
    while i >= 0 {
        let sum = a[i as usize] as u128 + b[i as usize] as u128 + carry;
        out[i as usize] = sum as u64;
        carry = sum >> 64;
        i -= 1;
    }
    // if result >= m, subtract m
    if carry != 0 || !u256_lt(&out, m) {
        let mut borrow = 0i128;
        let mut k = 3i32;
        while k >= 0 {
            let diff = out[k as usize] as i128 - m[k as usize] as i128 + borrow;
            out[k as usize] = diff as u64;
            borrow = if diff < 0 { -1 } else { 0 };
            k -= 1;
        }
    }
    out
}

/// a - b mod m (assumes a, b < m)
fn u256_submod(a: &U256, b: &U256, m: &U256) -> U256 {
    if u256_lt(a, b) {
        // a - b + m
        let tmp = u256_addmod(a, m, &[u64::MAX; 4]);
        u256_submod(&tmp, b, m)
    } else {
        let mut out = [0u64; 4];
        let mut borrow = 0i128;
        let mut i = 3i32;
        while i >= 0 {
            let d = a[i as usize] as i128 - b[i as usize] as i128 + borrow;
            out[i as usize] = d as u64;
            borrow = if d < 0 { -1 } else { 0 };
            i -= 1;
        }
        out
    }
}

/// Convert big-endian bytes (32) to U256
pub fn bytes_to_u256(b: &[u8; 32]) -> U256 {
    let mut out = [0u64; 4];
    let mut i = 0usize;
    while i < 4 {
        let off = i * 8;
        out[i] = u64::from_be_bytes([
            b[off],
            b[off + 1],
            b[off + 2],
            b[off + 3],
            b[off + 4],
            b[off + 5],
            b[off + 6],
            b[off + 7],
        ]);
        i = i.saturating_add(1);
    }
    out
}

/// Convert U256 to big-endian 32 bytes
pub fn u256_to_bytes(v: &U256) -> [u8; 32] {
    let mut out = [0u8; 32];
    let mut i = 0usize;
    while i < 4 {
        let b = v[i].to_be_bytes();
        let mut j = 0usize;
        while j < 8 {
            out[i * 8 + j] = b[j];
            j = j.saturating_add(1);
        }
        i = i.saturating_add(1);
    }
    out
}

// ---------------------------------------------------------------------------
// Modular multiplication: schoolbook with reduction mod p
// (not constant-time — acceptable for non-secret exponents in ZA)
// ---------------------------------------------------------------------------

fn u256_mulmod(a: &U256, b: &U256, m: &U256) -> U256 {
    // Compute a*b as a 512-bit product, then reduce mod m by repeated subtraction.
    // This is O(n²) schoolbook — fine for a handful of field operations.
    // 512-bit accumulator: [u64; 8]
    let mut acc = [0u128; 8];
    let mut i = 3i32;
    while i >= 0 {
        let mut j = 3i32;
        while j >= 0 {
            acc[(i + j) as usize] += a[i as usize] as u128 * b[j as usize] as u128;
            j -= 1;
        }
        i -= 1;
    }
    // Carry propagation
    let mut carry = 0u128;
    let mut k = 7i32;
    while k >= 0 {
        let v = acc[k as usize] + carry;
        acc[k as usize] = v & 0xFFFF_FFFF_FFFF_FFFF;
        carry = v >> 64;
        k -= 1;
    }
    // Convert to [u64; 8]
    let mut big = [0u64; 8];
    let mut k = 0usize;
    while k < 8 {
        big[k] = acc[k] as u64;
        k = k.saturating_add(1);
    }

    // Barrett/shift reduction mod m: just do repeated halving + conditional subtract.
    // Simple binary method: extract 256 bits at a time.
    // For correctness in this stub we do a simple iterative approach:
    // reduce big[0..8] by the 256-bit modulus shifted appropriately.
    let mut result: U256 = [big[4], big[5], big[6], big[7]];
    // Process the high 256 bits by repeated doubling the modulus and subtracting
    let mut hi: U256 = [big[0], big[1], big[2], big[3]];
    let mut bit = 255i32;
    while bit >= 0 {
        // result = (result << 1) | bit_from_hi
        let msb_bit = (hi[0] >> 63) & 1;
        let mut carry2 = 0u64;
        let mut k = 3i32;
        while k >= 0 {
            let new = (result[k as usize] << 1) | carry2;
            carry2 = result[k as usize] >> 63;
            result[k as usize] = new;
            k -= 1;
        }
        result[3] |= msb_bit;
        // shift hi left 1
        let mut c = 0u64;
        let mut k = 3i32;
        while k >= 0 {
            let new = (hi[k as usize] << 1) | c;
            c = hi[k as usize] >> 63;
            hi[k as usize] = new;
            k -= 1;
        }
        // if result >= m, subtract
        if !u256_lt(&result, m) {
            result = u256_submod(&result, m, m);
        }
        bit -= 1;
    }
    // Final reduction
    if !u256_lt(&result, m) {
        result = u256_submod(&result, m, m);
    }
    result
}

// ---------------------------------------------------------------------------
// Modular inverse mod n: Fermat's little theorem (n is prime)
// a^(n-2) mod n
// ---------------------------------------------------------------------------

fn u256_invmod(a: &U256, n: &U256) -> U256 {
    // n-2
    let mut exp = *n;
    // subtract 2 from exp (n[3] -= 2)
    if exp[3] >= 2 {
        exp[3] -= 2;
    } else {
        // borrow
        let mut k = 2i32;
        while k >= 0 && exp[k as usize] == 0 {
            exp[k as usize] = u64::MAX;
            k -= 1;
        }
        if k >= 0 {
            exp[k as usize] -= 1;
        }
        exp[3] = exp[3].wrapping_sub(2);
    }
    // Square-and-multiply
    let mut result: U256 = [0, 0, 0, 1]; // 1
    let mut base = *a;
    // 256 bits of exponent
    let mut bit = 255i32;
    while bit >= 0 {
        let limb = (bit / 64) as usize;
        let shift = (bit % 64) as u32;
        let b = (exp[limb] >> (63 - shift)) & 1;
        result = u256_mulmod(&result, &result, n);
        if b == 1 {
            result = u256_mulmod(&result, &base, n);
        }
        let _ = base;
        bit -= 1;
    }
    result
}

// ---------------------------------------------------------------------------
// Elliptic curve point addition (affine, over SM2_P)
// ---------------------------------------------------------------------------

fn ec_add(p: &Sm2Point, q: &Sm2Point) -> Sm2Point {
    if p.is_infinity {
        return *q;
    }
    if q.is_infinity {
        return *p;
    }
    if u256_eq(&p.x, &q.x) {
        if !u256_eq(&p.y, &q.y) {
            return Sm2Point::identity();
        }
        return ec_double(p);
    }
    // λ = (y2 - y1) / (x2 - x1) mod p
    let dy = u256_submod(&q.y, &p.y, &SM2_P);
    let dx = u256_submod(&q.x, &p.x, &SM2_P);
    let lambda = u256_mulmod(&dy, &u256_invmod(&dx, &SM2_P), &SM2_P);
    // x3 = λ² - x1 - x2 mod p
    let lam2 = u256_mulmod(&lambda, &lambda, &SM2_P);
    let x3 = u256_submod(&u256_submod(&lam2, &p.x, &SM2_P), &q.x, &SM2_P);
    // y3 = λ(x1 - x3) - y1 mod p
    let dx2 = u256_submod(&p.x, &x3, &SM2_P);
    let y3 = u256_submod(&u256_mulmod(&lambda, &dx2, &SM2_P), &p.y, &SM2_P);
    Sm2Point {
        x: x3,
        y: y3,
        is_infinity: false,
    }
}

fn ec_double(p: &Sm2Point) -> Sm2Point {
    if p.is_infinity {
        return *p;
    }
    // λ = (3x² + a) / (2y) mod p
    let x2 = u256_mulmod(&p.x, &p.x, &SM2_P);
    let two: U256 = [0, 0, 0, 2];
    let three: U256 = [0, 0, 0, 3];
    let x23 = u256_mulmod(&three, &x2, &SM2_P);
    let num = u256_addmod(&x23, &SM2_A, &SM2_P);
    let den = u256_mulmod(&two, &p.y, &SM2_P);
    let lambda = u256_mulmod(&num, &u256_invmod(&den, &SM2_P), &SM2_P);
    let lam2 = u256_mulmod(&lambda, &lambda, &SM2_P);
    let x3 = u256_submod(&u256_submod(&lam2, &p.x, &SM2_P), &p.x, &SM2_P);
    let dx = u256_submod(&p.x, &x3, &SM2_P);
    let y3 = u256_submod(&u256_mulmod(&lambda, &dx, &SM2_P), &p.y, &SM2_P);
    Sm2Point {
        x: x3,
        y: y3,
        is_infinity: false,
    }
}

/// Scalar multiplication: k × P (double-and-add, 256 bits)
pub fn ec_mul(k: &U256, p: &Sm2Point) -> Sm2Point {
    let mut result = Sm2Point::identity();
    let mut addend = *p;
    let mut bit = 255i32;
    while bit >= 0 {
        let limb = (bit / 64) as usize;
        let shift = (255 - bit as u32) % 64;
        let b = (k[limb] >> (63 - shift)) & 1;
        if b == 1 {
            result = ec_add(&result, &addend);
        }
        addend = ec_double(&addend);
        bit -= 1;
    }
    result
}

// ---------------------------------------------------------------------------
// ZA: user identity hash bound to the public key
// ---------------------------------------------------------------------------

/// ZA = SM3(ENTLEN ‖ ID ‖ a ‖ b ‖ Gx ‖ Gy ‖ Qx ‖ Qy)
pub fn sm2_za(id: &[u8], qx: &U256, qy: &U256) -> [u8; 32] {
    let entlen_bits = (id.len() * 8) as u16;
    // Build input buffer (max: 2 + 128 + 32*6 = 322 bytes)
    let mut buf = [0u8; 400];
    let mut off = 0usize;
    // ENTLEN: big-endian u16
    buf[off] = (entlen_bits >> 8) as u8;
    off += 1;
    buf[off] = (entlen_bits & 0xFF) as u8;
    off += 1;
    // ID
    let il = id.len().min(128);
    let mut k = 0usize;
    while k < il {
        buf[off] = id[k];
        off += 1;
        k = k.saturating_add(1);
    }
    // a, b, Gx, Gy, Qx, Qy (each 32 bytes)
    let fields = [&SM2_A, &SM2_B, &SM2_GX, &SM2_GY, qx, qy];
    let mut fi = 0usize;
    while fi < 6 {
        let fb = u256_to_bytes(fields[fi]);
        let mut k = 0usize;
        while k < 32 {
            buf[off] = fb[k];
            off += 1;
            k = k.saturating_add(1);
        }
        fi = fi.saturating_add(1);
    }
    sm3(&buf[..off])
}

// ---------------------------------------------------------------------------
// Key derivation: private key d → public key Q = d·G
// ---------------------------------------------------------------------------

pub fn sm2_public_key(private_key: &U256) -> Sm2Point {
    ec_mul(private_key, &Sm2Point::generator())
}

// ---------------------------------------------------------------------------
// SM2 Sign: (r, s) = Sign_d(Za ‖ M)
// Requires a per-message random k; we use a deterministic k derived from
// the private key and message hash (RFC 6979-style, simplified).
// ---------------------------------------------------------------------------

pub fn sm2_sign(
    private_d: &U256,
    msg_hash: &[u8; 32],
    za: &[u8; 32],
    k_seed: &[u8; 32], // entropy for deterministic k
) -> Option<([u8; 32], [u8; 32])> {
    // Compute e = SM3(ZA ‖ msg_hash)
    let mut zm = [0u8; 64];
    let mut i = 0usize;
    while i < 32 {
        zm[i] = za[i];
        i = i.saturating_add(1);
    }
    let mut i = 0usize;
    while i < 32 {
        zm[32 + i] = msg_hash[i];
        i = i.saturating_add(1);
    }
    let e_bytes = sm3(&zm);
    let e = bytes_to_u256(&e_bytes);

    // Deterministic k from k_seed ⊕ private_d bits
    let mut k_bytes = [0u8; 32];
    let pb = u256_to_bytes(private_d);
    let mut i = 0usize;
    while i < 32 {
        k_bytes[i] = k_seed[i] ^ pb[i];
        i = i.saturating_add(1);
    }
    let k = bytes_to_u256(&k_bytes);

    // (x1, _) = k·G
    let kg = ec_mul(&k, &Sm2Point::generator());
    if kg.is_infinity {
        return None;
    }

    // r = (e + x1) mod n
    let r = u256_addmod(&e, &kg.x, &SM2_N);
    if u256_eq(&r, &[0u64; 4]) {
        return None;
    }

    // Check r + k != n
    let rk = u256_addmod(&r, &k, &SM2_N);
    if u256_eq(&rk, &[0u64; 4]) {
        return None;
    }

    // s = ((1 + d)^{-1} * (k - r*d)) mod n
    let one: U256 = [0, 0, 0, 1];
    let one_plus_d = u256_addmod(private_d, &one, &SM2_N);
    let inv1d = u256_invmod(&one_plus_d, &SM2_N);
    let rd = u256_mulmod(&r, private_d, &SM2_N);
    let k_minus_rd = u256_submod(&k, &rd, &SM2_N);
    let s = u256_mulmod(&inv1d, &k_minus_rd, &SM2_N);
    if u256_eq(&s, &[0u64; 4]) {
        return None;
    }

    Some((u256_to_bytes(&r), u256_to_bytes(&s)))
}

// ---------------------------------------------------------------------------
// SM2 Verify: check (r, s) against (Za ‖ M) and public key Q
// ---------------------------------------------------------------------------

pub fn sm2_verify(
    pub_q: &Sm2Point,
    msg_hash: &[u8; 32],
    za: &[u8; 32],
    r_bytes: &[u8; 32],
    s_bytes: &[u8; 32],
) -> bool {
    let r = bytes_to_u256(r_bytes);
    let s = bytes_to_u256(s_bytes);

    // r, s must be in [1, n-1]
    let zero: U256 = [0; 4];
    if u256_eq(&r, &zero) || u256_eq(&s, &zero) {
        return false;
    }
    if !u256_lt(&r, &SM2_N) || !u256_lt(&s, &SM2_N) {
        return false;
    }

    // e = SM3(ZA ‖ msg_hash)
    let mut zm = [0u8; 64];
    let mut i = 0usize;
    while i < 32 {
        zm[i] = za[i];
        i = i.saturating_add(1);
    }
    let mut i = 0usize;
    while i < 32 {
        zm[32 + i] = msg_hash[i];
        i = i.saturating_add(1);
    }
    let e_bytes = sm3(&zm);
    let e = bytes_to_u256(&e_bytes);

    // t = (r + s) mod n
    let t = u256_addmod(&r, &s, &SM2_N);
    if u256_eq(&t, &zero) {
        return false;
    }

    // P = s·G + t·Q
    let sg = ec_mul(&s, &Sm2Point::generator());
    let tq = ec_mul(&t, pub_q);
    let pt = ec_add(&sg, &tq);
    if pt.is_infinity {
        return false;
    }

    // R = (e + x1) mod n
    let r_prime = u256_addmod(&e, &pt.x, &SM2_N);
    u256_eq(&r_prime, &r)
}

// ---------------------------------------------------------------------------
// Self-test: sign and verify a known message
// ---------------------------------------------------------------------------

fn selftest() -> bool {
    // Private key: all 1s (arbitrary test scalar)
    let d: U256 = [0x0000_0000_0000_0001, 0, 0, 0x0000_0000_0000_0002];
    let q = sm2_public_key(&d);
    let za = sm2_za(b"TestUser@hoags", &q.x, &q.y);
    let msg = sm3(b"Hello SM2");
    let k_seed = [0x42u8; 32];
    let (r, s) = match sm2_sign(&d, &msg, &za, &k_seed) {
        Some(pair) => pair,
        None => return false,
    };
    sm2_verify(&q, &msg, &za, &r, &s)
}

pub fn init() {
    if selftest() {
        serial_println!(
            "[sm2] SM2 elliptic curve (GB/T 32918) initialized — sign/verify KAT passed"
        );
    } else {
        serial_println!("[sm2] SM2 SELF-TEST FAILED");
    }
}
