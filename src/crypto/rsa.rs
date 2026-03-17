/// RSA public-key cryptography
///
/// Pure Rust implementation of RSA-2048/4096 with:
///   - Key generation (probabilistic prime finding)
///   - PKCS#1 v1.5 encryption/decryption padding
///   - OAEP (SHA-256) encryption/decryption padding
///   - PKCS#1 v1.5 signature/verification
///
/// Uses big-integer arithmetic built from u64 limbs.
/// All operations are constant-time where feasible.
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

/// Maximum number of 64-bit limbs for a 4096-bit number
const MAX_LIMBS: usize = 64;

/// Big unsigned integer (little-endian limbs, least significant first)
#[derive(Clone)]
pub struct BigUint {
    limbs: Vec<u64>,
}

impl BigUint {
    /// Create zero
    pub fn zero() -> Self {
        BigUint { limbs: vec![0] }
    }

    /// Create from a single u64
    pub fn from_u64(val: u64) -> Self {
        BigUint { limbs: vec![val] }
    }

    /// Create from big-endian bytes
    pub fn from_bytes_be(bytes: &[u8]) -> Self {
        let mut limbs = Vec::new();
        let mut i = bytes.len();
        while i > 0 {
            let start = if i >= 8 { i - 8 } else { 0 };
            let mut buf = [0u8; 8];
            let slice = &bytes[start..i];
            buf[8 - slice.len()..].copy_from_slice(slice);
            limbs.push(u64::from_be_bytes(buf));
            i = start;
        }
        if limbs.is_empty() {
            limbs.push(0);
        }
        let mut result = BigUint { limbs };
        result.trim();
        result
    }

    /// Convert to big-endian bytes
    pub fn to_bytes_be(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut started = false;
        for &limb in self.limbs.iter().rev() {
            let b = limb.to_be_bytes();
            for &byte in &b {
                if byte != 0 || started {
                    bytes.push(byte);
                    started = true;
                }
            }
        }
        if bytes.is_empty() {
            bytes.push(0);
        }
        bytes
    }

    /// Convert to big-endian bytes with fixed length (zero-padded)
    pub fn to_bytes_be_padded(&self, len: usize) -> Vec<u8> {
        let raw = self.to_bytes_be();
        if raw.len() >= len {
            return raw[raw.len() - len..].to_vec();
        }
        let mut padded = vec![0u8; len - raw.len()];
        padded.extend_from_slice(&raw);
        padded
    }

    /// Remove leading zero limbs
    fn trim(&mut self) {
        while self.limbs.len() > 1 && *self.limbs.last().unwrap_or(&1) == 0 {
            self.limbs.pop();
        }
    }

    /// Check if zero
    pub fn is_zero(&self) -> bool {
        self.limbs.iter().all(|&l| l == 0)
    }

    /// Check if odd
    pub fn is_odd(&self) -> bool {
        self.limbs[0] & 1 == 1
    }

    /// Bit length
    pub fn bit_len(&self) -> usize {
        if self.is_zero() {
            return 0;
        }
        let top = *self.limbs.last().unwrap_or(&0);
        let top_bits = 64 - top.leading_zeros() as usize;
        (self.limbs.len() - 1) * 64 + top_bits
    }

    /// Get bit at position
    pub fn bit(&self, pos: usize) -> bool {
        let limb_idx = pos / 64;
        let bit_idx = pos % 64;
        if limb_idx >= self.limbs.len() {
            return false;
        }
        (self.limbs[limb_idx] >> bit_idx) & 1 == 1
    }

    /// Addition: self + other
    pub fn add(&self, other: &BigUint) -> BigUint {
        let len = self.limbs.len().max(other.limbs.len());
        let mut result = vec![0u64; len + 1];
        let mut carry: u64 = 0;

        for i in 0..len {
            let a = if i < self.limbs.len() {
                self.limbs[i]
            } else {
                0
            };
            let b = if i < other.limbs.len() {
                other.limbs[i]
            } else {
                0
            };
            let sum = (a as u128) + (b as u128) + (carry as u128);
            result[i] = sum as u64;
            carry = (sum >> 64) as u64;
        }
        result[len] = carry;

        let mut r = BigUint { limbs: result };
        r.trim();
        r
    }

    /// Subtraction: self - other (assumes self >= other)
    pub fn sub(&self, other: &BigUint) -> BigUint {
        let mut result = vec![0u64; self.limbs.len()];
        let mut borrow: u64 = 0;

        for i in 0..self.limbs.len() {
            let a = self.limbs[i];
            let b = if i < other.limbs.len() {
                other.limbs[i]
            } else {
                0
            };
            let diff = (a as u128)
                .wrapping_sub(b as u128)
                .wrapping_sub(borrow as u128);
            result[i] = diff as u64;
            borrow = if diff >> 127 != 0 { 1 } else { 0 };
        }

        let mut r = BigUint { limbs: result };
        r.trim();
        r
    }

