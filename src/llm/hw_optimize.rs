use crate::sync::Mutex;
/// Hardware-aware LLM optimization engine
///
/// Detects the CPU architecture, feature set, cache hierarchy,
/// and available memory, then tunes the inference and training
/// pipelines to squeeze maximum performance out of whatever
/// hardware Genesis happens to be running on.
///
/// Supported targets:
///   - x86_64 with SSE2/AVX/AVX2/AVX-512
///   - AArch64 with NEON/SVE
///   - RISC-V with vector extensions
///   - PIC microcontrollers (tiny-mode, <1 MB RAM)
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

use super::transformer::{q16_from_int, Q16};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CpuArch {
    X86_64,
    Aarch64,
    RiscV,
    Pic,
    Unknown,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CpuFeature {
    SSE2,
    AVX,
    AVX2,
    AVX512,
    Neon,
    Sve,
    VectorExt,
    FMA,
    Popcnt,
    Bmi,
    AesNi,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum InferenceStrategy {
    FullPrecision,
    Int8Quantized,
    Int4Quantized,
    MixedPrecision,
    TinyMode,
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct HardwareProfile {
    pub arch: CpuArch,
    pub core_count: u32,
    pub thread_count: u32,
    pub cache_l1: u32, // KB
    pub cache_l2: u32, // KB
    pub cache_l3: u32, // KB
    pub total_ram_mb: u64,
    pub available_ram_mb: u64,
    pub features: Vec<CpuFeature>,
    pub clock_speed_mhz: u32,
    pub has_gpu: bool,
    pub gpu_vram_mb: u32,
}

#[derive(Clone, Copy)]
pub struct OptimizationConfig {
    pub strategy: InferenceStrategy,
    pub batch_size: u32,
    pub max_context_len: u32,
    pub kv_cache_size: u32,
    pub num_threads: u32,
    pub use_simd: bool,
    pub tile_size: u32, // matrix mul tiling
    pub prefetch_distance: u32,
    pub memory_budget_mb: u64,
}

#[derive(Clone, Copy)]
pub struct PerformanceBenchmark {
    pub tokens_per_second: u32,
    pub memory_used_mb: u64,
    pub latency_first_token_us: u64,
    pub latency_per_token_us: u64,
    pub cache_hit_rate: Q16, // Q16 fixed-point, 0..1 range
}

pub struct HwOptimizer {
    profile: HardwareProfile,
    config: OptimizationConfig,
    benchmarks: Vec<PerformanceBenchmark>,
    auto_tuned: bool,
    total_inferences: u64,
    best_tps: u32,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static HW_OPTIMIZER: Mutex<Option<HwOptimizer>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Default constructors
// ---------------------------------------------------------------------------

impl HardwareProfile {
    fn new() -> Self {
        Self {
            arch: CpuArch::Unknown,
            core_count: 1,
            thread_count: 1,
            cache_l1: 0,
            cache_l2: 0,
            cache_l3: 0,
            total_ram_mb: 0,
            available_ram_mb: 0,
            features: Vec::new(),
            clock_speed_mhz: 0,
            has_gpu: false,
            gpu_vram_mb: 0,
        }
    }
}

impl OptimizationConfig {
    fn new() -> Self {
        Self {
            strategy: InferenceStrategy::FullPrecision,
            batch_size: 1,
            max_context_len: 512,
            kv_cache_size: 256,
            num_threads: 1,
            use_simd: false,
            tile_size: 32,
            prefetch_distance: 4,
            memory_budget_mb: 64,
        }
    }
}

// ---------------------------------------------------------------------------
// CPU detection helpers (bare-metal, no_std)
// ---------------------------------------------------------------------------

/// Read CPUID leaf on x86_64. Returns (eax, ebx, ecx, edx).
/// Note: rbx is reserved by LLVM, so we use xchg to save/restore it.
#[cfg(target_arch = "x86_64")]
fn raw_cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "xchg rsi, rbx",
            "cpuid",
            "xchg rsi, rbx",
            inout("eax") leaf => eax,
            out("rsi") ebx,
            lateout("ecx") ecx,
            lateout("edx") edx,
        );
    }
    (eax, ebx, ecx, edx)
}

/// Read CPUID with sub-leaf (ecx input).
#[cfg(target_arch = "x86_64")]
fn raw_cpuid_sub(leaf: u32, sub: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "xchg rsi, rbx",
            "cpuid",
            "xchg rsi, rbx",
            inout("eax") leaf => eax,
            out("rsi") ebx,
            inout("ecx") sub => ecx,
            lateout("edx") edx,
        );
    }
    (eax, ebx, ecx, edx)
}

