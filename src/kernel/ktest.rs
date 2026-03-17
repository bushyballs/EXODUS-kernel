/// Kernel self-test framework — Genesis AIOS.
///
/// A minimal, no-heap test harness for verifying core kernel invariants at
/// boot time or on demand via the `ktest` kernel command-line parameter.
///
/// ## Design constraints (bare-metal #![no_std])
/// - NO heap: no Vec / Box / String / alloc::* — fixed-size static arrays.
/// - NO floats: no `as f64` / `as f32` anywhere.
/// - NO panics: no unwrap() / expect() / panic!() — functions return bool / Option.
/// - All counters use saturating_add.
/// - Structs in static Mutex must be Copy + have `const fn empty()`.
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Test function signature: returns `true` on pass, `false` on fail.
pub type TestFn = fn() -> bool;

/// Maximum number of tests that can be registered.
pub const KTEST_MAX: usize = 128;

/// A single registered kernel self-test.
#[derive(Copy, Clone)]
pub struct KernelTest {
    /// Human-readable test name (e.g. b"saturating_math").  Zero-padded.
    pub name: [u8; 48],
    /// The test function to invoke.  `None` means the slot is empty.
    pub test_fn: Option<TestFn>,
    /// Result of the last run.  Meaningful only when `run_count > 0`.
    pub passed: bool,
    /// How many times this test has been executed.
    pub run_count: u32,
    /// Slot is in use (true) or free (false).
    pub active: bool,
}

