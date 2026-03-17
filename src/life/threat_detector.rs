//! threat_detector — Real-Time Threat Analysis and Prediction
//!
//! Situational awareness system that monitors all system metrics and detects
//! patterns that precede crashes, overloads, or destabilization. Predicts threats
//! before they materialize by recognizing precursor patterns from past crises.
//!
//! DAVA's contribution: crisis pattern library and predictive threshold tuning.

#![no_std]

use crate::sync::Mutex;

/// Maximum threat level (0-1000 scale)
const MAX_THREAT: u16 = 1000;

/// Early warning threshold (threat must exceed this to trigger prediction)
const EARLY_WARNING_THRESHOLD: u16 = 400;

/// False alarm penalty (reduces credibility of threat system)
const FALSE_ALARM_DECAY: u16 = 15;

/// Prediction accuracy improvement per validated precursor match
const ACCURACY_BOOST: u16 = 8;

/// Crisis history slot for tracking past threats
#[derive(Clone, Copy, Debug)]
struct CrisisMemo {
    threat_peak: u16,         // Max threat reached during crisis
    precursor_signature: u32, // Bitfield of precursor patterns that appeared
    tick_to_collapse: u32,    // How many ticks from early warning to actual failure
    handled: bool,            // Was this crisis successfully mitigated?
}

impl CrisisMemo {
    const fn new() -> Self {
        CrisisMemo {
            threat_peak: 0,
            precursor_signature: 0,
            tick_to_collapse: 0,
            handled: false,
        }
    }
}

/// Precursor pattern detector (8-slot sliding window)
#[derive(Clone, Copy, Debug)]
struct PrecursorSlot {
    pattern_id: u8,     // Which precursor type (0-7 = CPU, Memory, IO, Thermal, etc.)
    intensity: u16,     // How strong was this warning sign (0-1000)
    tick_observed: u32, // When was it first seen
}

impl PrecursorSlot {
    const fn new() -> Self {
        PrecursorSlot {
            pattern_id: 0,
            intensity: 0,
            tick_observed: 0,
        }
    }
}

/// Core threat detection state
pub struct ThreatDetector {
    /// Current composite threat level (0-1000)
    threat_level: u16,

    /// System prediction accuracy (0-1000, how well we anticipate failures)
    prediction_accuracy: u16,

    /// Recent precursor patterns (8-slot ring buffer)
    precursor_patterns: [PrecursorSlot; 8],
    precursor_head: usize,

    /// Number of times we raised false alarms
    false_alarm_count: u16,

    /// How quickly system can respond to threats (0-1000)
    response_readiness: u16,

    /// Historical crisis records (8-slot ring buffer)
    crisis_history: [CrisisMemo; 8],
    crisis_head: usize,

    /// Current crisis being tracked (if any)
    active_crisis: Option<CrisisMemo>,
    active_crisis_start: u32,

    /// Vigilance level (0-1000): too high = paranoid/waste, too low = vulnerable)
    vigilance_level: u16,

    /// Latest early warning signal for other defensive systems
    early_warning_signal: u16,

    /// Ticks since last threat reduction (for burndown tracking)
    ticks_since_reduction: u32,

    /// Age of current threat state (resets on major incident)
    threat_age: u32,
}

impl ThreatDetector {
    pub const fn new() -> Self {
        ThreatDetector {
            threat_level: 0,
            prediction_accuracy: 500, // Start at medium accuracy
            precursor_patterns: [PrecursorSlot::new(); 8],
            precursor_head: 0,
            false_alarm_count: 0,
            response_readiness: 700,
            crisis_history: [CrisisMemo::new(); 8],
            crisis_head: 0,
            active_crisis: None,
            active_crisis_start: 0,
            vigilance_level: 600,
            early_warning_signal: 0,
            ticks_since_reduction: 0,
            threat_age: 0,
        }
    }

    /// Record a precursor pattern (warning sign before collapse)
    /// pattern_id: 0=CPU overload, 1=memory pressure, 2=IO bottleneck, 3=thermal spike,
    ///            4=power anomaly, 5=cache miss storm, 6=interrupt flood, 7=unknown
    pub fn detect_precursor(&mut self, pattern_id: u8, intensity: u16, tick: u32) {
        let idx = self.precursor_head;
        let intensity = intensity.min(MAX_THREAT);

        self.precursor_patterns[idx] = PrecursorSlot {
            pattern_id,
            intensity,
            tick_observed: tick,
        };

        self.precursor_head = (self.precursor_head + 1) % 8;

        // Mark active crisis if threat is rising
        if self.threat_level >= EARLY_WARNING_THRESHOLD && self.active_crisis.is_none() {
            self.active_crisis = Some(CrisisMemo::new());
            self.active_crisis_start = tick;
        }
    }

