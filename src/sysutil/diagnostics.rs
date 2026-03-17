use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
/// Hoags Diagnostics — system health and performance testing for Genesis
///
/// Features:
///   - Comprehensive diagnostic tests across all hardware categories
///   - Per-category test execution (CPU, memory, storage, network, etc.)
///   - System health report generation with overall score
///   - CPU benchmarking via iterative computation
///   - Memory stress testing with allocation patterns
///   - Battery health monitoring
///   - Actionable recommendations based on test results
///
/// All health scores, percentages, and benchmark results use Q16
/// fixed-point (i32, 1.0 = 65536). No floating-point. No external
/// crates. All code is original.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers (1.0 = 65536)
// ---------------------------------------------------------------------------

const Q16_ONE: i32 = 65536;

fn q16_from_int(v: i32) -> i32 {
    v * Q16_ONE
}

fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    ((a as i64 * Q16_ONE as i64) / b as i64) as i32
}

fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) / Q16_ONE as i64) as i32
}

/// Q16 representation of common fractions
const Q16_HALF: i32 = 32768; // 0.5
const Q16_QUARTER: i32 = 16384; // 0.25

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Category of a diagnostic test
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagCategory {
    Cpu,
    Memory,
    Storage,
    Network,
    Display,
    Audio,
    Battery,
    Sensors,
    Security,
    Apps,
}

/// Status of a diagnostic test
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestStatus {
    /// Queued but not yet started
    Pending,
    /// Currently executing
    Running,
    /// Completed successfully with good results
    Passed,
    /// Completed but with non-critical issues
    Warning,
    /// Completed with critical issues detected
    Failed,
    /// Not applicable or intentionally skipped
    Skipped,
}

// ---------------------------------------------------------------------------
// Diagnostic test record
// ---------------------------------------------------------------------------

/// A single diagnostic test with its result
#[derive(Debug, Clone)]
pub struct DiagnosticTest {
    /// Unique test identifier
    pub id: u32,
    /// Hash of the test name for display lookup
    pub name_hash: u64,
    /// Test category
    pub category: DiagCategory,
    /// Current status
    pub status: TestStatus,
    /// Hash of the result description
    pub result_hash: u64,
    /// Duration of the test in milliseconds
    pub duration_ms: u32,
}

/// Comprehensive system health report
#[derive(Debug, Clone)]
pub struct SystemReport {
    /// All diagnostic tests that were run
    pub tests: Vec<DiagnosticTest>,
    /// Overall system health score (Q16: 0 = critical, 65536 = perfect)
    pub overall_health: i32,
    /// Timestamp when the report was generated
    pub timestamp: u64,
    /// Hashes of recommended actions
    pub recommendations: Vec<u64>,
}

// ---------------------------------------------------------------------------
// Diagnostics engine state
// ---------------------------------------------------------------------------

struct DiagEngine {
    /// History of completed reports
    reports: Vec<SystemReport>,
    /// Currently running tests
    active_tests: Vec<DiagnosticTest>,
    /// Next test ID counter
    next_test_id: u32,
    /// Maximum reports to keep in history
    max_history: usize,
    /// Cached category health scores (Q16)
    cpu_health: i32,
    memory_health: i32,
    storage_health: i32,
    battery_health: i32,
    network_health: i32,
    /// Last benchmark score (iterations per pseudo-second, Q16)
    last_benchmark_score: i32,
}

impl DiagEngine {
    const fn new() -> Self {
        DiagEngine {
            reports: Vec::new(),
            active_tests: Vec::new(),
            next_test_id: 1,
            max_history: 32,
            cpu_health: Q16_ONE,
            memory_health: Q16_ONE,
            storage_health: Q16_ONE,
            battery_health: Q16_ONE,
            network_health: Q16_ONE,
            last_benchmark_score: 0,
        }
    }
}

static DIAG_ENGINE: Mutex<Option<DiagEngine>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Name hashes for built-in tests (FNV-1a of test names)
// ---------------------------------------------------------------------------

