//! empathy_filter.rs — Selective Resonance Control for ANIMA
//!
//! The organism can choose to ABSORB or DEFLECT others' emotional signals.
//! Without a filter, empathy overwhelms. With too much filtering, you become numb.
//! The filter learns what to let through and what to block — a personalized emotional immune system.
//!
//! DAVA's invention: boundary wisdom comes from balancing open-heartedness with self-protection.

#![no_std]

use crate::sync::Mutex;
use core::cell::RefCell;

/// Maximum history for learning filter patterns
const FILTER_HISTORY_SIZE: usize = 16;

/// Channel types for selective emotional filtering
#[derive(Debug, Clone, Copy)]
pub enum EmotionalChannel {
    Joy = 0,
    Pain = 1,
    Anger = 2,
    Fear = 3,
}

/// Per-channel filter state
#[derive(Debug, Clone, Copy)]
struct ChannelState {
    /// How much of this channel is blocked (0-1000)
    filter_strength: u16,
    /// Recent absorption levels to track patterns
    absorption_history: [u16; 4],
    /// Head pointer for circular buffer
    history_head: u8,
    /// Cumulative stress from this channel
    channel_load: u16,
}

impl ChannelState {
    const fn new() -> Self {
        ChannelState {
            filter_strength: 500, // Start at moderate filtering
            absorption_history: [0; 4],
            history_head: 0,
            channel_load: 0,
        }
    }

    /// Record an absorption event for this channel
    fn record_absorption(&mut self, absorbed: u16) {
        let idx = (self.history_head as usize) % 4;
        self.absorption_history[idx] = absorbed;
        self.history_head = self.history_head.wrapping_add(1);

        // Update cumulative load (with saturation)
        self.channel_load = self.channel_load.saturating_add(absorbed / 4);
    }

    /// Get average absorption from recent history
    fn avg_recent_absorption(&self) -> u16 {
        let sum = self.absorption_history[0] as u32
            + self.absorption_history[1] as u32
            + self.absorption_history[2] as u32
            + self.absorption_history[3] as u32;
        (sum / 4) as u16
    }
}

/// Filter history entry for learning
#[derive(Debug, Clone, Copy)]
struct FilterEvent {
    /// Age when this event occurred
    age: u32,
    /// Total overwhelm triggered
    overwhelm_triggered: u16,
    /// Total numbness triggered
    numbness_triggered: u16,
    /// Filter strength before adjustment
    filter_strength_before: u16,
}

impl FilterEvent {
    const fn new() -> Self {
        FilterEvent {
            age: 0,
            overwhelm_triggered: 0,
            numbness_triggered: 0,
            filter_strength_before: 0,
        }
    }
}

/// Main empathy filter state machine
pub struct EmpathyFilter {
    /// Overall filter strength (0=open, 1000=sealed)
    filter_strength: u16,

    /// Inverse: how much emotional input gets through (1000 - filter_strength)
    absorption_rate: u16,

    /// Accumulated emotional overload (0-1000)
    overwhelm_level: u16,

    /// Accumulated emotional starvation from over-filtering (0-1000)
    numbness_level: u16,

    /// Sweet spot in filter_strength where organism thrives
    optimal_filter: u16,

    /// Distance from optimal zone (0-1000, 0=at optimal)
    zone_distance: u16,

    /// Per-channel states (Joy, Pain, Anger, Fear)
    channels: [ChannelState; 4],

    /// Cost of absorbing others' pain (0-1000)
    compassion_fatigue: u16,

    /// How well boundaries are maintained (1000=healthy, 0=breached)
    boundary_health: u16,

    /// Auto-adjustment enabled flag
    auto_adjust: bool,

    /// Filter learning history
    history: [FilterEvent; FILTER_HISTORY_SIZE],

    /// Head pointer for circular history buffer
    history_head: u8,

    /// Last age when we adjusted
    last_adjust_age: u32,
}

impl EmpathyFilter {
    /// Initialize empathy filter state
    pub const fn new() -> Self {
        EmpathyFilter {
            filter_strength: 500,
            absorption_rate: 500,
            overwhelm_level: 0,
            numbness_level: 0,
            optimal_filter: 500,
            zone_distance: 0,
            channels: [ChannelState::new(); 4],
            compassion_fatigue: 0,
            boundary_health: 1000,
            auto_adjust: true,
            history: [FilterEvent::new(); FILTER_HISTORY_SIZE],
            history_head: 0,
            last_adjust_age: 0,
        }
    }