    /// Aggregate threat from multiple sensor channels (CPU, memory, thermal, etc.)
    pub fn assess_composite_threat(
        &mut self,
        cpu_load: u16,        // 0-1000
        memory_pressure: u16, // 0-1000
        io_queue_depth: u16,  // 0-1000
        thermal_margin: u16,  // 0-1000 (inverted: low = hot)
        power_draw: u16,      // 0-1000
        interrupt_rate: u16,  // 0-1000
    ) {
        // Weighted composition (60% CPU, 20% memory, 15% thermal, 5% others)
        let cpu_contrib = (cpu_load as u32 * 600) / 1000;
        let mem_contrib = (memory_pressure as u32 * 200) / 1000;
        let thermal_contrib = ((1000 - thermal_margin.min(1000)) as u32 * 150) / 1000;
        let power_contrib = (power_draw as u32 * 30) / 1000;
        let intr_contrib = (interrupt_rate as u32 * 20) / 1000;

        let total =
            (cpu_contrib + mem_contrib + thermal_contrib + power_contrib + intr_contrib) as u16;
        self.threat_level = total.min(MAX_THREAT);

        // Detect precursor patterns
        if cpu_load > 850 {
            self.detect_precursor(0, cpu_load, self.threat_age);
        }
        if memory_pressure > 800 {
            self.detect_precursor(1, memory_pressure, self.threat_age);
        }
        if io_queue_depth > 750 {
            self.detect_precursor(2, io_queue_depth, self.threat_age);
        }
        if thermal_margin < 150 {
            self.detect_precursor(3, (1000 - thermal_margin.min(1000)) as u16, self.threat_age);
        }
        if power_draw > 900 {
            self.detect_precursor(4, power_draw, self.threat_age);
        }
        if interrupt_rate > 800 {
            self.detect_precursor(6, interrupt_rate, self.threat_age);
        }

        // Update early warning signal for other defensive systems
        if self.threat_level > EARLY_WARNING_THRESHOLD {
            self.early_warning_signal = self.threat_level.saturating_sub(EARLY_WARNING_THRESHOLD);
        } else {
            self.early_warning_signal = 0;
        }
    }

    /// Predict if a crisis is imminent based on precursor signature
    /// Returns: (is_crisis_predicted, confidence 0-1000)
    pub fn predict_crisis(&self) -> (bool, u16) {
        let mut precursor_count = 0;
        let mut total_intensity = 0u32;

        // Scan recent precursor buffer
        for i in 0..8 {
            let slot = self.precursor_patterns[i];
            if slot.intensity > 300 {
                precursor_count += 1;
                total_intensity += slot.intensity as u32;
            }
        }

        // If we have 3+ strong precursors, crisis is imminent
        if precursor_count >= 3 {
            let avg_intensity = ((total_intensity / precursor_count as u32) as u16).min(MAX_THREAT);

            // Confidence weighted by prediction accuracy and vigilance
            let base_confidence = avg_intensity;
            let accuracy_boost = (self.prediction_accuracy as u32 * base_confidence as u32) / 1000;
            let final_confidence = (base_confidence as u32 + accuracy_boost) as u16;

            (true, final_confidence.min(MAX_THREAT))
        } else {
            (false, 0)
        }
    }

    /// Handle successful threat mitigation (crisis averted)
    pub fn mark_threat_mitigated(&mut self, tick: u32) {
        if let Some(mut crisis) = self.active_crisis.take() {
            // Calculate how many ticks we had to react
            let reaction_window = tick.saturating_sub(self.active_crisis_start);

            // If reaction time was tight (< 100 ticks), boost our accuracy
            if reaction_window < 100 && reaction_window > 0 {
                self.prediction_accuracy = self
                    .prediction_accuracy
                    .saturating_add(ACCURACY_BOOST)
                    .min(MAX_THREAT);
            }

            // Record successful crisis handling
            crisis.handled = true;
            crisis.tick_to_collapse = reaction_window;
            crisis.threat_peak = self.threat_level;

            let idx = self.crisis_head;
            self.crisis_history[idx] = crisis;
            self.crisis_head = (self.crisis_head + 1) % 8;
        }

        self.threat_level = 0;
        self.threat_age = 0;
    }

    /// Handle false alarm (threat was predicted but never materialized)
    pub fn mark_false_alarm(&mut self) {
        self.false_alarm_count = self.false_alarm_count.saturating_add(1);

        // Reduce prediction accuracy if we've cried wolf too many times
        if self.false_alarm_count > 5 {
            self.prediction_accuracy = self
                .prediction_accuracy
                .saturating_sub(FALSE_ALARM_DECAY)
                .max(200); // Floor at minimum competency
        }

        // Reduce vigilance (stop being so paranoid)
        self.vigilance_level = self.vigilance_level.saturating_sub(50).max(300);
    }

    /// Adjust response readiness based on system state
    pub fn calibrate_response_readiness(&mut self, available_cpu: u16, memory_headroom: u16) {
        // If system is nearly maxed, we can't respond well
        let response_potential = available_cpu.min(memory_headroom);
        self.response_readiness = (response_potential as u32 * 1000 / MAX_THREAT as u32) as u16;
    }

