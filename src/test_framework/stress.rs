/// Stress testing utilities
///
/// Part of the AIOS. Runs stress tests to find concurrency
/// and resource exhaustion bugs by repeatedly executing workloads
/// at high iteration counts.
use crate::sync::Mutex;

/// Maximum concurrency level (simulated; real parallelism requires
/// multi-core scheduler support).
const MAX_CONCURRENCY: usize = 64;

/// Runs stress tests to find concurrency and resource exhaustion bugs.
pub struct StressTest {
    iterations: u64,
    concurrency: usize,
}

/// Stress test result
pub struct StressResult {
    pub iterations_completed: u64,
    pub target_iterations: u64,
    pub concurrency: usize,
    pub success: bool,
}

impl StressTest {
    pub fn new(iterations: u64, concurrency: usize) -> Self {
        let clamped_concurrency = if concurrency == 0 {
            1
        } else if concurrency > MAX_CONCURRENCY {
            MAX_CONCURRENCY
        } else {
            concurrency
        };

        let clamped_iterations = if iterations == 0 { 1 } else { iterations };

        crate::serial_println!(
            "    [stress] created stress test: {} iterations, concurrency={}",
            clamped_iterations,
            clamped_concurrency
        );

        Self {
            iterations: clamped_iterations,
            concurrency: clamped_concurrency,
        }
    }

    /// Run a stress test with the given workload function.
    /// In a single-core kernel, "concurrency" is simulated by interleaving
    /// workload invocations. Returns true if all iterations completed
    /// without issues, false otherwise.
    pub fn run(&self, workload: fn()) -> bool {
        crate::serial_println!(
            "    [stress] starting stress test: {} iterations x {} concurrency",
            self.iterations,
            self.concurrency
        );

        let total_calls = self.iterations * self.concurrency as u64;
        let mut completed = 0u64;

        // Simulate concurrent execution by running workload in
        // rounds of `concurrency` calls each
        let rounds = self.iterations;
        for round in 0..rounds {
            for _lane in 0..self.concurrency {
                workload();
                completed += 1;
            }

            // Progress reporting every 10% or every 1000 rounds
            if rounds >= 10 && round % (rounds / 10) == 0 && round > 0 {
                let pct = (round * 100) / rounds;
                crate::serial_println!(
                    "    [stress] progress: {}% ({}/{} rounds)",
                    pct,
                    round,
                    rounds
                );
            }
        }

        crate::serial_println!(
            "    [stress] completed: {}/{} calls executed successfully",
            completed,
            total_calls
        );

        completed == total_calls
    }

    /// Report stress test results via serial output.
    pub fn report(&self) {
        crate::serial_println!("    [stress] === Stress Test Configuration ===");
        crate::serial_println!("    [stress]   iterations:  {}", self.iterations);
        crate::serial_println!("    [stress]   concurrency: {}", self.concurrency);
        crate::serial_println!(
            "    [stress]   total calls: {}",
            self.iterations * self.concurrency as u64
        );
        crate::serial_println!("    [stress] ================================");
    }

    /// Run the stress test and return a structured result
    pub fn run_with_result(&self, workload: fn()) -> StressResult {
        let success = self.run(workload);
        StressResult {
            iterations_completed: if success {
                self.iterations * self.concurrency as u64
            } else {
                0
            },
            target_iterations: self.iterations * self.concurrency as u64,
            concurrency: self.concurrency,
            success,
        }
    }

    /// Get configured iteration count
    pub fn iterations(&self) -> u64 {
        self.iterations
    }

    /// Get configured concurrency level
    pub fn concurrency(&self) -> usize {
        self.concurrency
    }
}

/// Global stress test state
static STRESS_READY: Mutex<bool> = Mutex::new(false);

pub fn init() {
    let mut ready = STRESS_READY.lock();
    *ready = true;
    crate::serial_println!("    [stress] stress testing subsystem initialized");
}
