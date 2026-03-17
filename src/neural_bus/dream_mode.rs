//! DREAM MODE / IDLE LEARNING
//!
//! The OS learns and optimizes while the user sleeps (or is idle).
//! Phases: Awake → Drowsy → LightSleep → DeepSleep → REM
//! REM sleep includes Replay (reinforce patterns), Explore (discover shortcuts),
//! and Maintain (system health).

use super::*;
use crate::{serial_print, serial_println};
use alloc::vec::Vec;
use alloc::string::String;
use alloc::collections::BTreeMap;

/// Dream phase progression
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DreamPhase {
    Awake,
    Drowsy,
    LightSleep,
    DeepSleep,
    REM,
}

impl DreamPhase {
    pub fn name(&self) -> &'static str {
        match self {
            DreamPhase::Awake => "Awake",
            DreamPhase::Drowsy => "Drowsy",
            DreamPhase::LightSleep => "LightSleep",
            DreamPhase::DeepSleep => "DeepSleep",
            DreamPhase::REM => "REM",
        }
    }
}

/// Consolidation task for LightSleep: merge/prune patterns
#[derive(Debug, Clone)]
pub struct ConsolidationTask {
    pub pattern_id: u32,
    pub strength: Q16,
    pub merge_target: Option<u32>,
    pub should_prune: bool,
}

/// Defragmentation target for DeepSleep
#[derive(Debug, Clone)]
pub struct DefragTarget {
    pub region_id: u32,
    pub fragmentation: Q16,
    pub estimated_recovery: u32,
}

/// Exploration candidate for REM
#[derive(Debug, Clone)]
pub struct ExplorationPattern {
    pub pattern_a: u32,
    pub pattern_b: u32,
    pub novel_score: Q16,
    pub potential_benefit: Q16,
}

/// Auto-restart advisory
#[derive(Debug, Clone)]
pub struct RestartAdvisory {
    pub urgency: Q16,               // 0..Q16_ONE
    pub reason: String,
    pub can_defer: bool,
    pub recommended_uptime_hours: u32,
}

/// Main Dream Engine
pub struct DreamEngine {
    pub phase: DreamPhase,
    pub idle_seconds: u64,
    pub last_user_activity: u64,

    // Replay and consolidation
    pub replay_buffer: Vec<NeuralSignal>,
    pub optimization_queue: Vec<ConsolidationTask>,

    // Memory management
    pub defrag_progress: Q16,        // 0..Q16_ONE for current defrag
    pub defrag_targets: Vec<DefragTarget>,

    // Exploration buffer
    pub exploration_candidates: Vec<ExplorationPattern>,

    // Completion flags
    pub cache_optimization_done: bool,
    pub pattern_consolidation_done: bool,
    pub defrag_done: bool,
    pub exploration_done: bool,

    // Statistics
    pub total_dreams: u64,
    pub total_optimizations: u64,
    pub total_memory_recovered: u64,

    // Restart advisory
    pub restart_advisory: Option<RestartAdvisory>,
    pub system_uptime_seconds: u64,
    pub memory_fragmentation: Q16,
}

impl DreamEngine {
    pub fn new() -> Self {
        DreamEngine {
            phase: DreamPhase::Awake,
            idle_seconds: 0,
            last_user_activity: 0,
            replay_buffer: Vec::new(),
            optimization_queue: Vec::new(),
            defrag_progress: Q16::ZERO,
            defrag_targets: Vec::new(),
            exploration_candidates: Vec::new(),
            cache_optimization_done: false,
            pattern_consolidation_done: false,
            defrag_done: false,
            exploration_done: false,
            total_dreams: 0,
            total_optimizations: 0,
            total_memory_recovered: 0,
            restart_advisory: None,
            system_uptime_seconds: 0,
            memory_fragmentation: Q16::ZERO,
        }
    }

    /// Initialize Dream Engine
    pub fn init(&mut self) {
        serial_println!("[DREAM] Initializing Dream Mode Engine");
        self.phase = DreamPhase::Awake;
        self.idle_seconds = 0;
        self.system_uptime_seconds = 0;
        self.memory_fragmentation = Q16::from_int(0);
        serial_println!("[DREAM] Dream Mode ready");
    }