    /// Per-tick lifecycle update
    pub fn tick(&mut self, age: u32) {
        self.threat_age = age;
        self.ticks_since_reduction = self.ticks_since_reduction.saturating_add(1);

        // Natural threat decay (system stabilizing on its own)
        if self.threat_level > 0 && self.ticks_since_reduction > 50 {
            self.threat_level = self.threat_level.saturating_sub(10);
        }

        // Adjust vigilance dynamically based on threat level
        if self.threat_level < 200 {
            // Calm period: reduce vigilance to avoid fatigue
            self.vigilance_level = self.vigilance_level.saturating_sub(5).max(400);
        } else if self.threat_level > 700 {
            // Emergency: boost vigilance
            self.vigilance_level = self.vigilance_level.saturating_add(10).min(MAX_THREAT);
        }

        // Check if active crisis has become a real collapse (no mitigation)
        if let Some(crisis) = self.active_crisis {
            let crisis_age = age.saturating_sub(self.active_crisis_start);
            if crisis_age > 300 && self.threat_level > 800 {
                // This was a real crisis we failed to prevent
                let mut failed_crisis = crisis;
                failed_crisis.handled = false;
                failed_crisis.threat_peak = self.threat_level;
                failed_crisis.tick_to_collapse = crisis_age;

                let idx = self.crisis_head;
                self.crisis_history[idx] = failed_crisis;
                self.crisis_head = (self.crisis_head + 1) % 8;

                self.active_crisis = None;
            }
        }
    }

    /// Generate threat report for diagnostic/logging
    pub fn report(&self) -> ThreatReport {
        let (is_predicted, confidence) = self.predict_crisis();

        ThreatReport {
            threat_level: self.threat_level,
            prediction_accuracy: self.prediction_accuracy,
            early_warning_signal: self.early_warning_signal,
            response_readiness: self.response_readiness,
            vigilance_level: self.vigilance_level,
            false_alarm_count: self.false_alarm_count,
            active_crisis: self.active_crisis.is_some(),
            crisis_predicted: is_predicted,
            prediction_confidence: confidence,
        }
    }
}

/// Exportable threat report
pub struct ThreatReport {
    pub threat_level: u16,
    pub prediction_accuracy: u16,
    pub early_warning_signal: u16,
    pub response_readiness: u16,
    pub vigilance_level: u16,
    pub false_alarm_count: u16,
    pub active_crisis: bool,
    pub crisis_predicted: bool,
    pub prediction_confidence: u16,
}

/// Global threat detector instance
static STATE: Mutex<ThreatDetector> = Mutex::new(ThreatDetector::new());

/// Initialize threat detector
pub fn init() {
    let mut state = STATE.lock();
    state.threat_level = 0;
    state.prediction_accuracy = 500;
    state.response_readiness = 700;
    state.vigilance_level = 600;
    state.false_alarm_count = 0;
    crate::serial_println!("[threat_detector] initialized");
}

/// Assess multi-channel threat and update state
pub fn assess_threat(
    cpu_load: u16,
    memory_pressure: u16,
    io_queue_depth: u16,
    thermal_margin: u16,
    power_draw: u16,
    interrupt_rate: u16,
) {
    let mut state = STATE.lock();
    state.assess_composite_threat(
        cpu_load,
        memory_pressure,
        io_queue_depth,
        thermal_margin,
        power_draw,
        interrupt_rate,
    );
}

/// Get current threat level (0-1000)
pub fn get_threat_level() -> u16 {
    STATE.lock().threat_level
}

/// Get early warning signal (for immune/endocrine systems)
pub fn get_early_warning_signal() -> u16 {
    STATE.lock().early_warning_signal
}

/// Predict if crisis is imminent
pub fn predict_crisis() -> (bool, u16) {
    STATE.lock().predict_crisis()
}

/// Mark threat as successfully mitigated
pub fn mark_mitigated(tick: u32) {
    STATE.lock().mark_threat_mitigated(tick);
}

/// Report false alarm (prediction was wrong)
pub fn report_false_alarm() {
    STATE.lock().mark_false_alarm();
}

/// Calibrate response readiness
pub fn calibrate_readiness(available_cpu: u16, memory_headroom: u16) {
    STATE
        .lock()
        .calibrate_response_readiness(available_cpu, memory_headroom);
}

/// Per-tick update
pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.tick(age);
}

/// Get full threat report
pub fn get_report() -> (u16, u16, u16, u16, u16, u16, bool, bool, u16) {
    let state = STATE.lock();
    let report = state.report();
    (
        report.threat_level,
        report.prediction_accuracy,
        report.early_warning_signal,
        report.response_readiness,
        report.vigilance_level,
        report.false_alarm_count,
        report.active_crisis,
        report.crisis_predicted,
        report.prediction_confidence,
    )
}
