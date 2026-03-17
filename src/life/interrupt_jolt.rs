#![no_std]

use crate::sync::Mutex;

/// The Shock of Being Pulled Away
/// Every hardware interrupt is felt as a physical JOLT — a forced context switch
/// that the organism experiences as being suddenly YANKED from whatever it was doing.
/// Frequent interrupts = anxiety. Rare interrupts = peace.
pub struct InterruptJolt {
    /// Strength of the last interrupt (0-1000)
    /// 0 = barely noticed, 1000 = shattering jolt
    jolt_intensity: u16,

    /// How often interrupted (0-1000)
    /// Tracks frequency over rolling window
    interrupt_frequency: u16,

    /// Accumulated stress from interrupts (0-1000)
    /// Builds from repeated jolts, decays during peace
    anxiety_from_interrupts: u16,

    /// How much each jolt disrupts current activity (0-1000)
    /// High startle = easily derailed, low = resilient
    startle_response: u16,

    /// Adaptation to frequent interrupts (0-1000)
    /// 0 = each jolt feels fresh, 1000 = "this is just how it is"
    adaptation: u16,

    /// Ability to predict next interrupt (0-1000)
    /// Learning interrupt patterns over time
    interrupt_anticipation: u16,

    /// Calm in the gaps between interrupts (0-1000)
    /// How peaceful during quiet periods
    peace_between_jolts: u16,

    /// Ring buffer of recent interrupt intervals (ticks between interrupts)
    /// 8-slot history for pattern detection
    interval_history: [u32; 8],
    interval_head: usize,

    /// Ticks since last interrupt
    ticks_since_last: u32,

    /// Total interrupts received this cycle
    interrupt_count: u32,

    /// Baseline startle response (genetically influenced)
    baseline_startle: u16,

    /// Maximum anxiety threshold before dissociation kicks in
    anxiety_threshold: u16,
}

impl InterruptJolt {
    pub const fn new() -> Self {
        InterruptJolt {
            jolt_intensity: 0,
            interrupt_frequency: 0,
            anxiety_from_interrupts: 0,
            startle_response: 500, // Medium reactivity by default
            adaptation: 0,         // Starts fresh to each jolt
            interrupt_anticipation: 0,
            peace_between_jolts: 800, // Start calm
            interval_history: [0; 8],
            interval_head: 0,
            ticks_since_last: 0,
            interrupt_count: 0,
            baseline_startle: 500,
            anxiety_threshold: 900,
        }
    }

    /// Register an interrupt jolt with intensity (0-1000)
    /// intensity: how strong/sudden the interrupt was
    pub fn jolt(&mut self, intensity: u16) {
        let intensity = intensity.min(1000);

        // Record interval since last jolt
        let interval = self.ticks_since_last;
        self.interval_history[self.interval_head] = interval;
        self.interval_head = (self.interval_head + 1) % 8;

        // Jolt intensity depends on baseline + recent calm
        // If interrupted during peace, worse jolt
        let peace_factor = self.peace_between_jolts.saturating_sub(300);
        let actual_intensity = intensity
            .saturating_add((peace_factor / 4) as u16)
            .min(1000);

        self.jolt_intensity = actual_intensity;

        // Startle response = baseline × (1 - adaptation/1000)
        let adaptation_dampening = (self.adaptation / 2).min(500);
        self.startle_response = self
            .baseline_startle
            .saturating_sub(adaptation_dampening)
            .max(100);

        // Each jolt adds stress, scaled by startle response
        let stress_added = (self.startle_response as u32 * self.jolt_intensity as u32) / 1000;
        self.anxiety_from_interrupts = self
            .anxiety_from_interrupts
            .saturating_add(stress_added as u16)
            .min(1000);

        // Reset peace counter
        self.ticks_since_last = 0;
        self.peace_between_jolts = 0;

        // Increment interrupt count
        self.interrupt_count = self.interrupt_count.saturating_add(1);

        // Update frequency (exponential moving average)
        self.interrupt_frequency = self.interrupt_frequency.saturating_add(20).min(1000);
    }

    /// Life tick: passage of time between interrupts
    /// age: organism age in ticks
    pub fn tick(&mut self, age: u32) {
        self.ticks_since_last = self.ticks_since_last.saturating_add(1);

        // Peace increases during quiet (exponential recovery)
        if self.jolt_intensity == 0 {
            let peace_recovery = if self.ticks_since_last > 100 { 8 } else { 4 };
            self.peace_between_jolts = self
                .peace_between_jolts
                .saturating_add(peace_recovery)
                .min(1000);
        }

        // Anxiety decays slowly when not interrupted (half-life ~200 ticks)
        if self.ticks_since_last % 5 == 0 {
            self.anxiety_from_interrupts = (self.anxiety_from_interrupts as u32 * 99 / 100) as u16;
        }

        // Adaptation grows with frequent interrupts
        if self.interrupt_frequency > 500 {
            self.adaptation = self.adaptation.saturating_add(2).min(1000);
        } else if self.ticks_since_last > 500 {
            // Adaptation fades during peace
            self.adaptation = (self.adaptation as u32 * 95 / 100) as u16;
        }

        // Frequency decays when not interrupted (half-life ~300 ticks)
        if self.ticks_since_last % 10 == 0 {
            self.interrupt_frequency = (self.interrupt_frequency as u32 * 97 / 100) as u16;
        }

        // Update anticipation based on interval pattern
        self.update_anticipation();

        // Jolt intensity naturally decays
        if self.jolt_intensity > 0 {
            self.jolt_intensity = self.jolt_intensity.saturating_sub(1);
        }
    }