    /// Comparison: -1 if self < other, 0 if equal, 1 if self > other
    pub fn cmp(&self, other: &BigUint) -> i32 {
        let a_len = self.limbs.len();
        let b_len = other.limbs.len();

        // Compare effective lengths (trimmed)
        if a_len != b_len {
            return if a_len < b_len { -1 } else { 1 };
        }

        for i in (0..a_len).rev() {
            if self.limbs[i] < other.limbs[i] {
                return -1;
            }
            if self.limbs[i] > other.limbs[i] {
                return 1;
            }
        }
        0
    }

    /// Left shift by 1 bit
    pub fn shl1(&self) -> BigUint {
        let mut result = vec![0u64; self.limbs.len() + 1];
        let mut carry: u64 = 0;
        for i in 0..self.limbs.len() {
            result[i] = (self.limbs[i] << 1) | carry;
            carry = self.limbs[i] >> 63;
        }
        result[self.limbs.len()] = carry;
        let mut r = BigUint { limbs: result };
        r.trim();
        r
    }

    /// Right shift by 1 bit
    pub fn shr1(&self) -> BigUint {
        let mut result = vec![0u64; self.limbs.len()];
        let mut carry: u64 = 0;
        for i in (0..self.limbs.len()).rev() {
            result[i] = (self.limbs[i] >> 1) | (carry << 63);
            carry = self.limbs[i] & 1;
        }
        let mut r = BigUint { limbs: result };
        r.trim();
        r
    }

    /// Modular reduction: self mod m (using repeated subtraction for small cases, binary division)
    pub fn modulo(&self, m: &BigUint) -> BigUint {
        if self.cmp(m) < 0 {
            return self.clone();
        }
        // Binary long division
        let mut remainder = BigUint::zero();
        let bits = self.bit_len();

        for i in (0..bits).rev() {
            remainder = remainder.shl1();
            if self.bit(i) {
                remainder.limbs[0] |= 1;
            }
            if remainder.cmp(m) >= 0 {
                remainder = remainder.sub(m);
            }
        }
        remainder
    }

    /// Modular multiplication: (self * other) mod m
    /// Uses shift-and-add method to avoid overflow
    pub fn mod_mul(&self, other: &BigUint, m: &BigUint) -> BigUint {
        let mut result = BigUint::zero();
        let mut base = self.modulo(m);
        let bits = other.bit_len();

        for i in 0..bits {
            if other.bit(i) {
                result = result.add(&base).modulo(m);
            }
            base = base.add(&base).modulo(m);
        }
        result
    }

    /// Modular exponentiation: self^exp mod m (square-and-multiply)
    pub fn mod_pow(&self, exp: &BigUint, m: &BigUint) -> BigUint {
        if m.is_zero() {
            return BigUint::zero();
        }
        let mut result = BigUint::from_u64(1);
        let mut base = self.modulo(m);
        let bits = exp.bit_len();

        for i in 0..bits {
            if exp.bit(i) {
                result = result.mod_mul(&base, m);
            }
            base = base.mod_mul(&base, m);
        }
        result
    }

    /// Extended GCD: returns (gcd, x, y) such that a*x + b*y = gcd
    /// Only returns x (the modular inverse coefficient)
    pub fn mod_inverse(&self, m: &BigUint) -> Option<BigUint> {
        // Extended Euclidean algorithm using signed big integers
        // Simplified: use Fermat's little theorem if m is prime
        // For RSA we use the iterative extended GCD approach
        let mut old_r = self.clone();
        let mut r = m.clone();
        let mut old_s = BigUint::from_u64(1);
        let mut s = BigUint::zero();
        let mut old_s_neg = false;
        let mut s_neg = false;

        while !r.is_zero() {
            // Compute quotient and remainder
            let q = big_div(&old_r, &r);
            let new_r = old_r.modulo(&r);

            old_r = r;
            r = new_r;

            // Update s: old_s - q * s
            let qs = q.mod_mul(&s, m);
            let (new_s, new_s_neg) = if old_s_neg == s_neg {
                if old_s.cmp(&qs) >= 0 {
                    (old_s.sub(&qs), old_s_neg)
                } else {
                    (qs.sub(&old_s), !old_s_neg)
                }
            } else {
                (old_s.add(&qs).modulo(m), old_s_neg)
            };

            old_s = s;
            old_s_neg = s_neg;
            s = new_s;
            s_neg = new_s_neg;
        }

        // If gcd != 1, no inverse
        if old_r.cmp(&BigUint::from_u64(1)) != 0 {
            return None;
        }

        if old_s_neg {
            Some(m.sub(&old_s.modulo(m)))
        } else {
            Some(old_s.modulo(m))
        }
    }
}

/// Simple big integer division: a / b (returns quotient)
fn big_div(a: &BigUint, b: &BigUint) -> BigUint {
    if b.is_zero() || a.cmp(b) < 0 {
        return BigUint::zero();
    }

    let mut quotient = BigUint::zero();
    let mut remainder = BigUint::zero();
    let bits = a.bit_len();

    // Need enough limbs for quotient
    quotient.limbs = vec![0u64; (bits + 63) / 64];

    for i in (0..bits).rev() {
        remainder = remainder.shl1();
        if a.bit(i) {
            remainder.limbs[0] |= 1;
        }
        if remainder.cmp(b) >= 0 {
            remainder = remainder.sub(b);
            let limb_idx = i / 64;
            let bit_idx = i % 64;
            if limb_idx < quotient.limbs.len() {
                quotient.limbs[limb_idx] |= 1u64 << bit_idx;
            }
        }
    }
    quotient.trim();
    quotient
}

