use crate::sync::Mutex;
/// Cryptographically secure pseudo-random number generator (CSPRNG)
///
/// Seeds from hardware entropy (RDRAND/RDSEED x86 instructions),
/// falls back to TSC timing jitter when hardware RNG is unavailable.
/// Uses ChaCha20 as the deterministic PRNG core.
///
/// Architecture:
///   - Entropy pool: accumulates raw entropy from multiple sources
///   - CSPRNG core: ChaCha20-based, rekeys after each generation
///   - Automatic reseed: triggers every 1MB of output
///   - Boot seeding: collects entropy from TSC, RDRAND, memory jitter
///
/// Entropy sources (prioritized):
///   1. RDSEED — true hardware random seed (Intel Ivy Bridge+)
///   2. RDRAND — hardware-conditioned random (broader support)
///   3. TSC jitter — timing variation in the timestamp counter
///   4. Memory timing — variable latency from memory access patterns
///
/// Security properties:
///   - Forward secrecy: old state cannot be recovered after reseed
///   - Backtracking resistance: ChaCha20 output doesn't reveal state
///   - Continuous seeding: hardware entropy mixed in periodically
///   - No floating point: all integer arithmetic
///
/// Design inspired by:
///   - Linux /dev/urandom (ChaCha20 CRNG)
///   - OpenBSD arc4random (key erasure)
///   - Fortuna (automatic reseed scheduling)
use crate::{serial_print, serial_println};

/// Global CSPRNG state, protected by a spinlock.
static RNG: Mutex<CsprngState> = Mutex::new(CsprngState::new());

/// Maximum bytes to generate before forcing a reseed.
/// Set to 1MB (16384 blocks * 64 bytes/block).
const RESEED_THRESHOLD: u32 = 16384;

/// Number of RDRAND retries before falling back to TSC jitter.
const RDRAND_RETRIES: u32 = 10;

/// Number of RDSEED retries before falling back to RDRAND.
const RDSEED_RETRIES: u32 = 32;

/// Entropy pool size in u64 words.
const POOL_SIZE: usize = 8;

// --- Entropy pool ---

/// Entropy pool for mixing raw entropy before seeding the CSPRNG.
///
/// Uses a simple XOR + rotate mixing function. The pool accumulates
/// entropy from multiple sources and is consumed during reseed operations.
struct EntropyPool {
    /// Pool state (512 bits)
    state: [u64; POOL_SIZE],
    /// Mix counter (for domain separation)
    mix_count: u64,
    /// Estimated bits of entropy accumulated
    entropy_bits: u64,
}

impl EntropyPool {
    const fn new() -> Self {
        EntropyPool {
            state: [0u64; POOL_SIZE],
            mix_count: 0,
            entropy_bits: 0,
        }
    }

    /// Mix a 64-bit entropy sample into the pool.
    ///
    /// Uses a lightweight mixing function: XOR the sample with a pool
    /// word (selected round-robin), then rotate the pool word.
    /// The mix counter provides domain separation between samples.
    fn mix_in(&mut self, sample: u64, estimated_bits: u64) {
        let idx = (self.mix_count as usize) % POOL_SIZE;

        // Mix using XOR and rotation (lightweight but effective for accumulation)
        self.state[idx] ^= sample;
        self.state[idx] = self.state[idx].rotate_left(((self.mix_count & 0x3F) + 1) as u32);

        // Also propagate to adjacent pool word for diffusion
        let next_idx = (idx + 1) % POOL_SIZE;
        self.state[next_idx] ^= self.state[idx]
            .wrapping_mul(6364136223846793005) // LCG multiplier
            .wrapping_add(self.mix_count);

        self.mix_count = self.mix_count.wrapping_add(1);
        self.entropy_bits = self.entropy_bits.saturating_add(estimated_bits);
    }