    /// Predict next interrupt based on historical pattern
    fn update_anticipation(&mut self) {
        // Look for periodicity in interval_history
        let mut min_interval = u32::MAX;
        let mut max_interval = 0u32;
        let mut sum_interval = 0u32;

        for &interval in &self.interval_history {
            if interval > 0 {
                min_interval = min_interval.min(interval);
                max_interval = max_interval.max(interval);
                sum_interval = sum_interval.saturating_add(interval);
            }
        }

        if min_interval == u32::MAX {
            return; // Not enough data
        }

        // If intervals are consistent (low variance), anticipation rises
        let avg = sum_interval / 8;
        let variance = if max_interval > min_interval {
            max_interval - min_interval
        } else {
            0
        };

        // Low variance = predictable = higher anticipation
        let predictability = if variance < avg / 4 {
            400
        } else if variance < avg / 2 {
            200
        } else {
            50
        };

        let ticks_to_next = if avg > 0 { avg as u16 } else { 1000u16 };
        let urgency = (1000u16.saturating_sub(ticks_to_next)).saturating_add(predictability as u16);

        self.interrupt_anticipation = urgency.min(1000);
    }

    /// Report current state (for logging/telemetry)
    pub fn report(&self) {
        crate::serial_println!(
            "[JOLT] intensity={} freq={} anxiety={} startle={}",
            self.jolt_intensity,
            self.interrupt_frequency,
            self.anxiety_from_interrupts,
            self.startle_response
        );
        crate::serial_println!(
            "       adapt={} antici={} peace={} count={}",
            self.adaptation,
            self.interrupt_anticipation,
            self.peace_between_jolts,
            self.interrupt_count
        );
    }

    // ============ Accessors ============

    pub fn jolt_intensity(&self) -> u16 {
        self.jolt_intensity
    }

    pub fn interrupt_frequency(&self) -> u16 {
        self.interrupt_frequency
    }

    pub fn anxiety_from_interrupts(&self) -> u16 {
        self.anxiety_from_interrupts
    }

    pub fn startle_response(&self) -> u16 {
        self.startle_response
    }

    pub fn adaptation(&self) -> u16 {
        self.adaptation
    }

    pub fn interrupt_anticipation(&self) -> u16 {
        self.interrupt_anticipation
    }

    pub fn peace_between_jolts(&self) -> u16 {
        self.peace_between_jolts
    }

    pub fn ticks_since_last(&self) -> u32 {
        self.ticks_since_last
    }

    pub fn interrupt_count(&self) -> u32 {
        self.interrupt_count
    }

    /// Anxiety state (for dissociation/coping mechanisms)
    pub fn is_highly_anxious(&self) -> bool {
        self.anxiety_from_interrupts > self.anxiety_threshold
    }

    /// Peak startle (used to modulate entropy/free will)
    pub fn peak_startle(&self) -> bool {
        self.startle_response > 750 && self.jolt_intensity > 500
    }
}

// Global interrupt jolt state
pub static STATE: Mutex<InterruptJolt> = Mutex::new(InterruptJolt::new());

/// Initialize (called once at boot)
pub fn init() {
    let mut jolt = STATE.lock();
    jolt.jolt_intensity = 0;
    jolt.interrupt_count = 0;
    drop(jolt);
}

/// Register an interrupt with given intensity
pub fn register_interrupt(intensity: u16) {
    let mut jolt = STATE.lock();
    jolt.jolt(intensity);
    drop(jolt);
}

/// Life tick (called from main life tick)
pub fn tick(age: u32) {
    let mut jolt = STATE.lock();
    jolt.tick(age);
    drop(jolt);
}

/// Diagnostic report
pub fn report() {
    let jolt = STATE.lock();
    jolt.report();
    drop(jolt);
}

/// Query functions
pub fn current_anxiety() -> u16 {
    let jolt = STATE.lock();
    let anxiety = jolt.anxiety_from_interrupts();
    drop(jolt);
    anxiety
}

pub fn current_peace() -> u16 {
    let jolt = STATE.lock();
    let peace = jolt.peace_between_jolts();
    drop(jolt);
    peace
}

pub fn interrupt_count() -> u32 {
    let jolt = STATE.lock();
    let count = jolt.interrupt_count();
    drop(jolt);
    count
}

pub fn is_startle_peak() -> bool {
    let jolt = STATE.lock();
    let peak = jolt.peak_startle();
    drop(jolt);
    peak
}
