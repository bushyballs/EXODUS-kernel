/// Test framework for Genesis — unit tests, integration tests, assertions
///
/// Provides test discovery, execution, result reporting,
/// and built-in kernel self-tests.
///
/// Inspired by: Rust's built-in test framework, Google Test. All code is original.
use crate::sync::Mutex;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Test result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestResult {
    Pass,
    Fail,
    Skip,
    Timeout,
}

/// A single test case
pub struct TestCase {
    pub name: String,
    pub suite: String,
    pub func: fn() -> TestResult,
    pub timeout_ms: u64,
}

/// Test suite
pub struct TestSuite {
    pub name: String,
    pub tests: Vec<TestCase>,
    pub setup: Option<fn()>,
    pub teardown: Option<fn()>,
}

impl TestSuite {
    pub fn new(name: &str) -> Self {
        TestSuite {
            name: String::from(name),
            tests: Vec::new(),
            setup: None,
            teardown: None,
        }
    }

    pub fn add_test(&mut self, name: &str, func: fn() -> TestResult) {
        self.tests.push(TestCase {
            name: String::from(name),
            suite: self.name.clone(),
            func,
            timeout_ms: 5000,
        });
    }
}

/// Test execution summary
pub struct TestSummary {
    pub total: u32,
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub timed_out: u32,
    pub duration_ms: u64,
    pub failures: Vec<String>,
}

/// Test runner
pub struct TestRunner {
    pub suites: Vec<TestSuite>,
    pub verbose: bool,
}

impl TestRunner {
    const fn new() -> Self {
        TestRunner {
            suites: Vec::new(),
            verbose: true,
        }
    }

    pub fn add_suite(&mut self, suite: TestSuite) {
        self.suites.push(suite);
    }

    pub fn run_all(&self) -> TestSummary {
        let mut summary = TestSummary {
            total: 0,
            passed: 0,
            failed: 0,
            skipped: 0,
            timed_out: 0,
            duration_ms: 0,
            failures: Vec::new(),
        };

        let start = crate::time::clock::unix_time();

        for suite in &self.suites {
            if self.verbose {
                crate::serial_println!("  [test] Suite: {}", suite.name);
            }

            if let Some(setup) = suite.setup {
                setup();
            }

            for test in &suite.tests {
                summary.total = summary.total.saturating_add(1);
                let result = (test.func)();
                match result {
                    TestResult::Pass => {
                        summary.passed = summary.passed.saturating_add(1);
                        if self.verbose {
                            crate::serial_println!("    PASS: {}", test.name);
                        }
                    }
                    TestResult::Fail => {
                        summary.failed = summary.failed.saturating_add(1);
                        summary
                            .failures
                            .push(format!("{}::{}", suite.name, test.name));
                        crate::serial_println!("    FAIL: {}", test.name);
                    }
                    TestResult::Skip => {
                        summary.skipped = summary.skipped.saturating_add(1);
                        if self.verbose {
                            crate::serial_println!("    SKIP: {}", test.name);
                        }
                    }
                    TestResult::Timeout => {
                        summary.timed_out = summary.timed_out.saturating_add(1);
                        summary
                            .failures
                            .push(format!("{}::{} (timeout)", suite.name, test.name));
                        crate::serial_println!("    TIMEOUT: {}", test.name);
                    }
                }
            }

            if let Some(teardown) = suite.teardown {
                teardown();
            }
        }

        let end = crate::time::clock::unix_time();
        summary.duration_ms = (end - start) * 1000;
        summary
    }

    pub fn suite_count(&self) -> usize {
        self.suites.len()
    }
}

// === Built-in kernel self-tests ===

fn test_heap_alloc() -> TestResult {
    let v: Vec<u32> = alloc::vec![1, 2, 3, 4, 5];
    if v.len() == 5 && v[0] == 1 && v[4] == 5 {
        TestResult::Pass
    } else {
        TestResult::Fail
    }
}

fn test_string_ops() -> TestResult {
    let mut s = String::from("Hello");
    s.push_str(", Genesis!");
    if s == "Hello, Genesis!" && s.len() == 15 {
        TestResult::Pass
    } else {
        TestResult::Fail
    }
}

fn test_btree_map() -> TestResult {
    use alloc::collections::BTreeMap;
    let mut map = BTreeMap::new();
    map.insert("key1", 100);
    map.insert("key2", 200);
    if map.get("key1") == Some(&100) && map.len() == 2 {
        TestResult::Pass
    } else {
        TestResult::Fail
    }
}

fn test_box_alloc() -> TestResult {
    let b = Box::new(42u64);
    if *b == 42 {
        TestResult::Pass
    } else {
        TestResult::Fail
    }
}

fn test_format_string() -> TestResult {
    let s = format!("Value: {}, Hex: {:x}", 255, 255);
    if s == "Value: 255, Hex: ff" {
        TestResult::Pass
    } else {
        TestResult::Fail
    }
}

pub fn create_kernel_tests() -> TestSuite {
    let mut suite = TestSuite::new("kernel-self-test");
    suite.add_test("heap_alloc", test_heap_alloc);
    suite.add_test("string_ops", test_string_ops);
    suite.add_test("btree_map", test_btree_map);
    suite.add_test("box_alloc", test_box_alloc);
    suite.add_test("format_string", test_format_string);
    suite
}

static RUNNER: Mutex<TestRunner> = Mutex::new(TestRunner::new());

pub fn init() {
    let mut runner = RUNNER.lock();
    runner.add_suite(create_kernel_tests());
    crate::serial_println!(
        "  [testing] Test framework initialized ({} suites)",
        runner.suite_count()
    );
}

pub fn run_all() -> TestSummary {
    RUNNER.lock().run_all()
}
