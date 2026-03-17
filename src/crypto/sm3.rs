/// SM3 — Chinese National Standard Hash (GB/T 32907-2016)
///
/// Produces a 256-bit digest from arbitrary-length input.
/// Structure mirrors SHA-256 (Merkle-Damgård, 512-bit blocks, 64 rounds)
/// but uses distinct constants, nonlinear functions, and a richer message
/// schedule with two expansion arrays W[68] and W'[64].
///
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Initial hash values H0..H7
const H0: [u32; 8] = [
    0x7380166f, 0x4914b2b9, 0x172442d7, 0xda8a0600, 0xa96f30bc, 0x163138aa, 0xe38dee4d, 0xb0fb0e4e,
];

/// Round constants T_j: T[j] = 79cc4519 for j in 0..16, 7a879d8a for j in 16..64
#[inline(always)]
const fn t_const(j: usize) -> u32 {
    if j < 16 {
        0x79cc4519
    } else {
        0x7a879d8a
    }
}

// ---------------------------------------------------------------------------
// Rotate left
// ---------------------------------------------------------------------------

#[inline(always)]
fn rotl(x: u32, n: u32) -> u32 {
    x.rotate_left(n)
}

// ---------------------------------------------------------------------------
// Boolean functions
// ---------------------------------------------------------------------------

/// FF_j: XOR for j<16, majority for j>=16
#[inline(always)]
fn ff(x: u32, y: u32, z: u32, j: usize) -> u32 {
    if j < 16 {
        x ^ y ^ z
    } else {
        (x & y) | (x & z) | (y & z)
    }
}

/// GG_j: XOR for j<16, choose for j>=16
#[inline(always)]
fn gg(x: u32, y: u32, z: u32, j: usize) -> u32 {
    if j < 16 {
        x ^ y ^ z
    } else {
        (x & y) | (!x & z)
    }
}

// ---------------------------------------------------------------------------
// Permutation functions
// ---------------------------------------------------------------------------

/// P0: used in compression
#[inline(always)]
fn p0(x: u32) -> u32 {
    x ^ rotl(x, 9) ^ rotl(x, 17)
}

/// P1: used in message expansion
#[inline(always)]
fn p1(x: u32) -> u32 {
    x ^ rotl(x, 15) ^ rotl(x, 23)
}

// ---------------------------------------------------------------------------
// Message expansion: W[68] and W'[64]
// ---------------------------------------------------------------------------

fn expand(block: &[u8; 64], w: &mut [u32; 68], wp: &mut [u32; 64]) {
    // Load 16 big-endian words
    let mut j = 0usize;
    while j < 16 {
        let off = j * 4;
        w[j] = u32::from_be_bytes([block[off], block[off + 1], block[off + 2], block[off + 3]]);
        j = j.saturating_add(1);
    }
    // Expand to W[68]
    while j < 68 {
        w[j] = p1(w[j - 16] ^ w[j - 9] ^ rotl(w[j - 3], 15)) ^ rotl(w[j - 13], 7) ^ w[j - 6];
        j = j.saturating_add(1);
    }
    // Derive W'[64]
    let mut k = 0usize;
    while k < 64 {
        wp[k] = w[k] ^ w[k + 4];
        k = k.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Compression function
// ---------------------------------------------------------------------------

fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 68];
    let mut wp = [0u32; 64];
    expand(block, &mut w, &mut wp);

    let mut a = state[0];
    let mut b = state[1];
    let mut c = state[2];
    let mut d = state[3];
    let mut e = state[4];
    let mut f = state[5];
    let mut g = state[6];
    let mut h = state[7];

    let mut j = 0usize;
    while j < 64 {
        let tj = t_const(j);
        let ss1 = rotl(
            rotl(a, 12)
                .wrapping_add(e)
                .wrapping_add(rotl(tj, j as u32 & 31)),
            7,
        );
        let ss2 = ss1 ^ rotl(a, 12);
        let tt1 = ff(a, b, c, j)
            .wrapping_add(d)
            .wrapping_add(ss2)
            .wrapping_add(wp[j]);
        let tt2 = gg(e, f, g, j)
            .wrapping_add(h)
            .wrapping_add(ss1)
            .wrapping_add(w[j]);
        d = c;
        c = rotl(b, 9);
        b = a;
        a = tt1;
        h = g;
        g = rotl(f, 19);
        f = e;
        e = p0(tt2);
        j = j.saturating_add(1);
    }

    state[0] ^= a;
    state[1] ^= b;
    state[2] ^= c;
    state[3] ^= d;
    state[4] ^= e;
    state[5] ^= f;
    state[6] ^= g;
    state[7] ^= h;
}

// ---------------------------------------------------------------------------
// SM3 context (no heap)
// ---------------------------------------------------------------------------

pub struct Sm3Context {
    state: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    bit_len: u64,
}