/// Small primes for trial division
const SMALL_PRIMES: [u64; 54] = [
    2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53, 59, 61, 67, 71, 73, 79, 83, 89, 97,
    101, 103, 107, 109, 113, 127, 131, 137, 139, 149, 151, 157, 163, 167, 173, 179, 181, 191, 193,
    197, 199, 211, 223, 227, 229, 233, 239, 241, 251,
];

/// Miller-Rabin primality test with k rounds
pub fn is_probably_prime(n: &BigUint, k: usize) -> bool {
    if n.cmp(&BigUint::from_u64(2)) < 0 {
        return false;
    }
    if !n.is_odd() {
        return n.cmp(&BigUint::from_u64(2)) == 0;
    }

    // Trial division by small primes
    for &p in &SMALL_PRIMES {
        let pm = BigUint::from_u64(p);
        if n.cmp(&pm) == 0 {
            return true;
        }
        if n.modulo(&pm).is_zero() {
            return false;
        }
    }

    // Write n-1 as 2^r * d where d is odd
    let n_minus_1 = n.sub(&BigUint::from_u64(1));
    let mut d = n_minus_1.clone();
    let mut r: u32 = 0;
    while !d.is_odd() {
        d = d.shr1();
        r += 1;
    }

    // Witness loop
    let witnesses: [u64; 7] = [2, 3, 5, 7, 11, 13, 17];
    let rounds = k.min(witnesses.len());

    for i in 0..rounds {
        let a = BigUint::from_u64(witnesses[i]);
        if a.cmp(n) >= 0 {
            continue;
        }

        let mut x = a.mod_pow(&d, n);

        if x.cmp(&BigUint::from_u64(1)) == 0 || x.cmp(&n_minus_1) == 0 {
            continue;
        }

        let mut composite = true;
        for _ in 0..r - 1 {
            x = x.mod_mul(&x, n);
            if x.cmp(&n_minus_1) == 0 {
                composite = false;
                break;
            }
        }

        if composite {
            return false;
        }
    }

    true
}

/// Generate a random prime of the given bit length
pub fn generate_prime(bit_len: usize) -> BigUint {
    loop {
        let byte_len = (bit_len + 7) / 8;
        let mut bytes = vec![0u8; byte_len];
        super::random::fill_bytes(&mut bytes);

        // Set the top two bits to ensure the number is large enough
        bytes[0] |= 0xC0;
        // Set the bottom bit to make it odd
        bytes[byte_len - 1] |= 0x01;

        // Clear excess bits if bit_len is not a multiple of 8
        let excess = byte_len * 8 - bit_len;
        if excess > 0 {
            bytes[0] &= 0xFF >> excess;
            bytes[0] |= 1 << (7 - excess); // Ensure top bit within range
        }

        let candidate = BigUint::from_bytes_be(&bytes);

        if is_probably_prime(&candidate, 7) {
            return candidate;
        }
    }
}

/// RSA public key
pub struct RsaPublicKey {
    pub n: BigUint, // modulus
    pub e: BigUint, // public exponent
}

impl RsaPublicKey {
    /// Parse an RSA public key from DER-encoded SubjectPublicKeyInfo key bytes.
    ///
    /// The input is the BIT STRING payload from SPKI, which contains:
    ///   SEQUENCE {
    ///     INTEGER n   (modulus, big-endian, possibly with leading 0x00 sign byte)
    ///     INTEGER e   (public exponent, typically 65537 = 0x010001)
    ///   }
    pub fn from_der(data: &[u8]) -> Option<Self> {
        // Read outer SEQUENCE tag (0x30)
        if data.len() < 4 || data[0] != 0x30 {
            return None;
        }
        // Parse DER length of outer SEQUENCE
        let (outer_len, outer_hdr) = der_read_length(&data[1..])?;
        let inner = &data[1 + outer_hdr..1 + outer_hdr + outer_len];
        if inner.len() < outer_len {
            return None;
        }

        // Parse n (first INTEGER)
        if inner[0] != 0x02 {
            return None;
        }
        let (n_len, n_hdr) = der_read_length(&inner[1..])?;
        let n_start = 1 + n_hdr;
        let n_bytes = &inner[n_start..n_start + n_len];

        // Strip leading 0x00 sign byte (DER positive integer encoding)
        let n_bytes = if n_bytes.first() == Some(&0x00) {
            &n_bytes[1..]
        } else {
            n_bytes
        };
        let n = BigUint::from_bytes_be(n_bytes);

        // Parse e (second INTEGER)
        let e_offset = n_start + n_len;
        if e_offset >= inner.len() || inner[e_offset] != 0x02 {
            return None;
        }
        let (e_len, e_hdr) = der_read_length(&inner[e_offset + 1..])?;
        let e_start = e_offset + 1 + e_hdr;
        let e_bytes = &inner[e_start..e_start + e_len];
        let e_bytes = if e_bytes.first() == Some(&0x00) {
            &e_bytes[1..]
        } else {
            e_bytes
        };
        let e = BigUint::from_bytes_be(e_bytes);

        Some(RsaPublicKey { n, e })
    }
}

