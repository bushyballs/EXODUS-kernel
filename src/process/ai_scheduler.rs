/// AI-powered process scheduling for Genesis
///
/// Predictive resource allocation, smart priority adjustment,
/// anomaly detection for runaway processes, app launch prediction.
///
/// All values use integer math (per-mille = parts per thousand instead of
/// floating-point percentages). CPU values are 0-1000 (per-mille).
///
/// Inspired by: Android LMKD, iOS Jetsam, Google EEVDF. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Process importance classification by AI
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProcessImportance {
    Foreground,
    Visible,
    Service,
    Background,
    Cached,
    Empty,
}

/// Resource prediction for a process (all integer, no floats)
pub struct ResourcePrediction {
    pub pid: u32,
    /// Predicted CPU usage in per-mille (0-1000)
    pub predicted_cpu_permille: u32,
    pub predicted_memory_mb: u32,
    /// Predicted I/O rate in KB/s
    pub predicted_io_kbps: u32,
    pub predicted_duration_ms: u64,
    /// Confidence in per-mille (0-1000)
    pub confidence_permille: u32,
}

/// App launch prediction (integer only)
pub struct LaunchPrediction {
    pub app_name: String,
    /// Launch probability in per-mille (0-1000)
    pub probability_permille: u32,
    pub typical_memory_mb: u32,
    /// Typical CPU in per-mille (0-1000)
    pub typical_cpu_permille: u32,
}

/// Process health status from AI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessHealth {
    Healthy,
    HighCpu,
    HighMemory,
    Stalled,
    Thrashing,
    Runaway,
    Zombie,
}

/// AI scheduler engine (all integer math, no floats)
pub struct AiSchedulerEngine {
    pub enabled: bool,
    pub process_history: BTreeMap<u32, ProcessHistory>,
    pub launch_patterns: Vec<LaunchPattern>,
    pub importance_overrides: BTreeMap<u32, ProcessImportance>,
    pub resource_predictions: Vec<ResourcePrediction>,
    pub prewarmed_apps: Vec<String>,
    pub total_adjustments: u64,
    pub total_kills: u64,
    /// Memory pressure threshold in per-mille (e.g., 850 = 85.0%)
    pub memory_pressure_permille: u32,
    /// CPU pressure threshold in per-mille (e.g., 900 = 90.0%)
    pub cpu_pressure_permille: u32,
}

pub struct ProcessHistory {
    pub pid: u32,
    pub name: String,
    /// Average CPU usage in per-mille (0-1000)
    pub avg_cpu_permille: u32,
    /// Peak CPU usage in per-mille
    pub peak_cpu_permille: u32,
    pub avg_memory_mb: u32,
    pub peak_memory_mb: u32,
    /// I/O rate in KB/s
    pub io_rate_kbps: u32,
    pub samples: u32,
    pub health: ProcessHealth,
    pub importance: ProcessImportance,
    pub last_foreground: u64,
}

pub struct LaunchPattern {
    pub app_name: String,
    pub hour: u8,
    pub day: u8,
    pub frequency: u32,
    pub avg_memory_mb: u32,
}

impl AiSchedulerEngine {
    const fn new() -> Self {
        AiSchedulerEngine {
            enabled: true,
            process_history: BTreeMap::new(),
            launch_patterns: Vec::new(),
            importance_overrides: BTreeMap::new(),
            resource_predictions: Vec::new(),
            prewarmed_apps: Vec::new(),
            total_adjustments: 0,
            total_kills: 0,
            memory_pressure_permille: 850,
            cpu_pressure_permille: 900,
        }
    }

    /// Update process metrics and detect anomalies.
    /// `cpu_permille`: CPU usage in per-mille (0-1000).
    /// `io_kbps`: I/O rate in KB/s.
    pub fn update_process(
        &mut self,
        pid: u32,
        name: &str,
        cpu_permille: u32,
        memory_mb: u32,
        io_kbps: u32,
    ) {
        let entry = self
            .process_history
            .entry(pid)
            .or_insert_with(|| ProcessHistory {
                pid,
                name: String::from(name),
                avg_cpu_permille: 0,
                peak_cpu_permille: 0,
                avg_memory_mb: 0,
                peak_memory_mb: 0,
                io_rate_kbps: 0,
                samples: 0,
                health: ProcessHealth::Healthy,
                importance: ProcessImportance::Background,
                last_foreground: 0,
            });

        entry.samples += 1;
        let n = entry.samples;

        // Incremental average using integer math:
        // new_avg = old_avg + (new_val - old_avg) / n
        if n > 1 {
            let diff_cpu = cpu_permille as i64 - entry.avg_cpu_permille as i64;
            entry.avg_cpu_permille = (entry.avg_cpu_permille as i64 + diff_cpu / n as i64) as u32;

            let diff_mem = memory_mb as i64 - entry.avg_memory_mb as i64;
            entry.avg_memory_mb = (entry.avg_memory_mb as i64 + diff_mem / n as i64) as u32;
        } else {
            entry.avg_cpu_permille = cpu_permille;
            entry.avg_memory_mb = memory_mb;
        }

        if cpu_permille > entry.peak_cpu_permille {
            entry.peak_cpu_permille = cpu_permille;
        }
        if memory_mb > entry.peak_memory_mb {
            entry.peak_memory_mb = memory_mb;
        }
        entry.io_rate_kbps = io_kbps;

        // Detect health issues (all comparisons in per-mille)
        entry.health = if cpu_permille > 950 && entry.samples > 10 {
            ProcessHealth::Runaway
        } else if cpu_permille > 800 {
            ProcessHealth::HighCpu
        } else if memory_mb > entry.avg_memory_mb.saturating_mul(3) && entry.samples > 5 {
            ProcessHealth::Thrashing
        } else if memory_mb > entry.peak_memory_mb.saturating_mul(2) {
            ProcessHealth::HighMemory
        } else {
            ProcessHealth::Healthy
        };
    }

