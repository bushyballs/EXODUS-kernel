/// System monitor / task manager for Genesis OS
///
/// Real-time system monitoring with per-core CPU usage tracking,
/// memory usage breakdown, process list with PID/CPU%/MEM, process
/// kill capability, I/O statistics, and uptime tracking. All
/// percentages and rates use Q16 fixed-point arithmetic.
///
/// Inspired by: htop, Windows Task Manager, GNOME System Monitor.
/// All code is original.

use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers
// ---------------------------------------------------------------------------

/// Q16 constant: 1.0 = 65536
const Q16_ONE: i32 = 65536;
/// Q16 constant: 100.0 (for percentage display)
const Q16_100: i32 = 6_553_600;

/// Q16 multiply
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Q16 divide (returns None on division by zero)
fn q16_div(a: i32, b: i32) -> Option<i32> {
    if b == 0 { return None; }
    Some((((a as i64) << 16) / (b as i64)) as i32)
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of tracked processes
const MAX_PROCESSES: usize = 1024;
/// Maximum CPU cores tracked
const MAX_CORES: usize = 64;
/// Maximum I/O device entries
const MAX_IO_DEVICES: usize = 32;
/// History sample depth per core
const MAX_HISTORY_SAMPLES: usize = 120;
/// Maximum number of memory regions
const MAX_MEMORY_REGIONS: usize = 64;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Process state
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProcessState {
    Running,
    Sleeping,
    Waiting,
    Zombie,
    Stopped,
    Dead,
}

/// Process priority level
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Priority {
    Idle,
    Low,
    Normal,
    High,
    Realtime,
}

/// Sort criteria for the process list
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProcessSort {
    Pid,
    CpuDesc,
    CpuAsc,
    MemDesc,
    MemAsc,
    NameHash,
}

/// Result codes for monitor operations
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MonitorResult {
    Success,
    ProcessNotFound,
    PermissionDenied,
    LimitReached,
    InvalidInput,
    IoError,
}

/// View mode for the monitor display
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ViewMode {
    ProcessList,
    CpuDetail,
    MemoryDetail,
    IoDetail,
    Overview,
}

/// A single process entry
#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: u32,
    pub name_hash: u64,
    pub state: ProcessState,
    pub priority: Priority,
    pub cpu_usage_q16: i32,
    pub memory_bytes: u64,
    pub memory_peak: u64,
    pub threads: u32,
    pub open_files: u32,
    pub start_time: u64,
    pub cpu_time_ms: u64,
    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
}

/// Per-core CPU statistics
#[derive(Debug, Clone)]
pub struct CoreStats {
    pub core_id: u32,
    pub usage_q16: i32,
    pub user_q16: i32,
    pub system_q16: i32,
    pub idle_q16: i32,
    pub iowait_q16: i32,
    pub frequency_mhz: u32,
    pub temperature_q16: i32,
    pub history: Vec<i32>,
}

/// Memory statistics
#[derive(Debug, Clone, Copy)]
pub struct MemoryStats {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    pub cached_bytes: u64,
    pub buffers_bytes: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    pub heap_used: u64,
    pub heap_total: u64,
    pub page_faults: u64,
}

/// Memory region descriptor
#[derive(Debug, Clone, Copy)]
pub struct MemoryRegion {
    pub base_address: u64,
    pub size_bytes: u64,
    pub region_type: MemoryRegionType,
    pub used_bytes: u64,
}

/// Memory region type
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MemoryRegionType {
    Kernel,
    UserHeap,
    Stack,
    FrameBuffer,
    DeviceMapped,
    PageCache,
    Free,
}

/// I/O device statistics
#[derive(Debug, Clone)]
pub struct IoDeviceStats {
    pub device_hash: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub read_ops: u64,
    pub write_ops: u64,
    pub read_rate_q16: i32,
    pub write_rate_q16: i32,
    pub queue_depth: u32,
    pub avg_latency_us: u32,
}

/// System-wide summary
#[derive(Debug, Clone, Copy)]
pub struct SystemSummary {
    pub total_processes: u32,
    pub running_processes: u32,
    pub sleeping_processes: u32,
    pub zombie_processes: u32,
    pub cpu_overall_q16: i32,
    pub memory_usage_q16: i32,
    pub swap_usage_q16: i32,
    pub uptime_seconds: u64,
    pub load_avg_1_q16: i32,
    pub load_avg_5_q16: i32,
    pub load_avg_15_q16: i32,
    pub context_switches: u64,
    pub interrupts: u64,
}

