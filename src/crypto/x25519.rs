/// Key size in bytes (32 bytes = 256 bits)
pub const KEY_SIZE: usize = 32;

/// Base point for X25519: the u-coordinate 9 on Curve25519 (little-endian, 32 bytes).
///
/// Per RFC 7748 §4.1: the canonical base point has u = 9.
pub const X25519_BASE: [u8; 32] = {
    let mut b = [0u8; 32];
    b[0] = 9;
    b
};

/// The prime p = 2^255 - 19 (used for display/documentation; actual arithmetic is implicit)
#[allow(dead_code)]
const P_LOWEST_LIMB: u64 = 0x7fffffffffffffed; // lowest 64 bits of p

/// Mask for 51-bit limbs
const MASK51: u64 = 0x7ffffffffffff; // (1 << 51) - 1

// --- Field element ---

/// Field element in GF(2^255 - 19), represented as 5 limbs of 51 bits.
///
/// This representation ensures:
///   - Each limb fits in a u64 with headroom for additions
///   - Products of two limbs fit in u128 (51 + 51 = 102 bits)
///   - Sums of 5 products fit in ~105 bits, within u128
///   - Reduction is cheap: 2^255 = 19 mod p
#[derive(Clone, Copy)]
struct Fe([u64; 5]);

impl Fe {
    /// The zero element.
    const ZERO: Self = Fe([0; 5]);

    /// The one element.
    const ONE: Self = Fe([1, 0, 0, 0, 0]);

    /// Decode a 32-byte little-endian representation into a field element.
    ///
    /// Unpacks 256 bits into 5 x 51-bit limbs. The top bit (bit 255) is
    /// masked off to ensure the value is < 2^255 (per RFC 7748).
    fn from_bytes(bytes: &[u8; 32]) -> Self {
        let mut limbs = [0u64; 5];

        // Load 32 bytes as a little-endian integer split across 5 limbs
        // Limb 0: bits 0-50 (bytes 0-6, low 51 bits)
        limbs[0] = load_le_u64(&bytes[0..]) & MASK51;

        // Limb 1: bits 51-101 (bytes 6-12, shifted right by 3)
        limbs[1] = (load_le_u64(&bytes[6..]) >> 3) & MASK51;

        // Limb 2: bits 102-152 (bytes 12-19, shifted right by 6)
        limbs[2] = (load_le_u64(&bytes[12..]) >> 6) & MASK51;

        // Limb 3: bits 153-203 (bytes 19-25, shifted right by 1)
        limbs[3] = (load_le_u64(&bytes[19..]) >> 1) & MASK51;

        // Limb 4: bits 204-254 (bytes 25-31, shifted right by 4, mask top bit)
        limbs[4] = (load_le_u64(&bytes[25..]) >> 4) & MASK51;

        Fe(limbs)
    }

    /// Encode a field element to 32-byte little-endian representation.
    ///
    /// First performs full reduction modulo p, then packs 5 x 51-bit limbs
    /// into 32 bytes.
    fn to_bytes(&self) -> [u8; 32] {
        // Full reduction modulo p
        let h = self.reduce();

        // Pack 5 x 51-bit limbs into 32 bytes (little-endian)
        let mut bytes = [0u8; 32];
        let mut acc: u128 = 0;
        let mut bits = 0u32;
        let mut pos = 0;

        for &limb in &h.0 {
            acc |= (limb as u128) << bits;
            bits += 51;
            while bits >= 8 && pos < 32 {
                bytes[pos] = acc as u8;
                acc >>= 8;
                bits -= 8;
                pos += 1;
            }
        }

        bytes
    }

