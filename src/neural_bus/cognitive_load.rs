use super::*;
use crate::{serial_print, serial_println};
use alloc::vec::Vec;
use alloc::string::String;
use alloc::collections::BTreeMap;

/// Cognitive state enumeration for user's current mental load status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CognitiveState {
    LowLoad,      // Relaxed, minimal cognitive demand
    ModerateLoad, // Normal work, manageable complexity
    HighLoad,     // Significant cognitive demand, near limits
    Overloaded,   // Exceeds user capacity, errors increasing
    FlowState,    // Optimal focus, sustained engagement (don't interrupt!)
    Recovery,     // Recovering from overload, reduced capacity
}

/// Detailed metrics driving cognitive load assessment.
#[derive(Debug, Clone)]
pub struct CognitiveMetrics {
    /// Context switching frequency (higher = more load). Range [0, Q16_ONE]
    pub task_switching_rate: Q16,
    /// Information density on screen (higher = more load). Range [0, Q16_ONE]
    pub information_density: Q16,
    /// Active windows/apps count (higher = more load). Range [0, Q16_ONE]
    pub multitasking_level: Q16,
    /// User response latency to prompts (slower = more load). Range [0, Q16_ONE]
    pub response_latency: Q16,
    /// Error rate: typos, misclicks, mistakes (higher = more load). Range [0, Q16_ONE]
    pub error_rate: Q16,
    /// Estimated reading speed from scroll behavior. Range [0, Q16_ONE]
    pub reading_speed: Q16,
    /// Dwell time on individual UI elements (less dwell = more load). Range [0, Q16_ONE]
    pub dwell_time: Q16,
}

impl Default for CognitiveMetrics {
    fn default() -> Self {
        Self {
            task_switching_rate: Q16::ZERO,
            information_density: Q16::ZERO,
            multitasking_level: Q16::ZERO,
            response_latency: Q16::ZERO,
            error_rate: Q16::ZERO,
            reading_speed: Q16::from_int(1),
            dwell_time: Q16::from_int(1),
        }
    }
}

/// Suggested responses to cognitive overload.
#[derive(Debug, Clone)]
pub struct CognitiveResponse {
    /// Should hide non-critical notifications.
    pub hide_notifications: bool,
    /// Should simplify UI (hide secondary elements, reduce visual complexity).
    pub simplify_ui: bool,
    /// Apps suggested for closing to reduce cognitive load.
    pub suggested_close_apps: Vec<String>,
    /// Should offer/activate Focus Mode.
    pub enter_focus_mode: bool,
    /// Optional break suggestion with recommended duration.
    pub break_suggestion: Option<String>,
}

/// Advisory for system restart timing relative to cognitive load.
#[derive(Debug, Clone)]
pub struct RestartAdvisory {
    /// Urgency of restart (0=not needed, Q16_ONE=critical). Range [0, Q16_ONE]
    pub urgency: Q16,
    /// Reason for restart (e.g., "memory pressure", "pending updates").
    pub reason: String,
    /// Optimal timing (e.g., "after current task", "during lunch break").
    pub optimal_timing: String,
}

/// Core cognitive load engine: tracks, detects, and responds to user mental load.
pub struct CognitiveLoadEngine {
    /// Current cognitive state.
    current_state: CognitiveState,
    /// Normalized load score (0 = relaxed, Q16_ONE = overloaded).
    load_score: Q16,
    /// Detailed metrics driving assessment.
    metrics: CognitiveMetrics,
    /// History of (timestamp, state) transitions (last 100 entries).
    state_history: Vec<(u64, CognitiveState)>,
    /// Cumulative minutes in flow state.
    flow_state_minutes: u32,
    /// Count of overload events triggered.
    overload_events: u32,
    /// Current number of active applications.
    active_apps: u32,
    /// Current number of open windows.
    open_windows: u32,
    /// Pending notifications awaiting delivery.
    pending_notifications: u32,
    /// Cumulative overload minutes today (for burnout prevention).
    daily_overload_minutes: u32,
    /// Total cognitive assessments performed.
    total_assessments: u64,
    /// Last timestamp of assessment.
    last_assessment_tick: u64,
    /// System health metrics (from auto-maintenance integration).
    system_restart_urgency: Q16,
}

impl Default for CognitiveLoadEngine {
    fn default() -> Self {
        Self {
            current_state: CognitiveState::LowLoad,
            load_score: Q16::ZERO,
            metrics: CognitiveMetrics::default(),
            state_history: Vec::new(),
            flow_state_minutes: 0,
            overload_events: 0,
            active_apps: 0,
            open_windows: 0,
            pending_notifications: 0,
            daily_overload_minutes: 0,
            total_assessments: 0,
            last_assessment_tick: 0,
            system_restart_urgency: Q16::ZERO,
        }
    }
}

