use crate::ipc::pipe;
use crate::ipc::semaphore;
/// IPC subsystem tests
///
/// Part of the AIOS. Tests pipe write/read, wrap-around behaviour,
/// named pipes, statistics, and System V semaphore acquire/release
/// semantics. All tests are deterministic and hardware-independent.
use crate::test_framework::runner::TestResult;

// ---------------------------------------------------------------------------
// Assertion helpers (local macros -- keep test_framework self-contained)
// ---------------------------------------------------------------------------

/// Return Fail if condition is false.
macro_rules! req {
    ($cond:expr, $msg:expr) => {
        if !$cond {
            return TestResult::Failed;
        }
    };
}

/// Return Fail if two usize values differ.
macro_rules! req_eq_usize {
    ($a:expr, $b:expr) => {
        if $a != $b {
            crate::serial_println!(
                "    [ipc-test] ASSERT: expected {} == {} (got {} vs {})",
                stringify!($a),
                stringify!($b),
                $a,
                $b
            );
            return TestResult::Failed;
        }
    };
}

// ---------------------------------------------------------------------------
// Pipe tests
// ---------------------------------------------------------------------------

/// Create a pipe, write a short message, read it back, verify byte-for-byte.
pub fn test_pipe_write_read() -> TestResult {
    crate::serial_println!("    [ipc-test] running test_pipe_write_read...");

    // Create a pipe between two synthetic PIDs (1000 = reader, 1001 = writer)
    let pipe_id = match pipe::create(1000, 1001) {
        Ok(id) => id,
        Err(e) => {
            crate::serial_println!("    [ipc-test] FAIL: pipe::create returned Err({})", e);
            return TestResult::Failed;
        }
    };

    let data = b"hello kernel";
    let written = match pipe::write(pipe_id, data) {
        Ok(n) => n,
        Err(e) => {
            crate::serial_println!("    [ipc-test] FAIL: pipe::write returned Err({})", e);
            return TestResult::Failed;
        }
    };
    req_eq_usize!(written, data.len());

    // Verify available byte count
    let avail = match pipe::available(pipe_id) {
        Ok(n) => n,
        Err(e) => {
            crate::serial_println!("    [ipc-test] FAIL: pipe::available Err({})", e);
            return TestResult::Failed;
        }
    };
    req_eq_usize!(avail, data.len());

    let mut buf = [0u8; 64];
    let read = match pipe::read(pipe_id, &mut buf) {
        Ok(n) => n,
        Err(e) => {
            crate::serial_println!("    [ipc-test] FAIL: pipe::read Err({})", e);
            return TestResult::Failed;
        }
    };
    req_eq_usize!(read, data.len());
    req!(&buf[..read] == data, "pipe data mismatch");

    // After read the pipe should be empty
    let avail_after = match pipe::available(pipe_id) {
        Ok(n) => n,
        Err(_) => usize::MAX,
    };
    req_eq_usize!(avail_after, 0);

    // Verify statistics were recorded
    let stats = match pipe::get_stats(pipe_id) {
        Ok(s) => s,
        Err(e) => {
            crate::serial_println!("    [ipc-test] FAIL: pipe::get_stats Err({})", e);
            return TestResult::Failed;
        }
    };
    req!(
        stats.bytes_written == data.len() as u64,
        "bytes_written mismatch"
    );
    req!(stats.bytes_read == data.len() as u64, "bytes_read mismatch");
    req!(stats.write_calls >= 1, "write_calls zero");
    req!(stats.read_calls >= 1, "read_calls zero");

    crate::serial_println!(
        "    [ipc-test] PASS: test_pipe_write_read ({} bytes)",
        data.len()
    );
    TestResult::Passed
}