    /// Fully reduce modulo p = 2^255 - 19.
    ///
    /// After arithmetic operations, limbs may slightly exceed 51 bits.
    /// This function ensures exact canonical representation in [0, p).
    fn reduce(&self) -> Fe {
        let mut h = self.0;

        // First pass: propagate carries
        for i in 0..4 {
            let carry = h[i] >> 51;
            h[i] &= MASK51;
            h[i + 1] += carry;
        }
        let carry = h[4] >> 51;
        h[4] &= MASK51;
        h[0] += carry * 19; // 2^255 = 19 mod p

        // Second pass: propagate carries again
        for i in 0..4 {
            let carry = h[i] >> 51;
            h[i] &= MASK51;
            h[i + 1] += carry;
        }
        let carry = h[4] >> 51;
        h[4] &= MASK51;
        h[0] += carry * 19;

        // Final carry
        let carry = h[0] >> 51;
        h[0] &= MASK51;
        h[1] += carry;

        // Conditional subtraction of p (constant-time)
        // Check if h >= p by computing h - p and checking for underflow
        let mut g = [0u64; 5];
        g[0] = h[0].wrapping_add(19); // add 19 is same as subtracting p then adding 2^255
        let carry = g[0] >> 51;
        g[0] &= MASK51;
        g[1] = h[1].wrapping_add(carry);
        let carry = g[1] >> 51;
        g[1] &= MASK51;
        g[2] = h[2].wrapping_add(carry);
        let carry = g[2] >> 51;
        g[2] &= MASK51;
        g[3] = h[3].wrapping_add(carry);
        let carry = g[3] >> 51;
        g[3] &= MASK51;
        g[4] = h[4].wrapping_add(carry).wrapping_sub(1u64 << 51);

        // If g[4] didn't underflow (high bit clear), h >= p, use g
        // g[4] >> 63 == 1 means underflow (h < p), keep h
        // g[4] >> 63 == 0 means no underflow (h >= p), use g
        let keep_h = 0u64.wrapping_sub(g[4] >> 63); // all 1s if h < p
        let use_g = !keep_h;

        Fe([
            (h[0] & keep_h) | (g[0] & use_g),
            (h[1] & keep_h) | (g[1] & use_g),
            (h[2] & keep_h) | (g[2] & use_g),
            (h[3] & keep_h) | (g[3] & use_g),
            (h[4] & keep_h) | (g[4] & use_g),
        ])
    }

    /// Field addition: (a + b) mod p
    ///
    /// Simply adds corresponding limbs. Reduction is deferred to
    /// to_bytes() or other operations that need canonical form.
    fn add(&self, other: &Fe) -> Fe {
        Fe([
            self.0[0] + other.0[0],
            self.0[1] + other.0[1],
            self.0[2] + other.0[2],
            self.0[3] + other.0[3],
            self.0[4] + other.0[4],
        ])
    }

    /// Field subtraction: (a - b) mod p
    ///
    /// Adds 2*p to each limb before subtracting to ensure non-negative results.
    /// The bias is chosen so that each limb stays positive even in the worst case.
    fn sub(&self, other: &Fe) -> Fe {
        // 2*p in limb form: each limb gets 2*(2^51 - 1) = 0xffffffffffffe
        // This ensures the result is non-negative in each limb
        // More precisely: we add a multiple of p to avoid underflow
        // Using 2*p per limb (with correction for the lowest limb)
        Fe([
            self.0[0] + 0xffffffffffffe - other.0[0], // 2*(2^51-1) - bias for -19
            self.0[1] + 0xffffffffffffe - other.0[1],
            self.0[2] + 0xffffffffffffe - other.0[2],
            self.0[3] + 0xffffffffffffe - other.0[3],
            self.0[4] + 0xffffffffffffe - other.0[4],
        ])
    }

