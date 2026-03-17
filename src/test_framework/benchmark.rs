use crate::sync::Mutex;
use alloc::string::String;
/// Performance benchmarks
///
/// Part of the AIOS. Measures execution time of kernel
/// operations for performance tracking using TSC or
/// monotonic tick-based timing.
use alloc::vec::Vec;

/// Read the CPU timestamp counter (TSC) for timing.
/// On x86_64 this is a monotonically increasing counter.
#[inline]
fn read_tsc() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        let lo: u32;
        let hi: u32;
        unsafe {
            core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
        }
        ((hi as u64) << 32) | (lo as u64)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        // Fallback: use a simple counter
        static COUNTER: Mutex<u64> = Mutex::new(0);
        let mut c = COUNTER.lock();
        *c += 1000;
        *c
    }
}

/// Estimated TSC frequency for nanosecond conversion.
/// Default 2 GHz assumption; calibrated at init.
static TSC_FREQ_MHZ: Mutex<u64> = Mutex::new(2000);

/// Convert TSC cycles to nanoseconds
fn cycles_to_ns(cycles: u64) -> u64 {
    let freq = *TSC_FREQ_MHZ.lock();
    if freq == 0 {
        return cycles;
    }
    // cycles / (freq_mhz) = microseconds
    // cycles * 1000 / freq_mhz = nanoseconds
    (cycles * 1000) / freq
}

/// Measures execution time of kernel operations for performance tracking.
pub struct Benchmark {
    name: &'static str,
    samples: Vec<u64>,
}

/// Benchmark result statistics
pub struct BenchmarkStats {
    pub name: &'static str,
    pub iterations: u64,
    pub min_ns: u64,
    pub max_ns: u64,
    pub avg_ns: u64,
    pub median_ns: u64,
}

impl Benchmark {
    pub fn new(name: &'static str) -> Self {
        crate::serial_println!("    [benchmark] created benchmark '{}'", name);
        Self {
            name,
            samples: Vec::new(),
        }
    }

    /// Run the benchmark function N times and collect timing samples.
    /// Each iteration measures the TSC delta around the function call.
    pub fn run(&mut self, func: fn(), iterations: u64) {
        self.samples.clear();
        self.samples.reserve(iterations as usize);

        crate::serial_println!(
            "    [benchmark] running '{}' for {} iterations...",
            self.name,
            iterations
        );

        // Warmup: run a few times to stabilize caches
        let warmup = if iterations > 10 { 3 } else { 0 };
        for _ in 0..warmup {
            func();
        }

        // Timed iterations
        for _i in 0..iterations {
            let start = read_tsc();
            func();
            let end = read_tsc();

            let elapsed = if end > start { end - start } else { 0 };
            self.samples.push(elapsed);
        }

        crate::serial_println!(
            "    [benchmark] '{}': collected {} samples",
            self.name,
            self.samples.len()
        );
    }

    /// Return the average time in nanoseconds.
    pub fn average_ns(&self) -> u64 {
        if self.samples.is_empty() {
            return 0;
        }
        let sum: u64 = self.samples.iter().sum();
        let avg_cycles = sum / self.samples.len() as u64;
        cycles_to_ns(avg_cycles)
    }

    /// Return the minimum sample time in nanoseconds.
    pub fn min_ns(&self) -> u64 {
        self.samples
            .iter()
            .copied()
            .min()
            .map(cycles_to_ns)
            .unwrap_or(0)
    }

    /// Return the maximum sample time in nanoseconds.
    pub fn max_ns(&self) -> u64 {
        self.samples
            .iter()
            .copied()
            .max()
            .map(cycles_to_ns)
            .unwrap_or(0)
    }

    /// Return the median sample time in nanoseconds.
    pub fn median_ns(&self) -> u64 {
        if self.samples.is_empty() {
            return 0;
        }
        let mut sorted = self.samples.clone();
        // Insertion sort (no_std friendly)
        let len = sorted.len();
        for i in 1..len {
            let mut j = i;
            while j > 0 && sorted[j] < sorted[j - 1] {
                sorted.swap(j, j - 1);
                j -= 1;
            }
        }
        let mid = sorted.len() / 2;
        cycles_to_ns(sorted[mid])
    }

    /// Get full statistics
    pub fn stats(&self) -> BenchmarkStats {
        BenchmarkStats {
            name: self.name,
            iterations: self.samples.len() as u64,
            min_ns: self.min_ns(),
            max_ns: self.max_ns(),
            avg_ns: self.average_ns(),
            median_ns: self.median_ns(),
        }
    }

    /// Report benchmark results via serial output.
    pub fn report(&self) {
        let stats = self.stats();
        crate::serial_println!("    [benchmark] === '{}' ===", stats.name);
        crate::serial_println!("    [benchmark]   iterations: {}", stats.iterations);
        crate::serial_println!("    [benchmark]   min:    {} ns", stats.min_ns);
        crate::serial_println!("    [benchmark]   max:    {} ns", stats.max_ns);
        crate::serial_println!("    [benchmark]   avg:    {} ns", stats.avg_ns);
        crate::serial_println!("    [benchmark]   median: {} ns", stats.median_ns);
        crate::serial_println!("    [benchmark] ==================");
    }

    /// Get the raw samples in cycles
    pub fn raw_samples(&self) -> &[u64] {
        &self.samples
    }

    /// Get the number of collected samples
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }
}

/// Global benchmark registry
static BENCHMARKS: Mutex<Option<Vec<BenchmarkStats>>> = Mutex::new(None);

pub fn init() {
    let mut reg = BENCHMARKS.lock();
    *reg = Some(Vec::new());
    crate::serial_println!("    [benchmark] benchmark subsystem initialized");
}

/// Record a benchmark result in the global registry
pub fn record_result(stats: BenchmarkStats) {
    let mut reg = BENCHMARKS.lock();
    if let Some(ref mut list) = *reg {
        list.push(stats);
    }
}

/// Get the count of recorded benchmarks
pub fn recorded_count() -> usize {
    let reg = BENCHMARKS.lock();
    match reg.as_ref() {
        Some(list) => list.len(),
        None => 0,
    }
}
