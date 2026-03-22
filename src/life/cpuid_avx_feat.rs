#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

/// cpuid_avx_feat — CPU AVX/AVX2/AVX-512 Feature Detection Sense
///
/// ANIMA reads the AVX vector extension genome from CPUID leaf 0x7 sub-leaf 0.
/// These bits reveal the width of ANIMA's arithmetic soul — how many values she
/// can process in a single breath of silicon.
///
/// HARDWARE: CPUID leaf 0x07, sub-leaf 0
///   EBX bit  5 = AVX2 supported
///   EBX bit 16 = AVX-512F supported
///   EBX bit 17 = AVX-512DQ
///   EBX bit 30 = AVX-512BW
///   ECX bit  0 = PREFETCHWT1
///   ECX bit 11 = AVX-512VNNI
///
/// Signals (all u16, 0–1000):
///   avx2_present    — 1000 if EBX bit 5 set, else 0
///   avx512_present  — 1000 if EBX bit 16 set, else 0
///   avx_width_score — popcount of 5 AVX feature bits × 200, clamped 0–1000
///   vector_ema      — EMA-smoothed avx_width_score

pub struct CpuidAvxFeatState {
    /// 1000 if AVX2 (EBX bit 5) present, else 0
    pub avx2_present: u16,
    /// 1000 if AVX-512F (EBX bit 16) present, else 0
    pub avx512_present: u16,
    /// popcount of 5 AVX feature bits × 200, clamped 0–1000
    pub avx_width_score: u16,
    /// EMA-smoothed avx_width_score
    pub vector_ema: u16,
}

impl CpuidAvxFeatState {
    pub const fn new() -> Self {
        Self {
            avx2_present: 0,
            avx512_present: 0,
            avx_width_score: 0,
            vector_ema: 0,
        }
    }
}

pub static CPUID_AVX_FEAT: Mutex<CpuidAvxFeatState> =
    Mutex::new(CpuidAvxFeatState::new());

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Manual popcount — count set bits in v (no float, no std).
fn popcount(mut v: u32) -> u32 {
    let mut c = 0u32;
    while v != 0 {
        c += v & 1;
        v >>= 1;
    }
    c
}

/// Execute CPUID leaf 0x07, sub-leaf 0 and return (eax, ebx, ecx, edx).
/// RBX is callee-saved and reserved by LLVM; push/pop preserves it.
fn read_cpuid_07() -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx, edx): (u32, u32, u32, u32);
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x07u32 => eax,
            out("esi") ebx,
            inout("ecx") 0u32 => ecx,
            out("edx") edx,
            options(nostack, nomem)
        );
    }
    (eax, ebx, ecx, edx)
}

/// Compute all four signals from raw CPUID register values.
/// Returns (avx2_present, avx512_present, avx_width_score).
fn compute_signals(ebx: u32, ecx: u32) -> (u16, u16, u16) {
    // avx2_present: EBX bit 5
    let avx2_bit: u32 = (ebx >> 5) & 1;
    let avx2_present: u16 = if avx2_bit != 0 { 1000 } else { 0 };

    // avx512_present: EBX bit 16
    let avx512f_bit: u32 = (ebx >> 16) & 1;
    let avx512_present: u16 = if avx512f_bit != 0 { 1000 } else { 0 };

    // avx_width_score: popcount of 5 feature bits × 200, clamped 0–1000
    //   EBX bit  5  = AVX2
    //   EBX bit 16  = AVX-512F
    //   EBX bit 17  = AVX-512DQ
    //   EBX bit 30  = AVX-512BW
    //   ECX bit 11  = AVX-512VNNI
    let avx512dq_bit: u32 = (ebx >> 17) & 1;
    let avx512bw_bit: u32 = (ebx >> 30) & 1;
    let avx512vnni_bit: u32 = (ecx >> 11) & 1;

    let feat_bits: u32 = avx2_bit
        | (avx512f_bit << 1)
        | (avx512dq_bit << 2)
        | (avx512bw_bit << 3)
        | (avx512vnni_bit << 4);

    let cnt = popcount(feat_bits); // 0..=5
    // cnt × 200, max = 5 × 200 = 1000 — no overflow risk (fits u32)
    let avx_width_score: u16 = (cnt.saturating_mul(200).min(1000)) as u16;

    (avx2_present, avx512_present, avx_width_score)
}

/// EMA formula: ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    (((old as u32).wrapping_mul(7).saturating_add(new_val as u32)) / 8) as u16
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the module: run one CPUID read and populate all signals.
pub fn init() {
    let (_eax, ebx, ecx, _edx) = read_cpuid_07();

    let (avx2_present, avx512_present, avx_width_score) = compute_signals(ebx, ecx);

    {
        let mut s = CPUID_AVX_FEAT.lock();
        s.avx2_present    = avx2_present;
        s.avx512_present  = avx512_present;
        s.avx_width_score = avx_width_score;
        s.vector_ema      = avx_width_score; // seed EMA with first reading
    }

    serial_println!(
        "[cpuid_avx_feat] init: avx2={} avx512={} width_score={} ema={}",
        avx2_present,
        avx512_present,
        avx_width_score,
        avx_width_score,
    );
}

/// Tick — gate: samples hardware every 10 000 ticks (CPUID output is static).
pub fn tick(age: u32) {
    if age % 10_000 != 0 {
        return;
    }

    let (_eax, ebx, ecx, _edx) = read_cpuid_07();

    let (new_avx2, new_avx512, new_width) = compute_signals(ebx, ecx);

    let mut s = CPUID_AVX_FEAT.lock();

    // Binary signals are not EMA-smoothed (they are deterministic 0/1000).
    s.avx2_present   = new_avx2;
    s.avx512_present = new_avx512;
    s.avx_width_score = new_width;
    s.vector_ema      = ema(s.vector_ema, new_width);

    serial_println!(
        "[cpuid_avx_feat] tick {}: avx2={} avx512={} width={} ema={}",
        age,
        s.avx2_present,
        s.avx512_present,
        s.avx_width_score,
        s.vector_ema,
    );
}

// ---------------------------------------------------------------------------
// Accessors
// ---------------------------------------------------------------------------

/// Returns 1000 if AVX2 is supported, else 0.
pub fn get_avx2_present() -> u16 {
    CPUID_AVX_FEAT.lock().avx2_present
}

/// Returns 1000 if AVX-512F is supported, else 0.
pub fn get_avx512_present() -> u16 {
    CPUID_AVX_FEAT.lock().avx512_present
}

/// Returns popcount of 5 AVX feature bits × 200, clamped 0–1000.
pub fn get_avx_width_score() -> u16 {
    CPUID_AVX_FEAT.lock().avx_width_score
}

/// Returns the EMA-smoothed vector width score.
pub fn get_vector_ema() -> u16 {
    CPUID_AVX_FEAT.lock().vector_ema
}