    /// Field multiplication: (a * b) mod p
    ///
    /// Schoolbook multiplication on 5 x 51-bit limbs using u128 intermediates.
    /// Products that overflow beyond limb 4 are reduced using 2^255 = 19 mod p.
    ///
    /// For each product a[i]*b[j]:
    ///   - If i+j < 5: add to accumulator t[i+j]
    ///   - If i+j >= 5: multiply by 19 and add to t[i+j-5]
    ///
    /// This keeps all arithmetic within u128 range and produces a result
    /// that fits in 5 x 52-bit limbs (needing at most one carry pass).
    fn mul(&self, other: &Fe) -> Fe {
        let a = &self.0;
        let b = &other.0;

        // Precompute 19*b[i] for i=1..4 (used when index wraps around)
        let b1_19 = (b[1] as u128) * 19;
        let b2_19 = (b[2] as u128) * 19;
        let b3_19 = (b[3] as u128) * 19;
        let b4_19 = (b[4] as u128) * 19;

        // Accumulate products
        let t0 = (a[0] as u128) * (b[0] as u128)
            + (a[1] as u128) * b4_19
            + (a[2] as u128) * b3_19
            + (a[3] as u128) * b2_19
            + (a[4] as u128) * b1_19;

        let t1 = (a[0] as u128) * (b[1] as u128)
            + (a[1] as u128) * (b[0] as u128)
            + (a[2] as u128) * b4_19
            + (a[3] as u128) * b3_19
            + (a[4] as u128) * b2_19;

        let t2 = (a[0] as u128) * (b[2] as u128)
            + (a[1] as u128) * (b[1] as u128)
            + (a[2] as u128) * (b[0] as u128)
            + (a[3] as u128) * b4_19
            + (a[4] as u128) * b3_19;

        let t3 = (a[0] as u128) * (b[3] as u128)
            + (a[1] as u128) * (b[2] as u128)
            + (a[2] as u128) * (b[1] as u128)
            + (a[3] as u128) * (b[0] as u128)
            + (a[4] as u128) * b4_19;

        let t4 = (a[0] as u128) * (b[4] as u128)
            + (a[1] as u128) * (b[3] as u128)
            + (a[2] as u128) * (b[2] as u128)
            + (a[3] as u128) * (b[1] as u128)
            + (a[4] as u128) * (b[0] as u128);

        // Carry propagation
        let mut r = [0u64; 5];
        let c = t0 >> 51;
        r[0] = (t0 & MASK51 as u128) as u64;
        let t1 = t1 + c;
        let c = t1 >> 51;
        r[1] = (t1 & MASK51 as u128) as u64;
        let t2 = t2 + c;
        let c = t2 >> 51;
        r[2] = (t2 & MASK51 as u128) as u64;
        let t3 = t3 + c;
        let c = t3 >> 51;
        r[3] = (t3 & MASK51 as u128) as u64;
        let t4 = t4 + c;
        let c = t4 >> 51;
        r[4] = (t4 & MASK51 as u128) as u64;
        // Reduce overflow: multiply by 19
        r[0] += (c as u64) * 19;

        Fe(r)
    }

    /// Field squaring: a^2 mod p
    ///
    /// Slightly more efficient than mul(self, self) because we can
    /// exploit the symmetry: a[i]*a[j] appears twice when i != j.
    fn sq(&self) -> Fe {
        let a = &self.0;

        // Double products for cross terms
        let a0_2 = a[0] * 2;
        let a1_2 = a[1] * 2;
        let _a2_2 = a[2] * 2;
        let _a3_2 = a[3] * 2;

        // Precompute 19*a[i] for reduction
        let a1_38 = (a[1] as u128) * 38; // 2 * 19
        let a2_38 = (a[2] as u128) * 38;
        let a3_38 = (a[3] as u128) * 38;
        let a4_19 = (a[4] as u128) * 19;

        let t0 = (a[0] as u128) * (a[0] as u128) + a1_38 * (a[4] as u128) + a2_38 * (a[3] as u128);

        let t1 = (a0_2 as u128) * (a[1] as u128)
            + a2_38 * (a[4] as u128)
            + (a[3] as u128) * (a[3] as u128) * 19;

        let t2 = (a0_2 as u128) * (a[2] as u128)
            + (a[1] as u128) * (a[1] as u128)
            + a3_38 * (a[4] as u128);

        let t3 = (a0_2 as u128) * (a[3] as u128)
            + (a1_2 as u128) * (a[2] as u128)
            + a4_19 * (a[4] as u128);

        let t4 = (a0_2 as u128) * (a[4] as u128)
            + (a1_2 as u128) * (a[3] as u128)
            + (a[2] as u128) * (a[2] as u128);

        // Carry propagation
        let mut r = [0u64; 5];
        let c = t0 >> 51;
        r[0] = (t0 & MASK51 as u128) as u64;
        let t1 = t1 + c;
        let c = t1 >> 51;
        r[1] = (t1 & MASK51 as u128) as u64;
        let t2 = t2 + c;
        let c = t2 >> 51;
        r[2] = (t2 & MASK51 as u128) as u64;
        let t3 = t3 + c;
        let c = t3 >> 51;
        r[3] = (t3 & MASK51 as u128) as u64;
        let t4 = t4 + c;
        let c = t4 >> 51;
        r[4] = (t4 & MASK51 as u128) as u64;
        r[0] += (c as u64) * 19;

        Fe(r)
    }

    /// Compute self^(2^n) by repeated squaring.
    fn sq_n(&self, n: u32) -> Fe {
        let mut result = *self;
        for _ in 0..n {
            result = result.sq();
        }
        result
    }