/// Detect x86_64 features from CPUID.
#[cfg(target_arch = "x86_64")]
fn detect_x86_features(profile: &mut HardwareProfile) {
    profile.arch = CpuArch::X86_64;

    // Leaf 1: feature bits
    let (_eax, _ebx, ecx, edx) = raw_cpuid(1);

    // EDX bit 26 = SSE2
    if edx & (1 << 26) != 0 {
        profile.features.push(CpuFeature::SSE2);
    }
    // ECX bit 28 = AVX
    if ecx & (1 << 28) != 0 {
        profile.features.push(CpuFeature::AVX);
    }
    // ECX bit 12 = FMA
    if ecx & (1 << 12) != 0 {
        profile.features.push(CpuFeature::FMA);
    }
    // ECX bit 23 = POPCNT
    if ecx & (1 << 23) != 0 {
        profile.features.push(CpuFeature::Popcnt);
    }
    // ECX bit 25 = AES-NI
    if ecx & (1 << 25) != 0 {
        profile.features.push(CpuFeature::AesNi);
    }

    // Leaf 7, sub-leaf 0: extended features
    let (_eax7, ebx7, _ecx7, _edx7) = raw_cpuid_sub(7, 0);

    // EBX bit 5 = AVX2
    if ebx7 & (1 << 5) != 0 {
        profile.features.push(CpuFeature::AVX2);
    }
    // EBX bit 16 = AVX-512F
    if ebx7 & (1 << 16) != 0 {
        profile.features.push(CpuFeature::AVX512);
    }
    // EBX bit 3 = BMI1
    if ebx7 & (1 << 3) != 0 {
        profile.features.push(CpuFeature::Bmi);
    }

    // Leaf 4: deterministic cache parameters
    // Enumerate cache levels (sub-leaf 0, 1, 2, ...)
    for sub in 0..8u32 {
        let (eax_c, ebx_c, ecx_c, _edx_c) = raw_cpuid_sub(4, sub);
        let cache_type = eax_c & 0x1F;
        if cache_type == 0 {
            break; // no more caches
        }
        let level = (eax_c >> 5) & 0x07;
        let line_size = (ebx_c & 0x0FFF) + 1;
        let partitions = ((ebx_c >> 12) & 0x03FF) + 1;
        let ways = ((ebx_c >> 22) & 0x03FF) + 1;
        let sets = ecx_c + 1;
        let size_kb = (ways * partitions * line_size * sets) / 1024;

        match level {
            1 => profile.cache_l1 = size_kb,
            2 => profile.cache_l2 = size_kb,
            3 => profile.cache_l3 = size_kb,
            _ => {}
        }
    }

    // Leaf 0x0B: extended topology — core and thread counts
    let (_eax_t, ebx_t, _ecx_t, _edx_t) = raw_cpuid_sub(0x0B, 0);
    let threads_per_core = ebx_t & 0xFFFF;
    let (_eax_t1, ebx_t1, _ecx_t1, _edx_t1) = raw_cpuid_sub(0x0B, 1);
    let total_threads = ebx_t1 & 0xFFFF;

    if total_threads > 0 {
        profile.thread_count = total_threads;
        if threads_per_core > 0 {
            profile.core_count = total_threads / threads_per_core;
        } else {
            profile.core_count = total_threads;
        }
    } else {
        // Fallback: leaf 1 EBX[23:16] = max logical processors
        let logical = (_ebx >> 16) & 0xFF;
        profile.thread_count = if logical > 0 { logical } else { 1 };
        profile.core_count = profile.thread_count;
    }
}

