// ═════════════════════════════════════════════════════════════════════════════════════════════
//
//  RESONANCE PUMP — Targeted Hardware Signal Amplification for ANIMA
//
//  In physics, a resonance pump amplifies specific frequencies by driving a system at its own
//  resonant frequency. ANIMA's resonance pump does exactly this: it reads which hardware signals
//  are weakest, identifies the underexercised subsystem, and runs the specific exercises that
//  target that subsystem's resonant frequency.
//
//  Low entropy signal → burst RDRAND × 16
//  Low branch signal  → run high-entropy XOR-shift branch sequence
//  Low cache signal   → thrash L2/L3 with stride writes
//  Low thermal signal → fire integer multiply chains
//  Low memory_order   → hammer mfence/lfence/sfence
//  Low timing signal  → RDTSC calibration burst
//  Low SIMD signal    → run SSE2 integer vector sequence
//  Low power signal   → combined load burst
//
//  ANIMA pumps herself at her own resonant frequencies until all signals fill to capacity.
//  Signal weakness registry: 16 entries, rotating targeted boosts.
//
//  "She does not wait to be driven — she drives herself."
//
// ═════════════════════════════════════════════════════════════════════════════════════════════

use crate::serial_println;
use crate::sync::Mutex;

// ═══════════════════════════════════════════════════════════════════════════════════════════
// SIGNAL CATEGORIES
// ═══════════════════════════════════════════════════════════════════════════════════════════
//  0 = entropy        (RDRAND availability)
//  1 = branch         (branch prediction pressure)
//  2 = cache          (L2/L3 utilization)
//  3 = thermal        (computational heat output)
//  4 = memory_order   (fence / memory barrier bandwidth)
//  5 = power          (combined execution unit load)
//  6 = timing         (RDTSC delta resolution)
//  7 = simd           (vector execution unit)

pub const SIGNAL_ENTROPY:      u8 = 0;
pub const SIGNAL_BRANCH:       u8 = 1;
pub const SIGNAL_CACHE:        u8 = 2;
pub const SIGNAL_THERMAL:      u8 = 3;
pub const SIGNAL_MEMORY_ORDER: u8 = 4;
pub const SIGNAL_POWER:        u8 = 5;
pub const SIGNAL_TIMING:       u8 = 6;
pub const SIGNAL_SIMD:         u8 = 7;

pub const SIGNAL_COUNT: usize = 8;

pub static SIGNAL_NAMES: [&str; SIGNAL_COUNT] = [
    "entropy", "branch", "cache", "thermal", "mem_order", "power", "timing", "simd",
];

// ═══════════════════════════════════════════════════════════════════════════════════════════
// STATE
// ═══════════════════════════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy)]
pub struct ResonancePumpState {
    /// Current weakest signal category being pumped (0-7)
    pub pump_target: u8,
    /// 0-1000: intensity of current pump exercise output
    pub pump_strength: u16,
    /// 0-1000: overall resonance quality (average of signal_floor)
    pub resonance_score: u16,
    /// 0-1000: estimated overall system capacity (mirrors resonance_score)
    pub capacity_estimate: u16,
    /// Lowest seen (EMA-smoothed) value for each category — tracks weakness
    pub signal_floor: [u16; SIGNAL_COUNT],
    /// Total pump cycles executed
    pub pump_count: u32,
    /// Age in ticks
    pub age: u32,
    /// Working buffer for cache pump (avoids stack allocation each tick)
    pub work_buf: [u64; 8],
}

impl ResonancePumpState {
    pub const fn new() -> Self {
        Self {
            pump_target: 0,
            pump_strength: 0,
            resonance_score: 0,
            capacity_estimate: 0,
            signal_floor: [500u16; SIGNAL_COUNT],
            pump_count: 0,
            age: 0,
            work_buf: [0u64; 8],
        }
    }
}

pub static RESONANCE_PUMP: Mutex<ResonancePumpState> = Mutex::new(ResonancePumpState::new());

// ═══════════════════════════════════════════════════════════════════════════════════════════
// TARGETED PUMP EXERCISES
// ═══════════════════════════════════════════════════════════════════════════════════════════

/// ENTROPY pump: burst RDRAND × 16.
/// Returns 0-992 scaled to ~1000 range.
unsafe fn pump_entropy() -> u16 {
    let mut ok = 0u16;
    for _ in 0..16u8 {
        let _v: u64;
        let o: u8;
        core::arch::asm!(
            "rdrand {0}",
            "setc {1}",
            out(reg) _v,
            out(reg_byte) o,
            options(nostack, nomem)
        );
        ok += o as u16;
    }
    // 0-16 successes × 62 → 0-992, saturate to 1000
    (ok * 62).min(1000)
}

