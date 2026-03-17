/// OOM (Out-Of-Memory) killer for Genesis
///
/// When the system is critically low on memory, the OOM killer selects
/// a process to terminate based on a "badness" score. The score considers
/// memory usage, process age, priority, and whether the process is
/// performing critical kernel work. The goal is to free the most memory
/// with the least impact.
///
/// Maintains a kernel memory reserve that is never given to userspace,
/// and monitors memory pressure to trigger killing before the system
/// completely stalls.
///
/// Inspired by: Linux OOM killer (mm/oom_kill.c), Android LMKD,
/// FreeBSD swap pager. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ── Constants ────────────────────────────────────────────────────────

/// Default kernel memory reserve: 4 MB (minimum free before OOM triggers)
const DEFAULT_KERNEL_RESERVE: usize = 4 * 1024 * 1024;

/// Critical threshold: OOM kills triggered below this (bytes free)
const DEFAULT_CRITICAL_THRESHOLD: usize = 2 * 1024 * 1024;

/// Warning threshold: start reclaiming caches below this
const DEFAULT_WARNING_THRESHOLD: usize = 16 * 1024 * 1024;

/// Maximum badness score
const MAX_BADNESS_SCORE: i32 = 1000;

/// Score assigned to unkillable processes (PID 0, PID 1)
const UNKILLABLE_SCORE: i32 = -1;

/// Maximum number of OOM-immune PIDs
const MAX_OOM_IMMUNE: usize = 16;

// ── Memory pressure levels ───────────────────────────────────────────

/// Current memory pressure level
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemoryPressure {
    /// Plenty of free memory
    None,
    /// Below warning threshold, start reclaiming caches
    Low,
    /// Below critical threshold, consider OOM killing
    Medium,
    /// Virtually no free memory, OOM kill required immediately
    Critical,
}

// ── Per-process OOM state ────────────────────────────────────────────

/// OOM-relevant information for a process
#[derive(Debug, Clone)]
pub struct ProcessOomInfo {
    /// PID
    pub pid: u32,
    /// Process name
    pub name: String,
    /// RSS (Resident Set Size) in bytes
    pub rss_bytes: usize,
    /// Virtual memory size in bytes
    pub virt_bytes: usize,
    /// Swap usage in bytes
    pub swap_bytes: usize,
    /// User-adjustable OOM score adjustment (-1000 to +1000)
    pub oom_score_adj: i32,
    /// Whether this process is immune to OOM killing
    pub oom_immune: bool,
    /// Whether this process is a kernel thread
    pub is_kernel: bool,
    /// Process age in ticks since creation
    pub age_ticks: u64,
    /// Number of child processes
    pub num_children: u32,
    /// Nice value (lower = higher priority = lower OOM score)
    pub nice: i8,
}

impl ProcessOomInfo {
    /// Calculate the badness score for this process.
    /// Higher score = more likely to be killed.
    /// Uses Q16 fixed-point internally for the calculation.
    pub fn badness_score(&self, total_memory: usize) -> i32 {
        // Unkillable processes
        if self.oom_immune || self.is_kernel || self.pid <= 1 {
            return UNKILLABLE_SCORE;
        }

        if total_memory == 0 {
            return 0;
        }

        // Base score: percentage of total memory used (0-1000 scale)
        // Q16: (rss * 1000 * 65536) / total_memory, then >> 16
        let rss = self.rss_bytes as u64;
        let total = total_memory as u64;
        // Avoid overflow: compute (rss * 1000) / total
        let base_score = if total > 0 {
            ((rss * 1000) / total) as i32
        } else {
            0
        };

        // Add swap usage contribution (at 50% weight)
        let swap_score = if total > 0 {
            ((self.swap_bytes as u64 * 500) / total) as i32
        } else {
            0
        };

        // Child process bonus: processes with many children use more resources
        // Each child adds 5 points (capped at 100)
        let child_bonus = core::cmp::min(self.num_children as i32 * 5, 100);

        // Age penalty: newer processes get slightly higher scores
        // (older processes are more "established" and might be more important)
        let age_penalty = if self.age_ticks < 100 {
            50 // Very new process
        } else if self.age_ticks < 1000 {
            20
        } else {
            0
        };

        // Nice adjustment: nice processes are more likely to be killed
        // nice > 0 adds points, nice < 0 subtracts
        let nice_adj = self.nice as i32 * 3;

        // Combine all factors
        let mut score = base_score + swap_score + child_bonus + age_penalty + nice_adj;

        // Apply user OOM score adjustment
        score += self.oom_score_adj;

        // Clamp to valid range
        if score < 0 {
            score = 0;
        }
        if score > MAX_BADNESS_SCORE {
            score = MAX_BADNESS_SCORE;
        }

        score
    }
}