impl CognitiveLoadEngine {
    /// Initialize the cognitive load engine.
    pub fn init() -> Self {
        let mut engine = Self::default();
        engine.total_assessments = 1;
        engine
    }

    /// Update cognitive state; call periodically (e.g., every 100ms or on input).
    pub fn tick(&mut self, tick_count: u64) {
        // Calculate new load score from current metrics.
        self.load_score = self.compute_load_score();

        // Update state based on load score and flow detection.
        let new_state = self.determine_state();

        // Record state transition if changed.
        if new_state != self.current_state {
            self.record_state_transition(tick_count, new_state);
            self.current_state = new_state;
        }

        // Update flow state and recovery tracking.
        match self.current_state {
            CognitiveState::FlowState => {
                if tick_count > self.last_assessment_tick {
                    let delta_ticks = tick_count - self.last_assessment_tick;
                    // Assume ~100ms per tick; convert to minutes
                    let delta_minutes = (delta_ticks / 600) as u32; // 600 ticks ~ 1 minute
                    self.flow_state_minutes = self.flow_state_minutes.saturating_add(delta_minutes);
                }
            }
            CognitiveState::Overloaded => {
                if tick_count > self.last_assessment_tick {
                    let delta_ticks = tick_count - self.last_assessment_tick;
                    let delta_minutes = (delta_ticks / 600) as u32;
                    self.daily_overload_minutes = self.daily_overload_minutes.saturating_add(delta_minutes);
                    if self.daily_overload_minutes > 60 {
                        // Burnout risk: user has been overloaded >60 minutes today
                        // This will be suggested in cognitive_response()
                    }
                }
            }
            _ => {}
        }

        self.last_assessment_tick = tick_count;
        self.total_assessments = self.total_assessments.saturating_add(1);
    }

    /// Compute normalized load score as weighted sum of metrics.
    fn compute_load_score(&self) -> Q16 {
        let m = &self.metrics;

        // Weights: task_switching=0.25, info_density=0.20, multitasking=0.20,
        //          error_rate=0.15, response_latency=0.10, inverse_dwell=0.10
        let w_switching = Q16::from_rational(25, 100);
        let w_density = Q16::from_rational(20, 100);
        let w_multitask = Q16::from_rational(20, 100);
        let w_error = Q16::from_rational(15, 100);
        let w_latency = Q16::from_rational(10, 100);
        let w_dwell = Q16::from_rational(10, 100);

        let mut score = Q16::ZERO;

        // Switching: direct contribution
        score = score + (m.task_switching_rate * w_switching);

        // Info density: direct contribution
        score = score + (m.information_density * w_density);

        // Multitasking: direct contribution
        score = score + (m.multitasking_level * w_multitask);

        // Error rate: direct contribution
        score = score + (m.error_rate * w_error);

        // Response latency: direct contribution (slow = high load)
        score = score + (m.response_latency * w_latency);

        // Dwell time: inverse contribution (low dwell = high load)
        let inverse_dwell = if m.dwell_time > Q16::ZERO {
            Q16::ONE - m.dwell_time
        } else {
            Q16::ONE
        };
        score = score + (inverse_dwell * w_dwell);

        // Clamp to [0, Q16_ONE]
        if score > Q16::ONE {
            Q16::ONE
        } else {
            score
        }
    }

    /// Determine cognitive state based on load score and flow detection.
    fn determine_state(&self) -> CognitiveState {
        let load = self.load_score;
        let switching = self.metrics.task_switching_rate;
        let dwell = self.metrics.dwell_time;

        // Flow State: Low switching + moderate load + sustained focus
        if switching < Q16::from_rational(25, 100)
            && load >= Q16::from_rational(30, 100)
            && load <= Q16::from_rational(70, 100)
            && dwell > Q16::from_rational(60, 100)
        {
            return CognitiveState::FlowState;
        }

        // Overloaded: High load and/or high error rate
        if load > Q16::from_rational(85, 100) || self.metrics.error_rate > Q16::from_rational(50, 100) {
            return CognitiveState::Overloaded;
        }

        // Recovery: Transitioning from overload
        if self.current_state == CognitiveState::Overloaded && load > Q16::from_rational(50, 100) {
            return CognitiveState::Recovery;
        }

        // HighLoad: Significant cognitive demand
        if load > Q16::from_rational(65, 100) {
            return CognitiveState::HighLoad;
        }

        // ModerateLoad: Normal work
        if load > Q16::from_rational(35, 100) {
            return CognitiveState::ModerateLoad;
        }

        // LowLoad: Relaxed
        CognitiveState::LowLoad
    }