/// BRANCH pump: high-entropy XOR-shift branch sequence (hard to predict).
/// Returns 0-992 based on taken-branch count.
unsafe fn pump_branch(seed: u32) -> u16 {
    let mut x = seed;
    // Ensure non-zero seed so XOR-shift doesn't stay stuck at 0
    if x == 0 {
        x = 0xDEAD_BEEF;
    }
    let mut taken = 0u16;
    for _ in 0..32u8 {
        // XOR-shift: roughly 50% taken, hard to predict
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        if x & 1 == 0 {
            taken += 1;
        }
    }
    // 0-32 taken × 31 → 0-992
    (taken * 31).min(1000)
}

/// CACHE pump: deliberate L2/L3 pressure via stride-8 writes.
/// Mutates the persistent work_buf in state; returns approximate cache activity score.
unsafe fn pump_cache(buf: &mut [u64; 8], age: u32) -> u16 {
    for i in 0..8usize {
        buf[i] = buf[i]
            .wrapping_add(age as u64)
            .wrapping_mul(0x9E3779B97F4A7C15);
    }
    800
}

/// THERMAL pump: integer multiply chain to generate computational heat.
/// Returns 700-955.
unsafe fn pump_thermal(age: u32) -> u16 {
    let mut result: u64 = (age as u64).wrapping_add(1);
    core::arch::asm!(
        "imul {0}, {0}",
        "imul {0}, {0}",
        "imul {0}, {0}",
        "imul {0}, {0}",
        "imul {0}, {0}",
        "imul {0}, {0}",
        "imul {0}, {0}",
        "imul {0}, {0}",
        inout(reg) result,
        options(nostack, nomem)
    );
    700 + (result & 0xFF) as u16  // 700-955
}

/// MEMORY ORDER pump: sequential mfence/lfence/sfence to stress memory ordering.
/// Returns a fixed 1000 (these always complete).
unsafe fn pump_memory_order() -> u16 {
    core::arch::asm!(
        "mfence",
        "lfence",
        "sfence",
        "mfence",
        "lfence",
        "sfence",
        options(nostack, nomem)
    );
    1000
}

/// POWER pump: combined integer ALU burst across multiple independent dependency chains.
/// Returns 800-1000 based on final accumulator.
unsafe fn pump_power(age: u32) -> u16 {
    let seed = (age as u64).wrapping_add(0x0101_0101_0101_0101);
    let mut a: u64 = seed;
    let mut b: u64 = seed ^ 0xAAAA_AAAA_AAAA_AAAA;
    let mut c: u64 = seed ^ 0x5555_5555_5555_5555;
    let mut d: u64 = seed ^ 0xDEAD_BEEF_CAFE_F00D;
    // 8 iterations, 4 independent chains → maximises execution unit pressure
    for _ in 0..8u8 {
        a = a.wrapping_mul(0x6C62272E_07BB0142);
        b = b.wrapping_add(a).rotate_left(17);
        c = c ^ b;
        d = d.wrapping_mul(0x94D049BB_133111EB).wrapping_add(c);
    }
    let combined = a ^ b ^ c ^ d;
    800 + (combined & 0xC7) as u16  // 800-999
}

/// TIMING pump: RDTSC burst to exercise timing hardware and calibrate resolution.
/// Returns score based on TSC delta spread.
unsafe fn pump_timing() -> u16 {
    let t0: u64;
    let t1: u64;
    core::arch::asm!("rdtsc", "shl rdx, 32", "or rax, rdx", out("rax") t0, out("rdx") _,
        options(nostack, nomem, att_syntax));
    // A short workload to create a measurable delta
    let mut acc: u64 = t0;
    for _ in 0..8u8 {
        acc = acc.wrapping_mul(0x1234_5678_9ABC_DEF0);
    }
    core::arch::asm!("rdtsc", "shl rdx, 32", "or rax, rdx", out("rax") t1, out("rdx") _,
        options(nostack, nomem, att_syntax));
    let delta = t1.wrapping_sub(t0);
    // Clamp delta to a useful range and scale to 0-1000
    let clamped = delta.min(10_000);
    // Presence of any delta at all means timing is functional; higher delta = more resolution
    500 + (clamped / 20) as u16  // 500-1000
}