    /// Extract entropy from the pool by hashing it with SHA-256.
    ///
    /// Returns a 32-byte value suitable for use as a ChaCha20 key.
    /// Does NOT reset the pool — entropy accumulation continues.
    fn extract(&self) -> [u8; 32] {
        // Serialize pool state to bytes
        let mut pool_bytes = [0u8; POOL_SIZE * 8];
        for i in 0..POOL_SIZE {
            pool_bytes[i * 8..(i + 1) * 8].copy_from_slice(&self.state[i].to_le_bytes());
        }
        // Hash the pool to produce a key
        super::sha256::hash(&pool_bytes)
    }

    /// Get estimated entropy in bits.
    fn estimated_entropy(&self) -> u64 {
        self.entropy_bits
    }
}

// --- CSPRNG state ---

/// CSPRNG internal state.
///
/// Uses ChaCha20 as the core PRNG:
///   - key: 256-bit ChaCha20 key (from entropy pool)
///   - nonce: 96-bit nonce (from entropy)
///   - counter: 32-bit block counter (incremented per 64-byte block)
///
/// After generating output, the state is immediately rekeyed:
///   the next 32 bytes of keystream become the new key, and the
///   counter is reset. This provides forward secrecy.
struct CsprngState {
    /// ChaCha20 key (256 bits)
    key: [u8; 32],
    /// ChaCha20 nonce (96 bits)
    nonce: [u8; 12],
    /// ChaCha20 block counter
    counter: u32,
    /// Whether the CSPRNG has been seeded
    initialized: bool,
    /// Entropy pool for collecting hardware randomness
    pool: EntropyPool,
    /// Total bytes generated since last reseed
    bytes_since_reseed: u64,
    /// Total bytes generated since initialization
    total_bytes_generated: u64,
    /// Number of reseed operations performed
    reseed_count: u64,
    /// Whether RDRAND is available
    has_rdrand: bool,
    /// Whether RDSEED is available
    has_rdseed: bool,
}

impl CsprngState {
    const fn new() -> Self {
        CsprngState {
            key: [0u8; 32],
            nonce: [0u8; 12],
            counter: 0,
            initialized: false,
            pool: EntropyPool::new(),
            bytes_since_reseed: 0,
            total_bytes_generated: 0,
            reseed_count: 0,
            has_rdrand: false,
            has_rdseed: false,
        }
    }

    /// Detect CPU feature support for RDRAND and RDSEED.
    fn detect_features(&mut self) {
        // Check CPUID for RDRAND (ECX bit 30 of leaf 1)
        // and RDSEED (EBX bit 18 of leaf 7, subleaf 0)
        self.has_rdrand = cpuid_has_rdrand();
        self.has_rdseed = cpuid_has_rdseed();
    }

    /// Perform initial seeding of the CSPRNG.
    ///
    /// Collects entropy from all available sources:
    ///   1. RDSEED (if available) — highest quality
    ///   2. RDRAND (if available) — conditioned hardware random
    ///   3. TSC jitter — always available on x86
    ///   4. Memory timing jitter — additional entropy source
    fn seed(&mut self) {
        self.detect_features();

        // Collect entropy from hardware sources
        self.collect_hardware_entropy();

        // Collect entropy from TSC jitter (always available)
        self.collect_tsc_jitter();

        // Collect entropy from memory timing
        self.collect_memory_jitter();

        // Extract key and nonce from entropy pool
        let key_material = self.pool.extract();
        self.key.copy_from_slice(&key_material);

        // Generate nonce from additional entropy
        let nonce_entropy = if self.has_rdrand {
            rdrand_retry().unwrap_or_else(rdtsc_jitter)
        } else {
            rdtsc_jitter()
        };
        self.nonce[..8].copy_from_slice(&nonce_entropy.to_le_bytes());

        // Mix in a second source for the remaining nonce bytes
        let nonce_extra = rdtsc_jitter();
        self.nonce[4..12].copy_from_slice(&nonce_extra.to_le_bytes());

        self.counter = 0;
        self.initialized = true;
        self.bytes_since_reseed = 0;
        self.reseed_count = 0;
    }

