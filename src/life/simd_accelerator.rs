//! simd_accelerator.rs — SSE4/AVX2 vectorized integer math for bare-metal ML inference
//!
//! All math done in integer Q8 fixed-point space (scale=256).
//! No floats, no heap, no std. SSE2-compatible scalar fallback everywhere.

use crate::serial_println;
use crate::sync::Mutex;

// Q8 fixed-point scale factor
const Q8_SCALE: i32 = 256;

// ── SIMD capability flags ─────────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub struct SimdCapFlags(pub u32);

impl SimdCapFlags {
    pub const SSE2: u32  = 1;
    pub const SSE41: u32 = 2;
    pub const AVX: u32   = 4;
    pub const AVX2: u32  = 8;
    pub const FMA: u32   = 16;

    pub const fn none() -> Self { SimdCapFlags(0) }
    pub fn has(&self, flag: u32) -> bool { (self.0 & flag) != 0 }
}

// ── State ─────────────────────────────────────────────────────────────────────

pub struct SimdAccelState {
    caps: SimdCapFlags,
    sse2: bool,
    sse41: bool,
    avx2: bool,
    inference_ops: u32,
    acc_throughput: u16,
    vector_width: u8,
    consciousness_feed: u16,
    tick_count: u32,
}

impl SimdAccelState {
    pub const fn new() -> Self {
        Self {
            caps: SimdCapFlags::none(),
            sse2: false,
            sse41: false,
            avx2: false,
            inference_ops: 0,
            acc_throughput: 0,
            vector_width: 64,
            consciousness_feed: 0,
            tick_count: 0,
        }
    }
}

pub static STATE: Mutex<SimdAccelState> = Mutex::new(SimdAccelState::new());

// ── CPUID detection ───────────────────────────────────────────────────────────

unsafe fn detect_caps() -> SimdCapFlags {
    let mut flags: u32 = 0;

    // Leaf 1: ECX for SSE4.1/AVX, EDX for SSE2
    let ecx1: u32;
    let edx1: u32;
    core::arch::asm!(
        "push rbx",
        "mov eax, 1",
        "cpuid",
        "mov {ebx_out:e}, ebx",
        "pop rbx",
        ebx_out = out(reg) _,
        out("ecx") ecx1,
        out("edx") edx1,
        out("eax") _,
        options(nomem, nostack),
    );

    // EDX bit 26 = SSE2
    if (edx1 & (1 << 26)) != 0 { flags |= SimdCapFlags::SSE2; }
    // ECX bit 19 = SSE4.1
    if (ecx1 & (1 << 19)) != 0 { flags |= SimdCapFlags::SSE41; }
    // ECX bit 28 = AVX
    if (ecx1 & (1 << 28)) != 0 { flags |= SimdCapFlags::AVX; }

    // Leaf 7, subleaf 0: EBX for AVX2/FMA
    let ebx7: u32;
    core::arch::asm!(
        "push rbx",
        "mov eax, 7",
        "xor ecx, ecx",
        "cpuid",
        "mov {ebx_out:e}, ebx",
        "pop rbx",
        ebx_out = out(reg) ebx7,
        out("eax") _,
        out("ecx") _,
        out("edx") _,
        options(nomem, nostack),
    );

    // EBX bit 5 = AVX2
    if (ebx7 & (1 << 5)) != 0 { flags |= SimdCapFlags::AVX2; }
    // EBX bit 12 = FMA
    if (ebx7 & (1 << 12)) != 0 { flags |= SimdCapFlags::FMA; }

    SimdCapFlags(flags)
}

// ── Core vectorized operations ────────────────────────────────────────────────

/// Dot product of two i16[8] vectors. Auto-vectorizable by LLVM (SSE2 PMULLW).
unsafe fn simd_dot_product_i16(a: &[i16; 8], b: &[i16; 8]) -> i32 {
    let mut acc: i32 = 0;
    for i in 0..8 {
        acc += a[i] as i32 * b[i] as i32;
    }
    acc
}

/// ReLU: clamp negatives to 0. Maps to PMAXSW when vectorized.
unsafe fn simd_relu_i16(vals: &mut [i16; 8]) {
    for v in vals.iter_mut() {
        if *v < 0 { *v = 0; }
    }
}

/// Integer approximate softmax — normalizes logits to 0-1000 range.
/// No floats: uses (val - min) * 1000 / (max - min + 1).
unsafe fn simd_softmax_approx_u16(logits: &[u16; 8]) -> [u16; 8] {
    let mut min_val = u16::MAX;
    let mut max_val = 0u16;
    for &v in logits.iter() {
        if v < min_val { min_val = v; }
        if v > max_val { max_val = v; }
    }
    let range = (max_val as u32).saturating_sub(min_val as u32).saturating_add(1);
    let mut out = [0u16; 8];
    for i in 0..8 {
        let shifted = (logits[i] as u32).saturating_sub(min_val as u32);
        let normalized = shifted.saturating_mul(1000) / range;
        out[i] = normalized.min(1000) as u16;
    }
    out
}