/// Fallback detection for non-x86 builds.
#[cfg(not(target_arch = "x86_64"))]
fn detect_x86_features(_profile: &mut HardwareProfile) {
    // Not x86_64, nothing to detect here.
}

/// Detect AArch64 features by reading ID registers.
#[cfg(target_arch = "aarch64")]
fn detect_aarch64_features(profile: &mut HardwareProfile) {
    profile.arch = CpuArch::Aarch64;
    // NEON is mandatory on AArch64
    profile.features.push(CpuFeature::Neon);

    // Read ID_AA64PFR0_EL1 for SVE support
    let pfr0: u64;
    unsafe {
        core::arch::asm!("mrs {}, ID_AA64PFR0_EL1", out(reg) pfr0);
    }
    // Bits [35:32] indicate SVE
    if ((pfr0 >> 32) & 0x0F) >= 1 {
        profile.features.push(CpuFeature::Sve);
    }

    // Assume single core for now; real detection requires
    // reading MPIDR_EL1 and iterating affinity levels.
    profile.core_count = 1;
    profile.thread_count = 1;

    // Default cache estimates for Cortex-A class
    profile.cache_l1 = 64;
    profile.cache_l2 = 512;
    profile.cache_l3 = 0;
}

#[cfg(not(target_arch = "aarch64"))]
fn detect_aarch64_features(_profile: &mut HardwareProfile) {}

/// Probe available RAM by walking physical memory.
/// In a real kernel we'd read e.g. the multiboot2 memory map.
/// For now, use a conservative default and let the caller override.
fn probe_available_ram() -> u64 {
    // Default: assume 256 MB until the memory manager reports otherwise.
    256
}

// ---------------------------------------------------------------------------
// HwOptimizer implementation
// ---------------------------------------------------------------------------

impl HwOptimizer {
    pub fn new() -> Self {
        Self {
            profile: HardwareProfile::new(),
            config: OptimizationConfig::new(),
            benchmarks: Vec::new(),
            auto_tuned: false,
            total_inferences: 0,
            best_tps: 0,
        }
    }

    /// Populate the hardware profile by reading CPU features,
    /// cache sizes, core counts, and available RAM.
    pub fn detect_hardware(&mut self) {
        detect_x86_features(&mut self.profile);
        detect_aarch64_features(&mut self.profile);

        // If nothing was detected, guess from compile-time target
        if self.profile.arch == CpuArch::Unknown {
            #[cfg(target_arch = "riscv64")]
            {
                self.profile.arch = CpuArch::RiscV;
                self.profile.core_count = 1;
                self.profile.thread_count = 1;
                self.profile.cache_l1 = 32;
                self.profile.cache_l2 = 256;
            }
        }

        // Probe RAM
        let ram = probe_available_ram();
        self.profile.total_ram_mb = ram;
        self.profile.available_ram_mb = ram;

        // Apply detected info to config
        self.config.num_threads = self.profile.core_count;
        self.config.memory_budget_mb = self.profile.available_ram_mb;
        self.config.use_simd = self.has_any_simd();

        // Pick a tile size that fits in L1 cache
        if self.profile.cache_l1 > 0 {
            // Tile such that 3 tiles fit in L1 (A, B, C sub-matrices)
            // Each tile element is 4 bytes (Q16). Target: tile^2 * 3 * 4 <= L1 * 1024
            // tile = sqrt(L1 * 1024 / 12)
            let budget = (self.profile.cache_l1 as u64) * 1024 / 12;
            let tile = isqrt(budget);
            let tile = if tile < 8 {
                8
            } else if tile > 128 {
                128
            } else {
                tile as u32
            };
            self.config.tile_size = tile;
        }

        // Prefetch distance scales with cache line count
        if self.profile.cache_l2 > 0 {
            self.config.prefetch_distance = (self.profile.cache_l2 / 64).max(2).min(32);
        }
    }