impl Sm3Context {
    pub const fn new() -> Self {
        Sm3Context {
            state: H0,
            buf: [0u8; 64],
            buf_len: 0,
            bit_len: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        let mut i = 0usize;
        while i < data.len() {
            self.buf[self.buf_len] = data[i];
            self.buf_len = self.buf_len.saturating_add(1);
            self.bit_len = self.bit_len.wrapping_add(8);
            if self.buf_len == 64 {
                let block = self.buf;
                compress(&mut self.state, &block);
                self.buf_len = 0;
            }
            i = i.saturating_add(1);
        }
    }

    /// Produce the 32-byte digest. Consumes self (SM3 padding is destructive).
    pub fn finalize(mut self) -> [u8; 32] {
        // Append bit 1
        self.buf[self.buf_len] = 0x80;
        self.buf_len = self.buf_len.saturating_add(1);

        // If not enough room for 8-byte length, flush and start new block
        if self.buf_len > 56 {
            // zero-fill rest of current block
            let mut k = self.buf_len;
            while k < 64 {
                self.buf[k] = 0;
                k = k.saturating_add(1);
            }
            let block = self.buf;
            compress(&mut self.state, &block);
            self.buf_len = 0;
        }

        // Zero-fill up to byte 56
        let mut k = self.buf_len;
        while k < 56 {
            self.buf[k] = 0;
            k = k.saturating_add(1);
        }

        // Append message length in bits (big-endian 64-bit)
        let lb = self.bit_len.to_be_bytes();
        let mut k = 0usize;
        while k < 8 {
            self.buf[56 + k] = lb[k];
            k = k.saturating_add(1);
        }

        let block = self.buf;
        compress(&mut self.state, &block);

        // Serialize state big-endian
        let mut out = [0u8; 32];
        let mut i = 0usize;
        while i < 8 {
            let b = self.state[i].to_be_bytes();
            let mut j = 0usize;
            while j < 4 {
                out[i * 4 + j] = b[j];
                j = j.saturating_add(1);
            }
            i = i.saturating_add(1);
        }
        out
    }
}

// ---------------------------------------------------------------------------
// One-shot hash
// ---------------------------------------------------------------------------

pub fn sm3(data: &[u8]) -> [u8; 32] {
    let mut ctx = Sm3Context::new();
    ctx.update(data);
    ctx.finalize()
}

// ---------------------------------------------------------------------------
// HMAC-SM3
// ---------------------------------------------------------------------------

pub fn hmac_sm3(key: &[u8], data: &[u8]) -> [u8; 32] {
    // Key normalization: hash if > 64 bytes, else zero-pad to 64
    let mut k = [0u8; 64];
    if key.len() > 64 {
        let h = sm3(key);
        let mut i = 0usize;
        while i < 32 {
            k[i] = h[i];
            i = i.saturating_add(1);
        }
    } else {
        let mut i = 0usize;
        while i < key.len() {
            k[i] = key[i];
            i = i.saturating_add(1);
        }
    }

    // ipad / opad
    let mut ipad = [0u8; 64];
    let mut opad = [0u8; 64];
    let mut i = 0usize;
    while i < 64 {
        ipad[i] = k[i] ^ 0x36;
        opad[i] = k[i] ^ 0x5c;
        i = i.saturating_add(1);
    }

    // inner = SM3(ipad || data)
    let mut ctx = Sm3Context::new();
    ctx.update(&ipad);
    ctx.update(data);
    let inner = ctx.finalize();

    // outer = SM3(opad || inner)
    let mut ctx2 = Sm3Context::new();
    ctx2.update(&opad);
    ctx2.update(&inner);
    ctx2.finalize()
}

// ---------------------------------------------------------------------------
// Self-test (known-answer from GB/T 32907 standard)
// ---------------------------------------------------------------------------

/// SM3("abc") must equal 66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0
fn selftest() -> bool {
    let h = sm3(b"abc");
    let expected: [u8; 32] = [
        0x66, 0xc7, 0xf0, 0xf4, 0x62, 0xee, 0xed, 0xd9, 0xd1, 0xf2, 0xd4, 0x6b, 0xdc, 0x10, 0xe4,
        0xe2, 0x41, 0x67, 0xc4, 0x87, 0x5c, 0xf2, 0xf7, 0xa2, 0x29, 0x7d, 0xa0, 0x2b, 0x8f, 0x4b,
        0xa8, 0xe0,
    ];
    let mut i = 0usize;
    while i < 32 {
        if h[i] != expected[i] {
            return false;
        }
        i = i.saturating_add(1);
    }
    true
}

pub fn init() {
    if selftest() {
        serial_println!("[sm3] SM3 hash (GB/T 32907-2016) initialized — KAT passed");
    } else {
        serial_println!("[sm3] SM3 SELF-TEST FAILED");
    }
}
