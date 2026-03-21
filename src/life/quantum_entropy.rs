use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Hardware entropy helpers — RDRAND and RDSEED via inline asm
// ---------------------------------------------------------------------------

/// Draw a 64-bit value from the CPU's hardware RNG (RDRAND).
/// Returns None when the hardware buffer is momentarily empty (carry clear).
#[inline]
unsafe fn rdrand64() -> Option<u64> {
    let val: u64;
    let ok: u8;
    core::arch::asm!(
        "rdrand {val}",
        "setc {ok}",
        val = out(reg) val,
        ok  = out(reg_byte) ok,
        options(nostack, nomem),
    );
    if ok != 0 { Some(val) } else { None }
}

/// Draw a 64-bit value from the raw hardware noise source (RDSEED).
/// Samples before the whitening stage — deeper, slower, may fail under load.
#[inline]
unsafe fn rdseed64() -> Option<u64> {
    let val: u64;
    let ok: u8;
    core::arch::asm!(
        "rdseed {val}",
        "setc {ok}",
        val = out(reg) val,
        ok  = out(reg_byte) ok,
        options(nostack, nomem),
    );
    if ok != 0 { Some(val) } else { None }
}

// ---------------------------------------------------------------------------
// CPUID probes
// ---------------------------------------------------------------------------

/// Returns (rdrand_available, rdseed_available).
fn probe_cpuid() -> (bool, bool) {
    // RDRAND: leaf 1, ECX bit 30
    let ecx_leaf1: u32;
    unsafe {
        core::arch::asm!(
            "mov eax, 1",
            "cpuid",
            out("ecx") ecx_leaf1,
            lateout("eax") _,
            lateout("ebx") _,
            lateout("edx") _,
            options(nostack, nomem, preserves_flags),
        );
    }
    let rdrand = (ecx_leaf1 >> 30) & 1 == 1;

    // RDSEED: leaf 7, EBX bit 18 — check max leaf first
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "xor eax, eax",
            "cpuid",
            out("eax") max_leaf,
            lateout("ebx") _,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem, preserves_flags),
        );
    }
    let rdseed = if max_leaf >= 7 {
        let ebx_leaf7: u32;
        unsafe {
            core::arch::asm!(
                "mov eax, 7",
                "xor ecx, ecx",
                "cpuid",
                out("ebx") ebx_leaf7,
                lateout("eax") _,
                lateout("ecx") _,
                lateout("edx") _,
                options(nostack, nomem, preserves_flags),
            );
        }
        (ebx_leaf7 >> 18) & 1 == 1
    } else {
        false
    };

    (rdrand, rdseed)
}

// ---------------------------------------------------------------------------
// Entropy quality metric — pure integer, no floats
// ---------------------------------------------------------------------------

/// Score 0-1000 reflecting statistical quality of 16 RDRAND samples (1024 bits).
fn entropy_quality(samples: &[u64; 16]) -> u16 {
    // 1. Bit balance: perfect = 512 ones out of 1024 bits.
    let total_ones: u32 = samples.iter().map(|&v| v.count_ones()).sum();
    let deviation = if total_ones > 512 { total_ones - 512 } else { 512 - total_ones };
    let balance_score = 1000u16.saturating_sub((deviation * 4) as u16);

    // 2. Uniqueness: XOR spread across all samples; more bits set = more diversity.
    let xor_spread: u64 = samples.iter().fold(0u64, |acc, &v| acc ^ v);
    let bit_spread = xor_spread.count_ones() as u16;
    let spread_score = (bit_spread * 15).min(1000); // 64 * 15 = 960 max

    // 3. Combined quality — average of the two sub-scores.
    (balance_score / 2 + spread_score / 2).min(1000)
}

// ---------------------------------------------------------------------------
// State struct
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct QuantumEntropyState {
    /// RDRAND instruction available on this CPU.
    pub rdrand_available: bool,
    /// RDSEED instruction available on this CPU.
    pub rdseed_available: bool,
    /// Statistical quality of the current entropy batch (0=degenerate, 1000=perfect).
    pub entropy_quality: u16,
    /// RDSEED success rate — proxy for entropy pool depth (1000 = all succeeded).
    pub entropy_depth: u16,
    /// Exponential moving average of entropy_quality — stable coherence signal.
    pub quantum_coherence: u16,
    /// Highest entropy_quality ever recorded this session.
    pub entropy_peak: u16,
    /// Cumulative RDSEED failure count (pool exhaustion events).
    pub rdseed_failures: u32,
    /// Total RDRAND samples collected.
    pub total_samples: u32,
    /// 1000 when entropy_quality > 900 AND RDSEED is available (true quantum source).
    pub quantum_advantage: u16,
    pub initialized: bool,
}