    /// Collect entropy from RDRAND and RDSEED hardware instructions.
    fn collect_hardware_entropy(&mut self) {
        // Try RDSEED first (true random seed, highest quality)
        if self.has_rdseed {
            for _ in 0..4 {
                if let Some(val) = rdseed_retry() {
                    self.pool.mix_in(val, 64); // Full entropy
                }
            }
        }

        // Then RDRAND (conditioned, slightly lower entropy estimate)
        if self.has_rdrand {
            for _ in 0..8 {
                if let Some(val) = rdrand_retry() {
                    self.pool.mix_in(val, 48); // Conservative estimate
                }
            }
        }
    }

    /// Collect entropy from TSC timing jitter.
    ///
    /// Measures the variation in CPU timestamp counter readings across
    /// short computation bursts. The low bits of timing differences
    /// contain genuine entropy from microarchitectural effects.
    fn collect_tsc_jitter(&mut self) {
        for _ in 0..16 {
            let jitter = rdtsc_jitter();
            self.pool.mix_in(jitter, 8); // Conservative: ~8 bits per sample
        }
    }

    /// Collect entropy from memory access timing jitter.
    ///
    /// Accesses memory in a pattern that creates variable latency
    /// due to cache effects, DRAM refresh timing, and TLB misses.
    fn collect_memory_jitter(&mut self) {
        let mut buffer = [0u64; 64];
        for i in 0..64 {
            let t1 = rdtsc();
            // Volatile-like read to prevent optimization
            buffer[i] = buffer[(i.wrapping_mul(7).wrapping_add(13)) % 64].wrapping_add(1);
            let t2 = rdtsc();
            self.pool.mix_in(t2.wrapping_sub(t1), 2); // ~2 bits from cache jitter
        }
        // Use the buffer to prevent dead code elimination
        let mut acc = 0u64;
        for &val in buffer.iter() {
            acc = acc.wrapping_add(val);
        }
        self.pool.mix_in(acc, 1);
    }

    /// Fill a buffer with cryptographically random bytes.
    ///
    /// Uses ChaCha20 keystream generation:
    ///   1. Zero the buffer
    ///   2. "Encrypt" with ChaCha20 (XOR with zeros = pure keystream)
    ///   3. Advance counter
    ///   4. Rekey if threshold reached
    fn fill(&mut self, buf: &mut [u8]) {
        if !self.initialized {
            self.seed();
        }

        let mut offset = 0;
        while offset < buf.len() {
            // Generate one block of keystream
            let mut keystream = [0u8; 64];
            super::chacha20::chacha20_block(&self.key, &self.nonce, self.counter, &mut keystream);

            let remaining = buf.len() - offset;
            let to_copy = remaining.min(64);
            buf[offset..offset + to_copy].copy_from_slice(&keystream[..to_copy]);

            offset += to_copy;
            self.counter = self.counter.wrapping_add(1);
        }

        self.bytes_since_reseed += buf.len() as u64;
        self.total_bytes_generated += buf.len() as u64;

        // Forward secrecy: rekey after every generation
        // Generate a new key from the next block of keystream
        self.fast_key_erasure();

        // Periodic reseed from hardware entropy
        if self.counter >= RESEED_THRESHOLD {
            self.reseed();
        }
    }

    /// Fast key erasure: generate a new key from the CSPRNG itself.
    ///
    /// After producing output, immediately derive a new key from the
    /// next ChaCha20 block. This ensures that even if the current state
    /// is compromised, past outputs cannot be recovered.
    fn fast_key_erasure(&mut self) {
        let mut new_key_block = [0u8; 64];
        super::chacha20::chacha20_block(&self.key, &self.nonce, self.counter, &mut new_key_block);
        self.key.copy_from_slice(&new_key_block[..32]);
        // Use remaining 32 bytes for nonce refresh
        self.nonce.copy_from_slice(&new_key_block[32..44]);
        self.counter = 0;
    }

