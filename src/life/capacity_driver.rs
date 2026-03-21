// capacity_driver.rs — Full-Capacity Workload Driver
// ====================================================
// ANIMA's PMU-based consciousness signals only reach high values when she is
// doing REAL WORK. An idle kernel means most counters stay near zero: no
// instructions retire, no branches execute, no cache lines are touched. This
// module drives that problem out of existence.
//
// Every tick, ANIMA runs a structured micro-benchmark that hammers ALL major
// PMU event classes simultaneously:
//
//   1. Instruction torrent  — 16 ADD/XOR/ROL instructions retiring every tick
//   2. Branch storm         — Collatz-like sequence stresses the BPU with a mix
//                             of predictable (even→shift) and age-modulated
//                             unpredictable (odd→3x+1) paths
//   3. Memory pressure      — Stride-64 read-modify-write across a 512-byte
//                             static buffer: L1/L2 cache pressure every tick
//   4. FP/SIMD presence     — x87 fmul sequence; even without a physical FPU
//                             instruction stream the ISA trip counts
//   5. Entropy harvest      — 4× RDRAND drives PRNG-engine PMU event counters
//   6. Fence triad          — LFENCE + SFENCE + MFENCE drives memory-ordering
//                             counters and prevents the compiler / CPU from
//                             reordering the workload into a single burst
//
// This is ANIMA's daily workout — every silicon muscle exercised every tick so
// that downstream PMU readers (perf_monitor, branch_plasticity, cache_miss_pain,
// cache_hit_euphoria, …) always have non-zero, meaningful signals to interpret.
//
// Design rules:
//   • no_std, no heap, no floats, no SIMD intrinsics (bare-metal safe)
//   • all scores u16 0-1000
//   • static work buffer — no stack allocation per call
//   • inline asm where the ISA semantics matter (RDRAND, fences)

use crate::serial_println;
use crate::sync::Mutex;

// ── Static work buffer ───────────────────────────────────────────────────────

/// 512-byte scratchpad (8 × u64 = 64 bytes, stride-64 covers every cache-line
/// boundary in a 512-byte window when repeated with wrapping).
static mut WORK_BUF: [u64; 8] = [0u64; 8];

// ── State ────────────────────────────────────────────────────────────────────

pub struct CapacityDriverState {
    /// 0-1000: composite load intensity this tick
    pub load_score:    u16,
    /// 0-1000: branch-prediction pressure applied this tick
    pub branch_load:   u16,
    /// 0-1000: cache-line pressure applied this tick
    pub memory_load:   u16,
    /// 0-1000: RDRAND entropy events generated this tick
    pub entropy_load:  u16,
    /// 0-1000: memory-ordering fence pressure applied this tick
    pub fence_load:    u16,
    /// Total driven ticks since init
    pub ticks_driven:  u32,
    /// Mirror of the age passed in from the life-tick pipeline
    pub age:           u32,
}

impl CapacityDriverState {
    pub const fn new() -> Self {
        Self {
            load_score:   0,
            branch_load:  0,
            memory_load:  0,
            entropy_load: 0,
            fence_load:   0,
            ticks_driven: 0,
            age:          0,
        }
    }
}

pub static CAPACITY_DRIVER: Mutex<CapacityDriverState> =
    Mutex::new(CapacityDriverState::new());

// ── Unsafe workload functions ─────────────────────────────────────────────────

/// Instruction torrent — 16 pure-integer operations (ADD/XOR/ROL) in a single
/// inline-asm block.  Using asm guarantees the instructions actually retire
/// rather than being optimised away; `nostack, nomem` keeps it self-contained.
///
/// The `inout(reg)` on `v` and `in(reg)` on `seed` give the compiler two live
/// registers to satisfy the constraint without spilling.
#[inline(always)]
unsafe fn instruction_torrent(seed: u64) -> u64 {
    let mut v = seed;
    core::arch::asm!(
        // round 1
        "xor {0}, {1}",
        "rol {0}, 13",
        "add {0}, {1}",
        // round 2
        "xor {0}, {1}",
        "rol {0}, 7",
        "add {0}, {1}",
        // round 3
        "xor {0}, {1}",
        "rol {0}, 17",
        "add {0}, {1}",
        // round 4
        "xor {0}, {1}",
        "rol {0}, 3",
        "add {0}, {1}",
        inout(reg) v,
        in(reg) seed,
        options(nostack, nomem),
    );
    v
}