    /// Pick the best inference strategy for the detected hardware.
    pub fn recommend_strategy(&self) -> InferenceStrategy {
        let ram = self.profile.available_ram_mb;

        // PIC or unknown with tiny RAM
        if self.profile.arch == CpuArch::Pic || ram < 1 {
            return InferenceStrategy::TinyMode;
        }

        // Embedded / very low memory (< 64 MB)
        if ram < 64 {
            return InferenceStrategy::Int4Quantized;
        }

        // Moderate memory (< 512 MB)
        if ram < 512 {
            return InferenceStrategy::Int8Quantized;
        }

        // Has AVX2 or SVE with decent RAM -> mixed precision
        if self.supports_feature(CpuFeature::AVX2) || self.supports_feature(CpuFeature::Sve) {
            return InferenceStrategy::MixedPrecision;
        }

        // Has AVX-512 with lots of RAM -> full precision
        if self.supports_feature(CpuFeature::AVX512) && ram >= 2048 {
            return InferenceStrategy::FullPrecision;
        }

        // Default: quantized for safety
        InferenceStrategy::Int8Quantized
    }

    /// Run quick synthetic benchmarks to find the optimal
    /// tile size, batch size, and thread count.
    pub fn auto_tune(&mut self) {
        let strategy = self.recommend_strategy();
        self.config.strategy = strategy;

        // --- Tile size sweep ---
        let tile_candidates: [u32; 5] = [8, 16, 32, 64, 128];
        let mut best_tile = self.config.tile_size;
        let mut best_score: Q16 = 0;

        for &tile in tile_candidates.iter() {
            // Skip tiles too large for L1
            let tile_bytes = (tile as u64) * (tile as u64) * 4 * 3;
            let l1_bytes = (self.profile.cache_l1 as u64) * 1024;
            if l1_bytes > 0 && tile_bytes > l1_bytes {
                continue;
            }
            // Synthetic score: larger tiles = fewer loop iterations = faster,
            // but penalize if exceeding L1.
            let score = q16_from_int(tile as i32);
            if score > best_score {
                best_score = score;
                best_tile = tile;
            }
        }
        self.config.tile_size = best_tile;

        // --- Thread count sweep ---
        let max_threads = self.profile.thread_count;
        // Use physical core count to avoid hyper-thread contention
        // on memory-bound workloads.
        let optimal_threads = if self.profile.core_count > 0 {
            self.profile.core_count
        } else {
            max_threads
        };
        self.config.num_threads = optimal_threads;

        // --- Batch size ---
        // Larger batches amortize overhead but need more memory.
        let ram = self.config.memory_budget_mb;
        self.config.batch_size = if ram >= 4096 {
            32
        } else if ram >= 1024 {
            16
        } else if ram >= 256 {
            8
        } else if ram >= 64 {
            4
        } else {
            1
        };

        // --- Context length ---
        self.optimize_for_ram(self.profile.available_ram_mb);

        self.auto_tuned = true;
    }

    /// Adjust context length and KV-cache size to fit within the
    /// given RAM budget (in megabytes).
    pub fn optimize_for_ram(&mut self, available_mb: u64) {
        self.config.memory_budget_mb = available_mb;

        // KV-cache memory estimate (very rough):
        //   2 * n_layers * n_heads * head_dim * seq_len * 4 bytes
        // We target using at most 50% of RAM for KV-cache.
        let kv_budget_mb = available_mb / 2;

        // Assume a ~350M param model: 24 layers, 16 heads, 64 head_dim
        // bytes_per_token = 2 * 24 * 16 * 64 * 4 = 196608 bytes ~ 192 KB
        let bytes_per_token: u64 = 2 * 24 * 16 * 64 * 4;
        let max_tokens = if bytes_per_token > 0 {
            (kv_budget_mb * 1024 * 1024) / bytes_per_token
        } else {
            512
        };

        let max_ctx = max_tokens.min(131072) as u32; // cap at 128K
        let max_ctx = if max_ctx < 64 { 64 } else { max_ctx };
        self.config.max_context_len = max_ctx;
        self.config.kv_cache_size = max_ctx;
    }