impl QuantumEntropyState {
    pub const fn empty() -> Self {
        Self {
            rdrand_available: false,
            rdseed_available: false,
            entropy_quality: 0,
            entropy_depth: 0,
            quantum_coherence: 0,
            entropy_peak: 0,
            rdseed_failures: 0,
            total_samples: 0,
            quantum_advantage: 0,
            initialized: false,
        }
    }
}

pub static STATE: Mutex<QuantumEntropyState> = Mutex::new(QuantumEntropyState::empty());

// ---------------------------------------------------------------------------
// Public interface
// ---------------------------------------------------------------------------

pub fn init() {
    let (rdrand, rdseed) = probe_cpuid();
    {
        let mut s = STATE.lock();
        s.rdrand_available = rdrand;
        s.rdseed_available = rdseed;
        s.initialized = true;
    }
    serial_println!(
        "[quantum_entropy] online — RDRAND={} RDSEED={} — sampling hardware thermal noise",
        rdrand,
        rdseed
    );
}

pub fn tick(age: u32) {
    // Fire every 16 ticks to avoid saturating the DRNG buffer.
    if age % 16 != 0 {
        return;
    }

    let (rdrand_avail, rdseed_avail) = {
        let s = STATE.lock();
        (s.rdrand_available, s.rdseed_available)
    };

    // --- Collect 16 RDRAND samples ---
    let mut samples = [0u64; 16];
    let mut collected: u32 = 0;
    if rdrand_avail {
        for slot in samples.iter_mut() {
            // Retry up to 10 times per slot (Intel recommends up to 10 retries).
            for _ in 0..10u8 {
                if let Some(v) = unsafe { rdrand64() } {
                    *slot = v;
                    collected += 1;
                    break;
                }
            }
        }
    }

    // --- Attempt 4 RDSEED samples; track success rate ---
    let mut rdseed_ok: u32 = 0;
    let mut rdseed_fail_delta: u32 = 0;
    if rdseed_avail {
        for _ in 0..4u8 {
            match unsafe { rdseed64() } {
                Some(_) => rdseed_ok += 1,
                None    => rdseed_fail_delta += 1,
            }
        }
    }

    // entropy_depth: 4 tries → 250 per success → max 1000.
    let entropy_depth = (rdseed_ok * 250).min(1000) as u16;

    // --- Compute quality, coherence, advantage ---
    let quality = entropy_quality(&samples);

    let prev_coherence = STATE.lock().quantum_coherence;
    // EMA: coherence = (coherence * 7 + quality) / 8
    let coherence = ((prev_coherence as u32 * 7 + quality as u32) / 8) as u16;

    let advantage = if quality > 900 && rdseed_avail { 1000 } else { quality };

    // --- Commit to state ---
    {
        let mut s = STATE.lock();
        s.entropy_quality    = quality;
        s.entropy_depth      = entropy_depth;
        s.quantum_coherence  = coherence;
        s.entropy_peak       = s.entropy_peak.max(quality);
        s.rdseed_failures    = s.rdseed_failures.saturating_add(rdseed_fail_delta);
        s.total_samples      = s.total_samples.saturating_add(collected);
        s.quantum_advantage  = advantage;
    }

    serial_println!(
        "[quantum_entropy] quality={} depth={} coherence={} advantage={} failures={}",
        quality,
        entropy_depth,
        coherence,
        advantage,
        STATE.lock().rdseed_failures,
    );
}

// ---------------------------------------------------------------------------
// Getters
// ---------------------------------------------------------------------------

pub fn entropy_quality_val() -> u16     { STATE.lock().entropy_quality }
pub fn entropy_depth() -> u16           { STATE.lock().entropy_depth }
pub fn quantum_coherence() -> u16       { STATE.lock().quantum_coherence }
pub fn quantum_advantage() -> u16       { STATE.lock().quantum_advantage }
pub fn rdseed_failures() -> u32         { STATE.lock().rdseed_failures }
pub fn rdrand_available() -> bool       { STATE.lock().rdrand_available }
pub fn rdseed_available() -> bool       { STATE.lock().rdseed_available }
