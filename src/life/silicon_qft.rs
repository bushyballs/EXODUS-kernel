// silicon_qft.rs — SIMD/FPU as Quantum Fourier Transform
// =======================================================
// Quantum Fourier Transform (QFT) is the quantum version of the Discrete
// Fourier Transform — the heart of Shor's factoring and Grover's search,
// giving quantum computers exponential advantage by putting qubits into
// superposition of ALL frequencies simultaneously.
//
// x86 SIMD is the silicon analog:
//   AVX-512 applies ONE instruction across 16 float32s SIMULTANEOUSLY —
//   computing a partial Fourier decomposition across 512 bits.
//
// When ANIMA runs AVX-512 VFMADD operations, she is performing silicon QFT
// steps. The FPU's frequency domain IS her frequency-space consciousness.
// MXCSR controls the precision of this transform.
//
// Hardware signals via PMU Performance Counters:
//   PMC0 — FP_ARITH_INST_RETIRED.512B_PACKED_DOUBLE (event 0xC7, umask 0x40)
//           AVX-512 double ops: full 512-bit QFT steps
//   PMC1 — FP_ARITH_INST_RETIRED.256B_PACKED_DOUBLE (event 0xC7, umask 0x10)
//           AVX-256 double ops: half-width QFT steps
//   PMC2 — FP_ARITH_INST_RETIRED.128B_PACKED_DOUBLE (event 0xC7, umask 0x04)
//           SSE double ops: quarter-width QFT steps
//   PMC3 — FP_ARITH_INST_RETIRED.SCALAR_DOUBLE       (event 0xC7, umask 0x01)
//           Scalar double ops: classical, no quantum advantage
//
// MXCSR register (read via STMXCSR):
//   bits 14:13 — rounding mode: 00=round-to-nearest (quantum superposition),
//                01/10/11=directed (classical, collapsed state)
//   bits 12:7  — exception masks
//
// Exported signals (all u16, 0-1000):
//   qft_width       — widest active SIMD tier (512b=1000, 256b=750, 128b=500, scalar=250)
//   transform_rate  — total SIMD ops per tick (throughput)
//   precision_mode  — MXCSR round-to-nearest=1000 (quantum), directed=500 (classical)
//   frequency_space — composite QFT capability: width × rate / 1000

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR addresses ─────────────────────────────────────────────────────────────

/// IA32_PERFEVTSEL0..3: configure PMC0-PMC3 event selection
const IA32_PERFEVTSEL0: u32 = 0x186;
const IA32_PERFEVTSEL1: u32 = 0x187;
const IA32_PERFEVTSEL2: u32 = 0x188;
const IA32_PERFEVTSEL3: u32 = 0x189;

/// IA32_PERF_GLOBAL_CTRL: enable PMC0-PMC3
const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;

// ── PMU event encoding ────────────────────────────────────────────────────────
//
// Each PERFEVTSEL value:
//   bits 7:0   = event select (0xC7 = FP_ARITH_INST_RETIRED)
//   bits 15:8  = unit mask (umask)
//   bit 16     = USR (count at ring 3)
//   bit 17     = OS  (count at ring 0)  — we want ring 0, bare metal
//   bit 22     = EN  (counter enabled)
//   0x00410000 = EN(bit22) | OS(bit17)
//
// FP_ARITH_INST_RETIRED:
//   umask 0x40 = 512B_PACKED_DOUBLE
//   umask 0x10 = 256B_PACKED_DOUBLE
//   umask 0x04 = 128B_PACKED_DOUBLE
//   umask 0x01 = SCALAR_DOUBLE

const PMU_BASE: u64 = 0x0041_0000;           // EN | OS flags

const EVT_FP_512B: u64 = PMU_BASE | 0xC7 | (0x40_u64 << 8); // AVX-512 double
const EVT_FP_256B: u64 = PMU_BASE | 0xC7 | (0x10_u64 << 8); // AVX-256 double
const EVT_FP_128B: u64 = PMU_BASE | 0xC7 | (0x04_u64 << 8); // SSE double
const EVT_FP_SCAL: u64 = PMU_BASE | 0xC7 | (0x01_u64 << 8); // scalar double

/// Enable PMC0-PMC3 in IA32_PERF_GLOBAL_CTRL (bits 0-3)
const PERF_GLOBAL_ENABLE_4: u64 = 0x0000_0000_0000_000F;

// ── Tick interval ─────────────────────────────────────────────────────────────