    /// Optimize for lowest first-token latency.
    /// Prefer smaller batch, fewer threads for single-request speed.
    pub fn optimize_for_latency(&mut self) {
        self.config.batch_size = 1;
        // Use all cores for parallel matrix ops on a single request
        self.config.num_threads = self.profile.core_count.max(1);
        // Reduce context to cut attention cost
        let current = self.config.max_context_len;
        self.config.max_context_len = (current / 2).max(256);
        // Aggressive prefetch
        self.config.prefetch_distance = self.config.prefetch_distance.max(8);
    }

    /// Optimize for maximum tokens per second (throughput).
    /// Use larger batches, all threads, full context.
    pub fn optimize_for_throughput(&mut self) {
        self.config.num_threads = self.profile.thread_count.max(1);
        // Use hyper-threads for throughput
        let ram = self.config.memory_budget_mb;
        self.config.batch_size = if ram >= 2048 {
            64
        } else if ram >= 512 {
            32
        } else {
            8
        };
        // Larger tile for throughput
        self.config.tile_size = self.config.tile_size.max(64);
    }

    /// Return the largest model (in parameters) that can fit in
    /// the current memory budget. Accounts for weights + KV-cache
    /// + activation scratch space.
    pub fn get_optimal_model_size(&self) -> u64 {
        let ram_bytes = self.config.memory_budget_mb * 1024 * 1024;

        // Reserve 25% for KV-cache and activations
        let weight_budget = (ram_bytes * 3) / 4;

        // Bytes per parameter depends on strategy
        let bytes_per_param: u64 = match self.config.strategy {
            InferenceStrategy::FullPrecision => 4, // Q16 = 4 bytes
            InferenceStrategy::Int8Quantized => 1,
            InferenceStrategy::Int4Quantized => 1, // 0.5 bytes, round up
            InferenceStrategy::MixedPrecision => 2, // average
            InferenceStrategy::TinyMode => 1,
        };

        if bytes_per_param > 0 {
            weight_budget / bytes_per_param
        } else {
            0
        }
    }

    /// Record a benchmark result.
    pub fn record_benchmark(&mut self, bench: PerformanceBenchmark) {
        if bench.tokens_per_second > self.best_tps {
            self.best_tps = bench.tokens_per_second;
        }
        self.total_inferences = self.total_inferences.saturating_add(1);
        self.benchmarks.push(bench);

        // Keep only the last 64 benchmarks to bound memory
        if self.benchmarks.len() > 64 {
            let start = self.benchmarks.len() - 64;
            self.benchmarks = self.benchmarks[start..].to_vec();
        }
    }

    /// Get a reference to the current optimization config.
    pub fn get_config(&self) -> &OptimizationConfig {
        &self.config
    }

    /// Check whether the detected hardware supports a given feature.
    pub fn supports_feature(&self, f: CpuFeature) -> bool {
        self.profile.features.iter().any(|feat| *feat == f)
    }

    /// Aggressive optimization for PIC / Raspberry Pi / embedded
    /// targets with very little RAM (< 4 MB).
    pub fn scale_for_embedded(&mut self) {
        self.config.strategy = InferenceStrategy::TinyMode;
        self.config.batch_size = 1;
        self.config.max_context_len = 64;
        self.config.kv_cache_size = 64;
        self.config.num_threads = 1;
        self.config.use_simd = false;
        self.config.tile_size = 8;
        self.config.prefetch_distance = 1;
        self.config.memory_budget_mb = self.profile.available_ram_mb.min(1);

        // If arch is unknown on embedded, mark as Pic
        if self.profile.arch == CpuArch::Unknown {
            self.profile.arch = CpuArch::Pic;
        }
    }