/// Persistent monitor state
struct MonitorState {
    processes: Vec<ProcessInfo>,
    cores: Vec<CoreStats>,
    memory: MemoryStats,
    memory_regions: Vec<MemoryRegion>,
    io_devices: Vec<IoDeviceStats>,
    next_pid: u32,
    sort_mode: ProcessSort,
    view_mode: ViewMode,
    filter_hash: u64,
    uptime_seconds: u64,
    context_switches: u64,
    interrupts: u64,
    load_avg_1_q16: i32,
    load_avg_5_q16: i32,
    load_avg_15_q16: i32,
    refresh_interval_ms: u32,
    timestamp_counter: u64,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static MONITOR: Mutex<Option<MonitorState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_memory() -> MemoryStats {
    MemoryStats {
        total_bytes: 1_073_741_824,
        used_bytes: 268_435_456,
        free_bytes: 805_306_368,
        cached_bytes: 134_217_728,
        buffers_bytes: 33_554_432,
        swap_total: 536_870_912,
        swap_used: 0,
        heap_used: 67_108_864,
        heap_total: 268_435_456,
        page_faults: 0,
    }
}

fn default_state() -> MonitorState {
    let mut cores = Vec::new();
    for i in 0..4 {
        cores.push(CoreStats {
            core_id: i,
            usage_q16: 0,
            user_q16: 0,
            system_q16: 0,
            idle_q16: Q16_ONE,
            iowait_q16: 0,
            frequency_mhz: 3200,
            temperature_q16: 40 * Q16_ONE,
            history: Vec::new(),
        });
    }

    let mut processes = Vec::new();
    // Seed kernel process
    processes.push(ProcessInfo {
        pid: 0,
        parent_pid: 0,
        name_hash: 0xDEAD_BEEF_0000_0001,
        state: ProcessState::Running,
        priority: Priority::Realtime,
        cpu_usage_q16: Q16_ONE / 10,
        memory_bytes: 4_194_304,
        memory_peak: 8_388_608,
        threads: 1,
        open_files: 0,
        start_time: 0,
        cpu_time_ms: 0,
        io_read_bytes: 0,
        io_write_bytes: 0,
    });
    // Init process
    processes.push(ProcessInfo {
        pid: 1,
        parent_pid: 0,
        name_hash: 0xDEAD_BEEF_0000_0002,
        state: ProcessState::Sleeping,
        priority: Priority::High,
        cpu_usage_q16: 0,
        memory_bytes: 1_048_576,
        memory_peak: 2_097_152,
        threads: 1,
        open_files: 3,
        start_time: 1,
        cpu_time_ms: 100,
        io_read_bytes: 4096,
        io_write_bytes: 2048,
    });

    let regions = vec![
        MemoryRegion {
            base_address: 0x0000_0000,
            size_bytes: 16_777_216,
            region_type: MemoryRegionType::Kernel,
            used_bytes: 4_194_304,
        },
        MemoryRegion {
            base_address: 0x0100_0000,
            size_bytes: 268_435_456,
            region_type: MemoryRegionType::UserHeap,
            used_bytes: 67_108_864,
        },
        MemoryRegion {
            base_address: 0xB000_0000,
            size_bytes: 16_777_216,
            region_type: MemoryRegionType::FrameBuffer,
            used_bytes: 16_777_216,
        },
    ];

    MonitorState {
        processes,
        cores,
        memory: default_memory(),
        memory_regions: regions,
        io_devices: Vec::new(),
        next_pid: 2,
        sort_mode: ProcessSort::CpuDesc,
        view_mode: ViewMode::Overview,
        filter_hash: 0,
        uptime_seconds: 0,
        context_switches: 0,
        interrupts: 0,
        load_avg_1_q16: 0,
        load_avg_5_q16: 0,
        load_avg_15_q16: 0,
        refresh_interval_ms: 1000,
        timestamp_counter: 1_700_000_000,
    }
}

fn next_timestamp(state: &mut MonitorState) -> u64 {
    state.timestamp_counter += 1;
    state.timestamp_counter
}

fn sort_processes(procs: &mut Vec<ProcessInfo>, mode: ProcessSort) {
    procs.sort_by(|a, b| match mode {
        ProcessSort::Pid => a.pid.cmp(&b.pid),
        ProcessSort::CpuDesc => b.cpu_usage_q16.cmp(&a.cpu_usage_q16),
        ProcessSort::CpuAsc => a.cpu_usage_q16.cmp(&b.cpu_usage_q16),
        ProcessSort::MemDesc => b.memory_bytes.cmp(&a.memory_bytes),
        ProcessSort::MemAsc => a.memory_bytes.cmp(&b.memory_bytes),
        ProcessSort::NameHash => a.name_hash.cmp(&b.name_hash),
    });
}

fn compute_overall_cpu(cores: &[CoreStats]) -> i32 {
    if cores.is_empty() { return 0; }
    let sum: i64 = cores.iter().map(|c| c.usage_q16 as i64).sum();
    (sum / cores.len() as i64) as i32
}

// ---------------------------------------------------------------------------
// Public API -- Process management
// ---------------------------------------------------------------------------

/// Register a new process
pub fn register_process(name_hash: u64, parent_pid: u32, priority: Priority) -> Result<u32, MonitorResult> {
    let mut guard = MONITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return Err(MonitorResult::IoError) };
    if state.processes.len() >= MAX_PROCESSES {
        return Err(MonitorResult::LimitReached);
    }

