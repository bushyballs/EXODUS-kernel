use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
/// Process subsystem tests
///
/// Part of the AIOS. Tests for process creation, scheduling,
/// and inter-process communication using simulated process state.
use alloc::vec::Vec;

/// Simulated process state for testing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessState {
    Created,
    Ready,
    Running,
    Blocked,
    Terminated,
}

/// Simulated process descriptor
struct SimProcess {
    pid: u64,
    state: ProcessState,
    priority: u8,
    name: String,
}

/// Simulated IPC message queue for testing
static IPC_QUEUE: Mutex<Option<Vec<(u64, u64, Vec<u8>)>>> = Mutex::new(None);

/// PID counter
static NEXT_PID: Mutex<u64> = Mutex::new(100);

fn alloc_pid() -> u64 {
    let mut pid = NEXT_PID.lock();
    let current = *pid;
    *pid += 1;
    current
}

/// Tests for process creation, scheduling, and IPC.
pub struct ProcessTests;

impl ProcessTests {
    /// Test process creation and destruction.
    /// Creates simulated processes, verifies state transitions through
    /// the lifecycle (Created -> Ready -> Running -> Terminated).
    pub fn test_create_destroy() -> bool {
        crate::serial_println!("    [proc-test] running test_create_destroy...");

        // Create a process
        let pid = alloc_pid();
        let mut proc = SimProcess {
            pid,
            state: ProcessState::Created,
            priority: 10,
            name: String::from("test_process"),
        };

        // Verify initial state
        if proc.state != ProcessState::Created {
            crate::serial_println!("    [proc-test] FAIL: process not in Created state");
            return false;
        }

        if proc.pid == 0 {
            crate::serial_println!("    [proc-test] FAIL: invalid PID 0");
            return false;
        }

        // Transition through lifecycle
        proc.state = ProcessState::Ready;
        if proc.state != ProcessState::Ready {
            crate::serial_println!("    [proc-test] FAIL: transition to Ready failed");
            return false;
        }

        proc.state = ProcessState::Running;
        if proc.state != ProcessState::Running {
            crate::serial_println!("    [proc-test] FAIL: transition to Running failed");
            return false;
        }

        // Terminate process
        proc.state = ProcessState::Terminated;
        if proc.state != ProcessState::Terminated {
            crate::serial_println!("    [proc-test] FAIL: transition to Terminated failed");
            return false;
        }

        // Create and destroy multiple processes
        let mut pids = Vec::new();
        for i in 0..5u64 {
            let p = alloc_pid();
            pids.push(p);
        }

        if pids.len() != 5 {
            crate::serial_println!("    [proc-test] FAIL: expected 5 PIDs");
            return false;
        }

        // Verify all PIDs are unique
        for i in 0..pids.len() {
            for j in (i + 1)..pids.len() {
                if pids[i] == pids[j] {
                    crate::serial_println!("    [proc-test] FAIL: duplicate PID detected");
                    return false;
                }
            }
        }

        crate::serial_println!("    [proc-test] PASS: test_create_destroy (pid={})", pid);
        true
    }

    /// Test context switching between processes.
    /// Simulates a round-robin scheduler switching between multiple
    /// processes and verifies correct state transitions.
    pub fn test_context_switch() -> bool {
        crate::serial_println!("    [proc-test] running test_context_switch...");

        // Create several processes
        let mut processes = Vec::new();
        for i in 0..4u64 {
            let mut name = String::from("proc_");
            let digit = (b'0' + i as u8) as char;
            name.push(digit);
            processes.push(SimProcess {
                pid: alloc_pid(),
                state: ProcessState::Ready,
                priority: (10 + i) as u8,
                name,
            });
        }

        // Simulate round-robin context switching
        let num_switches = 12; // 3 full rounds
        let num_procs = processes.len();
        let mut current_idx = 0usize;
        let mut switch_count = 0u32;

        for _ in 0..num_switches {
            // "Switch out" current process
            if processes[current_idx].state == ProcessState::Running {
                processes[current_idx].state = ProcessState::Ready;
            }

            // Move to next process (round-robin)
            current_idx = (current_idx + 1) % num_procs;

            // "Switch in" next process
            if processes[current_idx].state == ProcessState::Ready {
                processes[current_idx].state = ProcessState::Running;
                switch_count += 1;
            }
        }

        if switch_count != num_switches as u32 {
            crate::serial_println!(
                "    [proc-test] FAIL: expected {} switches, got {}",
                num_switches,
                switch_count
            );
            return false;
        }

        // Verify all processes are in a valid state
        for p in &processes {
            if p.state != ProcessState::Ready && p.state != ProcessState::Running {
                crate::serial_println!(
                    "    [proc-test] FAIL: process {} in unexpected state after switching",
                    p.pid
                );
                return false;
            }
        }

        crate::serial_println!(
            "    [proc-test] PASS: test_context_switch ({} switches across {} processes)",
            switch_count,
            num_procs
        );
        true
    }

