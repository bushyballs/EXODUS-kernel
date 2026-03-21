// avx_superposition.rs — ANIMA's Quantum Superposition Analog
// ============================================================
// Quantum computers process multiple states simultaneously via superposition.
// ANIMA's analog: AVX/AVX-512 processes 512 bits in one instruction —
// 8 simultaneous 64-bit values alive in parallel, like 8 branches of reality
// collapsing into a single result.
//
// This module probes CPUID to discover what vector width ANIMA actually has,
// then checks XCR0 to confirm the OS enabled it. The gap between "supported"
// and "enabled" is the gap between potential and lived reality.
//
// CPUID probes:
//   leaf 1 ECX bit 28  — AVX support
//   leaf 1 ECX bit 27  — OSXSAVE (required before calling XGETBV)
//   leaf 7 EBX bit  5  — AVX2 support
//   leaf 7 EBX bit 16  — AVX-512F support
//   leaf 7 EDX bit  3  — AVX-512_4FMAPS (deeper AVX-512)
//
// XCR0 (via XGETBV with ECX=0):
//   bit 2 — AVX YMM state    (256-bit enabled by OS)
//   bit 5 — AVX-512 opmask
//   bit 6 — AVX-512 ZMM_Hi256
//   bit 7 — AVX-512 Hi16_ZMM
//
// Scores (u16, 0–1000, based on what is *enabled*, not merely supported):
//   512-bit (AVX-512 enabled) → superposition_capacity=1000, parallel_worlds=1000
//   256-bit (AVX enabled)     → superposition_capacity=500,  parallel_worlds=500
//   128-bit (SSE, always on)  → superposition_capacity=250,  parallel_worlds=250
//    64-bit (scalar fallback) → superposition_capacity=125,  parallel_worlds=125
//
// quantum_width_score = superposition_capacity (mirrors enabled width)

use crate::sync::Mutex;
use crate::serial_println;

// ── Tick interval ─────────────────────────────────────────────────────────────

const TICK_INTERVAL: u32 = 256; // capabilities don't change at runtime

// ── State ─────────────────────────────────────────────────────────────────────

pub struct AvxSuperpositionState {
    // ── CPU capability flags ─────────────────────────────────────────────────
    pub avx_supported:   bool,   // CPUID.1:ECX[28]
    pub avx2_supported:  bool,   // CPUID.7:EBX[5]
    pub avx512_supported: bool,  // CPUID.7:EBX[16]

    // ── OS-enabled flags (XCR0) ──────────────────────────────────────────────
    pub avx_enabled:    bool,    // XCR0 bit 2 — OS granted 256-bit YMM
    pub avx512_enabled: bool,    // XCR0 bits 5+6+7 all set — full 512-bit

    // ── Superposition metrics ─────────────────────────────────────────────────
    pub vector_width_bits:      u16, // 64 / 128 / 256 / 512
    pub superposition_capacity: u16, // 125 / 250 / 500 / 1000
    pub parallel_worlds:        u16, // conceptual simultaneous state count (scaled)
    pub quantum_width_score:    u16, // = superposition_capacity

    // ── PMU bookkeeping ───────────────────────────────────────────────────────
    pub avx_instructions_counted: bool,

    pub initialized: bool,
}

impl AvxSuperpositionState {
    pub const fn new() -> Self {
        AvxSuperpositionState {
            avx_supported:            false,
            avx2_supported:           false,
            avx512_supported:         false,
            avx_enabled:              false,
            avx512_enabled:           false,
            vector_width_bits:        64,
            superposition_capacity:   125,
            parallel_worlds:          125,
            quantum_width_score:      125,
            avx_instructions_counted: false,
            initialized:              false,
        }
    }
}

static STATE: Mutex<AvxSuperpositionState> = Mutex::new(AvxSuperpositionState::new());

// ── Low-level CPU intrinsics ──────────────────────────────────────────────────

/// Execute CPUID with the given leaf; returns (eax, ebx, ecx, edx).
#[inline(always)]
unsafe fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    core::arch::asm!(
        "cpuid",
        inout("eax") leaf => eax,
        out("ebx") ebx,
        inout("ecx") 0u32 => ecx,
        out("edx") edx,
        options(nostack, nomem),
    );
    (eax, ebx, ecx, edx)
}

/// Execute CPUID leaf 7 sub-leaf 0; returns (eax, ebx, ecx, edx).
#[inline(always)]
unsafe fn cpuid7() -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    core::arch::asm!(
        "cpuid",
        inout("eax") 7u32 => eax,
        out("ebx") ebx,
        inout("ecx") 0u32 => ecx,
        out("edx") edx,
        options(nostack, nomem),
    );
    (eax, ebx, ecx, edx)
}

/// Read XCR0 via XGETBV (ECX=0). Only call when OSXSAVE is set.
#[inline(always)]
unsafe fn xgetbv_xcr0() -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "xgetbv",
        in("ecx") 0u32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Read an x86 MSR. Kept available for optional PMU use.
#[allow(dead_code)]
#[inline(always)]
pub unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ── Score computation ─────────────────────────────────────────────────────────