    let pid = state.next_pid;
    state.next_pid += 1;
    let ts = next_timestamp(state);

    state.processes.push(ProcessInfo {
        pid,
        parent_pid,
        name_hash,
        state: ProcessState::Running,
        priority,
        cpu_usage_q16: 0,
        memory_bytes: 0,
        memory_peak: 0,
        threads: 1,
        open_files: 0,
        start_time: ts,
        cpu_time_ms: 0,
        io_read_bytes: 0,
        io_write_bytes: 0,
    });
    sort_processes(&mut state.processes, state.sort_mode);
    Ok(pid)
}

/// Kill a process by PID
pub fn kill_process(pid: u32) -> MonitorResult {
    let mut guard = MONITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return MonitorResult::IoError };
    if pid == 0 { return MonitorResult::PermissionDenied; }

    if let Some(proc) = state.processes.iter_mut().find(|p| p.pid == pid) {
        proc.state = ProcessState::Dead;
        // Free memory
        let freed = proc.memory_bytes;
        proc.memory_bytes = 0;
        proc.cpu_usage_q16 = 0;
        state.memory.used_bytes = state.memory.used_bytes.saturating_sub(freed);
        state.memory.free_bytes += freed;
        MonitorResult::Success
    } else {
        MonitorResult::ProcessNotFound
    }
}

/// Remove dead processes from the list
pub fn reap_dead() -> u32 {
    let mut guard = MONITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return 0 };
    let before = state.processes.len();
    state.processes.retain(|p| p.state != ProcessState::Dead);
    (before - state.processes.len()) as u32
}

/// Update a process's CPU and memory stats
pub fn update_process_stats(pid: u32, cpu_q16: i32, memory_bytes: u64, threads: u32) -> MonitorResult {
    let mut guard = MONITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return MonitorResult::IoError };

    if let Some(proc) = state.processes.iter_mut().find(|p| p.pid == pid) {
        let old_mem = proc.memory_bytes;
        proc.cpu_usage_q16 = cpu_q16;
        proc.memory_bytes = memory_bytes;
        if memory_bytes > proc.memory_peak {
            proc.memory_peak = memory_bytes;
        }
        proc.threads = threads;
        // Update global memory tracking
        if memory_bytes > old_mem {
            let delta = memory_bytes - old_mem;
            state.memory.used_bytes += delta;
            state.memory.free_bytes = state.memory.free_bytes.saturating_sub(delta);
        } else {
            let delta = old_mem - memory_bytes;
            state.memory.used_bytes = state.memory.used_bytes.saturating_sub(delta);
            state.memory.free_bytes += delta;
        }
        sort_processes(&mut state.processes, state.sort_mode);
        MonitorResult::Success
    } else {
        MonitorResult::ProcessNotFound
    }
}