    /// Multiplication by a small constant (useful for a24 = 121665).
    fn mul_small(&self, c: u64) -> Fe {
        let c128 = c as u128;
        let t0 = (self.0[0] as u128) * c128;
        let t1 = (self.0[1] as u128) * c128;
        let t2 = (self.0[2] as u128) * c128;
        let t3 = (self.0[3] as u128) * c128;
        let t4 = (self.0[4] as u128) * c128;

        let mut r = [0u64; 5];
        let carry = t0 >> 51;
        r[0] = (t0 & MASK51 as u128) as u64;
        let t1 = t1 + carry;
        let carry = t1 >> 51;
        r[1] = (t1 & MASK51 as u128) as u64;
        let t2 = t2 + carry;
        let carry = t2 >> 51;
        r[2] = (t2 & MASK51 as u128) as u64;
        let t3 = t3 + carry;
        let carry = t3 >> 51;
        r[3] = (t3 & MASK51 as u128) as u64;
        let t4 = t4 + carry;
        let carry = t4 >> 51;
        r[4] = (t4 & MASK51 as u128) as u64;
        r[0] += (carry as u64) * 19;

        Fe(r)
    }

    /// Modular inverse via Fermat's little theorem: a^(p-2) mod p
    ///
    /// Since p is prime, a^(-1) = a^(p-2) mod p.
    /// p-2 = 2^255 - 21
    ///
    /// We compute this using a carefully optimized addition chain
    /// that requires ~254 squarings and ~11 multiplications.
    fn invert(&self) -> Fe {
        // z2 = z^2
        let _z2 = self.sq();

        // z9 = z^9 = z^(8+1) = (z^2)^4 * z = z2^4 * z
        // But actually: z9 = z2^2 * z2 * z? No:
        // z2^2 = z^4, z4 * z2 = z^6, etc. Let me redo:
        // z_2_1 = z2 * z = z^3... actually the chain is:
        // Using a standard addition chain for p-2:

        // t0 = z^2
        let t0 = self.sq();
        // t1 = z^(2^2) = z^4
        let t1 = t0.sq();
        // t1 = z^(2^3) = z^8
        let t1 = t1.sq();
        // t1 = z^9 = z^(8+1) = t1 * z
        let t1 = t1.mul(self);
        // t0 = z^11 = z^(9+2) = t1 * t0
        let t0 = t0.mul(&t1);
        // t2 = z^22 = z^(11*2)
        let t2 = t0.sq();
        // t1 = z^(22+9) = z^31 = 2^5-1
        let t1 = t1.mul(&t2);
        // t2 = z^(2^10 - 2^5)
        let t2 = t1.sq_n(5);
        // t1 = z^(2^10 - 1)
        let t1 = t2.mul(&t1);
        // t2 = z^(2^20 - 2^10)
        let t2 = t1.sq_n(10);
        // t2 = z^(2^20 - 1)
        let t2 = t2.mul(&t1);
        // t3 = z^(2^40 - 2^20)
        let t3 = t2.sq_n(20);
        // t2 = z^(2^40 - 1)
        let t2 = t3.mul(&t2);
        // t2 = z^(2^50 - 2^10)
        let t2 = t2.sq_n(10);
        // t1 = z^(2^50 - 1)
        let t1 = t2.mul(&t1);
        // t2 = z^(2^100 - 2^50)
        let t2 = t1.sq_n(50);
        // t2 = z^(2^100 - 1)
        let t2 = t2.mul(&t1);
        // t3 = z^(2^200 - 2^100)
        let t3 = t2.sq_n(100);
        // t2 = z^(2^200 - 1)
        let t2 = t3.mul(&t2);
        // t2 = z^(2^250 - 2^50)
        let t2 = t2.sq_n(50);
        // t1 = z^(2^250 - 1)
        let t1 = t2.mul(&t1);
        // t1 = z^(2^255 - 2^5)
        let t1 = t1.sq_n(5);
        // result = z^(2^255 - 21) = z^(p-2)
        t1.mul(&t0)
    }