/// Write exactly enough data to fill the 4096-byte ring buffer, read half,
/// then write more -- exercising the wrap-around path.
pub fn test_pipe_wrap_around() -> TestResult {
    crate::serial_println!("    [ipc-test] running test_pipe_wrap_around...");

    let pipe_id = match pipe::create(1002, 1003) {
        Ok(id) => id,
        Err(e) => {
            crate::serial_println!("    [ipc-test] FAIL: create Err({})", e);
            return TestResult::Failed;
        }
    };

    // Fill the buffer (4096 bytes is DEFAULT_BUF_SIZE in pipe.rs)
    let chunk = [0xABu8; 256];
    let mut total_written: usize = 0;
    for _ in 0..16 {
        // 16 x 256 = 4096 bytes -- fills the ring exactly
        match pipe::write(pipe_id, &chunk) {
            Ok(n) => total_written = total_written.saturating_add(n),
            Err(e) => {
                crate::serial_println!("    [ipc-test] FAIL: fill write Err({})", e);
                return TestResult::Failed;
            }
        }
    }
    req_eq_usize!(total_written, 4096);

    // Read half (2048 bytes) to free space at the start of the ring
    let mut read_buf = [0u8; 2048];
    let read1 = match pipe::read(pipe_id, &mut read_buf) {
        Ok(n) => n,
        Err(e) => {
            crate::serial_println!("    [ipc-test] FAIL: half read Err({})", e);
            return TestResult::Failed;
        }
    };
    req_eq_usize!(read1, 2048);

    // Write 2048 more bytes -- these must wrap around in the ring buffer
    let chunk2 = [0xCDu8; 256];
    let mut wrap_written: usize = 0;
    for _ in 0..8 {
        match pipe::write(pipe_id, &chunk2) {
            Ok(n) => wrap_written = wrap_written.saturating_add(n),
            Err(e) => {
                crate::serial_println!("    [ipc-test] FAIL: wrap write Err({})", e);
                return TestResult::Failed;
            }
        }
    }
    req_eq_usize!(wrap_written, 2048);

    // Read back remaining 2048 (original) + 2048 (new) = 4096 bytes total
    // First read the remaining original data (0xAB bytes)
    let mut verify_buf = [0u8; 2048];
    let read2 = match pipe::read(pipe_id, &mut verify_buf) {
        Ok(n) => n,
        Err(e) => {
            crate::serial_println!("    [ipc-test] FAIL: tail read Err({})", e);
            return TestResult::Failed;
        }
    };
    req_eq_usize!(read2, 2048);
    // All bytes should be 0xAB (original fill)
    for i in 0..read2 {
        if verify_buf[i] != 0xAB {
            crate::serial_println!(
                "    [ipc-test] FAIL: original tail byte[{}] = {:#x}, expected 0xAB",
                i,
                verify_buf[i]
            );
            return TestResult::Failed;
        }
    }

    // Now read the wrap-around data (0xCD bytes)
    let read3 = match pipe::read(pipe_id, &mut verify_buf) {
        Ok(n) => n,
        Err(e) => {
            crate::serial_println!("    [ipc-test] FAIL: wrap read Err({})", e);
            return TestResult::Failed;
        }
    };
    req_eq_usize!(read3, 2048);
    for i in 0..read3 {
        if verify_buf[i] != 0xCD {
            crate::serial_println!(
                "    [ipc-test] FAIL: wrap byte[{}] = {:#x}, expected 0xCD",
                i,
                verify_buf[i]
            );
            return TestResult::Failed;
        }
    }

    // Pipe should now be empty
    let final_avail = match pipe::available(pipe_id) {
        Ok(n) => n,
        Err(_) => usize::MAX,
    };
    req_eq_usize!(final_avail, 0);

    crate::serial_println!("    [ipc-test] PASS: test_pipe_wrap_around");
    TestResult::Passed
}