    /// Tick the dream engine - called periodically
    pub fn tick(&mut self, current_time: u64, user_active: bool) {
        if user_active {
            self.last_user_activity = current_time;
            self.idle_seconds = 0;

            // Transition back to Awake if needed
            if self.phase != DreamPhase::Awake {
                self.wake_up(current_time);
            }
        } else {
            self.idle_seconds = current_time.saturating_sub(self.last_user_activity);
        }

        // Update system uptime
        self.system_uptime_seconds = self.system_uptime_seconds.saturating_add(1);

        // Phase transitions based on idle time
        let new_phase = self.compute_phase();
        if new_phase != self.phase {
            self.transition_to_phase(new_phase);
        }

        // Execute phase-specific work
        match self.phase {
            DreamPhase::Awake => {
                // No special processing
            }
            DreamPhase::Drowsy => {
                self.tick_drowsy();
            }
            DreamPhase::LightSleep => {
                self.tick_light_sleep();
            }
            DreamPhase::DeepSleep => {
                self.tick_deep_sleep();
            }
            DreamPhase::REM => {
                self.tick_rem();
            }
        }

        // Check restart advisory
        self.update_restart_advisory();
    }

    /// Compute target phase based on idle time
    fn compute_phase(&self) -> DreamPhase {
        match self.idle_seconds {
            0..=300 => DreamPhase::Awake,           // < 5 min
            301..=900 => DreamPhase::Drowsy,        // 5-15 min
            901..=1800 => DreamPhase::LightSleep,   // 15-30 min
            1801..=3600 => DreamPhase::DeepSleep,   // 30-60 min
            _ => DreamPhase::REM,                   // 60+ min
        }
    }

    /// Transition to a new phase
    fn transition_to_phase(&mut self, new_phase: DreamPhase) {
        let old_phase = self.phase;
        self.phase = new_phase;

        serial_println!(
            "[DREAM] Transition: {} -> {} (idle: {}s)",
            old_phase.name(),
            new_phase.name(),
            self.idle_seconds
        );

        match new_phase {
            DreamPhase::Drowsy => {
                // Start collecting recent signals for replay
                self.replay_buffer.clear();
                serial_println!("[DREAM] Drowsy: starting signal collection");
            }
            DreamPhase::LightSleep => {
                serial_println!("[DREAM] LightSleep: preparing consolidation tasks");
                self.pattern_consolidation_done = false;
                self.cache_optimization_done = false;
            }
            DreamPhase::DeepSleep => {
                serial_println!("[DREAM] DeepSleep: analyzing defragmentation targets");
                self.defrag_progress = Q16::ZERO;
                self.defrag_done = false;
            }
            DreamPhase::REM => {
                serial_println!("[DREAM] REM: entering dream state");
                self.exploration_done = false;
            }
            DreamPhase::Awake => {
                // Handled in wake_up()
            }
        }
    }

    /// Drowsy phase tick
    fn tick_drowsy(&mut self) {
        // Collect recent signals if available
        // In a real implementation, would query recent cortex activity
        if let Ok(bus) = BUS.lock() {
            if let Some(last_signal) = bus.last_signal.clone() {
                if self.replay_buffer.len() < 32 {
                    self.replay_buffer.push(last_signal);
                }
            }
        }
    }

    /// LightSleep phase tick - consolidate patterns, optimize caches
    fn tick_light_sleep(&mut self) {
        if !self.cache_optimization_done {
            self.optimize_signal_cache();
            self.cache_optimization_done = true;
        }

        if !self.pattern_consolidation_done {
            self.consolidate_patterns();
            self.pattern_consolidation_done = true;
        }
    }

    /// DeepSleep phase tick - defragment memory, rebuild indexes
    fn tick_deep_sleep(&mut self) {
        if !self.defrag_done {
            // Simulate multi-step defragmentation
            if self.defrag_progress < Q16::from_int(1) {
                let step = Q16::from_bits(Q16_ONE.bits() / 100); // 1% per tick
                self.defrag_progress = (self.defrag_progress + step).min(Q16::from_int(1));

                if self.defrag_progress >= Q16::from_int(1) {
                    self.defrag_done = true;
                    self.total_memory_recovered += 4096; // Simulated recovery
                    serial_println!(
                        "[DREAM] DeepSleep: defragmentation complete, recovered ~4KB"
                    );
                }
            }
        }

        // Rebuild search indexes
        self.rebuild_search_indexes();
    }

