use crate::process::cfs_simple::{CfsRunQueue, CfsTask};
use crate::process::nice;
/// Scheduler subsystem tests
///
/// Part of the AIOS. Tests CFS run-queue ordering, vruntime accounting,
/// min_vruntime starvation prevention, dequeue-by-PID, and the
/// nice_to_weight lookup table.
///
/// No std, no float, no panics.
use crate::test_framework::runner::TestResult;

// ---------------------------------------------------------------------------
// Local assertion helpers
// ---------------------------------------------------------------------------

macro_rules! req {
    ($cond:expr, $msg:expr) => {
        if !$cond {
            crate::serial_println!("    [sched-test] ASSERT FAILED: {}", $msg);
            return TestResult::Failed;
        }
    };
}

macro_rules! req_eq_u32 {
    ($a:expr, $b:expr, $ctx:expr) => {
        if $a != $b {
            crate::serial_println!(
                "    [sched-test] ASSERT {}: expected {} got {}",
                $ctx,
                $b,
                $a
            );
            return TestResult::Failed;
        }
    };
}

// ---------------------------------------------------------------------------
// CfsTask constructor helper
// ---------------------------------------------------------------------------

/// Construct a CfsTask; delegates to CfsTask::new() to avoid touching
/// private padding fields from outside the process module.
fn make_task(pid: u32, vruntime: u64, weight: u32) -> CfsTask {
    CfsTask::new(pid, vruntime, weight)
}

// ---------------------------------------------------------------------------
// CFS run-queue tests
// ---------------------------------------------------------------------------

/// Three tasks enqueued out of vruntime order must be dequeued in ascending
/// vruntime order (lowest-vruntime first).
pub fn test_cfs_enqueue_dequeue_order() -> TestResult {
    crate::serial_println!("    [sched-test] running test_cfs_enqueue_dequeue_order...");

    let mut rq = CfsRunQueue::new();

    // Insert in reverse order: pid=3 (vruntime=300), pid=1 (100), pid=2 (200)
    rq.enqueue(make_task(3, 300, 1024));
    rq.enqueue(make_task(1, 100, 1024));
    rq.enqueue(make_task(2, 200, 1024));

    req_eq_u32!(rq.len() as u32, 3, "queue length after 3 enqueues");

    // First dequeue should be pid=1 (vruntime=100)
    match rq.dequeue_min() {
        Some(t1) => {
            req_eq_u32!(t1.pid, 1, "first dequeue pid");
        }
        None => {
            req!(false, "first dequeue should return Some");
        }
    }

    // Second: pid=2
    match rq.dequeue_min() {
        Some(t2) => {
            req_eq_u32!(t2.pid, 2, "second dequeue pid");
        }
        None => {
            req!(false, "second dequeue should return Some");
        }
    }

    // Third: pid=3
    match rq.dequeue_min() {
        Some(t3) => {
            req_eq_u32!(t3.pid, 3, "third dequeue pid");
        }
        None => {
            req!(false, "third dequeue should return Some");
        }
    }

    // Queue empty
    req!(
        rq.dequeue_min().is_none(),
        "fourth dequeue should return None"
    );
    req!(rq.is_empty(), "queue should be empty");
    req_eq_u32!(rq.total_weight, 0, "total_weight after full drain");

    crate::serial_println!("    [sched-test] PASS: test_cfs_enqueue_dequeue_order");
    TestResult::Passed
}