/// Parse a DER length field. Returns (length_value, bytes_consumed_by_length_field).
fn der_read_length(data: &[u8]) -> Option<(usize, usize)> {
    if data.is_empty() {
        return None;
    }
    let first = data[0];
    if first < 0x80 {
        Some((first as usize, 1))
    } else if first == 0x80 {
        None // indefinite length not valid in DER
    } else {
        let num_bytes = (first & 0x7F) as usize;
        if num_bytes > 4 || data.len() < 1 + num_bytes {
            return None;
        }
        let mut length: usize = 0;
        for i in 0..num_bytes {
            length = (length << 8) | (data[1 + i] as usize);
        }
        Some((length, 1 + num_bytes))
    }
}

/// RSA private key
pub struct RsaPrivateKey {
    pub n: BigUint, // modulus
    pub d: BigUint, // private exponent
    pub e: BigUint, // public exponent
}

/// Generate an RSA key pair with the given modulus bit length (e.g., 2048)
pub fn generate_keypair(bits: usize) -> (RsaPublicKey, RsaPrivateKey) {
    let half_bits = bits / 2;
    let e = BigUint::from_u64(65537);

    loop {
        let p = generate_prime(half_bits);
        let q = generate_prime(half_bits);

        let n = p.mod_mul(&q, &BigUint::from_bytes_be(&vec![0xFF; (bits / 8) + 1]));
        // Actually: n = p * q computed via repeated addition is too slow.
        // Use a simpler multiply for key generation
        let n = big_multiply(&p, &q);

        let p1 = p.sub(&BigUint::from_u64(1));
        let q1 = q.sub(&BigUint::from_u64(1));
        let phi = big_multiply(&p1, &q1);

        if let Some(d) = e.mod_inverse(&phi) {
            let public_key = RsaPublicKey {
                n: n.clone(),
                e: e.clone(),
            };
            let private_key = RsaPrivateKey { n, d, e };
            return (public_key, private_key);
        }
    }
}

/// Straightforward big integer multiplication
fn big_multiply(a: &BigUint, b: &BigUint) -> BigUint {
    let mut result = vec![0u64; a.limbs.len() + b.limbs.len()];

    for i in 0..a.limbs.len() {
        let mut carry: u64 = 0;
        for j in 0..b.limbs.len() {
            let product = (a.limbs[i] as u128) * (b.limbs[j] as u128)
                + (result[i + j] as u128)
                + (carry as u128);
            result[i + j] = product as u64;
            carry = (product >> 64) as u64;
        }
        result[i + b.limbs.len()] = carry;
    }

    let mut r = BigUint { limbs: result };
    r.trim();
    r
}

/// PKCS#1 v1.5 encryption padding
/// EM = 0x00 || 0x02 || PS (random non-zero) || 0x00 || M
pub fn pkcs1_v15_pad_encrypt(msg: &[u8], modulus_len: usize) -> Option<Vec<u8>> {
    if msg.len() > modulus_len - 11 {
        return None; // Message too long
    }

    let ps_len = modulus_len - msg.len() - 3;
    let mut em = vec![0u8; modulus_len];
    em[0] = 0x00;
    em[1] = 0x02;

    // Fill PS with random non-zero bytes
    let mut ps = vec![0u8; ps_len];
    super::random::fill_bytes(&mut ps);
    for byte in ps.iter_mut() {
        while *byte == 0 {
            let mut tmp = [0u8; 1];
            super::random::fill_bytes(&mut tmp);
            *byte = tmp[0];
        }
    }
    em[2..2 + ps_len].copy_from_slice(&ps);
    em[2 + ps_len] = 0x00;
    em[3 + ps_len..].copy_from_slice(msg);

    Some(em)
}

/// PKCS#1 v1.5 encryption unpadding
pub fn pkcs1_v15_unpad_encrypt(em: &[u8]) -> Option<Vec<u8>> {
    if em.len() < 11 || em[0] != 0x00 || em[1] != 0x02 {
        return None;
    }

    // Find the 0x00 separator after PS
    let mut sep_idx = None;
    for i in 2..em.len() {
        if em[i] == 0x00 {
            sep_idx = Some(i);
            break;
        }
    }

    let sep_idx = sep_idx?;
    if sep_idx < 10 {
        return None; // PS must be at least 8 bytes
    }

    Some(em[sep_idx + 1..].to_vec())
}

