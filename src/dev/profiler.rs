/// Profiler for Genesis — performance analysis
///
/// CPU profiling (sampling, instrumentation), memory profiling,
/// system tracing, flame graph generation, and allocation tracking.
///
/// Inspired by: perf, Valgrind, Instruments, Tracy. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Profile type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileType {
    CpuSampling,
    CpuTracing,
    Memory,
    Syscall,
    Lock,
    Io,
}

/// CPU sample
#[derive(Clone)]
pub struct CpuSample {
    pub timestamp: u64,
    pub pid: u32,
    pub tid: u32,
    pub instruction_ptr: u64,
    pub stack: Vec<u64>,
}

/// Function timing entry
pub struct FuncTiming {
    pub name: String,
    pub total_ns: u64,
    pub self_ns: u64,
    pub call_count: u64,
    pub min_ns: u64,
    pub max_ns: u64,
}

/// Profiler state
pub struct Profiler {
    pub active: bool,
    pub profile_type: ProfileType,
    pub cpu_samples: Vec<CpuSample>,
    pub timings: BTreeMap<String, FuncTiming>,
    pub sample_interval_us: u32,
    pub start_time: u64,
    pub total_samples: u64,
    pub max_samples: usize,
    pub live_allocs: BTreeMap<u64, usize>,
    pub peak_memory: usize,
    pub current_memory: usize,
    pub total_allocated: u64,
    pub total_freed: u64,
}

impl Profiler {
    const fn new() -> Self {
        Profiler {
            active: false,
            profile_type: ProfileType::CpuSampling,
            cpu_samples: Vec::new(),
            timings: BTreeMap::new(),
            sample_interval_us: 1000,
            start_time: 0,
            total_samples: 0,
            max_samples: 100_000,
            live_allocs: BTreeMap::new(),
            peak_memory: 0,
            current_memory: 0,
            total_allocated: 0,
            total_freed: 0,
        }
    }

    pub fn start(&mut self, profile_type: ProfileType) {
        self.active = true;
        self.profile_type = profile_type;
        self.cpu_samples.clear();
        self.timings.clear();
        self.total_samples = 0;
        self.start_time = crate::time::clock::unix_time();
    }

    pub fn stop(&mut self) {
        self.active = false;
    }

    pub fn record_sample(&mut self, pid: u32, tid: u32, ip: u64, stack: &[u64]) {
        if !self.active || self.cpu_samples.len() >= self.max_samples {
            return;
        }
        self.cpu_samples.push(CpuSample {
            timestamp: crate::time::clock::unix_time(),
            pid,
            tid,
            instruction_ptr: ip,
            stack: stack.to_vec(),
        });
        self.total_samples = self.total_samples.saturating_add(1);
    }

    pub fn record_alloc(&mut self, address: u64, size: usize) {
        if !self.active {
            return;
        }
        self.live_allocs.insert(address, size);
        self.current_memory = self.current_memory.saturating_add(size);
        self.total_allocated = self.total_allocated.saturating_add(size as u64);
        if self.current_memory > self.peak_memory {
            self.peak_memory = self.current_memory;
        }
    }

    pub fn record_free(&mut self, address: u64) {
        if !self.active {
            return;
        }
        if let Some(size) = self.live_allocs.remove(&address) {
            self.current_memory = self.current_memory.saturating_sub(size);
            self.total_freed = self.total_freed.saturating_add(size as u64);
        }
    }

    pub fn record_timing(&mut self, name: &str, duration_ns: u64) {
        if !self.active {
            return;
        }
        let entry = self
            .timings
            .entry(String::from(name))
            .or_insert(FuncTiming {
                name: String::from(name),
                total_ns: 0,
                self_ns: 0,
                call_count: 0,
                min_ns: u64::MAX,
                max_ns: 0,
            });
        entry.total_ns = entry.total_ns.saturating_add(duration_ns);
        entry.self_ns = entry.self_ns.saturating_add(duration_ns);
        entry.call_count = entry.call_count.saturating_add(1);
        if duration_ns < entry.min_ns {
            entry.min_ns = duration_ns;
        }
        if duration_ns > entry.max_ns {
            entry.max_ns = duration_ns;
        }
    }

    pub fn memory_summary(&self) -> String {
        format!(
            "Current: {} KB, Peak: {} KB, Alloc: {} KB, Free: {} KB, Live: {}",
            self.current_memory / 1024,
            self.peak_memory / 1024,
            self.total_allocated / 1024,
            self.total_freed / 1024,
            self.live_allocs.len()
        )
    }
}

static PROFILER: Mutex<Profiler> = Mutex::new(Profiler::new());

pub fn init() {
    crate::serial_println!("  [profiler] Performance profiler initialized");
}

pub fn start(profile_type: ProfileType) {
    PROFILER.lock().start(profile_type);
}
pub fn stop() {
    PROFILER.lock().stop();
}
pub fn record_alloc(addr: u64, size: usize) {
    PROFILER.lock().record_alloc(addr, size);
}
pub fn record_free(addr: u64) {
    PROFILER.lock().record_free(addr);
}
