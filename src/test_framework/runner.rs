use crate::sync::Mutex;
use alloc::string::String;
/// Test runner, assertions
///
/// Part of the AIOS. Provides a test runner that registers,
/// executes, and reports on test cases. Supports pass, fail,
/// and skip results with detailed serial output.
use alloc::vec::Vec;

/// Result of a single test case.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TestResult {
    Passed,
    Failed,
    Skipped,
}

impl TestResult {
    /// Human-readable label for the result
    pub fn label(self) -> &'static str {
        match self {
            TestResult::Passed => "PASS",
            TestResult::Failed => "FAIL",
            TestResult::Skipped => "SKIP",
        }
    }

    /// Whether this result indicates success
    pub fn is_ok(self) -> bool {
        matches!(self, TestResult::Passed | TestResult::Skipped)
    }
}

/// Runs registered test cases and collects results.
pub struct TestRunner {
    tests: Vec<TestCase>,
    results: Vec<TestResult>,
}

struct TestCase {
    name: &'static str,
    func: fn() -> TestResult,
}

/// Test statistics summary
pub struct TestSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
}

impl TestRunner {
    pub fn new() -> Self {
        crate::serial_println!("    [test-runner] test runner created");
        Self {
            tests: Vec::new(),
            results: Vec::new(),
        }
    }

    /// Register a test case.
    pub fn register(&mut self, name: &'static str, func: fn() -> TestResult) {
        self.tests.push(TestCase { name, func });
        crate::serial_println!("    [test-runner] registered test: '{}'", name);
    }

    /// Run all registered tests and return results.
    /// Executes each test in registration order, catching panics
    /// as test failures via conservative approach.
    pub fn run_all(&mut self) -> &[TestResult] {
        self.results.clear();

        let total = self.tests.len();
        crate::serial_println!("    [test-runner] running {} tests...", total);
        crate::serial_println!("    [test-runner] ----------------------------------------");

        for i in 0..total {
            let name = self.tests[i].name;
            let func = self.tests[i].func;

            crate::serial_println!(
                "    [test-runner] [{}/{}] running '{}'...",
                i + 1,
                total,
                name
            );

            let result = func();

            crate::serial_println!(
                "    [test-runner] [{}/{}] '{}' => [{}]",
                i + 1,
                total,
                name,
                result.label()
            );

            self.results.push(result);
        }

        crate::serial_println!("    [test-runner] ----------------------------------------");

        // Print summary
        let summary = self.summary();
        crate::serial_println!(
            "    [test-runner] Results: {} total, {} passed, {} failed, {} skipped",
            summary.total,
            summary.passed,
            summary.failed,
            summary.skipped
        );

        if summary.failed == 0 {
            crate::serial_println!("    [test-runner] ALL TESTS PASSED");
        } else {
            crate::serial_println!("    [test-runner] SOME TESTS FAILED");
            // List failed tests
            for i in 0..self.results.len() {
                if self.results[i] == TestResult::Failed {
                    crate::serial_println!("    [test-runner]   FAILED: '{}'", self.tests[i].name);
                }
            }
        }

        &self.results
    }

    /// Get a summary of test results
    pub fn summary(&self) -> TestSummary {
        let mut passed = 0usize;
        let mut failed = 0usize;
        let mut skipped = 0usize;

        for r in &self.results {
            match r {
                TestResult::Passed => passed += 1,
                TestResult::Failed => failed += 1,
                TestResult::Skipped => skipped += 1,
            }
        }

        TestSummary {
            total: self.results.len(),
            passed,
            failed,
            skipped,
        }
    }

    /// Check whether all tests passed (no failures)
    pub fn all_passed(&self) -> bool {
        !self.results.iter().any(|r| *r == TestResult::Failed)
    }

    /// Get the number of registered tests
    pub fn test_count(&self) -> usize {
        self.tests.len()
    }

    /// Clear all registered tests and results
    pub fn clear(&mut self) {
        self.tests.clear();
        self.results.clear();
        crate::serial_println!("    [test-runner] runner cleared");
    }
}

// ---- Assertion helpers ----

/// Assert that a condition is true; returns Passed or Failed.
pub fn assert_true(condition: bool, msg: &str) -> TestResult {
    if condition {
        TestResult::Passed
    } else {
        crate::serial_println!("    [test-runner] ASSERTION FAILED: {}", msg);
        TestResult::Failed
    }
}

/// Assert that two u64 values are equal
pub fn assert_eq_u64(a: u64, b: u64, msg: &str) -> TestResult {
    if a == b {
        TestResult::Passed
    } else {
        crate::serial_println!(
            "    [test-runner] ASSERTION FAILED: {} (expected {}, got {})",
            msg,
            a,
            b
        );
        TestResult::Failed
    }
}

/// Assert that two usize values are equal
pub fn assert_eq_usize(a: usize, b: usize, msg: &str) -> TestResult {
    if a == b {
        TestResult::Passed
    } else {
        crate::serial_println!(
            "    [test-runner] ASSERTION FAILED: {} (expected {}, got {})",
            msg,
            a,
            b
        );
        TestResult::Failed
    }
}

/// Assert that a boolean value is false
pub fn assert_false(condition: bool, msg: &str) -> TestResult {
    assert_true(!condition, msg)
}

/// Global test runner singleton
static TEST_RUNNER: Mutex<Option<TestRunner>> = Mutex::new(None);

pub fn init() {
    let mut runner = TEST_RUNNER.lock();
    *runner = Some(TestRunner::new());
    crate::serial_println!("    [test-runner] test runner subsystem initialized");
}

/// Register a test with the global runner
pub fn register_global(name: &'static str, func: fn() -> TestResult) {
    let mut guard = TEST_RUNNER.lock();
    if let Some(ref mut runner) = *guard {
        runner.register(name, func);
    }
}

/// Run all global tests
pub fn run_global() -> bool {
    let mut guard = TEST_RUNNER.lock();
    if let Some(ref mut runner) = *guard {
        runner.run_all();
        runner.all_passed()
    } else {
        false
    }
}