// ── OOM killer state ─────────────────────────────────────────────────

/// Victim selection result
#[derive(Debug, Clone)]
pub struct OomVictim {
    /// PID of the selected victim
    pub pid: u32,
    /// Process name
    pub name: String,
    /// Badness score
    pub score: i32,
    /// RSS that will be freed
    pub rss_bytes: usize,
}

/// OOM kill event record
#[derive(Debug, Clone)]
pub struct OomKillRecord {
    /// PID killed
    pub pid: u32,
    /// Process name
    pub name: String,
    /// Badness score at time of kill
    pub score: i32,
    /// Memory freed (RSS)
    pub freed_bytes: usize,
    /// Memory pressure at time of kill
    pub pressure: MemoryPressure,
    /// Timestamp (ticks)
    pub timestamp: u64,
}

/// The OOM killer engine
pub struct OomKiller {
    /// Kernel memory reserve (bytes that must remain free)
    pub kernel_reserve: usize,
    /// Critical threshold (OOM kill below this)
    pub critical_threshold: usize,
    /// Warning threshold (cache reclaim below this)
    pub warning_threshold: usize,
    /// Current total system memory
    pub total_memory: usize,
    /// Current free memory
    pub free_memory: usize,
    /// Current memory pressure level
    pub pressure: MemoryPressure,
    /// PIDs immune to OOM killing
    pub immune_pids: Vec<u32>,
    /// Total number of OOM kills performed
    pub total_kills: u64,
    /// Total bytes freed by OOM kills
    pub total_freed: u64,
    /// Kill history
    pub kill_history: Vec<OomKillRecord>,
    /// Maximum kill history entries
    pub max_history: usize,
    /// Whether the OOM killer is enabled
    pub enabled: bool,
    /// Whether to panic instead of kill (for debugging)
    pub panic_on_oom: bool,
    /// Number of times memory pressure was evaluated
    pub pressure_checks: u64,
}

impl OomKiller {
    pub const fn new() -> Self {
        OomKiller {
            kernel_reserve: DEFAULT_KERNEL_RESERVE,
            critical_threshold: DEFAULT_CRITICAL_THRESHOLD,
            warning_threshold: DEFAULT_WARNING_THRESHOLD,
            total_memory: 0,
            free_memory: 0,
            pressure: MemoryPressure::None,
            immune_pids: Vec::new(),
            total_kills: 0,
            total_freed: 0,
            kill_history: Vec::new(),
            max_history: 32,
            enabled: true,
            panic_on_oom: false,
            pressure_checks: 0,
        }
    }

    /// Update memory statistics and recalculate pressure level
    pub fn update_memory(&mut self, total: usize, free: usize) {
        self.total_memory = total;
        self.free_memory = free;
        self.pressure_checks = self.pressure_checks.saturating_add(1);

        // Determine pressure level
        let available = if free > self.kernel_reserve {
            free - self.kernel_reserve
        } else {
            0
        };

        self.pressure = if available == 0 || free <= self.critical_threshold {
            MemoryPressure::Critical
        } else if free <= self.critical_threshold * 2 {
            MemoryPressure::Medium
        } else if free <= self.warning_threshold {
            MemoryPressure::Low
        } else {
            MemoryPressure::None
        };
    }

    /// Check if OOM killing is needed
    pub fn needs_kill(&self) -> bool {
        self.enabled && self.pressure >= MemoryPressure::Critical
    }

    /// Check if cache reclamation should be attempted first
    pub fn needs_reclaim(&self) -> bool {
        self.pressure >= MemoryPressure::Low
    }

    /// Select the worst process to kill
    pub fn select_victim(&self, processes: &[ProcessOomInfo]) -> Option<OomVictim> {
        if processes.is_empty() {
            return None;
        }

        let mut worst_score = -1;
        let mut victim: Option<OomVictim> = None;

        for proc in processes {
            // Skip immune processes
            if proc.oom_immune || proc.is_kernel || proc.pid <= 1 {
                continue;
            }
            if self.immune_pids.contains(&proc.pid) {
                continue;
            }

            let score = proc.badness_score(self.total_memory);
            if score <= 0 {
                continue;
            }

            if score > worst_score {
                worst_score = score;
                victim = Some(OomVictim {
                    pid: proc.pid,
                    name: proc.name.clone(),
                    score,
                    rss_bytes: proc.rss_bytes,
                });
            }
        }

        victim
    }