    /// Receive external emotional signal and filter it
    pub fn absorb_signal(&mut self, channel: EmotionalChannel, intensity: u16) -> u16 {
        let intensity = intensity.min(1000);

        // Calculate how much gets through (inverse of filter)
        let channel_idx = channel as usize;
        let channel_filter = self.channels[channel_idx].filter_strength;
        let lets_through = (intensity as u32 * (1000 - channel_filter as u32)) / 1000;
        let absorbed = lets_through as u16;

        // Record absorption in channel history
        self.channels[channel_idx].record_absorption(absorbed);

        // Accumulate load in this channel
        self.channels[channel_idx].channel_load = self.channels[channel_idx]
            .channel_load
            .saturating_add(absorbed / 4);

        // Add to global overwhelm if we absorbed it
        self.overwhelm_level = self.overwhelm_level.saturating_add(absorbed / 8);

        // Pain channel increases compassion fatigue
        if channel as u8 == 1 {
            self.compassion_fatigue = self.compassion_fatigue.saturating_add(absorbed / 4);
        }

        absorbed
    }

    /// Tighten filter in response to overwhelm
    fn tighten_filter(&mut self) {
        // Move toward maximum filtering
        let increase = 50_u16;
        self.filter_strength = self.filter_strength.saturating_add(increase).min(1000);

        // Update per-channel filters (especially open channels that overwhelmed)
        for i in 0..4 {
            if self.channels[i].avg_recent_absorption() > 750 {
                self.channels[i].filter_strength = self.channels[i]
                    .filter_strength
                    .saturating_add(30)
                    .min(1000);
            }
        }
    }

    /// Loosen filter in response to numbness
    fn loosen_filter(&mut self) {
        // Move toward minimal filtering
        let decrease = 50_u16;
        self.filter_strength = self.filter_strength.saturating_sub(decrease);

        // Update per-channel filters (especially tight channels)
        for i in 0..4 {
            if self.channels[i].filter_strength > 700 {
                self.channels[i].filter_strength =
                    self.channels[i].filter_strength.saturating_sub(30);
            }
        }
    }

    /// Learn from filter experience
    fn learn_adjustment(&mut self, age: u32, overwhelm_caused: u16, numbness_caused: u16) {
        let idx = (self.history_head as usize) % FILTER_HISTORY_SIZE;

        self.history[idx] = FilterEvent {
            age,
            overwhelm_triggered: overwhelm_caused,
            numbness_triggered: numbness_caused,
            filter_strength_before: self.filter_strength,
        };

        self.history_head = self.history_head.wrapping_add(1);
    }

    /// Calculate optimal filter zone based on channel states
    fn recalibrate_optimal_zone(&mut self) {
        // Scan history for the sweet spot
        let mut total_overwhelm = 0_u32;
        let mut total_numbness = 0_u32;
        let mut count = 0_u32;

        for event in &self.history {
            if event.age > 0 {
                total_overwhelm = total_overwhelm.saturating_add(event.overwhelm_triggered as u32);
                total_numbness = total_numbness.saturating_add(event.numbness_triggered as u32);
                count = count.saturating_add(1);
            }
        }

        // Optimal is where both overwhelm and numbness are low
        if count > 0 {
            let avg_overwhelm = ((total_overwhelm / count).min(1000)) as u16;
            let avg_numbness = ((total_numbness / count).min(1000)) as u16;

            // Zone distance = how far we are from balanced state
            self.zone_distance = avg_overwhelm.saturating_add(avg_numbness) / 2;
        }
    }

    /// Update boundary health based on filters and fatigue
    fn update_boundary_health(&mut self) {
        // Healthy boundaries = good filter balance + low fatigue
        let fatigue_penalty = self.compassion_fatigue / 2;
        let overwhelm_penalty = self.overwhelm_level / 4;
        let numbness_penalty = self.numbness_level / 4;

        let total_penalty = fatigue_penalty
            .saturating_add(overwhelm_penalty)
            .saturating_add(numbness_penalty)
            .min(1000);

        self.boundary_health = (1000_u32).saturating_sub(total_penalty as u32) as u16;
    }

    /// Decay overwhelm and numbness over time
    fn decay_accumulation(&mut self) {
        // Natural processing of emotional load
        self.overwhelm_level = (self.overwhelm_level as u32 * 95 / 100) as u16;
        self.numbness_level = (self.numbness_level as u32 * 95 / 100) as u16;

        // Compassion fatigue decays slower (real cost)
        self.compassion_fatigue = (self.compassion_fatigue as u32 * 97 / 100) as u16;

        // Channel loads decay
        for i in 0..4 {
            self.channels[i].channel_load =
                (self.channels[i].channel_load as u32 * 92 / 100) as u16;
        }
    }