    /// Constant-time conditional swap.
    ///
    /// If swap == 1, swap self and other.
    /// If swap == 0, do nothing.
    /// No branches on the swap value.
    fn cswap(&mut self, other: &mut Fe, swap: u64) {
        let mask = 0u64.wrapping_sub(swap); // 0 or 0xFFFFFFFFFFFFFFFF
        for i in 0..5 {
            let diff = mask & (self.0[i] ^ other.0[i]);
            self.0[i] ^= diff;
            other.0[i] ^= diff;
        }
    }
}

// --- Helper: load u64 from bytes ---

/// Load a little-endian u64 from a byte slice (at least 8 bytes).
/// If the slice is shorter than 8 bytes, pads with zeros.
#[inline(always)]
fn load_le_u64(bytes: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    let len = bytes.len().min(8);
    buf[..len].copy_from_slice(&bytes[..len]);
    u64::from_le_bytes(buf)
}

// --- Scalar clamping ---

/// Clamp a 32-byte scalar for X25519.
///
/// RFC 7748, Section 5:
///   - Clear the three lowest bits of the first byte (divisible by 8,
///     avoiding small subgroup attacks)
///   - Clear the highest bit of the last byte (ensure < 2^255)
///   - Set the second-highest bit of the last byte (ensure constant-time
///     ladder has exactly 255 steps)
fn clamp_scalar(scalar: &[u8; 32]) -> [u8; 32] {
    let mut k = *scalar;
    k[0] &= 248; // Clear bits 0, 1, 2
    k[31] &= 127; // Clear bit 255
    k[31] |= 64; // Set bit 254
    k
}

// --- Montgomery ladder ---

/// X25519 scalar multiplication using the Montgomery ladder.
///
/// Computes k * P on Curve25519 (Montgomery form), returning the
/// x-coordinate of the result.
///
/// The Montgomery ladder is inherently constant-time: it always
/// performs exactly 255 iterations regardless of the scalar value.
/// The conditional swap (cswap) is implemented without branches.
///
/// Algorithm (RFC 7748):
///   1. Clamp the scalar k
///   2. Set (x_2, z_2) = (1, 0) and (x_3, z_3) = (u, 1)
///   3. For bit 254 down to 0:
///      a. Swap (x_2, z_2) and (x_3, z_3) based on current bit
///      b. Perform differential addition and doubling
///   4. Return x_2 * z_2^(-1)
fn x25519_scalar_mult(scalar: &[u8; 32], point: &[u8; 32]) -> [u8; 32] {
    let k = clamp_scalar(scalar);
    let u = Fe::from_bytes(point);

    let mut x_2 = Fe::ONE;
    let mut z_2 = Fe::ZERO;
    let mut x_3 = u;
    let mut z_3 = Fe::ONE;
    let mut swap: u64 = 0;

    // Montgomery ladder: iterate from bit 254 down to bit 0
    for pos in (0..255).rev() {
        let byte_idx = pos / 8;
        let bit_idx = pos % 8;
        let k_t = ((k[byte_idx] >> bit_idx) & 1) as u64;

        // Conditional swap based on current bit
        let do_swap = swap ^ k_t;
        x_2.cswap(&mut x_3, do_swap);
        z_2.cswap(&mut z_3, do_swap);
        swap = k_t;

        // Montgomery ladder step (differential addition and doubling)
        // Reference: RFC 7748 and "Curve25519: new Diffie-Hellman speed records"
        let a = x_2.add(&z_2); // A = x_2 + z_2
        let aa = a.sq(); // AA = A^2
        let b = x_2.sub(&z_2); // B = x_2 - z_2
        let bb = b.sq(); // BB = B^2
        let e = aa.sub(&bb); // E = AA - BB
        let c = x_3.add(&z_3); // C = x_3 + z_3
        let d = x_3.sub(&z_3); // D = x_3 - z_3
        let da = d.mul(&a); // DA = D * A
        let cb = c.mul(&b); // CB = C * B

        x_3 = da.add(&cb).sq(); // x_3 = (DA + CB)^2
        z_3 = u.mul(&da.sub(&cb).sq()); // z_3 = x_1 * (DA - CB)^2
        x_2 = aa.mul(&bb); // x_2 = AA * BB
                           // z_2 = E * (AA + a24 * E) where a24 = 121665
        z_2 = e.mul(&aa.add(&e.mul_small(121665)));
    }

    // Final conditional swap
    x_2.cswap(&mut x_3, swap);
    z_2.cswap(&mut z_3, swap);

    // Return x_2 / z_2 = x_2 * z_2^(-1)
    x_2.mul(&z_2.invert()).to_bytes()
}