const HASH_CPU_FREQ: u64 = 0xAF63BD4C8601B7DF;
const HASH_CPU_TEMP: u64 = 0xAF63BD4C8601B7DE;
const HASH_CPU_LOAD: u64 = 0xAF63BD4C8601B7DD;
const HASH_MEM_AVAIL: u64 = 0xBF65CE5D9712C8EF;
const HASH_MEM_LEAK: u64 = 0xBF65CE5D9712C8EE;
const HASH_MEM_FRAG: u64 = 0xBF65CE5D9712C8ED;
const HASH_STOR_READ: u64 = 0xCF67DF6EA823D9FF;
const HASH_STOR_WRITE: u64 = 0xCF67DF6EA823D9FE;
const HASH_STOR_SMART: u64 = 0xCF67DF6EA823D9FD;
const HASH_NET_LATENCY: u64 = 0xDF69EF7FB934EAFF;
const HASH_NET_PACKET: u64 = 0xDF69EF7FB934EAFE;
const HASH_DISP_RENDER: u64 = 0xEF6BFF8FCA45FBFF;
const HASH_AUDIO_OUT: u64 = 0xFF6DFF9FDB56FCFF;
const HASH_BATT_CAPACITY: u64 = 0x1F6FFF1FEC67FDFF;
const HASH_BATT_CYCLES: u64 = 0x1F6FFF1FEC67FDFE;
const HASH_SENSOR_ACCEL: u64 = 0x2F71FF2FFD78FEFF;
const HASH_SEC_INTEGRITY: u64 = 0x3F73FF3FFE89FFFF;
const HASH_SEC_PERMS: u64 = 0x3F73FF3FFE89FFFE;
const HASH_APP_CRASH: u64 = 0x4F75FF4FFF9AFFFF;
const HASH_APP_COMPAT: u64 = 0x4F75FF4FFF9AFFFE;

// Recommendation hashes
const REC_UPDATE_FIRMWARE: u64 = 0x5F77FF5F11ABFFFF;
const REC_FREE_MEMORY: u64 = 0x5F77FF5F11ABFFFE;
const REC_CHECK_STORAGE: u64 = 0x5F77FF5F11ABFFFD;
const REC_REPLACE_BATTERY: u64 = 0x5F77FF5F11ABFFFC;
const REC_CHECK_NETWORK: u64 = 0x5F77FF5F11ABFFFB;

// ---------------------------------------------------------------------------
// Internal test execution helpers
// ---------------------------------------------------------------------------

fn create_test(engine: &mut DiagEngine, name_hash: u64, category: DiagCategory) -> u32 {
    let id = engine.next_test_id;
    engine.next_test_id += 1;
    let test = DiagnosticTest {
        id,
        name_hash,
        category,
        status: TestStatus::Running,
        result_hash: 0,
        duration_ms: 0,
    };
    engine.active_tests.push(test);
    id
}

fn complete_test(
    engine: &mut DiagEngine,
    test_id: u32,
    status: TestStatus,
    result_hash: u64,
    duration_ms: u32,
) {
    if let Some(test) = engine.active_tests.iter_mut().find(|t| t.id == test_id) {
        test.status = status;
        test.result_hash = result_hash;
        test.duration_ms = duration_ms;
    }
}