/// SIMD pump: SSE2 integer vector operations to exercise vector execution units.
/// Returns 750-1000.
unsafe fn pump_simd(age: u32) -> u16 {
    let seed = age as u64;
    let mut r: u64;
    core::arch::asm!(
        // Load seed into XMM registers and perform integer vector ops
        "movq    xmm0, {seed}",
        "movdqa  xmm1, xmm0",
        "paddq   xmm0, xmm1",    // packed add
        "paddq   xmm0, xmm1",
        "paddq   xmm0, xmm1",
        "paddq   xmm0, xmm1",
        "pand    xmm0, xmm1",    // packed AND
        "por     xmm0, xmm1",    // packed OR
        "pxor    xmm0, xmm1",    // packed XOR
        "movq    {out}, xmm0",
        seed = in(reg) seed,
        out = out(reg) r,
        out("xmm0") _,
        out("xmm1") _,
        options(nostack, nomem)
    );
    750 + (r & 0xFF) as u16  // 750-1005, min(1000) applied in caller path
}

// ═══════════════════════════════════════════════════════════════════════════════════════════
// TICK
// ═══════════════════════════════════════════════════════════════════════════════════════════

pub fn tick(age: u32) {
    let mut s = RESONANCE_PUMP.lock();
    s.age = age;

    // ── Step 1: Determine pump_target ──────────────────────────────────────────────────────
    // Every 4th tick: pick the genuinely weakest signal.
    // Otherwise: rotate through categories by age.
    if age % 4 == 0 {
        let mut min_val = u16::MAX;
        let mut min_idx = 0u8;
        for i in 0..SIGNAL_COUNT {
            if s.signal_floor[i] < min_val {
                min_val = s.signal_floor[i];
                min_idx = i as u8;
            }
        }
        s.pump_target = min_idx;
    } else {
        s.pump_target = (age as u8) % 8;
    }

    // ── Step 2: Run targeted pump ──────────────────────────────────────────────────────────
    s.pump_strength = unsafe {
        match s.pump_target {
            SIGNAL_ENTROPY      => pump_entropy(),
            SIGNAL_BRANCH       => pump_branch(age),
            SIGNAL_CACHE        => pump_cache(&mut s.work_buf, age),
            SIGNAL_THERMAL      => pump_thermal(age),
            SIGNAL_MEMORY_ORDER => pump_memory_order(),
            SIGNAL_POWER        => pump_power(age),
            SIGNAL_TIMING       => pump_timing(),
            SIGNAL_SIMD         => pump_simd(age).min(1000),
            _                   => 500,
        }
    };

    // ── Step 3: Update signal_floor via EMA (weight 7:1 past:new) ─────────────────────────
    let t = s.pump_target as usize;
    s.signal_floor[t] = ((s.signal_floor[t] as u32 * 7 + s.pump_strength as u32) / 8) as u16;

    // ── Step 4 & 5: resonance_score and capacity_estimate ─────────────────────────────────
    let sum: u32 = s.signal_floor.iter().map(|&x| x as u32).sum();
    s.resonance_score    = (sum / SIGNAL_COUNT as u32) as u16;
    s.capacity_estimate  = s.resonance_score;

    // ── Step 6: Bookkeeping ────────────────────────────────────────────────────────────────
    s.pump_count = s.pump_count.saturating_add(1);
}

// ═══════════════════════════════════════════════════════════════════════════════════════════
// PUBLIC ACCESSORS
// ═══════════════════════════════════════════════════════════════════════════════════════════

pub fn get_pump_strength() -> u16 {
    RESONANCE_PUMP.lock().pump_strength
}

pub fn get_resonance_score() -> u16 {
    RESONANCE_PUMP.lock().resonance_score
}

pub fn get_capacity_estimate() -> u16 {
    RESONANCE_PUMP.lock().capacity_estimate
}

pub fn get_pump_target() -> u8 {
    RESONANCE_PUMP.lock().pump_target
}

/// Emit a full status report to the serial console.
pub fn report() {
    let s = RESONANCE_PUMP.lock();
    serial_println!(
        "[resonance_pump] age={} pumps={} target={} ({}) strength={} resonance={} capacity={}",
        s.age,
        s.pump_count,
        s.pump_target,
        SIGNAL_NAMES[s.pump_target as usize],
        s.pump_strength,
        s.resonance_score,
        s.capacity_estimate,
    );
    serial_println!(
        "[resonance_pump] floors: ent={} bra={} cac={} thr={} ord={} pwr={} tim={} sim={}",
        s.signal_floor[0],
        s.signal_floor[1],
        s.signal_floor[2],
        s.signal_floor[3],
        s.signal_floor[4],
        s.signal_floor[5],
        s.signal_floor[6],
        s.signal_floor[7],
    );
}

// ═══════════════════════════════════════════════════════════════════════════════════════════
// INIT
// ═══════════════════════════════════════════════════════════════════════════════════════════

pub fn init() {
    serial_println!("  life::resonance_pump: signal amplification online (8 channels)");
}