/// Closing the write end turns subsequent reads into EOF (Ok(0)).
pub fn test_pipe_close_write_eof() -> TestResult {
    crate::serial_println!("    [ipc-test] running test_pipe_close_write_eof...");

    let pipe_id = match pipe::create(1004, 1005) {
        Ok(id) => id,
        Err(e) => {
            crate::serial_println!("    [ipc-test] FAIL: create Err({})", e);
            return TestResult::Failed;
        }
    };

    // Write a small message
    let msg = b"eof test";
    let _ = pipe::write(pipe_id, msg);

    // Close the write end
    if let Err(e) = pipe::close_write(pipe_id) {
        crate::serial_println!("    [ipc-test] FAIL: close_write Err({})", e);
        return TestResult::Failed;
    }

    // First read should consume buffered data
    let mut buf = [0u8; 64];
    let r1 = match pipe::read(pipe_id, &mut buf) {
        Ok(n) => n,
        Err(e) => {
            crate::serial_println!("    [ipc-test] FAIL: first read Err({})", e);
            return TestResult::Failed;
        }
    };
    req_eq_usize!(r1, msg.len());

    // Second read on write-closed pipe with no buffered data -> EOF (Ok(0))
    let r2 = pipe::read(pipe_id, &mut buf);
    match r2 {
        Ok(0) => {} // correct EOF
        Ok(n) => {
            crate::serial_println!("    [ipc-test] FAIL: expected EOF (0) got {}", n);
            return TestResult::Failed;
        }
        Err(e) => {
            crate::serial_println!("    [ipc-test] FAIL: expected EOF got Err({})", e);
            return TestResult::Failed;
        }
    }

    crate::serial_println!("    [ipc-test] PASS: test_pipe_close_write_eof");
    TestResult::Passed
}

/// Named pipe: create by name, find by name, verify they resolve to same ID.
pub fn test_named_pipe() -> TestResult {
    crate::serial_println!("    [ipc-test] running test_named_pipe...");

    let name = "test_fifo_99";
    let pipe_id = match pipe::create_named(name, 2000, 2001) {
        Ok(id) => id,
        Err(e) => {
            crate::serial_println!("    [ipc-test] FAIL: create_named Err({})", e);
            return TestResult::Failed;
        }
    };

    // find_named should return the same ID
    let found_id = match pipe::find_named(name) {
        Some(id) => id,
        None => {
            crate::serial_println!("    [ipc-test] FAIL: find_named returned None");
            return TestResult::Failed;
        }
    };
    req_eq_usize!(found_id, pipe_id);

    // Duplicate name should fail
    req!(
        pipe::create_named(name, 2002, 2003).is_err(),
        "duplicate named pipe should fail"
    );

    // Non-existent name should return None
    req!(
        pipe::find_named("no_such_fifo").is_none(),
        "non-existent name should return None"
    );

    // Write and read through the named pipe
    let data = b"named pipe data";
    let _ = pipe::write(pipe_id, data);
    let mut buf = [0u8; 64];
    let read = match pipe::read(pipe_id, &mut buf) {
        Ok(n) => n,
        Err(_) => 0,
    };
    req_eq_usize!(read, data.len());
    req!(&buf[..read] == data, "named pipe data mismatch");

    crate::serial_println!("    [ipc-test] PASS: test_named_pipe");
    TestResult::Passed
}

// ---------------------------------------------------------------------------
// Semaphore tests
//
// The semaphore module uses raw integer returns:
//   semget(key, nsems, flags) -> i32  (>= 1 on success, negative on error)
//   semctl(semid, semnum, cmd, arg) -> i64  (>= 0 on success, negative on error)
//   semop(semid, &[Sembuf]) -> i32   (0 on success, negative on error)
//   Struct is Sembuf (lowercase b).
// ---------------------------------------------------------------------------