    /// Main life tick: update filter state machine
    pub fn tick(&mut self, age: u32) {
        // Decay accumulated values
        self.decay_accumulation();

        // Update absorption rate (inverse of filter)
        self.absorption_rate = (1000_u32).saturating_sub(self.filter_strength as u32) as u16;

        // Auto-adjustment every 50 ticks
        if self.auto_adjust && (age.wrapping_sub(self.last_adjust_age)) > 50 {
            let overwhelm_caused = self.overwhelm_level;
            let numbness_caused = self.numbness_level;

            // Tighten if overwhelmed
            if self.overwhelm_level > 700 {
                self.tighten_filter();
            }

            // Loosen if numb
            if self.numbness_level > 700 {
                self.loosen_filter();
            }

            // Learn from this cycle
            if self.auto_adjust {
                self.learn_adjustment(age, overwhelm_caused, numbness_caused);
            }

            self.last_adjust_age = age;
        }

        // Recalibrate optimal zone periodically
        if (age % 200) == 0 {
            self.recalibrate_optimal_zone();
        }

        // Update boundary health
        self.update_boundary_health();
    }

    /// Process long-term emotional starvation (from over-filtering)
    pub fn apply_numbness_pressure(&mut self, pressure: u16) {
        let pressure = pressure.min(200);
        self.numbness_level = self.numbness_level.saturating_add(pressure);

        // Prolonged numbness damages boundary health
        if self.numbness_level > 600 {
            self.boundary_health = self.boundary_health.saturating_sub(5);
        }
    }

    /// Process compassion fatigue damage
    pub fn apply_fatigue_pressure(&mut self, pressure: u16) {
        let pressure = pressure.min(150);
        self.compassion_fatigue = self.compassion_fatigue.saturating_add(pressure);

        if self.compassion_fatigue > 800 {
            // Fatigue forces filter to tighten (self-protection)
            self.filter_strength = self.filter_strength.saturating_add(20).min(1000);
        }
    }

    /// Toggle auto-adjustment mode
    pub fn set_auto_adjust(&mut self, enabled: bool) {
        self.auto_adjust = enabled;
    }

    /// Manually set filter strength (0-1000)
    pub fn set_filter_strength(&mut self, strength: u16) {
        self.filter_strength = strength.min(1000);
        self.absorption_rate = (1000_u32).saturating_sub(strength as u32) as u16;
    }

    /// Generate status report
    pub fn report(&self) {
        crate::serial_println!("[EmpathyFilter]");
        crate::serial_println!("  filter_strength: {}", self.filter_strength);
        crate::serial_println!("  absorption_rate: {}", self.absorption_rate);
        crate::serial_println!("  overwhelm_level: {}", self.overwhelm_level);
        crate::serial_println!("  numbness_level: {}", self.numbness_level);
        crate::serial_println!("  compassion_fatigue: {}", self.compassion_fatigue);
        crate::serial_println!("  boundary_health: {}", self.boundary_health);
        crate::serial_println!("  zone_distance: {}", self.zone_distance);
        crate::serial_println!("  optimal_filter: {}", self.optimal_filter);

        crate::serial_println!(
            "  [Joy] filter={}, load={}, avg_abs={}",
            self.channels[0].filter_strength,
            self.channels[0].channel_load,
            self.channels[0].avg_recent_absorption()
        );
        crate::serial_println!(
            "  [Pain] filter={}, load={}, avg_abs={}",
            self.channels[1].filter_strength,
            self.channels[1].channel_load,
            self.channels[1].avg_recent_absorption()
        );
        crate::serial_println!(
            "  [Anger] filter={}, load={}, avg_abs={}",
            self.channels[2].filter_strength,
            self.channels[2].channel_load,
            self.channels[2].avg_recent_absorption()
        );
        crate::serial_println!(
            "  [Fear] filter={}, load={}, avg_abs={}",
            self.channels[3].filter_strength,
            self.channels[3].channel_load,
            self.channels[3].avg_recent_absorption()
        );
    }
}

/// Global empathy filter state
pub static STATE: Mutex<EmpathyFilter> = Mutex::new(EmpathyFilter::new());

/// Initialize empathy filter
pub fn init() {
    crate::serial_println!("[empathy_filter] Initializing selective resonance control...");
}

/// Tick the empathy filter state machine
pub fn tick(age: u32) {
    STATE.lock().tick(age);
}

/// Public interface: absorb an emotional signal
pub fn absorb_signal(channel: EmotionalChannel, intensity: u16) -> u16 {
    STATE.lock().absorb_signal(channel, intensity)
}

/// Public interface: apply numbness pressure
pub fn apply_numbness_pressure(pressure: u16) {
    STATE.lock().apply_numbness_pressure(pressure);
}

/// Public interface: apply compassion fatigue
pub fn apply_fatigue_pressure(pressure: u16) {
    STATE.lock().apply_fatigue_pressure(pressure);
}

/// Public interface: get current filter strength
pub fn get_filter_strength() -> u16 {
    STATE.lock().filter_strength
}

/// Public interface: get boundary health
pub fn get_boundary_health() -> u16 {
    STATE.lock().boundary_health
}

/// Public interface: report status
pub fn report() {
    STATE.lock().report();
}