/// PKCS#1 v1.5 signature padding
/// EM = 0x00 || 0x01 || PS (0xFF bytes) || 0x00 || DigestInfo
pub fn pkcs1_v15_pad_sign(digest: &[u8; 32], modulus_len: usize) -> Option<Vec<u8>> {
    // SHA-256 DigestInfo DER prefix
    let digest_info_prefix: [u8; 19] = [
        0x30, 0x31, 0x30, 0x0D, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01,
        0x05, 0x00, 0x04, 0x20,
    ];

    let t_len = digest_info_prefix.len() + 32;
    if modulus_len < t_len + 11 {
        return None;
    }

    let ps_len = modulus_len - t_len - 3;
    let mut em = vec![0u8; modulus_len];
    em[0] = 0x00;
    em[1] = 0x01;
    for i in 0..ps_len {
        em[2 + i] = 0xFF;
    }
    em[2 + ps_len] = 0x00;
    em[3 + ps_len..3 + ps_len + digest_info_prefix.len()].copy_from_slice(&digest_info_prefix);
    em[3 + ps_len + digest_info_prefix.len()..].copy_from_slice(digest);

    Some(em)
}

/// RSA encrypt with PKCS#1 v1.5 padding
pub fn rsa_encrypt(public_key: &RsaPublicKey, plaintext: &[u8]) -> Option<Vec<u8>> {
    let mod_len = (public_key.n.bit_len() + 7) / 8;
    let padded = pkcs1_v15_pad_encrypt(plaintext, mod_len)?;
    let m = BigUint::from_bytes_be(&padded);
    let c = m.mod_pow(&public_key.e, &public_key.n);
    Some(c.to_bytes_be_padded(mod_len))
}

/// RSA decrypt with PKCS#1 v1.5 unpadding
pub fn rsa_decrypt(private_key: &RsaPrivateKey, ciphertext: &[u8]) -> Option<Vec<u8>> {
    let c = BigUint::from_bytes_be(ciphertext);
    let m = c.mod_pow(&private_key.d, &private_key.n);
    let mod_len = (private_key.n.bit_len() + 7) / 8;
    let em = m.to_bytes_be_padded(mod_len);
    pkcs1_v15_unpad_encrypt(&em)
}

/// RSA sign with PKCS#1 v1.5 (SHA-256 digest)
pub fn rsa_sign(private_key: &RsaPrivateKey, message: &[u8]) -> Option<Vec<u8>> {
    let digest = super::sha256::hash(message);
    let mod_len = (private_key.n.bit_len() + 7) / 8;
    let padded = pkcs1_v15_pad_sign(&digest, mod_len)?;
    let m = BigUint::from_bytes_be(&padded);
    let s = m.mod_pow(&private_key.d, &private_key.n);
    Some(s.to_bytes_be_padded(mod_len))
}

/// RSA verify PKCS#1 v1.5 signature
pub fn rsa_verify(public_key: &RsaPublicKey, message: &[u8], signature: &[u8]) -> bool {
    let s = BigUint::from_bytes_be(signature);
    let m = s.mod_pow(&public_key.e, &public_key.n);
    let mod_len = (public_key.n.bit_len() + 7) / 8;
    let em = m.to_bytes_be_padded(mod_len);

    let digest = super::sha256::hash(message);
    let expected = match pkcs1_v15_pad_sign(&digest, mod_len) {
        Some(e) => e,
        None => return false,
    };

    // Constant-time comparison
    if em.len() != expected.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for i in 0..em.len() {
        diff |= em[i] ^ expected[i];
    }
    diff == 0
}

/// OAEP encode (SHA-256): for RSA-OAEP encryption
pub fn oaep_encode(msg: &[u8], label: &[u8], mod_len: usize) -> Option<Vec<u8>> {
    let h_len = 32; // SHA-256
    let max_msg_len = mod_len - 2 * h_len - 2;
    if msg.len() > max_msg_len {
        return None;
    }

    let l_hash = super::sha256::hash(label);

    // DB = lHash || PS (zeros) || 0x01 || M
    let db_len = mod_len - h_len - 1;
    let mut db = vec![0u8; db_len];
    db[..h_len].copy_from_slice(&l_hash);
    // PS is zeros (already initialized)
    db[db_len - msg.len() - 1] = 0x01;
    db[db_len - msg.len()..].copy_from_slice(msg);

    // Generate random seed
    let mut seed = [0u8; 32];
    super::random::fill_bytes(&mut seed);

    // dbMask = MGF1(seed, db_len)
    let db_mask = mgf1(&seed, db_len);
    let mut masked_db = vec![0u8; db_len];
    for i in 0..db_len {
        masked_db[i] = db[i] ^ db_mask[i];
    }

    // seedMask = MGF1(maskedDB, h_len)
    let seed_mask = mgf1(&masked_db, h_len);
    let mut masked_seed = [0u8; 32];
    for i in 0..h_len {
        masked_seed[i] = seed[i] ^ seed_mask[i];
    }

    // EM = 0x00 || maskedSeed || maskedDB
    let mut em = vec![0u8; mod_len];
    em[0] = 0x00;
    em[1..1 + h_len].copy_from_slice(&masked_seed);
    em[1 + h_len..].copy_from_slice(&masked_db);

    Some(em)
}