/// Newly enqueued task with vruntime below min_vruntime is raised to
/// min_vruntime (starvation prevention).
pub fn test_cfs_min_vruntime_starvation_prevention() -> TestResult {
    crate::serial_println!(
        "    [sched-test] running test_cfs_min_vruntime_starvation_prevention..."
    );

    let mut rq = CfsRunQueue::new();

    // Seed the queue so min_vruntime advances
    rq.enqueue(make_task(10, 1000, 1024));
    rq.enqueue(make_task(11, 2000, 1024));
    rq.dequeue_min(); // removes pid=10, min_vruntime advances to 2000

    // Now enqueue a task with vruntime below min_vruntime
    rq.enqueue(make_task(20, 500, 1024));

    // Its vruntime should have been clamped to min_vruntime (2000)
    // → it should NOT appear at the front (pid=11 still has vruntime=2000 or was there first)
    // The new task (pid=20) got clamped; both have vruntime=2000, so pid order breaks tie.
    // The important invariant: pid=20 does NOT get scheduled before pid=11 who was waiting.

    // With tie on vruntime, the queue sorts by (vruntime, pid): pid=11 < pid=20
    let next_pid = rq.pick_next().unwrap_or(0);
    req_eq_u32!(
        next_pid,
        11,
        "existing task should run before clamped newcomer"
    );

    // Verify task 20's vruntime was raised
    let t11 = rq.dequeue_min().unwrap();
    let t20 = rq.dequeue_min().unwrap();
    req!(
        t20.vruntime >= t11.vruntime,
        "clamped task vruntime >= min_vruntime"
    );

    crate::serial_println!("    [sched-test] PASS: test_cfs_min_vruntime_starvation_prevention");
    TestResult::Passed
}

/// update_vruntime charges CPU time to a task and re-sorts the queue.
pub fn test_cfs_update_vruntime() -> TestResult {
    crate::serial_println!("    [sched-test] running test_cfs_update_vruntime...");

    let mut rq = CfsRunQueue::new();
    // Two tasks, same initial vruntime
    rq.enqueue(make_task(100, 1000, 1024));
    rq.enqueue(make_task(101, 1000, 1024));

    // Both at vruntime=1000 — pid=100 sorts first (lower PID breaks tie)
    req_eq_u32!(
        rq.pick_next().unwrap_or(u32::MAX),
        100,
        "pid=100 runs first (tie-break)"
    );

    // Charge 1024 ns to pid=100 (with weight=1024 → vruntime_delta = 1024*1024/1024 = 1024)
    rq.update_vruntime(100, 1024);

    // After update, pid=100's vruntime = 1000+1024 = 2024 > pid=101's 1000
    // So pid=101 should now be picked
    req_eq_u32!(
        rq.pick_next().unwrap_or(u32::MAX),
        101,
        "pid=101 picked after pid=100 charged"
    );

    crate::serial_println!("    [sched-test] PASS: test_cfs_update_vruntime");
    TestResult::Passed
}

/// dequeue() by PID removes only the target task; total_weight is updated.
pub fn test_cfs_dequeue_by_pid() -> TestResult {
    crate::serial_println!("    [sched-test] running test_cfs_dequeue_by_pid...");

    let mut rq = CfsRunQueue::new();
    rq.enqueue(make_task(5, 100, 512));
    rq.enqueue(make_task(6, 200, 256));
    rq.enqueue(make_task(7, 300, 128));

    let weight_before = rq.total_weight; // 512 + 256 + 128 = 896
    req_eq_u32!(weight_before, 896, "initial total_weight");

    // Remove pid=6 (not the head, not the tail)
    rq.dequeue(6);
    req_eq_u32!(rq.len() as u32, 2, "len after dequeue(6)");
    req_eq_u32!(rq.total_weight, 896 - 256, "total_weight after dequeue(6)");

    // Remaining tasks: pid=5, pid=7 in vruntime order
    let a = rq.dequeue_min().unwrap();
    let b = rq.dequeue_min().unwrap();
    req_eq_u32!(a.pid, 5, "remaining task 1");
    req_eq_u32!(b.pid, 7, "remaining task 2");
    req!(rq.is_empty(), "queue empty after draining");

    crate::serial_println!("    [sched-test] PASS: test_cfs_dequeue_by_pid");
    TestResult::Passed
}