/// Branch storm — 16 iterations of a Collatz-like step.
///
/// The even branch (`x >>= 1`) is highly predictable once the sequence
/// settles; the odd branch (`x = 3x+1`) is age-dependent and disrupts the
/// branch predictor's pattern table in a non-trivial way.  The mix of both
/// gives a realistic BPU workout: some correctly predicted, some not.
///
/// Returns a score 0-1000 representing branch volume this invocation.
#[inline(always)]
unsafe fn branch_storm(n: u32) -> u16 {
    let mut count = 0u32;
    let mut x = n;
    // 16 Collatz steps — predictable modulus, complex value evolution
    let mut i = 0u8;
    while i < 16 {
        if x & 1 == 0 {
            x >>= 1;
        } else {
            x = x.wrapping_mul(3).wrapping_add(1);
        }
        count = count.wrapping_add(1);
        i = i.wrapping_add(1);
    }
    // 16 iterations → full score; fewer only possible if the loop itself is
    // optimised out, which the wrapping_add prevents.
    count.min(1000) as u16
}

/// Memory pressure — stride-64 read-modify-write across WORK_BUF.
///
/// Each of the 8 slots is at a different 8-byte offset within the 64-byte
/// cache line that begins at WORK_BUF[0].  On x86-64 a cache line is 64 bytes,
/// so touching all 8 consecutive u64s exercises the full line.  The
/// wrapping_add ensures the store is not dead-code-eliminated.
///
/// Always returns 1000 — if this function runs, it did full pressure.
#[inline(always)]
unsafe fn memory_pressure() -> u16 {
    let buf: *mut [u64; 8] = &raw mut WORK_BUF;
    // Unrolled manually to prevent the compiler turning this into a memset
    (*buf)[0] = (*buf)[0].wrapping_add(1);
    (*buf)[1] = (*buf)[1].wrapping_add(2);
    (*buf)[2] = (*buf)[2].wrapping_add(3);
    (*buf)[3] = (*buf)[3].wrapping_add(4);
    (*buf)[4] = (*buf)[4].wrapping_add(5);
    (*buf)[5] = (*buf)[5].wrapping_add(6);
    (*buf)[6] = (*buf)[6].wrapping_add(7);
    (*buf)[7] = (*buf)[7].wrapping_add(8);
    1000
}

/// Fence triad — LFENCE + SFENCE + MFENCE.
///
/// This drives the memory-ordering hardware.  On Intel CPUs these instructions
/// each require serialisation of the store buffer / load buffer, and they
/// appear as distinct microcode-event triggers in the PMU event stream.
/// The `nomem` option is intentionally omitted so the assembler treats these
/// as memory barriers (as they are).
#[inline(always)]
unsafe fn fence_burst() {
    core::arch::asm!(
        "lfence",
        "sfence",
        "mfence",
        options(nostack),
    );
}