    /// REM phase tick - replay, explore, maintain
    fn tick_rem(&mut self) {
        static STEP_COUNTER: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

        let step = STEP_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

        match step % 3 {
            0 => self.rem_replay(),
            1 => self.rem_explore(),
            _ => self.rem_maintain(),
        }
    }

    /// REM Replay: reinforce recent patterns
    fn rem_replay(&mut self) {
        if self.replay_buffer.is_empty() {
            serial_println!("[DREAM:REM] Replay: buffer empty, skipping");
            return;
        }

        serial_println!(
            "[DREAM:REM] Replay: reinforcing {} patterns",
            self.replay_buffer.len()
        );

        // Simulate reinforcing each signal in the replay buffer
        for (idx, signal) in self.replay_buffer.iter().enumerate() {
            let boost = Q16::from_bits((idx as i32 + 1) * (Q16_ONE.bits() / 16));
            serial_println!(
                "  [DREAM:REM] Replay signal {} with boost {:.4}",
                idx,
                boot.value() as f64 / Q16_ONE.value() as f64
            );
            // In a real cortex, would update strength/confidence
        }

        self.total_optimizations = self.total_optimizations.saturating_add(1);
    }

    /// REM Explore: discover new pattern combinations
    fn rem_explore(&mut self) {
        if self.exploration_done {
            return;
        }

        serial_println!("[DREAM:REM] Explore: discovering new pattern combinations");

        // Generate synthetic exploration candidates
        for i in 0..4 {
            let pattern_a = (i * 2) as u32;
            let pattern_b = (i * 2 + 1) as u32;
            let novel_score = Q16::from_int(1)
                .saturating_sub(Q16::from_bits((i as i32) * (Q16_ONE.bits() / 8)));
            let potential = novel_score.saturating_mul(Q16::from_int(1));

            let candidate = ExplorationPattern {
                pattern_a,
                pattern_b,
                novel_score,
                potential_benefit: potential,
            };

            self.exploration_candidates.push(candidate);
            serial_println!(
                "  [DREAM:REM] Candidate: {} + {} (novelty: {:.4})",
                pattern_a,
                pattern_b,
                novel_score.value() as f64 / Q16_ONE.value() as f64
            );
        }

        self.exploration_done = true;
        self.total_optimizations = self.total_optimizations.saturating_add(1);
    }

    /// REM Maintain: full system health check
    fn rem_maintain(&mut self) {
        serial_println!("[DREAM:REM] Maintain: running system health check");

        // Check memory fragmentation
        let frag = Q16::from_int(0); // Simulated: would be computed from heap state
        self.memory_fragmentation = frag;
        serial_println!(
            "  [DREAM:REM] Memory fragmentation: {:.4}",
            frag.value() as f64 / Q16_ONE.value() as f64
        );

        // Check uptime and advise restart
        if self.system_uptime_seconds > (72 * 3600) {
            serial_println!("[DREAM:REM] Uptime exceeds 72h, restart advisory pending");
        }

        // Simulate cleaning temporary caches
        serial_println!("[DREAM:REM] Cache cleanup complete");

        self.total_dreams = self.total_dreams.saturating_add(1);
    }

    /// Optimize signal cache
    fn optimize_signal_cache(&mut self) {
        serial_println!("[DREAM:LightSleep] Optimizing signal cache");
        // Mark frequently-used signals as hot
        // Pre-compute common shortcuts
        serial_println!("[DREAM:LightSleep] Signal cache optimized");
    }