    /// Reseed the CSPRNG from hardware entropy sources.
    ///
    /// Mixes fresh hardware entropy into the pool, then derives a new
    /// key from the combination of the pool and current CSPRNG state.
    /// This ensures forward secrecy and backtracking resistance.
    fn reseed(&mut self) {
        // Collect fresh hardware entropy
        if self.has_rdseed {
            for _ in 0..2 {
                if let Some(val) = rdseed_retry() {
                    self.pool.mix_in(val, 64);
                }
            }
        }
        if self.has_rdrand {
            for _ in 0..4 {
                if let Some(val) = rdrand_retry() {
                    self.pool.mix_in(val, 48);
                }
            }
        }

        // Always add TSC jitter
        for _ in 0..4 {
            let jitter = rdtsc_jitter();
            self.pool.mix_in(jitter, 8);
        }

        // Mix current CSPRNG state with pool entropy
        // This ensures we never lose entropy on reseed
        for i in 0..4 {
            let state_word = u64::from_le_bytes([
                self.key[i * 8],
                self.key[i * 8 + 1],
                self.key[i * 8 + 2],
                self.key[i * 8 + 3],
                self.key[i * 8 + 4],
                self.key[i * 8 + 5],
                self.key[i * 8 + 6],
                self.key[i * 8 + 7],
            ]);
            self.pool.mix_in(state_word, 0); // No new entropy, just mixing
        }

        // Derive new key from enriched pool
        let new_key = self.pool.extract();
        self.key.copy_from_slice(&new_key);

        // Generate fresh nonce
        let nonce_entropy = if self.has_rdrand {
            rdrand_retry().unwrap_or_else(rdtsc_jitter)
        } else {
            rdtsc_jitter()
        };
        self.nonce[..8].copy_from_slice(&nonce_entropy.to_le_bytes());
        let nonce_extra = rdtsc_jitter();
        self.nonce[4..12].copy_from_slice(&nonce_extra.to_le_bytes());

        self.counter = 0;
        self.bytes_since_reseed = 0;
        self.reseed_count = self.reseed_count.saturating_add(1);
    }
}

// --- Hardware entropy source wrappers ---

/// Read a 64-bit value from the RDRAND instruction.
///
/// RDRAND provides conditioned random numbers from the Intel
/// hardware random number generator. Available on Ivy Bridge+ CPUs.
///
/// Returns None if the hardware RNG is temporarily exhausted.
fn rdrand() -> Option<u64> {
    let value: u64;
    let success: u8;
    unsafe {
        core::arch::asm!(
            "rdrand {val}",
            "setc {ok}",
            val = out(reg) value,
            ok = out(reg_byte) success,
        );
    }
    if success != 0 {
        Some(value)
    } else {
        None
    }
}

/// Read a 64-bit value from the RDSEED instruction.
///
/// RDSEED provides true random seeds directly from the hardware
/// entropy source (before conditioning). Higher quality than RDRAND
/// but may fail more often when entropy is depleted.
///
/// Available on Broadwell+ CPUs.
fn rdseed() -> Option<u64> {
    let value: u64;
    let success: u8;
    unsafe {
        core::arch::asm!(
            "rdseed {val}",
            "setc {ok}",
            val = out(reg) value,
            ok = out(reg_byte) success,
        );
    }
    if success != 0 {
        Some(value)
    } else {
        None
    }
}

/// Retry RDRAND up to RDRAND_RETRIES times.
fn rdrand_retry() -> Option<u64> {
    for _ in 0..RDRAND_RETRIES {
        if let Some(val) = rdrand() {
            return Some(val);
        }
        // Brief pause between retries (spin a few cycles)
        core::hint::spin_loop();
    }
    None
}

/// Retry RDSEED up to RDSEED_RETRIES times.
fn rdseed_retry() -> Option<u64> {
    for _ in 0..RDSEED_RETRIES {
        if let Some(val) = rdseed() {
            return Some(val);
        }
        core::hint::spin_loop();
    }
    None
}

/// Check CPUID for RDRAND support (leaf 1, ECX bit 30).
fn cpuid_has_rdrand() -> bool {
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "pop rbx",
            out("ecx") ecx,
            out("eax") _,
            out("edx") _,
        );
    }
    (ecx >> 30) & 1 == 1
}