/// Update a process's I/O statistics
pub fn update_process_io(pid: u32, read_bytes: u64, write_bytes: u64) -> MonitorResult {
    let mut guard = MONITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return MonitorResult::IoError };

    if let Some(proc) = state.processes.iter_mut().find(|p| p.pid == pid) {
        proc.io_read_bytes += read_bytes;
        proc.io_write_bytes += write_bytes;
        MonitorResult::Success
    } else {
        MonitorResult::ProcessNotFound
    }
}

/// Set process state
pub fn set_process_state(pid: u32, new_state: ProcessState) -> MonitorResult {
    let mut guard = MONITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return MonitorResult::IoError };

    if let Some(proc) = state.processes.iter_mut().find(|p| p.pid == pid) {
        proc.state = new_state;
        MonitorResult::Success
    } else {
        MonitorResult::ProcessNotFound
    }
}

/// Set process priority
pub fn set_process_priority(pid: u32, priority: Priority) -> MonitorResult {
    let mut guard = MONITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return MonitorResult::IoError };
    if pid == 0 { return MonitorResult::PermissionDenied; }

    if let Some(proc) = state.processes.iter_mut().find(|p| p.pid == pid) {
        proc.priority = priority;
        MonitorResult::Success
    } else {
        MonitorResult::ProcessNotFound
    }
}

/// Get the full process list
pub fn get_processes() -> Vec<ProcessInfo> {
    let guard = MONITOR.lock();
    match guard.as_ref() {
        Some(state) => state.processes.clone(),
        None => Vec::new(),
    }
}

/// Get a single process by PID
pub fn get_process(pid: u32) -> Option<ProcessInfo> {
    let guard = MONITOR.lock();
    let state = guard.as_ref()?;
    state.processes.iter().find(|p| p.pid == pid).cloned()
}

/// Get process count
pub fn process_count() -> u32 {
    let guard = MONITOR.lock();
    match guard.as_ref() {
        Some(state) => state.processes.len() as u32,
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Public API -- CPU
// ---------------------------------------------------------------------------

/// Update per-core CPU statistics
pub fn update_core_stats(core_id: u32, user_q16: i32, system_q16: i32, idle_q16: i32, iowait_q16: i32) -> MonitorResult {
    let mut guard = MONITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return MonitorResult::IoError };

    if let Some(core) = state.cores.iter_mut().find(|c| c.core_id == core_id) {
        core.user_q16 = user_q16;
        core.system_q16 = system_q16;
        core.idle_q16 = idle_q16;
        core.iowait_q16 = iowait_q16;
        core.usage_q16 = Q16_ONE - idle_q16;
        // Record history
        core.history.push(core.usage_q16);
        if core.history.len() > MAX_HISTORY_SAMPLES {
            core.history.remove(0);
        }
        MonitorResult::Success
    } else if (core_id as usize) < MAX_CORES {
        let mut core = CoreStats {
            core_id,
            usage_q16: Q16_ONE - idle_q16,
            user_q16,
            system_q16,
            idle_q16,
            iowait_q16,
            frequency_mhz: 3200,
            temperature_q16: 40 * Q16_ONE,
            history: Vec::new(),
        };
        core.history.push(core.usage_q16);
        state.cores.push(core);
        MonitorResult::Success
    } else {
        MonitorResult::LimitReached
    }
}

/// Set core frequency
pub fn set_core_frequency(core_id: u32, freq_mhz: u32) -> MonitorResult {
    let mut guard = MONITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return MonitorResult::IoError };

    if let Some(core) = state.cores.iter_mut().find(|c| c.core_id == core_id) {
        core.frequency_mhz = freq_mhz;
        MonitorResult::Success
    } else {
        MonitorResult::ProcessNotFound
    }
}

/// Set core temperature (Q16 in degrees C)
pub fn set_core_temperature(core_id: u32, temp_q16: i32) -> MonitorResult {
    let mut guard = MONITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return MonitorResult::IoError };

    if let Some(core) = state.cores.iter_mut().find(|c| c.core_id == core_id) {
        core.temperature_q16 = temp_q16;
        MonitorResult::Success
    } else {
        MonitorResult::ProcessNotFound
    }
}

/// Get per-core stats
pub fn get_core_stats() -> Vec<CoreStats> {
    let guard = MONITOR.lock();
    match guard.as_ref() {
        Some(state) => state.cores.clone(),
        None => Vec::new(),
    }
}