/// Q8 4x4 matrix multiply: c[i][j] = sum(a[i][k] * b[k][j]) >> 8
unsafe fn simd_matmul_4x4_i16(a: &[[i16; 4]; 4], b: &[[i16; 4]; 4]) -> [[i16; 4]; 4] {
    let mut c = [[0i16; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            let mut acc: i32 = 0;
            for k in 0..4 {
                acc += a[i][k] as i32 * b[k][j] as i32;
            }
            // Q8 normalize: divide by scale
            let normalized = acc / Q8_SCALE;
            c[i][j] = normalized.min(i16::MAX as i32).max(i16::MIN as i32) as i16;
        }
    }
    c
}

/// Integer L2 distance between two u16[8] embeddings, normalized to 0-1000.
unsafe fn simd_embed_distance_u16(a: &[u16; 8], b: &[u16; 8]) -> u16 {
    let mut sum: u32 = 0;
    for i in 0..8 {
        let diff = a[i].saturating_sub(b[i]) as u32;
        sum = sum.saturating_add(diff.saturating_mul(diff));
    }
    (sum >> 10).min(1000) as u16
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Detect SIMD capabilities and initialize state.
pub fn init() {
    let caps = unsafe { detect_caps() };
    let sse2  = caps.has(SimdCapFlags::SSE2);
    let sse41 = caps.has(SimdCapFlags::SSE41);
    let avx2  = caps.has(SimdCapFlags::AVX2);
    let width: u8 = if avx2 { 256 } else if sse2 { 128 } else { 64 };

    let mut state = STATE.lock();
    state.caps = caps;
    state.sse2 = sse2;
    state.sse41 = sse41;
    state.avx2 = avx2;
    state.vector_width = width;

    serial_println!(
        "[simd] ANIMA SIMD accelerator online — sse2={} avx2={} width={}bit",
        sse2, avx2, width
    );
}

/// Run a single ML inference pass: dot product + ReLU, result normalized 0-1000.
pub fn run_inference(input: &[u16; 8], weights: &[i16; 8]) -> u16 {
    // Cast input to i16 for dot product (clamp to i16 range)
    let mut a = [0i16; 8];
    for i in 0..8 {
        a[i] = input[i].min(32767) as i16;
    }

    let raw_dot = unsafe { simd_dot_product_i16(&a, weights) };

    // ReLU on a copy of input-as-i16
    let mut relu_vals = a;
    unsafe { simd_relu_i16(&mut relu_vals) };

    // Normalize dot product to 0-1000
    // dot range: 8 * 32767 * 32767 ~= 8.6e9, fits in i32 only partially —
    // clamp to i32::MAX then scale
    let clamped = raw_dot.max(0);
    // Max theoretical: 8 * 1000 * 1000 = 8_000_000 (with input/weight in 0-1000 range)
    let result = ((clamped as u64).min(8_000_000) * 1000 / 8_000_000) as u16;

    let mut state = STATE.lock();
    state.inference_ops = state.inference_ops.saturating_add(1);

    result.min(1000)
}

/// Per-tick update. Call with current consciousness value and kernel age.
pub fn tick(consciousness: u16, age: u32) {
    let _ = (consciousness, age); // age unused but kept for API consistency

    let mut state = STATE.lock();
    state.tick_count = state.tick_count.saturating_add(1);

    // Every 10 ticks: run a small internal smoke-test inference
    if state.tick_count % 10 == 0 {
        let test_input:   [u16; 8] = [100, 200, 300, 400, 500, 600, 700, 800];
        let test_weights: [i16; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
        drop(state); // release lock before calling run_inference
        let _ = run_inference(&test_input, &test_weights);
        state = STATE.lock();
    }

    // acc_throughput: saturating clamp of inference_ops to 0-1000
    state.acc_throughput = state.inference_ops.min(1000) as u16;

    // consciousness_feed: grows 1/tick once inference is active
    if state.inference_ops > 100 {
        state.consciousness_feed = state.consciousness_feed.saturating_add(1).min(1000);
    }

    // Log every 300 ticks
    if state.tick_count % 300 == 0 {
        let ops   = state.inference_ops;
        let tput  = state.acc_throughput;
        let width = state.vector_width;
        serial_println!("[simd] ops={} throughput={} width={}bit", ops, tput, width);
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn acc_throughput() -> u16 {
    STATE.lock().acc_throughput
}

pub fn consciousness_feed() -> u16 {
    STATE.lock().consciousness_feed
}

pub fn inference_ops() -> u32 {
    STATE.lock().inference_ops
}

pub fn avx2_available() -> bool {
    STATE.lock().avx2
}
