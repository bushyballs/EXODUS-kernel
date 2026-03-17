use crate::security::caps;
use crate::security::caps::CapabilitySet;
use crate::security::seccomp;
use crate::security::seccomp::SeccompAction;
/// Security subsystem tests
///
/// Part of the AIOS. Tests Linux-compatible POSIX capability sets
/// and seccomp syscall filtering.
///
/// No std, no float, no panics.
use crate::test_framework::runner::TestResult;

// ---------------------------------------------------------------------------
// Local assertion helpers
// ---------------------------------------------------------------------------

macro_rules! req {
    ($cond:expr, $msg:expr) => {
        if !$cond {
            crate::serial_println!("    [sec-test] ASSERT FAILED: {}", $msg);
            return TestResult::Failed;
        }
    };
}

// ---------------------------------------------------------------------------
// Capability tests
// ---------------------------------------------------------------------------

/// Root capability set must have all bits set in permitted and effective.
pub fn test_cap_root_has_all() -> TestResult {
    crate::serial_println!("    [sec-test] running test_cap_root_has_all...");

    let root = CapabilitySet::root();

    req!(
        root.effective == caps::CAP_ALL_VALID,
        "root effective == ALL_VALID"
    );
    req!(
        root.permitted == caps::CAP_ALL_VALID,
        "root permitted == ALL_VALID"
    );
    req!(
        root.bounding == caps::CAP_ALL_VALID,
        "root bounding == ALL_VALID"
    );

    req!(
        root.check(caps::CAP_SYS_ADMIN).is_ok(),
        "root has CAP_SYS_ADMIN"
    );
    req!(
        root.check(caps::CAP_NET_ADMIN).is_ok(),
        "root has CAP_NET_ADMIN"
    );
    req!(root.check(caps::CAP_CHOWN).is_ok(), "root has CAP_CHOWN");
    req!(root.check(caps::CAP_SETUID).is_ok(), "root has CAP_SETUID");
    req!(
        root.check(caps::CAP_SYS_BOOT).is_ok(),
        "root has CAP_SYS_BOOT"
    );

    crate::serial_println!("    [sec-test] PASS: test_cap_root_has_all");
    TestResult::Passed
}

/// Empty capability set must have zero effective bits.
pub fn test_cap_empty_has_none() -> TestResult {
    crate::serial_println!("    [sec-test] running test_cap_empty_has_none...");

    let empty = CapabilitySet::empty();

    req!(empty.effective == 0, "empty effective == 0");
    req!(empty.permitted == 0, "empty permitted == 0");
    req!(empty.inheritable == 0, "empty inheritable == 0");
    req!(empty.ambient == 0, "empty ambient == 0");
    req!(
        empty.bounding == caps::CAP_ALL_VALID,
        "empty bounding == ALL_VALID"
    );

    req!(
        empty.check(caps::CAP_SYS_ADMIN).is_err(),
        "empty lacks CAP_SYS_ADMIN"
    );
    req!(
        empty.check(caps::CAP_CHOWN).is_err(),
        "empty lacks CAP_CHOWN"
    );
    req!(
        empty.check(caps::CAP_NET_RAW).is_err(),
        "empty lacks CAP_NET_RAW"
    );

    crate::serial_println!("    [sec-test] PASS: test_cap_empty_has_none");
    TestResult::Passed
}

/// drop_cap removes a capability from all sets (except bounding).
pub fn test_cap_drop() -> TestResult {
    crate::serial_println!("    [sec-test] running test_cap_drop...");

    let mut cs = CapabilitySet::root();
    req!(
        cs.has(caps::CAP_NET_ADMIN),
        "before drop: has CAP_NET_ADMIN"
    );

    cs.drop_cap(caps::CAP_NET_ADMIN);

    req!(!cs.has(caps::CAP_NET_ADMIN), "after drop: effective clear");
    req!(
        cs.permitted & caps::CAP_NET_ADMIN == 0,
        "after drop: permitted clear"
    );
    req!(
        cs.inheritable & caps::CAP_NET_ADMIN == 0,
        "after drop: inheritable clear"
    );
    req!(
        cs.ambient & caps::CAP_NET_ADMIN == 0,
        "after drop: ambient clear"
    );
    req!(
        cs.check(caps::CAP_NET_ADMIN).is_err(),
        "check after drop returns Err"
    );
    req!(cs.has(caps::CAP_SYS_ADMIN), "unrelated cap still present");

    crate::serial_println!("    [sec-test] PASS: test_cap_drop");
    TestResult::Passed
}