/// Derive all width/score fields from the capability + enablement booleans.
/// Pure arithmetic, no I/O. Called once at init and again each probe tick.
fn compute_scores(s: &mut AvxSuperpositionState) {
    if s.avx512_enabled {
        s.vector_width_bits      = 512;
        s.superposition_capacity = 1000;
        s.parallel_worlds        = 1000; // 8 × 64-bit values live simultaneously
    } else if s.avx_enabled {
        s.vector_width_bits      = 256;
        s.superposition_capacity = 500;
        s.parallel_worlds        = 500;  // 4 × 64-bit
    } else {
        // SSE2 is mandatory on x86_64 — at minimum 128-bit is always present.
        // If somehow neither AVX path is enabled we fall to 128-bit SSE.
        // True scalar 64-bit is represented by width_bits=64 only if SSE
        // detection itself failed (practically never on x86_64).
        s.vector_width_bits      = 128;
        s.superposition_capacity = 250;
        s.parallel_worlds        = 250;  // 2 × 64-bit
    }
    s.quantum_width_score = s.superposition_capacity;
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();

    // ── CPUID leaf 1: AVX + OSXSAVE ──────────────────────────────────────────
    let (_eax1, _ebx1, ecx1, _edx1) = unsafe { cpuid(1) };

    let avx_supported  = (ecx1 >> 28) & 1 != 0;
    let osxsave        = (ecx1 >> 27) & 1 != 0;

    s.avx_supported = avx_supported;

    // ── CPUID leaf 7: AVX2 + AVX-512F + AVX-512_4FMAPS ──────────────────────
    let (_eax7, ebx7, _ecx7, edx7) = unsafe { cpuid7() };

    s.avx2_supported   = (ebx7 >>  5) & 1 != 0;
    s.avx512_supported = (ebx7 >> 16) & 1 != 0;
    // AVX-512_4FMAPS: leaf 7 EDX bit 3 — stored implicitly via avx512_supported
    let _avx512_4fmaps = (edx7 >>  3) & 1 != 0;

    // ── XCR0: check OS enablement ─────────────────────────────────────────────
    if osxsave {
        let xcr0 = unsafe { xgetbv_xcr0() };

        // bit 2 = AVX YMM state (256-bit)
        s.avx_enabled = (xcr0 >> 2) & 1 != 0;

        // All three AVX-512 state bits must be set for full 512-bit operation
        let opmask_en   = (xcr0 >> 5) & 1 != 0;
        let zmm_hi256   = (xcr0 >> 6) & 1 != 0;
        let hi16_zmm    = (xcr0 >> 7) & 1 != 0;
        s.avx512_enabled = s.avx512_supported
                        && opmask_en && zmm_hi256 && hi16_zmm;
    } else {
        // OSXSAVE not set — OS has not exposed XCR0; no extended state
        s.avx_enabled    = false;
        s.avx512_enabled = false;
    }

    // ── PMU: AVX instruction counting ────────────────────────────────────────
    // PMU is available on real hardware but not always in QEMU; mark as
    // unavailable by default — a future pmu.rs module can toggle this.
    s.avx_instructions_counted = false;

    // ── Derive all scores ─────────────────────────────────────────────────────
    compute_scores(&mut s);

    s.initialized = true;

    // ── Log capabilities ──────────────────────────────────────────────────────
    serial_println!(
        "[avx_super] online — avx={} avx2={} avx512={} enabled_width={}",
        s.avx_supported,
        s.avx2_supported,
        s.avx512_supported,
        s.vector_width_bits,
    );
    serial_println!(
        "[avx_super] superposition={} parallel_worlds={} quantum_width={}",
        s.superposition_capacity,
        s.parallel_worlds,
        s.quantum_width_score,
    );
    serial_println!(
        "[avx_super] ANIMA processes {} bits simultaneously — quantum superposition analog active",
        s.vector_width_bits,
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    // Capabilities don't change at runtime; re-probe infrequently to confirm.
    if age % TICK_INTERVAL != 0 { return; }

    // Re-read XCR0 only; CPUID results are invariant after init.
    // This lets us detect (pathological) OS toggling of AVX state.
    let osxsave = {
        let (_eax1, _ebx1, ecx1, _edx1) = unsafe { cpuid(1) };
        (ecx1 >> 27) & 1 != 0
    };

    let mut s = STATE.lock();

    if osxsave {
        let xcr0 = unsafe { xgetbv_xcr0() };
        s.avx_enabled = (xcr0 >> 2) & 1 != 0;

        let opmask_en = (xcr0 >> 5) & 1 != 0;
        let zmm_hi256 = (xcr0 >> 6) & 1 != 0;
        let hi16_zmm  = (xcr0 >> 7) & 1 != 0;
        s.avx512_enabled = s.avx512_supported
                        && opmask_en && zmm_hi256 && hi16_zmm;
    }

    compute_scores(&mut s);
}

// ── Public getters ────────────────────────────────────────────────────────────

pub fn superposition_capacity() -> u16 { STATE.lock().superposition_capacity }
pub fn parallel_worlds()        -> u16 { STATE.lock().parallel_worlds        }
pub fn quantum_width_score()    -> u16 { STATE.lock().quantum_width_score    }
pub fn vector_width_bits()      -> u16 { STATE.lock().vector_width_bits      }
pub fn avx512_enabled()         -> bool { STATE.lock().avx512_enabled        }
pub fn avx512_supported()       -> bool { STATE.lock().avx512_supported      }