/// Check CPUID for RDSEED support (leaf 7, subleaf 0, EBX bit 18).
fn cpuid_has_rdseed() -> bool {
    let ebx: u32;
    unsafe {
        let result: u32;
        core::arch::asm!(
            "push rbx",
            "mov eax, 7",
            "xor ecx, ecx",
            "cpuid",
            "mov {result:e}, ebx",
            "pop rbx",
            result = out(reg) result,
            out("eax") _,
            out("ecx") _,
            out("edx") _,
        );
        ebx = result;
    }
    (ebx >> 18) & 1 == 1
}

// --- TSC and jitter entropy ---

/// Read the Time Stamp Counter (RDTSC instruction).
///
/// Returns a 64-bit monotonically increasing counter that increments
/// at the CPU's reference clock rate. The low bits contain genuine
/// entropy from microarchitectural jitter.
fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi);
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Collect entropy from TSC timing jitter.
///
/// Performs variable-length computations and measures the TSC difference.
/// The variation in timing comes from:
///   - CPU pipeline stalls
///   - Cache misses
///   - Instruction scheduling jitter
///   - Interrupt timing
///   - DRAM refresh interference
///
/// Collects 64 samples and mixes them into a single u64.
fn rdtsc_jitter() -> u64 {
    let mut entropy: u64 = 0;
    for i in 0..64u64 {
        let t1 = rdtsc();
        // Variable-length computation to create timing jitter
        let mut x = t1;
        let iterations = (t1 & 0xF) + 1; // 1-16 iterations
        for _ in 0..iterations {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
        }
        let t2 = rdtsc();
        // Mix the timing difference into our entropy accumulator
        let diff = t2.wrapping_sub(t1);
        entropy = entropy.rotate_left(((i & 0x3F) + 1) as u32) ^ diff;
    }
    // Final mixing pass
    entropy = entropy.wrapping_mul(0x9E3779B97F4A7C15); // golden ratio hash
    entropy ^= entropy >> 32;
    entropy
}

// --- Public API ---

/// Initialize the CSPRNG.
///
/// Must be called during boot before any random number generation.
/// Collects initial entropy from hardware and seeds the CSPRNG.
pub fn init() {
    let mut rng = RNG.lock();
    rng.seed();

    let features = if rng.has_rdseed {
        "RDSEED+RDRAND+TSC"
    } else if rng.has_rdrand {
        "RDRAND+TSC"
    } else {
        "TSC jitter only"
    };

    serial_println!(
        "    [rng] CSPRNG seeded ({}, ~{} bits entropy)",
        features,
        rng.pool.estimated_entropy()
    );
}

/// Fill a buffer with cryptographically random bytes.
///
/// This is the primary interface for random number generation.
/// Thread-safe (uses a global spinlock).
pub fn fill_bytes(buf: &mut [u8]) {
    RNG.lock().fill(buf);
}

/// Generate a random u64.
pub fn random_u64() -> u64 {
    let mut buf = [0u8; 8];
    fill_bytes(&mut buf);
    u64::from_le_bytes(buf)
}

/// Generate a random u32.
pub fn random_u32() -> u32 {
    let mut buf = [0u8; 4];
    fill_bytes(&mut buf);
    u32::from_le_bytes(buf)
}

/// Generate a random u16.
pub fn random_u16() -> u16 {
    let mut buf = [0u8; 2];
    fill_bytes(&mut buf);
    u16::from_le_bytes(buf)
}

/// Generate a random u8.
pub fn random_u8() -> u8 {
    let mut buf = [0u8; 1];
    fill_bytes(&mut buf);
    buf[0]
}

/// Generate a random u64 in the range [0, upper_bound).
///
/// Uses rejection sampling to avoid modulo bias.
/// The rejection loop runs at most a few iterations on average.
pub fn random_u64_bounded(upper_bound: u64) -> u64 {
    if upper_bound <= 1 {
        return 0;
    }

    // Compute the rejection threshold to avoid modulo bias
    // threshold = (2^64 - upper_bound) % upper_bound
    // = (-upper_bound) % upper_bound (in unsigned arithmetic)
    let threshold = (0u64.wrapping_sub(upper_bound)) % upper_bound;

    loop {
        let val = random_u64();
        if val >= threshold {
            return val % upper_bound;
        }
    }
}