    /// Record a state transition in history (keep last 100).
    fn record_state_transition(&mut self, tick_count: u64, new_state: CognitiveState) {
        self.state_history.push((tick_count, new_state));
        if self.state_history.len() > 100 {
            self.state_history.remove(0);
        }
        if new_state == CognitiveState::Overloaded {
            self.overload_events = self.overload_events.saturating_add(1);
        }
    }

    /// Get current cognitive load score (0 to Q16_ONE).
    pub fn current_load(&self) -> Q16 {
        self.load_score
    }

    /// Get current cognitive state.
    pub fn current_state(&self) -> CognitiveState {
        self.current_state
    }

    /// Update active applications count.
    pub fn set_active_apps(&mut self, count: u32) {
        self.active_apps = count;
        self.metrics.multitasking_level = Q16::from_int(count as i32).min(Q16::ONE);
    }

    /// Update open windows count.
    pub fn set_open_windows(&mut self, count: u32) {
        self.open_windows = count;
    }

    /// Update pending notifications count.
    pub fn set_pending_notifications(&mut self, count: u32) {
        self.pending_notifications = count;
    }

    /// Update task switching rate (0 = no switching, Q16_ONE = constant switching).
    pub fn set_task_switching_rate(&mut self, rate: Q16) {
        self.metrics.task_switching_rate = rate.min(Q16::ONE);
    }

    /// Update information density (0 = sparse, Q16_ONE = dense).
    pub fn set_information_density(&mut self, density: Q16) {
        self.metrics.information_density = density.min(Q16::ONE);
    }

    /// Update user response latency (0 = instant, Q16_ONE = very slow).
    pub fn set_response_latency(&mut self, latency: Q16) {
        self.metrics.response_latency = latency.min(Q16::ONE);
    }

    /// Update error rate (0 = no errors, Q16_ONE = all inputs erroneous).
    pub fn set_error_rate(&mut self, rate: Q16) {
        self.metrics.error_rate = rate.min(Q16::ONE);
    }

    /// Update dwell time (0 = instant, Q16_ONE = very sustained).
    pub fn set_dwell_time(&mut self, dwell: Q16) {
        self.metrics.dwell_time = dwell.min(Q16::ONE);
    }

    /// Update reading speed estimate (0 = not reading, Q16_ONE = fast).
    pub fn set_reading_speed(&mut self, speed: Q16) {
        self.metrics.reading_speed = speed.min(Q16::ONE);
    }

    /// Set system restart urgency from auto-maintenance module.
    pub fn set_system_restart_urgency(&mut self, urgency: Q16) {
        self.system_restart_urgency = urgency.min(Q16::ONE);
    }

    /// Generate cognitive response: recommendations for UI/notification adjustments.
    pub fn respond(&self) -> CognitiveResponse {
        let mut response = CognitiveResponse {
            hide_notifications: false,
            simplify_ui: false,
            suggested_close_apps: Vec::new(),
            enter_focus_mode: false,
            break_suggestion: None,
        };

        match self.current_state {
            CognitiveState::Overloaded => {
                response.hide_notifications = true;
                response.simplify_ui = true;
                response.enter_focus_mode = true;
                if self.daily_overload_minutes > 60 {
                    response.break_suggestion = Some("Take a 15-minute break to recover.".to_string());
                }
                // Suggest closing least-used apps
                if self.active_apps > 4 {
                    response.suggested_close_apps.push("Background app 1".to_string());
                    response.suggested_close_apps.push("Background app 2".to_string());
                }
            }
            CognitiveState::HighLoad => {
                response.hide_notifications = true;
                response.simplify_ui = false; // Keep UI normal
                if self.pending_notifications > 5 {
                    response.hide_notifications = true;
                }
            }
            CognitiveState::FlowState => {
                // Never interrupt during flow state
                response.hide_notifications = true;
            }
            CognitiveState::Recovery => {
                response.simplify_ui = true;
                response.break_suggestion = Some("Light activity recommended; avoid heavy tasks.".to_string());
            }
            CognitiveState::ModerateLoad | CognitiveState::LowLoad => {
                // Normal operation
            }
        }

        response
    }