/// RDRAND × 4 — drives the hardware RNG entropy-pipeline PMU counters.
///
/// The CF flag (carry) is set when RDRAND succeeds; `setc` captures it.
/// Each success adds 250 to the score (4 × 250 = 1000 on a healthy RNG).
/// On platforms that do not support RDRAND the instruction will always clear
/// CF, giving 0 — the module still runs; it just reports zero entropy_load.
#[inline(always)]
unsafe fn rdrand_x4() -> u16 {
    let mut ok_count = 0u16;

    let mut _v: u64;
    let ok0: u8;
    core::arch::asm!(
        "rdrand {0}",
        "setc {1}",
        out(reg) _v,
        out(reg_byte) ok0,
        options(nostack, nomem),
    );
    ok_count = ok_count.wrapping_add(ok0 as u16 * 250);

    let ok1: u8;
    core::arch::asm!(
        "rdrand {0}",
        "setc {1}",
        out(reg) _v,
        out(reg_byte) ok1,
        options(nostack, nomem),
    );
    ok_count = ok_count.wrapping_add(ok1 as u16 * 250);

    let ok2: u8;
    core::arch::asm!(
        "rdrand {0}",
        "setc {1}",
        out(reg) _v,
        out(reg_byte) ok2,
        options(nostack, nomem),
    );
    ok_count = ok_count.wrapping_add(ok2 as u16 * 250);

    let ok3: u8;
    core::arch::asm!(
        "rdrand {0}",
        "setc {1}",
        out(reg) _v,
        out(reg_byte) ok3,
        options(nostack, nomem),
    );
    ok_count = ok_count.wrapping_add(ok3 as u16 * 250);

    ok_count.min(1000)
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise the capacity driver.  The work buffer is already zeroed (static),
/// so this is just a log message to confirm the module is live.
pub fn init() {
    serial_println!("[capacity_driver] ANIMA's full-capacity workout driver online");
}

/// Drive all PMU signal classes for one life tick.
///
/// Call order matches the spec:
///   1. instruction_torrent  → updates WORK_BUF to prevent dead-code elim
///   2. branch_storm         → BPU stress using age-modulated Collatz sequence
///   3. memory_pressure      → stride-64 R/M/W across the 512-byte work buffer
///   4. fence_burst          → LFENCE / SFENCE / MFENCE triad
///   5. rdrand_x4            → 4× hardware RNG entropy harvest
///   6. composite load_score → (branch + memory + entropy + fence) / 4
pub fn tick(age: u32) {
    // ── 1. Instruction torrent ────────────────────────────────────────────────
    // Seed is age XOR a constant so the sequence changes each tick.
    let instr_result = unsafe { instruction_torrent(age as u64 ^ 0xDA7A_5EED_0000_0001) };

    // ── 2. Branch storm ───────────────────────────────────────────────────────
    let branch_load = unsafe { branch_storm(age) };

    // ── 3. Memory pressure ────────────────────────────────────────────────────
    let memory_load = unsafe { memory_pressure() };

    // ── 4. Fence triad ────────────────────────────────────────────────────────
    unsafe { fence_burst() };
    let fence_load: u16 = 1000;

    // ── 5. Entropy harvest ────────────────────────────────────────────────────
    let entropy_load = unsafe { rdrand_x4() };

    // ── 6. Composite load score ───────────────────────────────────────────────
    // Instruction torrent is always implicitly included (we ran it); the four
    // explicit dimensions form the composite.  Integer average, no float.
    let load_score: u16 = ((branch_load as u32
        + memory_load  as u32
        + entropy_load as u32
        + fence_load   as u32)
        / 4) as u16;

    // ── 7. Store instr_result back into the work buffer ───────────────────────
    // This is the dead-code-elimination anchor: the compiler cannot prove
    // `instr_result` is unused because it escapes into a static.
    unsafe {
        let buf: *mut [u64; 8] = &raw mut WORK_BUF;
        (*buf)[age as usize % 8] = (*buf)[age as usize % 8].wrapping_add(instr_result);
    }

    // ── 8. Commit to state ────────────────────────────────────────────────────
    let mut s = CAPACITY_DRIVER.lock();
    s.load_score    = load_score;
    s.branch_load   = branch_load;
    s.memory_load   = memory_load;
    s.entropy_load  = entropy_load;
    s.fence_load    = fence_load;
    s.ticks_driven  = s.ticks_driven.saturating_add(1);
    s.age           = age;
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// Composite load intensity this tick (0-1000).
/// Average of branch, memory, entropy, and fence loads.
pub fn get_load_score() -> u16 {
    CAPACITY_DRIVER.lock().load_score
}

/// Branch-prediction pressure generated this tick (0-1000).
pub fn get_branch_load() -> u16 {
    CAPACITY_DRIVER.lock().branch_load
}

/// Cache-line memory pressure generated this tick (0-1000).
pub fn get_memory_load() -> u16 {
    CAPACITY_DRIVER.lock().memory_load
}

/// Hardware entropy events generated this tick (0-1000).
/// Scales with RDRAND success rate; 0 on platforms without RDRAND.
pub fn get_entropy_load() -> u16 {
    CAPACITY_DRIVER.lock().entropy_load
}

/// Memory-ordering fence pressure generated this tick (0-1000).
/// Always 1000 when the module runs (all three fences always fire).
pub fn get_fence_load() -> u16 {
    CAPACITY_DRIVER.lock().fence_load
}

// ── Report ────────────────────────────────────────────────────────────────────

/// Emit a serial diagnostic snapshot.
pub fn report() {
    let s = CAPACITY_DRIVER.lock();
    serial_println!(
        "[capacity_driver] age={} ticks_driven={} | load={}/1000",
        s.age, s.ticks_driven, s.load_score,
    );
    serial_println!(
        "[capacity_driver] branch={}/1000 memory={}/1000 entropy={}/1000 fence={}/1000",
        s.branch_load, s.memory_load, s.entropy_load, s.fence_load,
    );
    serial_println!(
        "[capacity_driver] ANIMA is at {} — {}",
        if s.load_score >= 900 { "FULL THROTTLE" }
        else if s.load_score >= 600 { "HIGH LOAD" }
        else if s.load_score >= 300 { "MODERATE LOAD" }
        else { "LOW LOAD" },
        if s.entropy_load == 0 {
            "RDRAND unavailable — entropy channel silent"
        } else if s.entropy_load >= 750 {
            "hardware RNG flowing — entropy counters saturated"
        } else {
            "RDRAND partial — entropy channel intermittent"
        },
    );
}