/// Heavy tasks (low weight / high nice) accumulate vruntime faster and
/// are therefore preempted sooner relative to light tasks.
pub fn test_cfs_weight_fairness() -> TestResult {
    crate::serial_println!("    [sched-test] running test_cfs_weight_fairness...");

    let mut rq = CfsRunQueue::new();

    // high-priority (weight=2048) and low-priority (weight=512) tasks
    // both start at vruntime=0
    let w_high: u32 = 2048;
    let w_low: u32 = 512;
    rq.enqueue(make_task(200, 0, w_high));
    rq.enqueue(make_task(201, 0, w_low));

    // Simulate charging the same 1024 ns to both
    // high-weight task: vruntime_delta = 1024 * 1024 / 2048 = 512
    // low-weight task:  vruntime_delta = 1024 * 1024 / 512  = 2048
    rq.update_vruntime(200, 1024);
    rq.update_vruntime(201, 1024);

    // After equal real time, high-weight task should have lower vruntime
    // and therefore still be picked next
    let next = rq.pick_next().unwrap_or(u32::MAX);
    req_eq_u32!(
        next,
        200,
        "high-weight task should have lower vruntime after equal real time"
    );

    crate::serial_println!("    [sched-test] PASS: test_cfs_weight_fairness");
    TestResult::Passed
}

/// Single-task queue: dequeue_min on empty returns None; pick_next on empty returns None.
pub fn test_cfs_empty_queue() -> TestResult {
    crate::serial_println!("    [sched-test] running test_cfs_empty_queue...");

    let mut rq = CfsRunQueue::new();
    req!(rq.is_empty(), "new queue is empty");
    req!(
        rq.dequeue_min().is_none(),
        "dequeue_min on empty returns None"
    );
    req!(rq.pick_next().is_none(), "pick_next on empty returns None");
    req_eq_u32!(rq.total_weight, 0, "total_weight on empty queue");
    req_eq_u32!(rq.len() as u32, 0, "len on empty queue");

    // Single task
    rq.enqueue(make_task(99, 42, 1024));
    req_eq_u32!(rq.len() as u32, 1, "len after one enqueue");
    req_eq_u32!(rq.pick_next().unwrap_or(0), 99, "pick_next returns pid=99");
    rq.dequeue_min();
    req!(rq.is_empty(), "queue empty after single dequeue");

    crate::serial_println!("    [sched-test] PASS: test_cfs_empty_queue");
    TestResult::Passed
}

// ---------------------------------------------------------------------------
// nice_to_weight tests
// ---------------------------------------------------------------------------

/// nice(0) must return 1024 (NICE_0_WEIGHT — the Linux standard baseline).
pub fn test_nice_to_weight_nice0() -> TestResult {
    crate::serial_println!("    [sched-test] running test_nice_to_weight_nice0...");

    req_eq_u32!(nice::nice_to_weight(0), 1024, "nice(0) weight");

    crate::serial_println!("    [sched-test] PASS: test_nice_to_weight_nice0");
    TestResult::Passed
}

/// nice(-20) must have strictly higher weight than nice(0).
pub fn test_nice_to_weight_negative_higher() -> TestResult {
    crate::serial_println!("    [sched-test] running test_nice_to_weight_negative_higher...");

    req!(
        nice::nice_to_weight(-20) > nice::nice_to_weight(0),
        "nice -20 must have higher weight than nice 0"
    );

    crate::serial_println!("    [sched-test] PASS: test_nice_to_weight_negative_higher");
    TestResult::Passed
}

/// nice(19) must have strictly lower weight than nice(0).
pub fn test_nice_to_weight_positive_lower() -> TestResult {
    crate::serial_println!("    [sched-test] running test_nice_to_weight_positive_lower...");

    req!(
        nice::nice_to_weight(19) < nice::nice_to_weight(0),
        "nice 19 must have lower weight than nice 0"
    );

    crate::serial_println!("    [sched-test] PASS: test_nice_to_weight_positive_lower");
    TestResult::Passed
}

/// Weight table must be strictly monotonically decreasing from nice=-20 to nice=19.
pub fn test_nice_to_weight_monotone() -> TestResult {
    crate::serial_println!("    [sched-test] running test_nice_to_weight_monotone...");

    for n in -20i8..19i8 {
        let w_low = nice::nice_to_weight(n);
        let w_high = nice::nice_to_weight(n + 1);
        if w_low <= w_high {
            crate::serial_println!(
                "    [sched-test] ASSERT: weight[{}]={} should be > weight[{}]={}",
                n,
                w_low,
                n + 1,
                w_high
            );
            return TestResult::Failed;
        }
    }

    crate::serial_println!("    [sched-test] PASS: test_nice_to_weight_monotone");
    TestResult::Passed
}