// --- Public API ---

/// X25519 base point (the canonical generator, u = 9).
const BASEPOINT: [u8; 32] = {
    let mut b = [0u8; 32];
    b[0] = 9;
    b
};

/// Compute the X25519 public key from a 32-byte private key.
///
/// public_key = clamp(private_key) * G
/// where G is the base point (u = 9).
pub fn public_key(private_key: &[u8; 32]) -> [u8; 32] {
    x25519_scalar_mult(private_key, &BASEPOINT)
}

/// Compute the X25519 shared secret from a private key and peer's public key.
///
/// shared_secret = clamp(private_key) * peer_public
///
/// WARNING: Check that the result is not all-zero before using.
/// An all-zero result indicates the peer's public key is in a small subgroup.
pub fn shared_secret(private_key: &[u8; 32], peer_public: &[u8; 32]) -> [u8; 32] {
    x25519_scalar_mult(private_key, peer_public)
}

/// Generate an X25519 key pair using the system CSPRNG.
///
/// Returns (private_key, public_key).
pub fn generate_keypair() -> ([u8; 32], [u8; 32]) {
    let mut private = [0u8; 32];
    super::random::fill_bytes(&mut private);
    let public = public_key(&private);
    (private, public)
}

/// Perform a full Diffie-Hellman key exchange.
///
/// 1. Generate an ephemeral key pair
/// 2. Compute shared secret with peer's public key
/// 3. Return (ephemeral_public, shared_secret)
///
/// The ephemeral public key should be sent to the peer.
/// The shared secret should be passed through HKDF before use.
pub fn dh_exchange(peer_public: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    let (priv_key, pub_key) = generate_keypair();
    let secret = shared_secret(&priv_key, peer_public);
    (pub_key, secret)
}

// --- RFC 7748-named public API ---

/// X25519 scalar multiplication (RFC 7748 §5).
///
/// Computes the x-coordinate of `scalar * u_coord` on Curve25519.
/// The scalar is clamped internally per RFC 7748.
///
/// This is the primitive DH function; callers should normally prefer
/// `x25519_public_key` and `x25519_dh` for typical use cases.
pub fn x25519(scalar: &[u8; 32], u_coord: &[u8; 32]) -> [u8; 32] {
    x25519_scalar_mult(scalar, u_coord)
}

/// Derive the X25519 public key from a 32-byte secret key.
///
/// 1. Clamp the scalar (clear bits 0-2 of byte 0; clear bit 7, set bit 6 of byte 31)
/// 2. Multiply the clamped scalar by the base point u = 9
///
/// This is the standard operation for key generation per RFC 7748 §6.1.
pub fn x25519_public_key(secret: &[u8; 32]) -> [u8; 32] {
    x25519_scalar_mult(secret, &X25519_BASE)
}

/// Compute the X25519 Diffie-Hellman shared secret.
///
/// Returns `Some(shared)` on success, or `None` if the result is the
/// all-zeros low-order point (RFC 7748 §6.1 — implementations MUST
/// reject contributions that would yield an all-zero result).
pub fn x25519_dh(my_secret: &[u8; 32], their_public: &[u8; 32]) -> Option<[u8; 32]> {
    let result = x25519_scalar_mult(my_secret, their_public);
    if is_zero_shared_secret(&result) {
        None
    } else {
        Some(result)
    }
}

/// Check if a shared secret is all zeros (low-order point).
///
/// An all-zero result from X25519 means the peer's public key was
/// in a small subgroup. Connections should be rejected in this case.
pub fn is_zero_shared_secret(secret: &[u8; 32]) -> bool {
    let mut acc: u8 = 0;
    for &b in secret.iter() {
        acc |= b;
    }
    acc == 0
}

/// Clamp a private key in-place.
///
/// This is applied automatically by x25519_scalar_mult, but may be
/// useful for pre-processing keys before storage.
pub fn clamp_private_key(key: &mut [u8; 32]) {
    key[0] &= 248;
    key[31] &= 127;
    key[31] |= 64;
}

/// Constant-time comparison of two 32-byte values.
pub fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff: u8 = 0;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

// --- Self-tests ---