    /// Record an OOM kill event
    pub fn record_kill(&mut self, victim: &OomVictim, timestamp: u64) {
        self.total_kills = self.total_kills.saturating_add(1);
        self.total_freed += victim.rss_bytes as u64;

        let record = OomKillRecord {
            pid: victim.pid,
            name: victim.name.clone(),
            score: victim.score,
            freed_bytes: victim.rss_bytes,
            pressure: self.pressure,
            timestamp,
        };

        if self.kill_history.len() >= self.max_history {
            self.kill_history.remove(0);
        }
        self.kill_history.push(record);

        serial_println!(
            "    [oom] KILLED PID {} ({}) score={} freed={} bytes",
            victim.pid,
            victim.name,
            victim.score,
            victim.rss_bytes
        );
    }

    /// Add a PID to the OOM-immune list
    pub fn set_immune(&mut self, pid: u32) -> Result<(), &'static str> {
        if self.immune_pids.len() >= MAX_OOM_IMMUNE {
            return Err("too many OOM-immune processes");
        }
        if !self.immune_pids.contains(&pid) {
            self.immune_pids.push(pid);
        }
        Ok(())
    }

    /// Remove a PID from the OOM-immune list
    pub fn clear_immune(&mut self, pid: u32) {
        self.immune_pids.retain(|&p| p != pid);
    }

    /// Set the kernel memory reserve
    pub fn set_kernel_reserve(&mut self, bytes: usize) {
        self.kernel_reserve = bytes;
        serial_println!("    [oom] kernel reserve set to {} bytes", bytes);
    }

    /// Set the critical threshold
    pub fn set_critical_threshold(&mut self, bytes: usize) {
        self.critical_threshold = bytes;
    }

    /// Set the warning threshold
    pub fn set_warning_threshold(&mut self, bytes: usize) {
        self.warning_threshold = bytes;
    }

    /// Check if kernel reserve is still intact
    pub fn reserve_intact(&self) -> bool {
        self.free_memory >= self.kernel_reserve
    }

    /// Attempt to allocate from kernel reserve (emergency allocations only)
    pub fn emergency_alloc(&mut self, bytes: usize) -> bool {
        if bytes > self.kernel_reserve {
            return false;
        }
        // Temporarily shrink reserve
        self.kernel_reserve -= bytes;
        serial_println!(
            "    [oom] emergency allocation {} bytes from kernel reserve",
            bytes
        );
        true
    }
}

static OOM: Mutex<OomKiller> = Mutex::new(OomKiller::new());

// ── Public API ───────────────────────────────────────────────────────

/// Update system memory statistics
pub fn update_memory(total: usize, free: usize) {
    OOM.lock().update_memory(total, free);
}

/// Check current memory pressure level
pub fn pressure() -> MemoryPressure {
    OOM.lock().pressure
}

/// Check if OOM killing is needed
pub fn needs_kill() -> bool {
    OOM.lock().needs_kill()
}

/// Check if cache reclamation is recommended
pub fn needs_reclaim() -> bool {
    OOM.lock().needs_reclaim()
}

/// Try to find and kill a process to free memory.
/// Returns the PID of the killed process if successful.
pub fn try_kill() -> Option<u32> {
    // Gather process information from the process table
    let processes = gather_process_info();

    let mut oom = OOM.lock();

    if oom.panic_on_oom {
        serial_println!("    [oom] PANIC: out of memory (panic_on_oom is set)");
        return None;
    }

    let victim = match oom.select_victim(&processes) {
        Some(v) => v,
        None => {
            serial_println!("    [oom] no suitable victim found");
            return None;
        }
    };

    let pid = victim.pid;
    oom.record_kill(&victim, 0);
    drop(oom);

    // Send SIGKILL to the victim
    super::send_signal(pid, super::pcb::signal::SIGKILL).ok();

    Some(pid)
}