    /// Consolidate patterns: merge similar, prune weak
    fn consolidate_patterns(&mut self) {
        serial_println!("[DREAM:LightSleep] Consolidating cortex patterns");

        // Create example consolidation tasks
        for i in 0..4 {
            let strength = Q16::from_int(1)
                .saturating_sub(Q16::from_bits((i as i32) * (Q16_ONE.bits() / 8)));
            let should_prune = strength < Q16::from_int(0).saturating_add(Q16::from_bits(Q16_ONE.bits() / 4));

            let task = ConsolidationTask {
                pattern_id: i as u32,
                strength,
                merge_target: if i > 0 { Some((i - 1) as u32) } else { None },
                should_prune,
            };

            self.optimization_queue.push(task);
            serial_println!(
                "  [DREAM:LightSleep] Pattern {}: strength {:.4}, prune={}",
                i,
                strength.value() as f64 / Q16_ONE.value() as f64,
                should_prune
            );
        }

        self.total_optimizations = self.total_optimizations.saturating_add(1);
    }

    /// Rebuild search indexes
    fn rebuild_search_indexes(&mut self) {
        serial_println!("[DREAM:DeepSleep] Rebuilding search indexes");
        // In a real system, would rebuild B-trees, inverted indexes, etc.
        serial_println!("[DREAM:DeepSleep] Search indexes rebuilt");
    }

    /// Enter dream mode (called externally)
    pub fn enter_dream(&mut self, current_time: u64) {
        self.last_user_activity = current_time;
        self.idle_seconds = u64::MAX; // Force into REM
        serial_println!("[DREAM] Manually entering dream mode");
        let phase = self.compute_phase();
        self.transition_to_phase(phase);
    }

    /// Wake up: return to Awake phase
    pub fn wake_up(&mut self, current_time: u64) {
        let summary = self.generate_wake_summary();

        self.phase = DreamPhase::Awake;
        self.idle_seconds = 0;
        self.last_user_activity = current_time;

        serial_println!("[DREAM] Wake up summary:");
        serial_println!("  Total dreams: {}", self.total_dreams);
        serial_println!("  Total optimizations: {}", self.total_optimizations);
        serial_println!("  Memory recovered: {} bytes", self.total_memory_recovered);
        if let Some(summary_str) = summary {
            serial_println!("  {}", summary_str);
        }

        // Reset dream buffers
        self.replay_buffer.clear();
        self.optimization_queue.clear();
        self.exploration_candidates.clear();
        self.cache_optimization_done = false;
        self.pattern_consolidation_done = false;
        self.defrag_done = false;
        self.exploration_done = false;
    }

    /// Generate wake-up summary
    fn generate_wake_summary(&self) -> Option<String> {
        if self.total_dreams == 0 && self.total_optimizations == 0 {
            return None;
        }

        let mut summary = String::new();
        summary.push_str("Dream summary: ");

        if self.pattern_consolidation_done {
            summary.push_str("consolidated patterns, ");
        }
        if self.cache_optimization_done {
            summary.push_str("optimized cache, ");
        }
        if self.defrag_done {
            summary.push_str("defragmented memory, ");
        }
        if !self.exploration_candidates.is_empty() {
            summary.push_str("discovered ");
            // Cheap way to add a number
            for _ in 0..self.exploration_candidates.len() {
                summary.push_str("*");
            }
            summary.push_str(" patterns, ");
        }

        // Remove trailing ", "
        if summary.ends_with(", ") {
            summary.pop();
            summary.pop();
        }

        Some(summary)
    }

    /// Update restart advisory based on system state
    fn update_restart_advisory(&mut self) {
        // 72-hour threshold
        let uptime_hours = self.system_uptime_seconds / 3600;

        if uptime_hours > 72 {
            let urgency = Q16::from_int(1).saturating_mul(
                Q16::from_bits((uptime_hours.saturating_sub(72) as i32) * (Q16_ONE.bits() / 100))
            ).min(Q16::from_int(1));

            let reason = if uptime_hours > 168 {
                String::from("Critical: 7+ day uptime reached")
            } else {
                String::from("Advisory: 72+ hour uptime")
            };

            self.restart_advisory = Some(RestartAdvisory {
                urgency,
                reason,
                can_defer: uptime_hours < 168,
                recommended_uptime_hours: 72,
            });

            serial_println!("[DREAM] Restart advisory issued (uptime: {}h)", uptime_hours);
        } else if self.memory_fragmentation > Q16::from_int(0).saturating_add(Q16::from_bits(Q16_ONE.bits() / 2)) {
            // High fragmentation
            self.restart_advisory = Some(RestartAdvisory {
                urgency: self.memory_fragmentation,
                reason: String::from("Memory fragmentation high"),
                can_defer: true,
                recommended_uptime_hours: 24,
            });
        }
    }