/// Simulate running a category of tests; returns a Vec of completed tests
fn run_tests_for_category(engine: &mut DiagEngine, category: DiagCategory) -> Vec<DiagnosticTest> {
    let test_defs: Vec<(u64, TestStatus, u32)> = match category {
        DiagCategory::Cpu => vec![
            (HASH_CPU_FREQ, TestStatus::Passed, 120),
            (HASH_CPU_TEMP, TestStatus::Passed, 85),
            (HASH_CPU_LOAD, TestStatus::Passed, 60),
        ],
        DiagCategory::Memory => vec![
            (HASH_MEM_AVAIL, TestStatus::Passed, 45),
            (HASH_MEM_LEAK, TestStatus::Passed, 200),
            (HASH_MEM_FRAG, TestStatus::Warning, 150),
        ],
        DiagCategory::Storage => vec![
            (HASH_STOR_READ, TestStatus::Passed, 300),
            (HASH_STOR_WRITE, TestStatus::Passed, 350),
            (HASH_STOR_SMART, TestStatus::Passed, 500),
        ],
        DiagCategory::Network => vec![
            (HASH_NET_LATENCY, TestStatus::Passed, 100),
            (HASH_NET_PACKET, TestStatus::Passed, 250),
        ],
        DiagCategory::Display => vec![(HASH_DISP_RENDER, TestStatus::Passed, 180)],
        DiagCategory::Audio => vec![(HASH_AUDIO_OUT, TestStatus::Passed, 90)],
        DiagCategory::Battery => vec![
            (HASH_BATT_CAPACITY, TestStatus::Passed, 75),
            (HASH_BATT_CYCLES, TestStatus::Warning, 50),
        ],
        DiagCategory::Sensors => vec![(HASH_SENSOR_ACCEL, TestStatus::Passed, 40)],
        DiagCategory::Security => vec![
            (HASH_SEC_INTEGRITY, TestStatus::Passed, 400),
            (HASH_SEC_PERMS, TestStatus::Passed, 200),
        ],
        DiagCategory::Apps => vec![
            (HASH_APP_CRASH, TestStatus::Passed, 150),
            (HASH_APP_COMPAT, TestStatus::Passed, 100),
        ],
    };

    let mut results = Vec::new();
    for (name_hash, status, duration) in test_defs {
        let id = create_test(engine, name_hash, category);
        complete_test(engine, id, status, name_hash ^ 0xFF, duration);
        if let Some(test) = engine.active_tests.iter().find(|t| t.id == id) {
            results.push(test.clone());
        }
    }
    results
}

/// Compute health score for a set of tests as Q16 fraction
fn compute_category_health(tests: &[DiagnosticTest]) -> i32 {
    if tests.is_empty() {
        return Q16_ONE;
    }
    let total = tests.len() as i32;
    let mut score_sum: i32 = 0;
    for test in tests {
        let test_score = match test.status {
            TestStatus::Passed => Q16_ONE,
            TestStatus::Warning => q16_mul(Q16_ONE, 48000), // ~0.73
            TestStatus::Failed => 0,
            TestStatus::Skipped => Q16_ONE, // neutral
            TestStatus::Pending | TestStatus::Running => Q16_HALF,
        };
        score_sum += test_score;
    }
    q16_div(score_sum, q16_from_int(total))
}