/// clamp_nice must return values in [-20, 19] for any i8 input.
pub fn test_nice_clamp() -> TestResult {
    crate::serial_println!("    [sched-test] running test_nice_clamp...");

    // Edge cases
    let c_min = nice::clamp_nice(-100i8);
    let c_max = nice::clamp_nice(100i8);
    if c_min != nice::NICE_MIN {
        crate::serial_println!(
            "    [sched-test] ASSERT: clamp(-100)={}, expected {}",
            c_min,
            nice::NICE_MIN
        );
        return TestResult::Failed;
    }
    if c_max != nice::NICE_MAX {
        crate::serial_println!(
            "    [sched-test] ASSERT: clamp(100)={}, expected {}",
            c_max,
            nice::NICE_MAX
        );
        return TestResult::Failed;
    }

    // In-range values are unchanged
    for n in -20i8..=19i8 {
        let c = nice::clamp_nice(n);
        if c != n {
            crate::serial_println!(
                "    [sched-test] ASSERT: clamp({})={} should be unchanged",
                n,
                c
            );
            return TestResult::Failed;
        }
    }

    crate::serial_println!("    [sched-test] PASS: test_nice_clamp");
    TestResult::Passed
}

/// Each step in nice increases weight by approximately 25% (1.25×).
/// The Linux table guarantees at least 20% increase per step downward in nice.
pub fn test_nice_step_ratio() -> TestResult {
    crate::serial_println!("    [sched-test] running test_nice_step_ratio...");

    for n in -20i8..18i8 {
        let w0 = nice::nice_to_weight(n) as u64;
        let w1 = nice::nice_to_weight(n + 1) as u64;
        // w0 should be at least 1.2× w1 (20% more per step)
        // Use integer arithmetic: w0 * 10 >= w1 * 12
        if w0 * 10 < w1 * 12 {
            crate::serial_println!(
                "    [sched-test] ASSERT: weight ratio step {}->{} is less than 1.2x",
                n,
                n + 1
            );
            return TestResult::Failed;
        }
    }

    crate::serial_println!("    [sched-test] PASS: test_nice_step_ratio");
    TestResult::Passed
}

// ---------------------------------------------------------------------------
// run_all
// ---------------------------------------------------------------------------

pub fn run_all() {
    crate::serial_println!("    [sched-test] ==============================");
    crate::serial_println!("    [sched-test] Running scheduler test suite");
    crate::serial_println!("    [sched-test] ==============================");

    let mut passed = 0u32;
    let mut failed = 0u32;

    macro_rules! run {
        ($f:expr, $name:literal) => {
            match $f() {
                TestResult::Passed => {
                    passed += 1;
                    crate::serial_println!("    [sched-test] [PASS] {}", $name);
                }
                TestResult::Skipped => {
                    crate::serial_println!("    [sched-test] [SKIP] {}", $name);
                }
                TestResult::Failed => {
                    failed += 1;
                    crate::serial_println!("    [sched-test] [FAIL] {}", $name);
                }
            }
        };
    }

    // CFS run-queue
    run!(test_cfs_enqueue_dequeue_order, "cfs_enqueue_dequeue_order");
    run!(
        test_cfs_min_vruntime_starvation_prevention,
        "cfs_min_vruntime_starvation"
    );
    run!(test_cfs_update_vruntime, "cfs_update_vruntime");
    run!(test_cfs_dequeue_by_pid, "cfs_dequeue_by_pid");
    run!(test_cfs_weight_fairness, "cfs_weight_fairness");
    run!(test_cfs_empty_queue, "cfs_empty_queue");

    // nice-to-weight
    run!(test_nice_to_weight_nice0, "nice_to_weight_nice0");
    run!(
        test_nice_to_weight_negative_higher,
        "nice_to_weight_negative_higher"
    );
    run!(
        test_nice_to_weight_positive_lower,
        "nice_to_weight_positive_lower"
    );
    run!(test_nice_to_weight_monotone, "nice_to_weight_monotone");
    run!(test_nice_clamp, "nice_clamp");
    run!(test_nice_step_ratio, "nice_step_ratio");

    crate::serial_println!(
        "    [sched-test] Results: {} passed, {} failed",
        passed,
        failed
    );
}