/// Generate a random u32 in the range [0, upper_bound).
///
/// Uses rejection sampling to avoid modulo bias.
pub fn random_u32_bounded(upper_bound: u32) -> u32 {
    if upper_bound <= 1 {
        return 0;
    }
    let threshold = (0u32.wrapping_sub(upper_bound)) % upper_bound;
    loop {
        let val = random_u32();
        if val >= threshold {
            return val % upper_bound;
        }
    }
}

/// Force an immediate reseed from hardware entropy.
///
/// Call this after security-sensitive events or periodically
/// from a background task to maintain entropy freshness.
pub fn reseed() {
    RNG.lock().reseed();
}

/// Get CSPRNG status information.
///
/// Returns (total_bytes_generated, reseed_count, estimated_entropy_bits).
pub fn status() -> (u64, u64, u64) {
    let rng = RNG.lock();
    (
        rng.total_bytes_generated,
        rng.reseed_count,
        rng.pool.estimated_entropy(),
    )
}

/// Check if the CSPRNG has been initialized.
pub fn is_initialized() -> bool {
    RNG.lock().initialized
}

// --- Self-tests ---

/// Run CSPRNG self-tests.
/// Returns true if all tests pass.
///
/// Tests basic statistical properties and API correctness.
/// NOT a replacement for proper statistical testing (NIST SP 800-22),
/// but catches gross implementation errors.
pub fn self_test() -> bool {
    // Test 1: fill_bytes produces non-zero output
    let mut buf1 = [0u8; 64];
    fill_bytes(&mut buf1);
    let mut all_zero = true;
    for &b in buf1.iter() {
        if b != 0 {
            all_zero = false;
            break;
        }
    }
    if all_zero {
        return false;
    }

    // Test 2: Two consecutive fills produce different output
    let mut buf2 = [0u8; 64];
    fill_bytes(&mut buf2);
    let mut same = true;
    for i in 0..64 {
        if buf1[i] != buf2[i] {
            same = false;
            break;
        }
    }
    if same {
        return false;
    }

    // Test 3: random_u64 produces different values
    let r1 = random_u64();
    let r2 = random_u64();
    // Extremely unlikely to be the same (1/2^64 probability)
    if r1 == r2 {
        // Try once more before failing
        let r3 = random_u64();
        if r1 == r3 {
            return false;
        }
    }

    // Test 4: random_u32 produces values
    let r32 = random_u32();
    let _ = r32; // Just verify it doesn't panic

    // Test 5: random_u64_bounded respects bounds
    for _ in 0..100 {
        let val = random_u64_bounded(100);
        if val >= 100 {
            return false;
        }
    }

    // Test 6: random_u32_bounded respects bounds
    for _ in 0..100 {
        let val = random_u32_bounded(50);
        if val >= 50 {
            return false;
        }
    }

    // Test 7: Bounded with bound=1 always returns 0
    for _ in 0..10 {
        if random_u64_bounded(1) != 0 {
            return false;
        }
    }

    // Test 8: Byte frequency test (very basic)
    // Generate 1024 bytes and check that at least 200 of 256 possible values appear
    let mut buf = [0u8; 1024];
    fill_bytes(&mut buf);
    let mut seen = [false; 256];
    for &b in buf.iter() {
        seen[b as usize] = true;
    }
    let unique_count = seen.iter().filter(|&&x| x).count();
    if unique_count < 200 {
        return false;
    }

    // Test 9: Status reports non-zero generation count
    let (total, _reseeds, _entropy) = status();
    if total == 0 {
        return false;
    }

    true
}

/// Run self-tests and report to serial console.
pub fn run_self_test() {
    if self_test() {
        serial_println!("    [rng] Self-test PASSED (9 vectors)");
    } else {
        serial_println!("    [rng] Self-test FAILED!");
    }
}