    /// Check if system restart is advisable given current cognitive state.
    /// Returns RestartAdvisory with urgency and optimal timing.
    pub fn restart_check(&self) -> RestartAdvisory {
        let mut advisory = RestartAdvisory {
            urgency: Q16::ZERO,
            reason: String::new(),
            optimal_timing: "When convenient".to_string(),
        };

        // Only recommend restart if user is not in FlowState
        if self.current_state == CognitiveState::FlowState {
            advisory.urgency = Q16::ZERO;
            advisory.reason = "Deferring: user in flow state".to_string();
            advisory.optimal_timing = "After flow state ends".to_string();
            return advisory;
        }

        // If system restart is urgent AND user is in LowLoad or moderation, recommend now
        if self.system_restart_urgency > Q16::from_rational(70, 100) {
            match self.current_state {
                CognitiveState::LowLoad => {
                    advisory.urgency = self.system_restart_urgency;
                    advisory.reason = "System maintenance needed".to_string();
                    advisory.optimal_timing = "Now (low cognitive load detected)".to_string();
                }
                CognitiveState::ModerateLoad => {
                    advisory.urgency = self.system_restart_urgency * Q16::from_rational(70, 100);
                    advisory.reason = "System maintenance pending".to_string();
                    advisory.optimal_timing = "After current task".to_string();
                }
                CognitiveState::HighLoad | CognitiveState::Overloaded | CognitiveState::Recovery => {
                    advisory.urgency = Q16::ZERO;
                    advisory.reason = "Deferring: user cognitive load too high".to_string();
                    advisory.optimal_timing = "Once load decreases".to_string();
                }
                CognitiveState::FlowState => {
                    // Already handled above
                }
            }
        }

        advisory
    }

    /// Get diagnostic summary (for debugging/UI display).
    pub fn diagnostic_summary(&self) -> (u64, u32, u32, u32, u32) {
        (
            self.total_assessments,
            self.overload_events,
            self.flow_state_minutes,
            self.daily_overload_minutes,
            self.active_apps,
        )
    }
}

/// Global cognitive load engine instance wrapped in Mutex.
pub static COGNITIVE_ENGINE: Mutex<CognitiveLoadEngine> = Mutex::new(CognitiveLoadEngine {
    current_state: CognitiveState::LowLoad,
    load_score: Q16::ZERO,
    metrics: CognitiveMetrics {
        task_switching_rate: Q16::ZERO,
        information_density: Q16::ZERO,
        multitasking_level: Q16::ZERO,
        response_latency: Q16::ZERO,
        error_rate: Q16::ZERO,
        reading_speed: Q16::ONE,
        dwell_time: Q16::ONE,
    },
    state_history: Vec::new(),
    flow_state_minutes: 0,
    overload_events: 0,
    active_apps: 0,
    open_windows: 0,
    pending_notifications: 0,
    daily_overload_minutes: 0,
    total_assessments: 0,
    last_assessment_tick: 0,
    system_restart_urgency: Q16::ZERO,
});

/// Public API: Initialize the cognitive load engine.
pub fn init_cognitive_engine() {
    let mut engine = COGNITIVE_ENGINE.lock();
    *engine = CognitiveLoadEngine::init();
}

/// Public API: Update the cognitive engine (call periodically).
pub fn tick_cognitive_engine(tick_count: u64) {
    let mut engine = COGNITIVE_ENGINE.lock();
    engine.tick(tick_count);
}

/// Public API: Get current cognitive load (0 to Q16_ONE).
pub fn get_cognitive_load() -> Q16 {
    let engine = COGNITIVE_ENGINE.lock();
    engine.current_load()
}

/// Public API: Get current cognitive state.
pub fn get_cognitive_state() -> CognitiveState {
    let engine = COGNITIVE_ENGINE.lock();
    engine.current_state()
}

/// Public API: Get recommended cognitive response.
pub fn get_cognitive_response() -> CognitiveResponse {
    let engine = COGNITIVE_ENGINE.lock();
    engine.respond()
}

/// Public API: Check system restart advisability.
pub fn check_restart_advisory() -> RestartAdvisory {
    let engine = COGNITIVE_ENGINE.lock();
    engine.restart_check()
}

/// Public API: Update context (convenience wrapper).
pub fn update_cognitive_metrics(
    switching_rate: Q16,
    info_density: Q16,
    active_apps: u32,
    response_latency: Q16,
    error_rate: Q16,
    dwell_time: Q16,
    pending_notifs: u32,
) {
    let mut engine = COGNITIVE_ENGINE.lock();
    engine.set_task_switching_rate(switching_rate);
    engine.set_information_density(info_density);
    engine.set_active_apps(active_apps);
    engine.set_response_latency(response_latency);
    engine.set_error_rate(error_rate);
    engine.set_dwell_time(dwell_time);
    engine.set_pending_notifications(pending_notifs);
}

/// Public API: Get diagnostic data for monitoring.
pub fn get_cognitive_diagnostics() -> (u64, u32, u32, u32, u32) {
    let engine = COGNITIVE_ENGINE.lock();
    engine.diagnostic_summary()
}