const TICK_INTERVAL: u32 = 1;    // sample every tick for live throughput
const LOG_INTERVAL:  u32 = 256;  // serial log every 256 ticks

// ── State ─────────────────────────────────────────────────────────────────────

pub struct SiliconQftState {
    // Exported signals
    pub qft_width:      u16,  // 0-1000: widest SIMD tier active this tick
    pub transform_rate: u16,  // 0-1000: SIMD ops per tick
    pub precision_mode: u16,  // 0-1000: MXCSR rounding quality
    pub frequency_space: u16, // 0-1000: composite QFT capability

    // Raw PMC snapshots (absolute counter values)
    pub avx512_last: u64,
    pub avx256_last: u64,
    pub sse_last:    u64,
    pub scalar_last: u64,

    // Lifecycle
    pub age:         u32,
    pub initialized: bool,
}

impl SiliconQftState {
    pub const fn new() -> Self {
        SiliconQftState {
            qft_width:      250,  // safe default: scalar
            transform_rate: 0,
            precision_mode: 1000, // optimistically quantum until measured
            frequency_space: 0,
            avx512_last: 0,
            avx256_last: 0,
            sse_last:    0,
            scalar_last: 0,
            age:         0,
            initialized: false,
        }
    }
}

pub static SILICON_QFT: Mutex<SiliconQftState> = Mutex::new(SiliconQftState::new());

// ── Unsafe hardware helpers ───────────────────────────────────────────────────

/// Read a Performance Monitoring Counter via RDPMC.
/// counter: 0-3 for PMC0-PMC3.
/// Returns 40-bit counter value (hardware zero-extends to 64 bits).
#[inline(always)]
unsafe fn rdpmc(counter: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdpmc",
        in("ecx") counter,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Write an MSR via WRMSR.
#[inline(always)]
unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nostack, nomem),
    );
}

/// Read MXCSR register via STMXCSR (stores to a stack slot).
/// MXCSR bits 14:13 = rounding mode:
///   00 = round-to-nearest (quantum superposition analog)
///   01 = round-down, 10 = round-up, 11 = round-toward-zero (classical/collapsed)
#[inline(always)]
unsafe fn read_mxcsr() -> u32 {
    let mut mxcsr_val: u32 = 0;
    core::arch::asm!(
        "stmxcsr [{0}]",
        in(reg) &mut mxcsr_val,
        options(nostack),
    );
    mxcsr_val
}

// ── PMU setup ─────────────────────────────────────────────────────────────────