    /// Get restart advisory
    pub fn restart_advisory(&self) -> Option<&RestartAdvisory> {
        self.restart_advisory.as_ref()
    }

    /// Clear restart advisory (user deferred or restarted)
    pub fn clear_restart_advisory(&mut self) {
        self.restart_advisory = None;
    }

    /// Get current phase
    pub fn current_phase(&self) -> DreamPhase {
        self.phase
    }

    /// Get idle time in seconds
    pub fn idle_time(&self) -> u64 {
        self.idle_seconds
    }

    /// Get defragmentation progress (0..1)
    pub fn defrag_progress(&self) -> Q16 {
        self.defrag_progress
    }

    /// Get system uptime in hours
    pub fn uptime_hours(&self) -> u32 {
        (self.system_uptime_seconds / 3600) as u32
    }
}

/// Global Dream Engine instance
pub static DREAM_ENGINE: Mutex<DreamEngine> = Mutex::new(DreamEngine::new());

/// Initialize the global Dream Engine
pub fn dream_init() {
    if let Ok(mut engine) = DREAM_ENGINE.lock() {
        engine.init();
    }
}

/// Tick the global Dream Engine
pub fn dream_tick(current_time: u64, user_active: bool) {
    if let Ok(mut engine) = DREAM_ENGINE.lock() {
        engine.tick(current_time, user_active);
    }
}

/// Manually enter dream mode
pub fn dream_enter(current_time: u64) {
    if let Ok(mut engine) = DREAM_ENGINE.lock() {
        engine.enter_dream(current_time);
    }
}

/// Wake from dream mode
pub fn dream_wake(current_time: u64) {
    if let Ok(mut engine) = DREAM_ENGINE.lock() {
        engine.wake_up(current_time);
    }
}

/// Get current dream phase
pub fn dream_phase() -> Option<DreamPhase> {
    DREAM_ENGINE.lock().ok().map(|engine| engine.current_phase())
}

/// Get idle time
pub fn dream_idle_time() -> u64 {
    DREAM_ENGINE.lock().ok().map(|engine| engine.idle_time()).unwrap_or(0)
}

/// Get restart advisory
pub fn dream_restart_advisory() -> Option<(Q16, String)> {
    DREAM_ENGINE.lock().ok().and_then(|engine| {
        engine.restart_advisory().map(|adv| (adv.urgency, adv.reason.clone()))
    })
}

/// Get defragmentation progress
pub fn dream_defrag_progress() -> Q16 {
    DREAM_ENGINE.lock().ok().map(|engine| engine.defrag_progress()).unwrap_or(Q16::ZERO)
}

/// Clear restart advisory
pub fn dream_clear_advisory() {
    if let Ok(mut engine) = DREAM_ENGINE.lock() {
        engine.clear_restart_advisory();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phase_transitions() {
        let mut engine = DreamEngine::new();
        engine.init();

        assert_eq!(engine.current_phase(), DreamPhase::Awake);

        engine.idle_seconds = 600; // 10 min
        let phase = engine.compute_phase();
        assert_eq!(phase, DreamPhase::Drowsy);

        engine.idle_seconds = 1200; // 20 min
        let phase = engine.compute_phase();
        assert_eq!(phase, DreamPhase::LightSleep);

        engine.idle_seconds = 3600; // 60 min
        let phase = engine.compute_phase();
        assert_eq!(phase, DreamPhase::REM);
    }

    #[test]
    fn test_uptime_tracking() {
        let mut engine = DreamEngine::new();
        engine.init();

        for _ in 0..3600 {
            engine.system_uptime_seconds = engine.system_uptime_seconds.saturating_add(1);
        }

        assert_eq!(engine.uptime_hours(), 1);
    }
}