/// OAEP decode (SHA-256): for RSA-OAEP decryption
pub fn oaep_decode(em: &[u8], label: &[u8]) -> Option<Vec<u8>> {
    let h_len = 32;
    if em.len() < 2 * h_len + 2 || em[0] != 0x00 {
        return None;
    }

    let masked_seed = &em[1..1 + h_len];
    let masked_db = &em[1 + h_len..];

    // Recover seed
    let seed_mask = mgf1(masked_db, h_len);
    let mut seed = [0u8; 32];
    for i in 0..h_len {
        seed[i] = masked_seed[i] ^ seed_mask[i];
    }

    // Recover DB
    let db_mask = mgf1(&seed, masked_db.len());
    let mut db = vec![0u8; masked_db.len()];
    for i in 0..masked_db.len() {
        db[i] = masked_db[i] ^ db_mask[i];
    }

    // Verify lHash
    let l_hash = super::sha256::hash(label);
    let mut valid = true;
    for i in 0..h_len {
        if db[i] != l_hash[i] {
            valid = false;
        }
    }

    if !valid {
        return None;
    }

    // Find 0x01 separator
    let mut sep_idx = None;
    for i in h_len..db.len() {
        if db[i] == 0x01 {
            sep_idx = Some(i);
            break;
        } else if db[i] != 0x00 {
            return None;
        }
    }

    let sep_idx = sep_idx?;
    Some(db[sep_idx + 1..].to_vec())
}

/// MGF1 mask generation function (using SHA-256)
fn mgf1(seed: &[u8], length: usize) -> Vec<u8> {
    let mut output = Vec::with_capacity(length);
    let mut counter: u32 = 0;

    while output.len() < length {
        let mut input = Vec::with_capacity(seed.len() + 4);
        input.extend_from_slice(seed);
        input.extend_from_slice(&counter.to_be_bytes());

        let hash = super::sha256::hash(&input);
        let remaining = length - output.len();
        let to_copy = if remaining < 32 { remaining } else { 32 };
        output.extend_from_slice(&hash[..to_copy]);

        counter += 1;
    }
    output.truncate(length);
    output
}

/// RSA-OAEP encrypt
pub fn rsa_oaep_encrypt(
    public_key: &RsaPublicKey,
    plaintext: &[u8],
    label: &[u8],
) -> Option<Vec<u8>> {
    let mod_len = (public_key.n.bit_len() + 7) / 8;
    let em = oaep_encode(plaintext, label, mod_len)?;
    let m = BigUint::from_bytes_be(&em);
    let c = m.mod_pow(&public_key.e, &public_key.n);
    Some(c.to_bytes_be_padded(mod_len))
}

/// RSA-OAEP decrypt
pub fn rsa_oaep_decrypt(
    private_key: &RsaPrivateKey,
    ciphertext: &[u8],
    label: &[u8],
) -> Option<Vec<u8>> {
    let c = BigUint::from_bytes_be(ciphertext);
    let m = c.mod_pow(&private_key.d, &private_key.n);
    let mod_len = (private_key.n.bit_len() + 7) / 8;
    let em = m.to_bytes_be_padded(mod_len);
    oaep_decode(&em, label)
}

pub fn init() {
    serial_println!("    [rsa] RSA-2048/4096 (PKCS#1 v1.5/OAEP, sign/verify) ready");
}

// ===========================================================================
// No-heap RSA public-key types for module verification
//
// These types use fixed-size arrays only (no Vec, no Box, no alloc).
// They are usable in static statics and Copy types.
// ===========================================================================

/// Fixed-size RSA public key for up to 2048-bit moduli (256 bytes).
///
/// `n` and `e` are stored in big-endian byte order.
/// `n_len` records how many of the 256 bytes are significant (from the
/// most-significant end — i.e., the actual modulus occupies `n[256-n_len..]`).
#[derive(Clone, Copy)]
pub struct RsaPublicKeyFixed {
    /// Modulus bytes (big-endian, zero-padded on the left to 256 bytes)
    pub n: [u8; 256],
    /// Number of significant bytes in `n`
    pub n_len: usize,
    /// Public exponent (typically 65537)
    pub e: u32,
    /// Key size in bits
    pub bits: u32,
}