    /// Return summary stats: (best tokens/sec, total inferences, RAM used MB).
    pub fn get_stats(&self) -> (u32, u64, u64) {
        let ram_used = if !self.benchmarks.is_empty() {
            self.benchmarks.last().unwrap().memory_used_mb
        } else {
            0
        };
        (self.best_tps, self.total_inferences, ram_used)
    }

    // --- Internal helpers ---

    fn has_any_simd(&self) -> bool {
        for f in self.profile.features.iter() {
            match f {
                CpuFeature::SSE2
                | CpuFeature::AVX
                | CpuFeature::AVX2
                | CpuFeature::AVX512
                | CpuFeature::Neon
                | CpuFeature::Sve
                | CpuFeature::VectorExt => return true,
                _ => {}
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Utility: integer square root (no floats)
// ---------------------------------------------------------------------------

fn isqrt(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

// ---------------------------------------------------------------------------
// Strategy description (for serial output)
// ---------------------------------------------------------------------------

fn strategy_name(s: InferenceStrategy) -> &'static str {
    match s {
        InferenceStrategy::FullPrecision => "full-precision (Q16)",
        InferenceStrategy::Int8Quantized => "INT8 quantized",
        InferenceStrategy::Int4Quantized => "INT4 quantized",
        InferenceStrategy::MixedPrecision => "mixed precision (INT8+Q16)",
        InferenceStrategy::TinyMode => "tiny mode (embedded)",
    }
}

fn arch_name(a: CpuArch) -> &'static str {
    match a {
        CpuArch::X86_64 => "x86_64",
        CpuArch::Aarch64 => "AArch64",
        CpuArch::RiscV => "RISC-V",
        CpuArch::Pic => "PIC/embedded",
        CpuArch::Unknown => "unknown",
    }
}

// ---------------------------------------------------------------------------
// Public init
// ---------------------------------------------------------------------------

pub fn init() {
    let mut opt = HwOptimizer::new();
    opt.detect_hardware();

    let strategy = opt.recommend_strategy();
    opt.config.strategy = strategy;

    let arch = arch_name(opt.profile.arch);
    let strat = strategy_name(strategy);
    let cores = opt.profile.core_count;
    let threads = opt.profile.thread_count;
    let ram = opt.profile.available_ram_mb;
    let l1 = opt.profile.cache_l1;
    let l2 = opt.profile.cache_l2;
    let l3 = opt.profile.cache_l3;
    let tile = opt.config.tile_size;
    let simd = opt.config.use_simd;
    let max_params = opt.get_optimal_model_size();

    serial_println!(
        "  [hw_optimize] arch={}, cores={}, threads={}, RAM={}MB",
        arch,
        cores,
        threads,
        ram
    );
    serial_println!("  [hw_optimize] cache: L1={}KB L2={}KB L3={}KB", l1, l2, l3);
    serial_println!(
        "  [hw_optimize] strategy={}, tile={}, simd={}",
        strat,
        tile,
        simd
    );
    serial_println!("  [hw_optimize] max model size: {} params", max_params);

    let feature_count = opt.profile.features.len();
    serial_println!("  [hw_optimize] detected {} CPU features", feature_count);

    // Run auto-tune
    opt.auto_tune();
    let batch = opt.config.batch_size;
    let ctx = opt.config.max_context_len;
    serial_println!(
        "  [hw_optimize] auto-tuned: batch={}, ctx_len={}, threads={}",
        batch,
        ctx,
        opt.config.num_threads
    );

    let mut guard = HW_OPTIMIZER.lock();
    *guard = Some(opt);
}