    /// Get AI-recommended priority for a process
    pub fn recommend_priority(&self, pid: u32) -> Option<ProcessImportance> {
        if let Some(over) = self.importance_overrides.get(&pid) {
            return Some(*over);
        }
        self.process_history.get(&pid).map(|h| h.importance)
    }

    /// Set process as foreground (highest importance)
    pub fn set_foreground(&mut self, pid: u32) {
        if let Some(history) = self.process_history.get_mut(&pid) {
            history.importance = ProcessImportance::Foreground;
            history.last_foreground = crate::time::clock::unix_time();
        }
    }

    /// Get processes to kill under memory pressure (ordered by importance)
    pub fn get_kill_candidates(&self, needed_mb: u32) -> Vec<(u32, u32)> {
        let mut candidates: Vec<(u32, u32, ProcessImportance)> = self
            .process_history
            .iter()
            .map(|(&pid, h)| (pid, h.avg_memory_mb, h.importance))
            .collect();

        // Sort by importance (kill least important first)
        candidates.sort_by(|a, b| b.2.cmp(&a.2));

        let mut result = Vec::new();
        let mut freed = 0u32;
        for (pid, mem, _) in candidates {
            if freed >= needed_mb {
                break;
            }
            result.push((pid, mem));
            freed += mem;
        }
        result
    }

    /// Predict which apps will be launched and prewarm them
    pub fn predict_launches(&mut self) -> Vec<LaunchPrediction> {
        let now = crate::time::clock::unix_time();
        let hour = ((now / 3600) % 24) as u8;
        let _day = ((now / 86400) % 7) as u8;

        let mut predictions: Vec<LaunchPrediction> = self
            .launch_patterns
            .iter()
            .filter(|p| p.hour == hour)
            .map(|p| {
                // probability = frequency / 30, clamped to 1000 per-mille
                let prob = ((p.frequency as u64 * 1000) / 30).min(1000) as u32;
                LaunchPrediction {
                    app_name: p.app_name.clone(),
                    probability_permille: prob,
                    typical_memory_mb: p.avg_memory_mb,
                    typical_cpu_permille: 50, // 5.0% expressed as 50 per-mille
                }
            })
            .collect();

        predictions.sort_by(|a, b| b.probability_permille.cmp(&a.probability_permille));
        predictions.truncate(3);
        predictions
    }

    /// Record an app launch for pattern learning
    pub fn record_launch(&mut self, app_name: &str, memory_mb: u32) {
        let now = crate::time::clock::unix_time();
        let hour = ((now / 3600) % 24) as u8;
        let day = ((now / 86400) % 7) as u8;

        if let Some(pattern) = self
            .launch_patterns
            .iter_mut()
            .find(|p| p.app_name == app_name && p.hour == hour && p.day == day)
        {
            pattern.frequency += 1;
            pattern.avg_memory_mb = (pattern.avg_memory_mb + memory_mb) / 2;
        } else {
            self.launch_patterns.push(LaunchPattern {
                app_name: String::from(app_name),
                hour,
                day,
                frequency: 1,
                avg_memory_mb: memory_mb,
            });
        }
    }

    /// Get list of runaway/unhealthy processes
    pub fn get_unhealthy(&self) -> Vec<(u32, ProcessHealth)> {
        self.process_history
            .iter()
            .filter(|(_, h)| !matches!(h.health, ProcessHealth::Healthy))
            .map(|(&pid, h)| (pid, h.health))
            .collect()
    }
}

static AI_SCHED: Mutex<AiSchedulerEngine> = Mutex::new(AiSchedulerEngine::new());

pub fn init() {
    crate::serial_println!("    [ai-sched] AI scheduler initialized (prediction, health, LMKD)");
}

pub fn update_process(pid: u32, name: &str, cpu_permille: u32, mem_mb: u32, io_kbps: u32) {
    AI_SCHED
        .lock()
        .update_process(pid, name, cpu_permille, mem_mb, io_kbps);
}

pub fn set_foreground(pid: u32) {
    AI_SCHED.lock().set_foreground(pid);
}

pub fn record_launch(app: &str, mem_mb: u32) {
    AI_SCHED.lock().record_launch(app, mem_mb);
}

pub fn get_unhealthy() -> Vec<(u32, ProcessHealth)> {
    AI_SCHED.lock().get_unhealthy()
}