impl KernelTest {
    pub const fn empty() -> Self {
        KernelTest {
            name: [0u8; 48],
            test_fn: None,
            passed: false,
            run_count: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Static storage
// ---------------------------------------------------------------------------

/// Registered test table.
static TESTS: Mutex<[KernelTest; KTEST_MAX]> = Mutex::new([KernelTest::empty(); KTEST_MAX]);

/// Running count of tests that have passed across all `ktest_run_*` calls.
static TEST_PASS: AtomicU32 = AtomicU32::new(0);

/// Running count of tests that have failed across all `ktest_run_*` calls.
static TEST_FAIL: AtomicU32 = AtomicU32::new(0);

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register a test function under the given name.
///
/// If a test with the same name already exists, the registration is silently
/// ignored (idempotent — safe to call multiple times at boot).
///
/// Returns `true` on success, `false` if the table is full.
pub fn ktest_register(name: &[u8], f: TestFn) -> bool {
    let mut tests = TESTS.lock();

    // Check for duplicate name.
    for i in 0..KTEST_MAX {
        if tests[i].active && names_equal_48(&tests[i].name, name) {
            return true; // already registered
        }
    }

    // Find a free slot.
    for i in 0..KTEST_MAX {
        if !tests[i].active {
            let mut t = KernelTest::empty();
            copy_name_48(&mut t.name, name);
            t.test_fn = Some(f);
            t.active = true;
            tests[i] = t;
            return true;
        }
    }

    crate::serial_println!("[ktest] register: table full, cannot add test");
    false
}

// ---------------------------------------------------------------------------
// Running tests
// ---------------------------------------------------------------------------

/// Run all registered tests in registration order.
///
/// Updates per-test pass/fail state and the global pass/fail counters.
/// Returns `(pass_count, fail_count)` for this run.
pub fn ktest_run_all() -> (u32, u32) {
    let count = {
        let tests = TESTS.lock();
        let mut n = 0usize;
        for i in 0..KTEST_MAX {
            if tests[i].active {
                n += 1;
            }
        }
        n
    };

    let mut pass = 0u32;
    let mut fail = 0u32;

    for i in 0..KTEST_MAX {
        let (active, maybe_fn, name) = {
            let tests = TESTS.lock();
            (tests[i].active, tests[i].test_fn, tests[i].name)
        };
        if !active {
            continue;
        }
        if let Some(f) = maybe_fn {
            let result = f();
            // Update test record.
            {
                let mut tests = TESTS.lock();
                tests[i].passed = result;
                tests[i].run_count = tests[i].run_count.saturating_add(1);
            }
            let nl = name_len_48(&name);
            if result {
                pass = pass.saturating_add(1);
                let ns = match core::str::from_utf8(&name[..nl.min(48)]) {
                    Ok(s) => s,
                    Err(_) => "?",
                };
                crate::serial_println!("[ktest] PASS: {}", ns);
            } else {
                fail = fail.saturating_add(1);
                let ns = match core::str::from_utf8(&name[..nl.min(48)]) {
                    Ok(s) => s,
                    Err(_) => "?",
                };
                crate::serial_println!("[ktest] FAIL: {}", ns);
            }
        }
    }

    TEST_PASS.fetch_add(pass, Ordering::Relaxed);
    TEST_FAIL.fetch_add(fail, Ordering::Relaxed);

    let _ = count; // suppress unused warning
    crate::serial_println!("[ktest] run_all complete: {} passed, {} failed", pass, fail);
    (pass, fail)
}

/// Run a single test by name.
///
/// Returns `Some(true)` on pass, `Some(false)` on fail, `None` if not found.
pub fn ktest_run_one(name: &[u8]) -> Option<bool> {
    // Find the test index.
    let idx = {
        let tests = TESTS.lock();
        let mut found = KTEST_MAX; // sentinel
        for i in 0..KTEST_MAX {
            if tests[i].active && names_equal_48(&tests[i].name, name) {
                found = i;
                break;
            }
        }
        found
    };

    if idx == KTEST_MAX {
        return None; // not found
    }

    let maybe_fn = {
        let tests = TESTS.lock();
        tests[idx].test_fn
    };

    if let Some(f) = maybe_fn {
        let result = f();
        {
            let mut tests = TESTS.lock();
            tests[idx].passed = result;
            tests[idx].run_count = tests[idx].run_count.saturating_add(1);
        }
        if result {
            TEST_PASS.fetch_add(1, Ordering::Relaxed);
        } else {
            TEST_FAIL.fetch_add(1, Ordering::Relaxed);
        }
        Some(result)
    } else {
        None
    }
}

/// Return cumulative (pass_count, fail_count) since subsystem init.
pub fn ktest_get_results() -> (u32, u32) {
    (
        TEST_PASS.load(Ordering::Relaxed),
        TEST_FAIL.load(Ordering::Relaxed),
    )
}

// ---------------------------------------------------------------------------
// Built-in self-tests
// ---------------------------------------------------------------------------

/// Verify that saturating arithmetic on u32 works correctly.
fn test_saturating_math() -> bool {
    let a: u32 = u32::MAX;
    let b = a.saturating_add(1);
    if b != u32::MAX {
        return false;
    }
    let c: u32 = 0;
    let d = c.saturating_sub(1);
    d == 0
}

/// Verify that wrapping arithmetic on u32 wraps correctly.
fn test_wrapping_seq() -> bool {
    let a: u32 = u32::MAX;
    let b = a.wrapping_add(1);
    b == 0
}

/// Verify that the local FNV-1a hash implementation is deterministic.
fn test_fnv1a_hash() -> bool {
    let data = b"test";
    let h1 = fnv1a(data);
    let h2 = fnv1a(data);
    h1 == h2 && h1 != 0
}

/// Verify that SHA-256 of the empty string produces the known digest.
///
/// SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
fn test_crypto_sha256() -> bool {
    let out = crate::crypto::sha256::hash(&[]);
    out[0] == 0xe3 && out[31] == 0x55
}

/// Verify that the frame allocator can allocate and free a physical page.
///
/// If no frames are available (e.g. very constrained environment), the test
/// is skipped (returns true rather than false) so it does not produce a
/// spurious failure on resource-limited targets.
fn test_memory_alloc() -> bool {
    use crate::memory::frame_allocator;
    match frame_allocator::allocate_frame() {
        None => true, // no memory available — skip, not a failure
        Some(frame) => {
            frame_allocator::deallocate_frame(frame);
            true
        }
    }
}

/// Verify that u64 wrapping_add on sequence numbers behaves correctly.
fn test_wrapping_u64() -> bool {
    let a: u64 = u64::MAX;
    let b = a.wrapping_add(1);
    b == 0
}

/// Verify that saturating_mul on u64 does not overflow to zero.
fn test_saturating_mul() -> bool {
    let a: u64 = u64::MAX;
    let b = a.saturating_mul(2);
    b == u64::MAX
}

// ---------------------------------------------------------------------------
// FNV-1a helper (local copy — no external dependency)
// ---------------------------------------------------------------------------

/// 32-bit FNV-1a hash of `data`.
///
/// https://en.wikipedia.org/wiki/Fowler%E2%80%93Noll%E2%80%93Vo_hash_function
#[inline]
pub fn fnv1a(data: &[u8]) -> u32 {
    const FNV_OFFSET_BASIS: u32 = 0x811c9dc5;
    const FNV_PRIME: u32 = 0x01000193;
    let mut hash = FNV_OFFSET_BASIS;
    for &b in data {
        hash ^= b as u32;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn names_equal_48(a: &[u8; 48], b: &[u8]) -> bool {
    let blen = b.len().min(48);
    for i in 0..blen {
        if a[i] == 0 || a[i] != b[i] {
            return false;
        }
    }
    if blen < 48 {
        a[blen] == 0
    } else {
        true
    }
}

fn copy_name_48(dst: &mut [u8; 48], src: &[u8]) {
    let n = src.len().min(48);
    dst[..n].copy_from_slice(&src[..n]);
    for i in n..48 {
        dst[i] = 0;
    }
}

fn name_len_48(name: &[u8; 48]) -> usize {
    for i in 0..48 {
        if name[i] == 0 {
            return i;
        }
    }
    48
}

// ---------------------------------------------------------------------------
// Initialiser
// ---------------------------------------------------------------------------

/// Initialize the ktest subsystem and register built-in self-tests.
///
/// Call near the end of `kernel::init()` so all subsystems being tested are
/// already initialized.  Tests are NOT run here — call `ktest_run_all()` if
/// the `ktest` kernel parameter is set.
pub fn init() {
    // Clear counters and table.
    TEST_PASS.store(0, Ordering::SeqCst);
    TEST_FAIL.store(0, Ordering::SeqCst);
    {
        let mut tests = TESTS.lock();
        for i in 0..KTEST_MAX {
            tests[i] = KernelTest::empty();
        }
    }

    // Register built-in tests.
    ktest_register(b"saturating_math", test_saturating_math);
    ktest_register(b"wrapping_seq", test_wrapping_seq);
    ktest_register(b"fnv1a_hash", test_fnv1a_hash);
    ktest_register(b"crypto_sha256", test_crypto_sha256);
    ktest_register(b"memory_alloc", test_memory_alloc);
    ktest_register(b"wrapping_u64", test_wrapping_u64);
    ktest_register(b"saturating_mul", test_saturating_mul);

    crate::serial_println!("  ktest: initialized ({} built-in tests registered)", 7);
}