/// Get overall CPU usage (average across all cores) in Q16
pub fn get_overall_cpu() -> i32 {
    let guard = MONITOR.lock();
    match guard.as_ref() {
        Some(state) => compute_overall_cpu(&state.cores),
        None => 0,
    }
}

/// Get core count
pub fn core_count() -> u32 {
    let guard = MONITOR.lock();
    match guard.as_ref() {
        Some(state) => state.cores.len() as u32,
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Public API -- Memory
// ---------------------------------------------------------------------------

/// Update memory statistics
pub fn update_memory(used: u64, cached: u64, buffers: u64, swap_used: u64) {
    let mut guard = MONITOR.lock();
    if let Some(state) = guard.as_mut() {
        state.memory.used_bytes = used;
        state.memory.free_bytes = state.memory.total_bytes.saturating_sub(used);
        state.memory.cached_bytes = cached;
        state.memory.buffers_bytes = buffers;
        state.memory.swap_used = swap_used;
    }
}

/// Get memory statistics
pub fn get_memory() -> MemoryStats {
    let guard = MONITOR.lock();
    match guard.as_ref() {
        Some(state) => state.memory,
        None => default_memory(),
    }
}

/// Get memory usage as Q16 percentage
pub fn get_memory_usage_percent() -> i32 {
    let guard = MONITOR.lock();
    match guard.as_ref() {
        Some(state) => {
            if state.memory.total_bytes == 0 { return 0; }
            q16_div(
                (state.memory.used_bytes as i32) * (Q16_100 / 1024),
                (state.memory.total_bytes / 1024) as i32,
            ).unwrap_or(0)
        }
        None => 0,
    }
}

/// Get memory regions
pub fn get_memory_regions() -> Vec<MemoryRegion> {
    let guard = MONITOR.lock();
    match guard.as_ref() {
        Some(state) => state.memory_regions.clone(),
        None => Vec::new(),
    }
}

/// Add or update a memory region
pub fn update_memory_region(base: u64, size: u64, rtype: MemoryRegionType, used: u64) -> MonitorResult {
    let mut guard = MONITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return MonitorResult::IoError };

    if let Some(region) = state.memory_regions.iter_mut().find(|r| r.base_address == base) {
        region.size_bytes = size;
        region.region_type = rtype;
        region.used_bytes = used;
        MonitorResult::Success
    } else if state.memory_regions.len() < MAX_MEMORY_REGIONS {
        state.memory_regions.push(MemoryRegion { base_address: base, size_bytes: size, region_type: rtype, used_bytes: used });
        MonitorResult::Success
    } else {
        MonitorResult::LimitReached
    }
}

// ---------------------------------------------------------------------------
// Public API -- I/O
// ---------------------------------------------------------------------------

/// Register or update an I/O device
pub fn update_io_device(device_hash: u64, read_bytes: u64, write_bytes: u64, read_ops: u64, write_ops: u64) -> MonitorResult {
    let mut guard = MONITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return MonitorResult::IoError };

    if let Some(dev) = state.io_devices.iter_mut().find(|d| d.device_hash == device_hash) {
        // Compute rates (delta-based, simplified)
        let dr = read_bytes.saturating_sub(dev.read_bytes);
        let dw = write_bytes.saturating_sub(dev.write_bytes);
        dev.read_rate_q16 = (dr as i32) << 4;
        dev.write_rate_q16 = (dw as i32) << 4;
        dev.read_bytes = read_bytes;
        dev.write_bytes = write_bytes;
        dev.read_ops = read_ops;
        dev.write_ops = write_ops;
        MonitorResult::Success
    } else if state.io_devices.len() < MAX_IO_DEVICES {
        state.io_devices.push(IoDeviceStats {
            device_hash,
            read_bytes,
            write_bytes,
            read_ops,
            write_ops,
            read_rate_q16: 0,
            write_rate_q16: 0,
            queue_depth: 0,
            avg_latency_us: 0,
        });
        MonitorResult::Success
    } else {
        MonitorResult::LimitReached
    }
}