/// raise_effective and lower_effective manipulate only the effective set.
pub fn test_cap_raise_lower_effective() -> TestResult {
    crate::serial_println!("    [sec-test] running test_cap_raise_lower_effective...");

    let mut cs = CapabilitySet::root();

    cs.lower_effective(caps::CAP_SYS_ADMIN);
    req!(!cs.has(caps::CAP_SYS_ADMIN), "effective clear after lower");
    req!(
        cs.permitted & caps::CAP_SYS_ADMIN != 0,
        "permitted unchanged after lower"
    );

    req!(
        cs.raise_effective(caps::CAP_SYS_ADMIN).is_ok(),
        "raise_effective succeeds"
    );
    req!(cs.has(caps::CAP_SYS_ADMIN), "effective set after raise");

    let mut empty = CapabilitySet::empty();
    req!(
        empty.raise_effective(caps::CAP_SYS_ADMIN).is_err(),
        "raise without permitted fails"
    );

    crate::serial_println!("    [sec-test] PASS: test_cap_raise_lower_effective");
    TestResult::Passed
}

/// drop_bounding caps the privilege ceiling without touching effective.
pub fn test_cap_drop_bounding() -> TestResult {
    crate::serial_println!("    [sec-test] running test_cap_drop_bounding...");

    let mut cs = CapabilitySet::root();
    req!(
        cs.bounding & caps::CAP_SYS_MODULE != 0,
        "bounding has CAP_SYS_MODULE initially"
    );

    cs.drop_bounding(caps::CAP_SYS_MODULE);
    req!(
        cs.bounding & caps::CAP_SYS_MODULE == 0,
        "bounding clear after drop_bounding"
    );
    req!(
        cs.has(caps::CAP_SYS_MODULE),
        "effective still has cap after bounding drop"
    );

    crate::serial_println!("    [sec-test] PASS: test_cap_drop_bounding");
    TestResult::Passed
}

/// on_exec with no file caps and no ambient: new effective must be 0.
pub fn test_cap_on_exec_no_file_caps() -> TestResult {
    crate::serial_println!("    [sec-test] running test_cap_on_exec_no_file_caps...");

    let proc_caps = CapabilitySet {
        permitted: caps::CAP_NET_BIND_SERVICE,
        effective: caps::CAP_NET_BIND_SERVICE,
        inheritable: 0,
        ambient: 0,
        bounding: caps::CAP_ALL_VALID,
    };
    let file_caps = CapabilitySet::empty();
    let after = proc_caps.on_exec(&file_caps);

    req!(
        after.effective == 0,
        "effective 0 (no file eff, no ambient)"
    );
    req!(
        after.permitted == 0,
        "permitted 0 (no inh intersect, no ambient)"
    );
    req!(after.inheritable == 0, "inheritable unchanged");
    req!(after.bounding == caps::CAP_ALL_VALID, "bounding unchanged");

    crate::serial_println!("    [sec-test] PASS: test_cap_on_exec_no_file_caps");
    TestResult::Passed
}

/// Global POSIX cap table: set, get, check, and remove round-trip.
pub fn test_cap_global_table() -> TestResult {
    crate::serial_println!("    [sec-test] running test_cap_global_table...");

    let pid: u32 = 9999;
    let mut cs = CapabilitySet::empty();
    cs.permitted = caps::CAP_NET_BIND_SERVICE;
    cs.effective = caps::CAP_NET_BIND_SERVICE;

    caps::set_linux_caps(pid, cs);

    req!(
        caps::process_has_cap(pid, caps::CAP_NET_BIND_SERVICE),
        "has installed cap"
    );
    req!(
        !caps::process_has_cap(pid, caps::CAP_SYS_ADMIN),
        "lacks uninstalled cap"
    );

    let retrieved = caps::get_linux_caps(pid);
    req!(
        retrieved.effective == caps::CAP_NET_BIND_SERVICE,
        "retrieved effective correct"
    );

    let unknown = caps::get_linux_caps(77777);
    req!(unknown.effective == 0, "unknown PID returns empty set");

    caps::remove_linux_caps(pid);
    req!(
        !caps::process_has_cap(pid, caps::CAP_NET_BIND_SERVICE),
        "cap gone after remove"
    );

    crate::serial_println!("    [sec-test] PASS: test_cap_global_table");
    TestResult::Passed
}