impl RsaPublicKeyFixed {
    /// Construct a zeroed, invalid key (placeholder for statics).
    pub const fn empty() -> Self {
        RsaPublicKeyFixed {
            n: [0u8; 256],
            n_len: 0,
            e: 0,
            bits: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Big-integer helpers — all operate on 256-byte big-endian arrays, no alloc
// ---------------------------------------------------------------------------

/// Compare two 256-byte big-endian integers.
/// Returns `true` if `a < b` (lexicographic, which is numerically correct
/// because the arrays are big-endian and the same length).
fn fixed_lt(a: &[u8; 256], b: &[u8; 256]) -> bool {
    for i in 0..256 {
        if a[i] < b[i] {
            return true;
        }
        if a[i] > b[i] {
            return false;
        }
    }
    false // equal
}

/// In-place left-shift by 1 bit.  Returns the carry (overflow) bit.
fn fixed_shl1(a: &mut [u8; 256]) -> bool {
    let mut carry: u8 = 0;
    for i in (0..256).rev() {
        let new_carry = a[i] >> 7;
        a[i] = (a[i] << 1) | carry;
        carry = new_carry;
    }
    carry != 0
}

/// `out = a - b` (assumes `a >= b`, big-endian 256-byte).
fn fixed_sub(a: &[u8; 256], b: &[u8; 256], out: &mut [u8; 256]) {
    let mut borrow: u16 = 0;
    for i in (0..256).rev() {
        let diff = (a[i] as i16) - (b[i] as i16) - (borrow as i16);
        if diff < 0 {
            out[i] = (diff + 256) as u8;
            borrow = 1;
        } else {
            out[i] = diff as u8;
            borrow = 0;
        }
    }
}

/// In-place modular reduction: `a = a mod n`.
/// `n_len` is the number of significant bytes in `n` (from `n[256-n_len..]`).
fn fixed_reduce(a: &mut [u8; 256], n: &[u8; 256]) {
    // Subtract n while a >= n
    while !fixed_lt(a, n) {
        let mut tmp = [0u8; 256];
        fixed_sub(a, n, &mut tmp);
        *a = tmp;
    }
}

/// `out = (a + b) mod n`, all 256-byte big-endian.
fn fixed_add_mod(a: &[u8; 256], b: &[u8; 256], n: &[u8; 256], out: &mut [u8; 256]) {
    // Add a + b with carry into a 256-byte result (carry absorbed via reduction)
    let mut carry: u16 = 0;
    for i in (0..256).rev() {
        let sum = (a[i] as u16) + (b[i] as u16) + carry;
        out[i] = sum as u8;
        carry = sum >> 8;
    }
    // If there was carry-out the sum wrapped; add carry back is not needed
    // since we reduce mod n next.
    fixed_reduce(out, n);
}

/// `out = (a * b) mod n`, all 256-byte big-endian.
///
/// Uses the binary left-to-right double-and-add algorithm to keep
/// intermediate results within 256 bytes (only requires one call to
/// `fixed_add_mod` per bit, which keeps everything in-register).
fn fixed_mulmod(a: &[u8; 256], b: &[u8; 256], n: &[u8; 256], out: &mut [u8; 256]) {
    let mut result = [0u8; 256];
    let mut base = *a;
    fixed_reduce(&mut base, n);

    // Iterate over all 2048 bits of b, LSB of last byte first (little-endian bit order)
    for byte_idx in (0..256).rev() {
        let byte_val = b[byte_idx];
        for bit_idx in 0..8u8 {
            if (byte_val >> bit_idx) & 1 != 0 {
                fixed_add_mod(&result, &base, n, out);
                result = *out;
            }
            // Double base
            let mut doubled = [0u8; 256];
            fixed_add_mod(&base, &base, n, &mut doubled);
            base = doubled;
        }
    }
    *out = result;
}

// ---------------------------------------------------------------------------
// RSA public-key exponentiation (signature verification)
// ---------------------------------------------------------------------------

/// Compute `out = msg^e mod n` where `e` is the u32 public exponent.
///
/// For the common case `e = 65537 = 0x10001 = 2^16 + 1` this uses the
/// factorisation:  `msg^65537 = msg^(2^16) * msg`  so only 17 squarings
/// and 1 multiply are needed.
///
/// Falls back to a generic square-and-multiply loop for other exponents.
///
/// Returns `false` if the key is invalid (zero modulus or exponent).
pub fn rsa_public_op_fixed(key: &RsaPublicKeyFixed, msg: &[u8; 256], out: &mut [u8; 256]) -> bool {
    if key.n_len == 0 || key.e == 0 {
        return false;
    }
    let n = &key.n;

    if key.e == 65537 {
        // e = 2^16 + 1 fast path: 16 squarings then one multiply
        let mut base = *msg;
        fixed_reduce(&mut base, n);

        // 16 squarings: base = msg^(2^16) mod n
        for _ in 0..16 {
            let mut sq = [0u8; 256];
            fixed_mulmod(&base, &base, n, &mut sq);
            base = sq;
        }

        // out = base * msg mod n  (= msg^(2^16) * msg^1 = msg^65537)
        fixed_mulmod(&base, msg, n, out);
    } else {
        // Generic square-and-multiply, MSB first over the 32-bit exponent
        let mut result = [0u8; 256];
        // result = 1
        result[255] = 1;
        let mut base = *msg;
        fixed_reduce(&mut base, n);

        let mut exp = key.e;
        // Find highest set bit
        let mut bits_left: u32 = 0;
        {
            let mut tmp = exp;
            while tmp != 0 {
                bits_left = bits_left.saturating_add(1);
                tmp >>= 1;
            }
        }
        for _ in 0..bits_left {
            // Square
            let mut sq = [0u8; 256];
            fixed_mulmod(&result, &result, n, &mut sq);
            result = sq;
            // Multiply if bit set (MSB of current exp)
            if exp & (1u32 << (bits_left.saturating_sub(1))) != 0 {
                let mut tmp = [0u8; 256];
                fixed_mulmod(&result, &base, n, &mut tmp);
                result = tmp;
            }
            exp <<= 1;
        }
        *out = result;
    }
    true
}

/// Parse an RSA public key from a DER-encoded SubjectPublicKeyInfo (SPKI)
/// BIT-STRING payload — i.e. the bytes after the leading unused-bits byte.
///
/// Expected format:
/// ```text
/// SEQUENCE {
///   INTEGER   n   (modulus)
///   INTEGER   e   (public exponent)
/// }
/// ```
pub fn rsa_parse_public_key_fixed(der: &[u8]) -> Option<RsaPublicKeyFixed> {
    use super::asn1::{Asn1Der, TAG_INTEGER, TAG_SEQUENCE};

    let mut p = Asn1Der::new(der);
    let (tag, seq_val) = p.read_tlv()?;
    if tag != TAG_SEQUENCE {
        return None;
    }
    let mut inner = Asn1Der::new(seq_val);

    // Read modulus n
    let mut n_buf = [0u8; 512];
    let n_len = inner.read_large_integer(&mut n_buf)?;
    if n_len == 0 || n_len > 256 {
        return None;
    }
    // Copy right-justified into 256-byte array (big-endian)
    let mut n = [0u8; 256];
    let src_start = 512usize.saturating_sub(n_len);
    n.copy_from_slice(&n_buf[src_start..src_start + 256]);

    // Read exponent e (must fit in u32)
    let e_val = inner.read_integer_u64()?;
    if e_val > u32::MAX as u64 {
        return None;
    }
    let e = e_val as u32;

    // Compute bit length of n
    let bits = {
        let mut b: u32 = 0;
        for i in 0..256usize {
            if n[i] != 0 {
                let leading = n[i].leading_zeros();
                b = ((256u32.saturating_sub(i as u32)).saturating_mul(8)).saturating_sub(leading);
                break;
            }
        }
        b
    };

    Some(RsaPublicKeyFixed { n, n_len, e, bits })
}

// ---------------------------------------------------------------------------
// PKCS#1 v1.5 SHA-256 signature verification (no heap)
// ---------------------------------------------------------------------------

/// SHA-256 DigestInfo prefix for PKCS#1 v1.5:
/// `SEQUENCE { SEQUENCE { OID sha-256, NULL } OCTET STRING (32) }`
const PKCS1_SHA256_DIGESTINFO_PREFIX: [u8; 19] = [
    0x30, 0x31, 0x30, 0x0D, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05,
    0x00, 0x04, 0x20,
];

/// Verify a PKCS#1 v1.5 / SHA-256 RSA signature.
///
/// * `key`       — RSA-2048 public key (fixed, no heap)
/// * `signature` — 256-byte raw RSA signature block (big-endian)
/// * `digest`    — 32-byte SHA-256 hash of the signed data
///
/// Returns `true` iff the signature is valid.
pub fn rsa_pkcs1_verify_sha256_fixed(
    key: &RsaPublicKeyFixed,
    signature: &[u8; 256],
    digest: &[u8; 32],
) -> bool {
    // Step 1: RSA public operation  em = sig^e mod n
    let mut em = [0u8; 256];
    if !rsa_public_op_fixed(key, signature, &mut em) {
        return false;
    }

    // Step 2: Verify PKCS#1 v1.5 padding for a 256-byte (2048-bit) key
    // Expected: 0x00 0x01 [0xFF...] 0x00 DigestInfo SHA-256-OID digest
    //
    // DigestInfo length = 19 prefix + 32 digest = 51 bytes
    // Minimum PS length = 8 (mandated by RFC 8017 §8.2.1)
    // Total = 1 (0x00) + 1 (0x01) + 8+ (PS) + 1 (0x00) + 51 = 62 bytes min
    // For 256-byte key: PS length = 256 - 3 - 51 = 202 bytes

    if em[0] != 0x00 || em[1] != 0x01 {
        return false;
    }

    let di_total_len = PKCS1_SHA256_DIGESTINFO_PREFIX.len() + 32; // 51
    if 256 < 3 + 8 + di_total_len {
        return false; // key too small (never true for 256-byte key)
    }
    let ps_len = 256usize - 3 - di_total_len; // = 202

    // All PS bytes must be 0xFF
    for i in 2..2 + ps_len {
        if em[i] != 0xFF {
            return false;
        }
    }
    // 0x00 separator
    let sep = 2 + ps_len;
    if em[sep] != 0x00 {
        return false;
    }

    // DigestInfo prefix
    let di_start = sep + 1;
    let prefix_end = di_start + PKCS1_SHA256_DIGESTINFO_PREFIX.len();
    if prefix_end + 32 != 256 {
        return false;
    }
    let mut mismatch: u8 = 0;
    for (i, &expected) in PKCS1_SHA256_DIGESTINFO_PREFIX.iter().enumerate() {
        mismatch |= em[di_start + i] ^ expected;
    }
    // Digest bytes (constant-time compare)
    for i in 0..32 {
        mismatch |= em[prefix_end + i] ^ digest[i];
    }
    mismatch == 0
}