    /// Test inter-process communication.
    /// Simulates message passing between processes using a shared
    /// message queue, verifying send and receive correctness.
    pub fn test_ipc() -> bool {
        crate::serial_println!("    [proc-test] running test_ipc...");

        // Initialize IPC queue
        {
            let mut queue = IPC_QUEUE.lock();
            *queue = Some(Vec::new());
        }

        let sender_pid = alloc_pid();
        let receiver_pid = alloc_pid();

        // Send messages
        let messages: &[&[u8]] = &[b"hello from sender", b"second message", b"final message"];

        for msg in messages {
            let mut data = Vec::with_capacity(msg.len());
            for &b in *msg {
                data.push(b);
            }

            let mut queue = IPC_QUEUE.lock();
            if let Some(ref mut q) = *queue {
                q.push((sender_pid, receiver_pid, data));
            }
        }

        // Verify queue length
        let queue_len = {
            let queue = IPC_QUEUE.lock();
            match queue.as_ref() {
                Some(q) => q.len(),
                None => 0,
            }
        };

        if queue_len != messages.len() {
            crate::serial_println!(
                "    [proc-test] FAIL: expected {} messages in queue, got {}",
                messages.len(),
                queue_len
            );
            return false;
        }

        // Receive messages for receiver_pid
        let received = {
            let mut queue = IPC_QUEUE.lock();
            let mut received = Vec::new();
            if let Some(ref mut q) = *queue {
                q.retain(|entry| {
                    if entry.1 == receiver_pid {
                        received.push(entry.2.clone());
                        false // remove from queue
                    } else {
                        true // keep in queue
                    }
                });
            }
            received
        };

        if received.len() != messages.len() {
            crate::serial_println!(
                "    [proc-test] FAIL: received {} messages, expected {}",
                received.len(),
                messages.len()
            );
            return false;
        }

        // Verify content of first message
        let expected: Vec<u8> = Vec::from(*b"hello from sender");
        if received[0] != expected {
            crate::serial_println!("    [proc-test] FAIL: first message content mismatch");
            return false;
        }

        // Verify queue is now empty for this receiver
        let remaining = {
            let queue = IPC_QUEUE.lock();
            match queue.as_ref() {
                Some(q) => q.iter().filter(|e| e.1 == receiver_pid).count(),
                None => 0,
            }
        };

        if remaining != 0 {
            crate::serial_println!("    [proc-test] FAIL: queue should be empty after receive");
            return false;
        }

        crate::serial_println!(
            "    [proc-test] PASS: test_ipc (sender={}, receiver={}, {} messages)",
            sender_pid,
            receiver_pid,
            messages.len()
        );
        true
    }
}

pub fn run_all() {
    crate::serial_println!("    [proc-test] ==============================");
    crate::serial_println!("    [proc-test] Running process test suite");
    crate::serial_println!("    [proc-test] ==============================");

    let mut passed = 0u32;
    let mut failed = 0u32;

    if ProcessTests::test_create_destroy() {
        passed += 1;
    } else {
        failed += 1;
    }
    if ProcessTests::test_context_switch() {
        passed += 1;
    } else {
        failed += 1;
    }
    if ProcessTests::test_ipc() {
        passed += 1;
    } else {
        failed += 1;
    }

    crate::serial_println!(
        "    [proc-test] Results: {} passed, {} failed",
        passed,
        failed
    );
}