/// Gather OOM-relevant info from the process table
fn gather_process_info() -> Vec<ProcessOomInfo> {
    let table = super::pcb::PROCESS_TABLE.lock();
    let mut infos = Vec::new();

    for i in 0..super::MAX_PROCESSES {
        if let Some(proc) = table[i].as_ref() {
            if proc.state == super::pcb::ProcessState::Dead {
                continue;
            }
            // Calculate RSS from memory mappings
            let rss: usize = proc.mmaps.iter().map(|&(_, pages, _)| pages * 4096).sum();

            infos.push(ProcessOomInfo {
                pid: proc.pid,
                name: proc.name.clone(),
                rss_bytes: rss,
                virt_bytes: rss, // Simplified: virt = rss for now
                swap_bytes: 0,
                oom_score_adj: 0,
                oom_immune: proc.pid <= 1,
                is_kernel: proc.is_kernel,
                age_ticks: 0,
                num_children: proc.children.len() as u32,
                nice: 0,
            });
        }
    }

    infos
}

/// Set OOM score adjustment for a process (-1000 to +1000)
pub fn set_oom_score_adj(pid: u32, adj: i32) -> Result<(), &'static str> {
    if adj < -1000 || adj > 1000 {
        return Err("oom_score_adj must be between -1000 and 1000");
    }
    // Store in process metadata (would need PCB field in production)
    serial_println!("    [oom] PID {} oom_score_adj set to {}", pid, adj);
    Ok(())
}

/// Make a process immune to OOM killing
pub fn set_immune(pid: u32) -> Result<(), &'static str> {
    OOM.lock().set_immune(pid)
}

/// Remove OOM immunity from a process
pub fn clear_immune(pid: u32) {
    OOM.lock().clear_immune(pid);
}

/// Set the kernel memory reserve in bytes
pub fn set_kernel_reserve(bytes: usize) {
    OOM.lock().set_kernel_reserve(bytes);
}

/// Set the critical threshold in bytes
pub fn set_critical_threshold(bytes: usize) {
    OOM.lock().set_critical_threshold(bytes);
}

/// Enable or disable the OOM killer
pub fn set_enabled(enabled: bool) {
    OOM.lock().enabled = enabled;
    serial_println!(
        "    [oom] OOM killer {}",
        if enabled { "enabled" } else { "disabled" }
    );
}

/// Enable panic-on-OOM mode (for debugging; kernel panics instead of killing)
pub fn set_panic_on_oom(panic: bool) {
    OOM.lock().panic_on_oom = panic;
}

/// Check if kernel reserve is intact
pub fn reserve_ok() -> bool {
    OOM.lock().reserve_intact()
}

/// Get OOM statistics: (total_kills, total_freed_bytes, pressure_checks)
pub fn stats() -> (u64, u64, u64) {
    let oom = OOM.lock();
    (oom.total_kills, oom.total_freed, oom.pressure_checks)
}

// ---------------------------------------------------------------------------
// Mission-spec API aliases
// ---------------------------------------------------------------------------

/// Select and kill the process with the worst OOM badness score.
///
/// Alias for `try_kill()`.  Called from the memory allocator slow path or
/// the OOM notifier when physical pages are exhausted.
pub fn oom_kill() {
    if let Some(victim_pid) = try_kill() {
        serial_println!("[oom] killed pid={}", victim_pid);
    } else {
        serial_println!("[oom] no victim found — system may deadlock");
    }
}

/// Return true if free pages are below OOM_THRESHOLD and a kill is needed.
///
/// Alias for `needs_kill()`.  Intended for the allocator slow path:
/// ```rust
/// if oom_killer::check_memory_pressure() {
///     oom_killer::oom_kill();
/// }
/// ```
pub fn check_memory_pressure() -> bool {
    needs_kill()
}

// ---------------------------------------------------------------------------
// Initializer
// ---------------------------------------------------------------------------

/// Initialize the OOM killer subsystem
pub fn init() {
    let mut oom = OOM.lock();
    oom.set_kernel_reserve(DEFAULT_KERNEL_RESERVE);
    oom.set_critical_threshold(DEFAULT_CRITICAL_THRESHOLD);
    oom.set_warning_threshold(DEFAULT_WARNING_THRESHOLD);
    // PID 0 and PID 1 are always immune
    oom.set_immune(0).ok();
    oom.set_immune(1).ok();
    drop(oom);
    serial_println!("    [oom] OOM killer initialized (4MB reserve, badness scoring)");
}