/// Create a semaphore set with initial value 1. Acquire (semop -1) should
/// succeed. A second acquire while value is 0 should block (return error).
/// Post (semop +1) should allow a third acquire to succeed.
pub fn test_semaphore_wait_post() -> TestResult {
    crate::serial_println!("    [ipc-test] running test_semaphore_wait_post...");

    // Ensure the semaphore subsystem is up
    semaphore::init();

    // Use IPC_PRIVATE (key=0) -- creates a new private set
    let sem_id = semaphore::semget(0, 1, semaphore::IPC_CREAT);
    if sem_id < 0 {
        crate::serial_println!("    [ipc-test] FAIL: semget returned {}", sem_id);
        return TestResult::Failed;
    }

    // Set initial value to 1 via SETVAL
    let rc = semaphore::semctl(sem_id, 0, semaphore::SETVAL, 1);
    if rc < 0 {
        crate::serial_println!("    [ipc-test] FAIL: semctl SETVAL returned {}", rc);
        return TestResult::Failed;
    }

    // Verify value is 1
    let val = semaphore::semctl(sem_id, 0, semaphore::GETVAL, 0);
    if val != 1 {
        crate::serial_println!("    [ipc-test] FAIL: expected semval=1, got {}", val);
        return TestResult::Failed;
    }

    // First wait (sem_op = -1): value 1->0, should succeed
    let acquire_op = semaphore::Sembuf {
        sem_num: 0,
        sem_op: -1,
        sem_flg: semaphore::IPC_NOWAIT,
    };
    let r1 = semaphore::semop(sem_id, &[acquire_op]);
    req!(r1 == 0, "first acquire should succeed");

    // Verify value is now 0
    let val2 = semaphore::semctl(sem_id, 0, semaphore::GETVAL, 0);
    if val2 != 0 {
        crate::serial_println!(
            "    [ipc-test] FAIL: expected semval=0 after acquire, got {}",
            val2
        );
        return TestResult::Failed;
    }

    // Second wait with value=0 should return error (would block)
    let r2 = semaphore::semop(sem_id, &[acquire_op]);
    req!(r2 < 0, "second acquire on zero semaphore should block/fail");

    // Post (sem_op = +1): value 0->1
    let release_op = semaphore::Sembuf {
        sem_num: 0,
        sem_op: 1,
        sem_flg: 0,
    };
    let r3 = semaphore::semop(sem_id, &[release_op]);
    req!(r3 == 0, "post should succeed");

    // Verify value is 1 again
    let val3 = semaphore::semctl(sem_id, 0, semaphore::GETVAL, 0);
    if val3 != 1 {
        crate::serial_println!(
            "    [ipc-test] FAIL: expected semval=1 after post, got {}",
            val3
        );
        return TestResult::Failed;
    }

    // Third wait should now succeed again
    let r4 = semaphore::semop(sem_id, &[acquire_op]);
    req!(r4 == 0, "wait after post should succeed");

    // Clean up: IPC_RMID
    let _ = semaphore::semctl(sem_id, 0, semaphore::IPC_RMID, 0);

    crate::serial_println!("    [ipc-test] PASS: test_semaphore_wait_post");
    TestResult::Passed
}

/// Create a multi-semaphore set, verify independent control of each slot.
pub fn test_semaphore_multi() -> TestResult {
    crate::serial_println!("    [ipc-test] running test_semaphore_multi...");

    semaphore::init();

    // Three-semaphore set
    let sem_id = semaphore::semget(0, 3, semaphore::IPC_CREAT);
    if sem_id < 0 {
        crate::serial_println!("    [ipc-test] FAIL: semget returned {}", sem_id);
        return TestResult::Failed;
    }

    // Set values: sem[0]=5, sem[1]=0, sem[2]=3
    let _ = semaphore::semctl(sem_id, 0, semaphore::SETVAL, 5);
    let _ = semaphore::semctl(sem_id, 1, semaphore::SETVAL, 0);
    let _ = semaphore::semctl(sem_id, 2, semaphore::SETVAL, 3);

    let v0 = semaphore::semctl(sem_id, 0, semaphore::GETVAL, 0);
    let v1 = semaphore::semctl(sem_id, 1, semaphore::GETVAL, 0);
    let v2 = semaphore::semctl(sem_id, 2, semaphore::GETVAL, 0);
    if v0 != 5 || v1 != 0 || v2 != 3 {
        crate::serial_println!(
            "    [ipc-test] FAIL: initial values: sem[0]={} sem[1]={} sem[2]={}",
            v0,
            v1,
            v2
        );
        return TestResult::Failed;
    }

    // Decrement sem[0] by 3: 5->2
    let dec_op = semaphore::Sembuf {
        sem_num: 0,
        sem_op: -3,
        sem_flg: semaphore::IPC_NOWAIT,
    };
    let r1 = semaphore::semop(sem_id, &[dec_op]);
    req!(r1 == 0, "decrement sem[0] by 3 should succeed");

    let v0b = semaphore::semctl(sem_id, 0, semaphore::GETVAL, 0);
    if v0b != 2 {
        crate::serial_println!("    [ipc-test] FAIL: sem[0] should be 2, got {}", v0b);
        return TestResult::Failed;
    }

    // sem[1] is 0 -- wait-for-zero op (sem_op=0) should succeed
    let zero_op = semaphore::Sembuf {
        sem_num: 1,
        sem_op: 0,
        sem_flg: semaphore::IPC_NOWAIT,
    };
    let r2 = semaphore::semop(sem_id, &[zero_op]);
    req!(r2 == 0, "wait-for-zero on sem[1]=0 should succeed");

    // sem[2] has value 3 -- wait-for-zero should fail
    let zero_op2 = semaphore::Sembuf {
        sem_num: 2,
        sem_op: 0,
        sem_flg: semaphore::IPC_NOWAIT,
    };
    let r3 = semaphore::semop(sem_id, &[zero_op2]);
    req!(r3 < 0, "wait-for-zero on sem[2]=3 should fail");

    let _ = semaphore::semctl(sem_id, 0, semaphore::IPC_RMID, 0);

    crate::serial_println!("    [ipc-test] PASS: test_semaphore_multi");
    TestResult::Passed
}