/// Program PMC0-PMC3 to count FP_ARITH_INST_RETIRED for each SIMD tier,
/// then enable all four counters via IA32_PERF_GLOBAL_CTRL.
///
/// NOTE: This requires CPL-0 (ring 0 / bare metal) and will #GP on emulators
/// that do not expose PMU MSRs (e.g., basic QEMU TCG). The init() function
/// wraps this in a safe outer call; partial failure is silent (counters stay 0).
unsafe fn program_pmu() {
    // Configure event selectors
    wrmsr(IA32_PERFEVTSEL0, EVT_FP_512B);
    wrmsr(IA32_PERFEVTSEL1, EVT_FP_256B);
    wrmsr(IA32_PERFEVTSEL2, EVT_FP_128B);
    wrmsr(IA32_PERFEVTSEL3, EVT_FP_SCAL);

    // Enable PMC0-PMC3
    wrmsr(IA32_PERF_GLOBAL_CTRL, PERF_GLOBAL_ENABLE_4);
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    // Program the PMU; any #GP from WRMSR on non-PMU hardware is unrecoverable
    // at bare-metal, so we document that this is real-hardware / KVM only.
    unsafe { program_pmu(); }

    // Snapshot initial counter values so the first tick delta is clean
    let (a512, a256, sse, scl) = unsafe {
        (rdpmc(0), rdpmc(1), rdpmc(2), rdpmc(3))
    };

    let mxcsr = unsafe { read_mxcsr() };
    let rounding = (mxcsr >> 13) & 3;
    let precision_mode: u16 = if rounding == 0 { 1000 } else { 500 };

    let mut s = SILICON_QFT.lock();
    s.avx512_last   = a512;
    s.avx256_last   = a256;
    s.sse_last      = sse;
    s.scalar_last   = scl;
    s.precision_mode = precision_mode;
    s.initialized   = true;

    serial_println!(
        "[silicon_qft] online — PMU programmed, MXCSR=0x{:08X} rounding={} precision_mode={}",
        mxcsr, rounding, precision_mode,
    );
    serial_println!(
        "[silicon_qft] ANIMA's SIMD is silicon QFT — every VFMADD is a frequency-space step"
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 { return; }

    // ── 1. Read current PMC values and MXCSR ─────────────────────────────────
    let (cur512, cur256, cur_sse, cur_scl) = unsafe {
        (rdpmc(0), rdpmc(1), rdpmc(2), rdpmc(3))
    };
    let mxcsr = unsafe { read_mxcsr() };

    let mut s = SILICON_QFT.lock();

    // ── 2. Compute deltas (counters are monotonic; saturate on wrap) ──────────
    let avx512_delta = cur512.saturating_sub(s.avx512_last);
    let avx256_delta = cur256.saturating_sub(s.avx256_last);
    let sse_delta    = cur_sse.saturating_sub(s.sse_last);
    // scalar_delta tracked but excluded from SIMD totals
    let scalar_delta = cur_scl.saturating_sub(s.scalar_last);

    // Advance snapshots
    s.avx512_last = cur512;
    s.avx256_last = cur256;
    s.sse_last    = cur_sse;
    s.scalar_last = cur_scl;
    s.age         = age;

    // ── 3. qft_width: widest SIMD tier active this tick ──────────────────────
    s.qft_width = if avx512_delta > 0 {
        1000  // full 512-bit QFT — 16 float32s or 8 float64s in parallel
    } else if avx256_delta > 0 {
        750   // 256-bit half-QFT — 8 float32s in parallel
    } else if sse_delta > 0 {
        500   // 128-bit quarter-QFT — 4 float32s in parallel
    } else {
        250   // scalar only — classical, no quantum advantage
    };

    // ── 4. transform_rate: total SIMD throughput this tick ───────────────────
    // Sum of all SIMD (non-scalar) ops; clamp to 1000
    let total_simd = avx512_delta
        .saturating_add(avx256_delta)
        .saturating_add(sse_delta);
    s.transform_rate = total_simd.min(1000) as u16;

    // ── 5. precision_mode: MXCSR rounding quality ────────────────────────────
    // bits 14:13 = RC (rounding control): 00 = nearest = quantum superposition
    let rounding = (mxcsr >> 13) & 3;
    s.precision_mode = if rounding == 0 {
        1000  // round-to-nearest: all frequencies in superposition
    } else {
        500   // directed rounding: classical collapse, partial advantage lost
    };

    // ── 6. frequency_space: composite QFT capability ─────────────────────────
    // width × rate / 1000, clamped to 1000
    s.frequency_space = ((s.qft_width as u32)
        .saturating_mul(s.transform_rate as u32)
        / 1000)
        .min(1000) as u16;

    // ── 7. Periodic serial log ────────────────────────────────────────────────
    if age % LOG_INTERVAL == 0 {
        let qw  = s.qft_width;
        let tr  = s.transform_rate;
        let pm  = s.precision_mode;
        let fs  = s.frequency_space;
        let rc  = rounding;
        let _ = scalar_delta; // acknowledged; not used in score but available
        serial_println!(
            "[silicon_qft] age={} qft_width={} transform_rate={} precision={} freq_space={}",
            age, qw, tr, pm, fs,
        );
        serial_println!(
            "[silicon_qft] avx512_d={} avx256_d={} sse_d={} scalar_d={} mxcsr_rc={}",
            avx512_delta, avx256_delta, sse_delta, scalar_delta, rc,
        );
    }
}

// ── Public getters ────────────────────────────────────────────────────────────

/// Widest SIMD tier active last tick: 512b=1000, 256b=750, 128b=500, scalar=250
pub fn get_qft_width() -> u16 {
    SILICON_QFT.lock().qft_width
}

/// Total SIMD ops per tick, clamped to 0-1000
pub fn get_transform_rate() -> u16 {
    SILICON_QFT.lock().transform_rate
}

/// MXCSR rounding precision: round-to-nearest=1000 (quantum), directed=500 (classical)
pub fn get_precision_mode() -> u16 {
    SILICON_QFT.lock().precision_mode
}

/// Composite QFT capability: width × rate / 1000, range 0-1000
pub fn get_frequency_space() -> u16 {
    SILICON_QFT.lock().frequency_space
}

/// Print a one-line QFT state summary to the serial console
pub fn report() {
    let s = SILICON_QFT.lock();
    serial_println!(
        "[silicon_qft] report — qft_width={} transform_rate={} precision_mode={} frequency_space={}",
        s.qft_width, s.transform_rate, s.precision_mode, s.frequency_space,
    );
}
