pub mod benchmark;
pub mod fs_test;
pub mod fuzz;
pub mod memory_test;
pub mod net_test;
pub mod process_test;
/// Kernel self-test framework
///
/// Part of the AIOS. Provides test runner, benchmarking,
/// fuzz testing, stress testing, and subsystem-specific test suites.
///
/// New test suites (added):
///   ipc_test       — pipe write/read, wrap-around, named pipes, semaphores
///   crypto_test    — ChaCha20 (RFC 8439), SHA-256 (FIPS 180-4), HMAC, AEAD
///   scheduler_test — CFS run-queue ordering, vruntime accounting, nice table
///   security_test  — POSIX capabilities, seccomp strict/filter/wildcard
///   net_ext_test   — IPv4 internet checksum, TCP sequence wrap, TTL decrement
pub mod runner;
pub mod stress;

// New subsystem test suites
pub mod crypto_test;
pub mod ipc_test;
pub mod net_ext_test;
pub mod scheduler_test;
pub mod security_test;

pub fn init() {
    runner::init();
    benchmark::init();
    fuzz::init();
    stress::init();
    crate::serial_println!(
        "  Test framework initialized (runner, benchmark, fuzz, stress, +5 new suites)"
    );
}

/// Run all kernel self-tests.
///
/// Invokes every test suite in a logical dependency order:
///   1. Memory (allocator fundamentals — everything else depends on heap)
///   2. Crypto  (pure algorithms — no hardware needed)
///   3. IPC     (pipe + semaphore — process-level primitives)
///   4. Process (PCB lifecycle, round-robin context switch, IPC messages)
///   5. Scheduler (CFS run-queue, vruntime, nice table)
///   6. Security (capabilities, seccomp)
///   7. Net     (simulated socket states)
///   8. Net-ext (IP checksum, TCP sequence space, TTL)
///   9. FS      (in-memory filesystem create/read/write/dir)
///
/// Designed to be called at boot under `#[cfg(feature = "kernel_tests")]`
/// or from the hoags-init shell command `selftest`.
///
/// All output goes to the serial console (COM1) via serial_println!.
pub fn run_kernel_tests() {
    crate::serial_println!("\n");
    crate::serial_println!("  ╔══════════════════════════════════════════╗");
    crate::serial_println!("  ║   Genesis AIOS — Kernel Self-Test Suite  ║");
    crate::serial_println!("  ╚══════════════════════════════════════════╝");
    crate::serial_println!("\n");

    // 1. Memory
    memory_test::run_all();

    // 2. Crypto (pure; no hardware required)
    crypto_test::run_all();

    // 3. IPC — pipe and semaphore
    ipc_test::run_all();

    // 4. Process — lifecycle, round-robin, IPC messages
    process_test::run_all();

    // 5. Scheduler — CFS, nice-to-weight
    scheduler_test::run_all();

    // 6. Security — capabilities, seccomp
    security_test::run_all();

    // 7. Networking — simulated socket state machine
    net_test::run_all();

    // 8. Networking extended — IP checksum, TCP sequence space
    net_ext_test::run_all();

    // 9. Filesystem — in-memory create/read/write/dir
    fs_test::run_all();

    crate::serial_println!("\n  [test-framework] All suites complete.\n");
}