/// semctl IPC_RMID removes the set; subsequent GETVAL should fail.
pub fn test_semaphore_rmid() -> TestResult {
    crate::serial_println!("    [ipc-test] running test_semaphore_rmid...");

    semaphore::init();

    let sem_id = semaphore::semget(0, 4, semaphore::IPC_CREAT);
    if sem_id < 0 {
        crate::serial_println!("    [ipc-test] FAIL: semget returned {}", sem_id);
        return TestResult::Failed;
    }

    // Set a value so we know the set is alive
    let _ = semaphore::semctl(sem_id, 0, semaphore::SETVAL, 42);
    let before = semaphore::semctl(sem_id, 0, semaphore::GETVAL, 0);
    if before != 42 {
        crate::serial_println!("    [ipc-test] FAIL: expected 42, got {}", before);
        return TestResult::Failed;
    }

    // IPC_RMID cleans up
    let _ = semaphore::semctl(sem_id, 0, semaphore::IPC_RMID, 0);

    // After removal, GETVAL should fail (return negative)
    let after = semaphore::semctl(sem_id, 0, semaphore::GETVAL, 0);
    req!(after < 0, "GETVAL on removed set should fail");

    crate::serial_println!("    [ipc-test] PASS: test_semaphore_rmid");
    TestResult::Passed
}

// ---------------------------------------------------------------------------
// run_all
// ---------------------------------------------------------------------------

pub fn run_all() {
    crate::serial_println!("    [ipc-test] ==============================");
    crate::serial_println!("    [ipc-test] Running IPC test suite");
    crate::serial_println!("    [ipc-test] ==============================");

    let mut passed = 0u32;
    let mut failed = 0u32;

    macro_rules! run {
        ($f:expr, $name:literal) => {
            match $f() {
                TestResult::Passed => {
                    passed += 1;
                    crate::serial_println!("    [ipc-test] [PASS] {}", $name);
                }
                TestResult::Skipped => {
                    crate::serial_println!("    [ipc-test] [SKIP] {}", $name);
                }
                TestResult::Failed => {
                    failed += 1;
                    crate::serial_println!("    [ipc-test] [FAIL] {}", $name);
                }
            }
        };
    }

    run!(test_pipe_write_read, "pipe_write_read");
    run!(test_pipe_wrap_around, "pipe_wrap_around");
    run!(test_pipe_close_write_eof, "pipe_close_write_eof");
    run!(test_named_pipe, "named_pipe");
    run!(test_semaphore_wait_post, "semaphore_wait_post");
    run!(test_semaphore_multi, "semaphore_multi");
    run!(test_semaphore_rmid, "semaphore_rmid");

    crate::serial_println!(
        "    [ipc-test] Results: {} passed, {} failed",
        passed,
        failed
    );
}