// ---------------------------------------------------------------------------
// Seccomp tests
//
// The seccomp module exposes: seccomp_set_profile, seccomp_add_rule,
// seccomp_check, seccomp_remove_profile.
// SeccompAction variants: Allow, KillThread, KillProcess, Trap, Log, Errno.
// ---------------------------------------------------------------------------

/// Set up a profile that allows read(0), write(1) and denies everything else.
/// Verify that allowed syscalls return Allow and others return the default.
pub fn test_seccomp_allow_deny() -> TestResult {
    crate::serial_println!("    [sec-test] running test_seccomp_allow_deny...");

    let pid: u32 = 50000;

    // Create a profile with default action = KillProcess
    seccomp::seccomp_set_profile(pid, SeccompAction::KillProcess);

    // Allow read(0) and write(1) explicitly
    seccomp::seccomp_add_rule(pid, 0, SeccompAction::Allow, 0);
    seccomp::seccomp_add_rule(pid, 1, SeccompAction::Allow, 0);

    // Check allowed syscalls
    let r0 = seccomp::seccomp_check(pid, 0);
    req!(matches!(r0, SeccompAction::Allow), "read(0) allowed");

    let r1 = seccomp::seccomp_check(pid, 1);
    req!(matches!(r1, SeccompAction::Allow), "write(1) allowed");

    // Check denied syscall (falls through to default = KillProcess)
    let r2 = seccomp::seccomp_check(pid, 2);
    req!(matches!(r2, SeccompAction::KillProcess), "open(2) killed");

    let r56 = seccomp::seccomp_check(pid, 56);
    req!(
        matches!(r56, SeccompAction::KillProcess),
        "clone(56) killed"
    );

    // Clean up
    seccomp::seccomp_remove_profile(pid);

    crate::serial_println!("    [sec-test] PASS: test_seccomp_allow_deny");
    TestResult::Passed
}

/// Default action Errno: unmatched syscalls return Errno action.
pub fn test_seccomp_errno_default() -> TestResult {
    crate::serial_println!("    [sec-test] running test_seccomp_errno_default...");

    let pid: u32 = 50001;

    // Create profile with default Errno action
    seccomp::seccomp_set_profile(pid, SeccompAction::Errno);

    // Allow read and write
    seccomp::seccomp_add_rule(pid, 0, SeccompAction::Allow, 0);
    seccomp::seccomp_add_rule(pid, 1, SeccompAction::Allow, 0);

    let r_read = seccomp::seccomp_check(pid, 0);
    req!(matches!(r_read, SeccompAction::Allow), "read allowed");

    let r_write = seccomp::seccomp_check(pid, 1);
    req!(matches!(r_write, SeccompAction::Allow), "write allowed");

    // Unmatched syscall -> Errno
    let r_other = seccomp::seccomp_check(pid, 200);
    req!(
        matches!(r_other, SeccompAction::Errno),
        "unmatched -> Errno"
    );

    seccomp::seccomp_remove_profile(pid);

    crate::serial_println!("    [sec-test] PASS: test_seccomp_errno_default");
    TestResult::Passed
}

/// No profile installed: all syscalls should be allowed.
pub fn test_seccomp_no_profile() -> TestResult {
    crate::serial_println!("    [sec-test] running test_seccomp_no_profile...");

    // Use a PID that has no profile installed
    let pid: u32 = 50002;
    // Make sure it is clean
    seccomp::seccomp_remove_profile(pid);

    for nr in [0u64, 1, 2, 56, 101, 200] {
        let action = seccomp::seccomp_check(pid, nr);
        req!(
            matches!(action, SeccompAction::Allow),
            "no profile allows all"
        );
    }

    crate::serial_println!("    [sec-test] PASS: test_seccomp_no_profile");
    TestResult::Passed
}