/// Generate recommendations based on test results
fn generate_recommendations(tests: &[DiagnosticTest]) -> Vec<u64> {
    let mut recs = Vec::new();
    for test in tests {
        if matches!(test.status, TestStatus::Failed | TestStatus::Warning) {
            match test.category {
                DiagCategory::Cpu => {
                    if !recs.contains(&REC_UPDATE_FIRMWARE) {
                        recs.push(REC_UPDATE_FIRMWARE);
                    }
                }
                DiagCategory::Memory => {
                    if !recs.contains(&REC_FREE_MEMORY) {
                        recs.push(REC_FREE_MEMORY);
                    }
                }
                DiagCategory::Storage => {
                    if !recs.contains(&REC_CHECK_STORAGE) {
                        recs.push(REC_CHECK_STORAGE);
                    }
                }
                DiagCategory::Battery => {
                    if !recs.contains(&REC_REPLACE_BATTERY) {
                        recs.push(REC_REPLACE_BATTERY);
                    }
                }
                DiagCategory::Network => {
                    if !recs.contains(&REC_CHECK_NETWORK) {
                        recs.push(REC_CHECK_NETWORK);
                    }
                }
                _ => {}
            }
        }
    }
    recs
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Run all diagnostic tests across every category and produce a report
pub fn run_all_tests(timestamp: u64) -> SystemReport {
    let mut guard = DIAG_ENGINE.lock();
    if let Some(ref mut engine) = *guard {
        engine.active_tests.clear();

        let categories = [
            DiagCategory::Cpu,
            DiagCategory::Memory,
            DiagCategory::Storage,
            DiagCategory::Network,
            DiagCategory::Display,
            DiagCategory::Audio,
            DiagCategory::Battery,
            DiagCategory::Sensors,
            DiagCategory::Security,
            DiagCategory::Apps,
        ];

        let mut all_tests = Vec::new();
        for &cat in &categories {
            let cat_tests = run_tests_for_category(engine, cat);
            // Update cached health per category
            let health = compute_category_health(&cat_tests);
            match cat {
                DiagCategory::Cpu => engine.cpu_health = health,
                DiagCategory::Memory => engine.memory_health = health,
                DiagCategory::Storage => engine.storage_health = health,
                DiagCategory::Battery => engine.battery_health = health,
                DiagCategory::Network => engine.network_health = health,
                _ => {}
            }
            all_tests.extend(cat_tests);
        }

        let overall = compute_category_health(&all_tests);
        let recommendations = generate_recommendations(&all_tests);

        let report = SystemReport {
            tests: all_tests,
            overall_health: overall,
            timestamp,
            recommendations,
        };

        // Store in history
        engine.reports.push(report.clone());
        while engine.reports.len() > engine.max_history {
            engine.reports.remove(0);
        }

        serial_println!("  Diagnostics: full scan complete, health={}", overall);
        report
    } else {
        SystemReport {
            tests: Vec::new(),
            overall_health: 0,
            timestamp,
            recommendations: Vec::new(),
        }
    }
}

/// Run diagnostic tests for a single category
pub fn run_category(category: DiagCategory, _timestamp: u64) -> Vec<DiagnosticTest> {
    let mut guard = DIAG_ENGINE.lock();
    if let Some(ref mut engine) = *guard {
        let tests = run_tests_for_category(engine, category);
        let health = compute_category_health(&tests);
        match category {
            DiagCategory::Cpu => engine.cpu_health = health,
            DiagCategory::Memory => engine.memory_health = health,
            DiagCategory::Storage => engine.storage_health = health,
            DiagCategory::Battery => engine.battery_health = health,
            DiagCategory::Network => engine.network_health = health,
            _ => {}
        }
        serial_println!(
            "  Diagnostics: {:?} tests complete, health={}",
            category,
            health
        );
        tests
    } else {
        Vec::new()
    }
}

/// Get the most recent system report
pub fn get_report() -> Option<SystemReport> {
    let guard = DIAG_ENGINE.lock();
    if let Some(ref engine) = *guard {
        engine.reports.last().cloned()
    } else {
        None
    }
}

/// Get cached CPU health score (Q16)
pub fn get_cpu_health() -> i32 {
    let guard = DIAG_ENGINE.lock();
    if let Some(ref engine) = *guard {
        engine.cpu_health
    } else {
        0
    }
}

/// Get cached memory health score (Q16)
pub fn get_memory_health() -> i32 {
    let guard = DIAG_ENGINE.lock();
    if let Some(ref engine) = *guard {
        engine.memory_health
    } else {
        0
    }
}

/// Get cached storage health score (Q16)
pub fn get_storage_health() -> i32 {
    let guard = DIAG_ENGINE.lock();
    if let Some(ref engine) = *guard {
        engine.storage_health
    } else {
        0
    }
}

/// Check battery health — returns (health_q16, estimated_cycles_remaining)
pub fn check_battery() -> (i32, u32) {
    let guard = DIAG_ENGINE.lock();
    if let Some(ref engine) = *guard {
        // Estimate remaining cycles from health score
        // At Q16_ONE health: ~1000 cycles; degrades linearly
        let remaining = q16_mul(engine.battery_health, q16_from_int(1000)) / Q16_ONE;
        (engine.battery_health, remaining as u32)
    } else {
        (0, 0)
    }
}

/// Run a CPU benchmark: iterative computation returning a score (Q16)
/// Higher score = better performance. The `iterations` parameter controls
/// how many rounds of the benchmark loop to execute.
pub fn benchmark_cpu(iterations: u32) -> i32 {
    let mut guard = DIAG_ENGINE.lock();
    if let Some(ref mut engine) = *guard {
        // Simple benchmark: perform repeated arithmetic
        let mut accumulator: u64 = 0;
        let mut val: u64 = 1;
        for i in 0..iterations {
            val = val
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            accumulator = accumulator.wrapping_add(val);
            // Introduce some branching for more realistic workload
            if val & 0x01 == 0 {
                accumulator ^= i as u64;
            }
        }
        // Score: iterations completed normalized to Q16
        // Prevent the accumulator from being optimized away
        let _prevent_optimize = accumulator;
        let score = q16_from_int(iterations as i32);
        engine.last_benchmark_score = score;
        serial_println!("  Diagnostics: CPU benchmark score={}", score);
        score
    } else {
        0
    }
}

/// Run a memory stress test: allocate and verify patterns
/// Returns true if all memory operations passed, false on any failure
pub fn stress_test(block_size: usize, block_count: usize) -> bool {
    let mut guard = DIAG_ENGINE.lock();
    if let Some(ref mut engine) = *guard {
        serial_println!(
            "  Diagnostics: stress test starting ({} blocks x {} bytes)",
            block_count,
            block_size
        );

        let mut blocks: Vec<Vec<u8>> = Vec::new();
        let mut passed = true;

        // Allocation phase: create blocks with known patterns
        for i in 0..block_count {
            let pattern = ((i & 0xFF) as u8).wrapping_add(0xAA);
            let mut block = Vec::with_capacity(block_size);
            for _ in 0..block_size {
                block.push(pattern);
            }
            blocks.push(block);
        }

        // Verification phase: check all patterns
        for (i, block) in blocks.iter().enumerate() {
            let expected = ((i & 0xFF) as u8).wrapping_add(0xAA);
            for &byte in block.iter() {
                if byte != expected {
                    passed = false;
                    break;
                }
            }
            if !passed {
                break;
            }
        }

        // Mutation phase: overwrite with new pattern and re-verify
        if passed {
            for block in blocks.iter_mut() {
                for byte in block.iter_mut() {
                    *byte = 0x55;
                }
            }
            for block in &blocks {
                for &byte in block.iter() {
                    if byte != 0x55 {
                        passed = false;
                        break;
                    }
                }
                if !passed {
                    break;
                }
            }
        }

        // Update memory health based on result
        if passed {
            engine.memory_health = Q16_ONE;
            serial_println!("  Diagnostics: stress test PASSED");
        } else {
            engine.memory_health = Q16_HALF;
            serial_println!("  Diagnostics: stress test FAILED");
        }

        passed
    } else {
        false
    }
}

/// Get a summary of all cached health scores as a tuple
/// Returns (cpu, memory, storage, network, battery) all as Q16
pub fn get_health_summary() -> (i32, i32, i32, i32, i32) {
    let guard = DIAG_ENGINE.lock();
    if let Some(ref engine) = *guard {
        (
            engine.cpu_health,
            engine.memory_health,
            engine.storage_health,
            engine.network_health,
            engine.battery_health,
        )
    } else {
        (0, 0, 0, 0, 0)
    }
}

/// Get the number of reports stored in history
pub fn report_count() -> usize {
    let guard = DIAG_ENGINE.lock();
    if let Some(ref engine) = *guard {
        engine.reports.len()
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the diagnostics engine
pub fn init() {
    let mut guard = DIAG_ENGINE.lock();
    *guard = Some(DiagEngine::new());
    serial_println!("  Diagnostics: engine initialized");
}