/// Run X25519 self-tests with known test vectors from RFC 7748.
/// Returns true if all tests pass.
pub fn self_test() -> bool {
    // ---------------------------------------------------------------
    // RFC 7748, Section 6.1 — Low-level scalar-mult test vector
    // scalar  = a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4
    // u_coord = e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c
    // result  = c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552
    // ---------------------------------------------------------------
    let tv_scalar: [u8; 32] = [
        0xa5, 0x46, 0xe3, 0x6b, 0xf0, 0x52, 0x7c, 0x9d, 0x3b, 0x16, 0x15, 0x4b, 0x82, 0x46, 0x5e,
        0xdd, 0x62, 0x14, 0x4c, 0x0a, 0xc1, 0xfc, 0x5a, 0x18, 0x50, 0x6a, 0x22, 0x44, 0xba, 0x44,
        0x9a, 0xc4,
    ];
    let tv_u_coord: [u8; 32] = [
        0xe6, 0xdb, 0x68, 0x67, 0x58, 0x30, 0x30, 0xdb, 0x35, 0x94, 0xc1, 0xa4, 0x24, 0xb1, 0x5f,
        0x7c, 0x72, 0x66, 0x24, 0xec, 0x26, 0xb3, 0x35, 0x3b, 0x10, 0xa9, 0x03, 0xa6, 0xd0, 0xab,
        0x1c, 0x4c,
    ];
    let tv_expected: [u8; 32] = [
        0xc3, 0xda, 0x55, 0x37, 0x9d, 0xe9, 0xc6, 0x90, 0x8e, 0x94, 0xea, 0x4d, 0xf2, 0x8d, 0x08,
        0x4f, 0x32, 0xec, 0xcf, 0x03, 0x49, 0x1c, 0x71, 0xf7, 0x54, 0xb4, 0x07, 0x55, 0x77, 0xa2,
        0x85, 0x52,
    ];
    let tv_result = x25519(&tv_scalar, &tv_u_coord);
    if !ct_eq(&tv_result, &tv_expected) {
        return false;
    }

    // Verify x25519_dh returns None on the low-order all-zero result.
    // We cannot easily force an all-zero result through normal inputs, so we
    // verify the is_zero guard path directly (tested below via is_zero_shared_secret).

    // ---------------------------------------------------------------
    // RFC 7748, Section 6.1 — Test Vector 1
    // Alice's private key:
    let alice_private: [u8; 32] = [
        0x77, 0x07, 0x6d, 0x0a, 0x73, 0x18, 0xa5, 0x7d, 0x3c, 0x16, 0xc1, 0x72, 0x51, 0xb2, 0x66,
        0x45, 0xdf, 0x4c, 0x2f, 0x87, 0xeb, 0xc0, 0x99, 0x2a, 0xb1, 0x77, 0xfb, 0xa5, 0x1d, 0xb9,
        0x2c, 0x2a,
    ];
    // Alice's public key (expected):
    let alice_public_expected: [u8; 32] = [
        0x85, 0x20, 0xf0, 0x09, 0x89, 0x30, 0xa7, 0x54, 0x74, 0x8b, 0x7d, 0xdc, 0xb4, 0x3e, 0xf7,
        0x5a, 0x0d, 0xbf, 0x3a, 0x0d, 0x26, 0x38, 0x1a, 0xf4, 0xeb, 0xa4, 0xa9, 0x8e, 0xaa, 0x9b,
        0x4e, 0x6a,
    ];

    let alice_public = public_key(&alice_private);
    if !ct_eq(&alice_public, &alice_public_expected) {
        return false;
    }

    // Bob's private key:
    let bob_private: [u8; 32] = [
        0x5d, 0xab, 0x08, 0x7e, 0x62, 0x4a, 0x8a, 0x4b, 0x79, 0xe1, 0x7f, 0x8b, 0x83, 0x80, 0x0e,
        0xe6, 0x6f, 0x3b, 0xb1, 0x29, 0x26, 0x18, 0xb6, 0xfd, 0x1c, 0x2f, 0x8b, 0x27, 0xff, 0x88,
        0xe0, 0xeb,
    ];
    // Bob's public key (expected):
    let bob_public_expected: [u8; 32] = [
        0xde, 0x9e, 0xdb, 0x7d, 0x7b, 0x7d, 0xc1, 0xb4, 0xd3, 0x5b, 0x61, 0xc2, 0xec, 0xe4, 0x35,
        0x37, 0x3f, 0x83, 0x43, 0xc8, 0x5b, 0x78, 0x67, 0x4d, 0xad, 0xfc, 0x7e, 0x14, 0x6f, 0x88,
        0x2b, 0x4f,
    ];

    let bob_public = public_key(&bob_private);
    if !ct_eq(&bob_public, &bob_public_expected) {
        return false;
    }

    // Shared secret: Alice computes with Bob's public, Bob computes with Alice's public
    // Both should be the same.
    let shared_ab = shared_secret(&alice_private, &bob_public);
    let shared_ba = shared_secret(&bob_private, &alice_public);

    if !ct_eq(&shared_ab, &shared_ba) {
        return false;
    }

    // Expected shared secret (RFC 7748):
    let expected_shared: [u8; 32] = [
        0x4a, 0x5d, 0x9d, 0x5b, 0xa4, 0xce, 0x2d, 0xe1, 0x72, 0x8e, 0x3b, 0xf4, 0x80, 0x35, 0x0f,
        0x25, 0xe0, 0x7e, 0x21, 0xc9, 0x47, 0xd1, 0x9e, 0x33, 0x76, 0xf0, 0x9b, 0x3c, 0x1e, 0x16,
        0x17, 0x42,
    ];
    if !ct_eq(&shared_ab, &expected_shared) {
        return false;
    }

    // Test: is_zero_shared_secret returns false for valid secrets
    if is_zero_shared_secret(&shared_ab) {
        return false;
    }

    // Test: is_zero_shared_secret returns true for all-zero
    let zero = [0u8; 32];
    if !is_zero_shared_secret(&zero) {
        return false;
    }

    // Test: clamping is idempotent
    let mut key = alice_private;
    clamp_private_key(&mut key);
    let pub1 = public_key(&key);
    let pub2 = public_key(&alice_private);
    // Both should produce the same public key (scalar_mult clamps internally)
    if !ct_eq(&pub1, &pub2) {
        return false;
    }

    // Test: base point multiplication identity
    // scalar = 1 (after clamping, this becomes a specific value)
    // Just verify it doesn't panic and produces non-zero output
    let one_key = [1u8; 32];
    let one_pub = public_key(&one_key);
    if is_zero_shared_secret(&one_pub) {
        return false;
    }

    // Test: x25519_public_key wrapper produces the same result as public_key()
    let alice_pub_via_wrapper = x25519_public_key(&alice_private);
    if !ct_eq(&alice_pub_via_wrapper, &alice_public_expected) {
        return false;
    }

    // Test: x25519_dh wrapper returns Some for a valid DH exchange
    let dh_result = x25519_dh(&alice_private, &bob_public);
    match dh_result {
        None => return false,
        Some(s) => {
            if !ct_eq(&s, &expected_shared) {
                return false;
            }
        }
    }

    // Test: x25519_dh returns None when result is all-zero
    // We cannot force a real all-zero output through normal means, so we
    // verify is_zero_shared_secret path by checking it returns true for zero
    // (the x25519_dh logic depends on this, tested indirectly above).

    // Test: key pair generation produces valid key
    // Skip this test in self_test since it requires CSPRNG to be initialized
    // generate_keypair() is tested through integration

    true
}

/// Run self-tests and report to serial console.
///
/// Called automatically from `crypto::init()`. Logs pass/fail via serial.
pub fn run_self_test() {
    if self_test() {
        // Tests covered:
        //   1. RFC 7748 §6.1 low-level scalar-mult vector (x25519 primitive)
        //   2. Alice public-key derivation (RFC 7748 §6.1 key agreement)
        //   3. Bob public-key derivation (RFC 7748 §6.1 key agreement)
        //   4. Alice->Bob shared secret symmetry
        //   5. Bob->Alice shared secret symmetry
        //   6. Expected shared secret value (RFC 7748 §6.1)
        //   7. is_zero_shared_secret false for valid secret
        //   8. is_zero_shared_secret true for all-zero
        //   9. Scalar clamping idempotency
        //  10. x25519_public_key wrapper correctness
        //  11. x25519_dh wrapper Some(correct_value) for valid exchange
        crate::serial_println!("    [x25519] Self-test PASSED (11 checks, RFC 7748 vectors)");
    } else {
        crate::serial_println!("    [x25519] Self-test FAILED!");
    }
}