/// Profile removal: after removing a profile, all syscalls should be allowed again.
pub fn test_seccomp_remove_profile() -> TestResult {
    crate::serial_println!("    [sec-test] running test_seccomp_remove_profile...");

    let pid: u32 = 50003;

    // Install restrictive profile
    seccomp::seccomp_set_profile(pid, SeccompAction::KillProcess);

    // Verify a syscall is killed
    let before = seccomp::seccomp_check(pid, 99);
    req!(
        matches!(before, SeccompAction::KillProcess),
        "before removal: killed"
    );

    // Remove profile
    let removed = seccomp::seccomp_remove_profile(pid);
    req!(removed, "remove_profile returned true");

    // Now the same syscall should be allowed
    let after = seccomp::seccomp_check(pid, 99);
    req!(
        matches!(after, SeccompAction::Allow),
        "after removal: allowed"
    );

    crate::serial_println!("    [sec-test] PASS: test_seccomp_remove_profile");
    TestResult::Passed
}

/// Multiple rules: specific rules override the default action.
pub fn test_seccomp_multiple_rules() -> TestResult {
    crate::serial_println!("    [sec-test] running test_seccomp_multiple_rules...");

    let pid: u32 = 50004;

    seccomp::seccomp_set_profile(pid, SeccompAction::Allow);

    // Add rules for specific syscalls
    seccomp::seccomp_add_rule(pid, 2, SeccompAction::KillProcess, 0); // open -> kill
    seccomp::seccomp_add_rule(pid, 9, SeccompAction::Allow, 0); // mmap -> allow
    seccomp::seccomp_add_rule(pid, 12, SeccompAction::KillThread, 0); // brk -> kill thread

    req!(
        matches!(seccomp::seccomp_check(pid, 2), SeccompAction::KillProcess),
        "open killed"
    );
    req!(
        matches!(seccomp::seccomp_check(pid, 9), SeccompAction::Allow),
        "mmap allowed"
    );
    req!(
        matches!(seccomp::seccomp_check(pid, 12), SeccompAction::KillThread),
        "brk kill thread"
    );
    req!(
        matches!(seccomp::seccomp_check(pid, 99), SeccompAction::Allow),
        "unmatched -> default Allow"
    );

    seccomp::seccomp_remove_profile(pid);

    crate::serial_println!("    [sec-test] PASS: test_seccomp_multiple_rules");
    TestResult::Passed
}

/// Seccomp log violation: just call it to verify it does not crash.
pub fn test_seccomp_log_violation() -> TestResult {
    crate::serial_println!("    [sec-test] running test_seccomp_log_violation...");

    // This should not panic or crash
    seccomp::seccomp_log_violation(12345, 999);

    crate::serial_println!("    [sec-test] PASS: test_seccomp_log_violation");
    TestResult::Passed
}

// ---------------------------------------------------------------------------
// run_all
// ---------------------------------------------------------------------------

pub fn run_all() {
    crate::serial_println!("    [sec-test] ==============================");
    crate::serial_println!("    [sec-test] Running security test suite");
    crate::serial_println!("    [sec-test] ==============================");

    let mut passed = 0u32;
    let mut failed = 0u32;

    macro_rules! run {
        ($f:expr, $name:literal) => {
            match $f() {
                TestResult::Passed => {
                    passed += 1;
                    crate::serial_println!("    [sec-test] [PASS] {}", $name);
                }
                TestResult::Skipped => {
                    crate::serial_println!("    [sec-test] [SKIP] {}", $name);
                }
                TestResult::Failed => {
                    failed += 1;
                    crate::serial_println!("    [sec-test] [FAIL] {}", $name);
                }
            }
        };
    }

    run!(test_cap_root_has_all, "cap_root_has_all");
    run!(test_cap_empty_has_none, "cap_empty_has_none");
    run!(test_cap_drop, "cap_drop");
    run!(test_cap_raise_lower_effective, "cap_raise_lower_effective");
    run!(test_cap_drop_bounding, "cap_drop_bounding");
    run!(test_cap_on_exec_no_file_caps, "cap_on_exec_no_file_caps");
    run!(test_cap_global_table, "cap_global_table");

    run!(test_seccomp_allow_deny, "seccomp_allow_deny");
    run!(test_seccomp_errno_default, "seccomp_errno_default");
    run!(test_seccomp_no_profile, "seccomp_no_profile");
    run!(test_seccomp_remove_profile, "seccomp_remove_profile");
    run!(test_seccomp_multiple_rules, "seccomp_multiple_rules");
    run!(test_seccomp_log_violation, "seccomp_log_violation");

    crate::serial_println!(
        "    [sec-test] Results: {} passed, {} failed",
        passed,
        failed
    );
}