/// Get I/O device stats
pub fn get_io_devices() -> Vec<IoDeviceStats> {
    let guard = MONITOR.lock();
    match guard.as_ref() {
        Some(state) => state.io_devices.clone(),
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Public API -- System summary
// ---------------------------------------------------------------------------

/// Update the system uptime
pub fn tick_uptime(seconds: u64) {
    let mut guard = MONITOR.lock();
    if let Some(state) = guard.as_mut() {
        state.uptime_seconds += seconds;
    }
}

/// Record context switches and interrupts
pub fn record_system_events(ctx_switches: u64, irqs: u64) {
    let mut guard = MONITOR.lock();
    if let Some(state) = guard.as_mut() {
        state.context_switches += ctx_switches;
        state.interrupts += irqs;
    }
}

/// Update load averages (Q16 values)
pub fn update_load_avg(avg_1: i32, avg_5: i32, avg_15: i32) {
    let mut guard = MONITOR.lock();
    if let Some(state) = guard.as_mut() {
        state.load_avg_1_q16 = avg_1;
        state.load_avg_5_q16 = avg_5;
        state.load_avg_15_q16 = avg_15;
    }
}

/// Get a system-wide summary snapshot
pub fn get_summary() -> SystemSummary {
    let guard = MONITOR.lock();
    match guard.as_ref() {
        Some(state) => {
            let running = state.processes.iter().filter(|p| p.state == ProcessState::Running).count() as u32;
            let sleeping = state.processes.iter().filter(|p| p.state == ProcessState::Sleeping).count() as u32;
            let zombie = state.processes.iter().filter(|p| p.state == ProcessState::Zombie).count() as u32;
            let mem_pct = if state.memory.total_bytes > 0 {
                q16_div(
                    (state.memory.used_bytes as i32) * (Q16_100 / 1024),
                    (state.memory.total_bytes / 1024) as i32,
                ).unwrap_or(0)
            } else { 0 };
            let swap_pct = if state.memory.swap_total > 0 {
                q16_div(
                    (state.memory.swap_used as i32) * (Q16_100 / 1024),
                    (state.memory.swap_total / 1024) as i32,
                ).unwrap_or(0)
            } else { 0 };
            SystemSummary {
                total_processes: state.processes.len() as u32,
                running_processes: running,
                sleeping_processes: sleeping,
                zombie_processes: zombie,
                cpu_overall_q16: compute_overall_cpu(&state.cores),
                memory_usage_q16: mem_pct,
                swap_usage_q16: swap_pct,
                uptime_seconds: state.uptime_seconds,
                load_avg_1_q16: state.load_avg_1_q16,
                load_avg_5_q16: state.load_avg_5_q16,
                load_avg_15_q16: state.load_avg_15_q16,
                context_switches: state.context_switches,
                interrupts: state.interrupts,
            }
        }
        None => SystemSummary {
            total_processes: 0, running_processes: 0, sleeping_processes: 0,
            zombie_processes: 0, cpu_overall_q16: 0, memory_usage_q16: 0,
            swap_usage_q16: 0, uptime_seconds: 0, load_avg_1_q16: 0,
            load_avg_5_q16: 0, load_avg_15_q16: 0, context_switches: 0,
            interrupts: 0,
        },
    }
}

// ---------------------------------------------------------------------------
// Public API -- View / sort controls
// ---------------------------------------------------------------------------

/// Set the process sort mode
pub fn set_sort(mode: ProcessSort) {
    let mut guard = MONITOR.lock();
    if let Some(state) = guard.as_mut() {
        state.sort_mode = mode;
        sort_processes(&mut state.processes, mode);
    }
}

/// Set the view mode
pub fn set_view(mode: ViewMode) {
    let mut guard = MONITOR.lock();
    if let Some(state) = guard.as_mut() {
        state.view_mode = mode;
    }
}

/// Get the current view mode
pub fn get_view() -> ViewMode {
    let guard = MONITOR.lock();
    match guard.as_ref() {
        Some(state) => state.view_mode,
        None => ViewMode::Overview,
    }
}

/// Set a filter (only show processes matching this name hash, 0 = no filter)
pub fn set_filter(name_hash: u64) {
    let mut guard = MONITOR.lock();
    if let Some(state) = guard.as_mut() {
        state.filter_hash = name_hash;
    }
}

/// Set the refresh interval in milliseconds
pub fn set_refresh_interval(ms: u32) {
    let mut guard = MONITOR.lock();
    if let Some(state) = guard.as_mut() {
        state.refresh_interval_ms = ms;
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the system monitor subsystem
pub fn init() {
    let mut guard = MONITOR.lock();
    *guard = Some(default_state());
    serial_println!("    System monitor ready (4 cores, process tracking)");
}
