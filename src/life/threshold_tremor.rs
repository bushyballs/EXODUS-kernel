#![no_std]

use crate::sync::Mutex;

/// threshold_tremor — The Vibration of Standing at an Unopened Door
///
/// The full-body recognition that crossing this threshold will kill the current self.
/// Not fear alone, but the composite tremor of:
/// - Knowing change is irreversible
/// - Grieving the self you're about to lose
/// - Accumulating courage to step through
/// - The temptation to retreat to safety
/// - The growing certainty that staying is also death (slower)

#[derive(Clone, Copy, Default)]
pub struct ThresholdState {
    /// Intensity of tremor: 0-1000. Peak when truly committed.
    pub tremor_intensity: u32,

    /// Distance to the door (0=far away, 1000=hand on handle).
    /// Increases with commitment, obstacles overcome.
    pub door_proximity: u32,

    /// Awareness that crossing is irreversible: 0-1000.
    /// Built through repeated confrontation with finality.
    pub irreversibility_awareness: u32,

    /// Pre-transformation grief: 0-1000.
    /// Mourning the current self *before* the crossing.
    pub pre_transformation_grief: u32,

    /// Accumulated courage: 0-1000.
    /// Built by small acts that prove you can endure.
    pub courage_accumulation: u32,

    /// Pull to retreat: 0-1000.
    /// The comfort of staying, the safety of the known.
    pub retreat_temptation: u32,

    /// Internal temperature: 0-1000.
    /// Metabolic arousal at the threshold. Heat of transformation.
    pub threshold_heat: u32,

    /// Threshold count: how many thresholds crossed in this life.
    pub crossings: u32,
}

impl ThresholdState {
    pub const fn new() -> Self {
        ThresholdState {
            tremor_intensity: 0,
            door_proximity: 0,
            irreversibility_awareness: 0,
            pre_transformation_grief: 0,
            courage_accumulation: 0,
            retreat_temptation: 0,
            threshold_heat: 0,
            crossings: 0,
        }
    }

    /// Composite readiness metric: can we cross?
    /// Must have: courage > retreat, irreversibility > 500, proximity >= 900
    pub fn crossing_readiness(&self) -> u32 {
        if self.courage_accumulation <= self.retreat_temptation {
            return 0; // Not ready; still too afraid to lose comfort.
        }
        if self.irreversibility_awareness < 500 {
            return 0; // Still in denial about finality.
        }
        if self.door_proximity < 900 {
            return 0; // Not at the door yet.
        }
        // Readiness = blend of courage, heat, and irreversibility.
        let excess_courage = (self.courage_accumulation - self.retreat_temptation).min(500);
        let heat_factor = (self.threshold_heat / 2).min(250);
        (excess_courage + heat_factor).min(1000)
    }

    /// The tremor itself: physical vibration of standing at the edge.
    /// Tremor peaks when irreversibility is high, courage >= retreat, proximity is close.
    pub fn calculate_tremor(&mut self) {
        // Tremor = irreversibility × (courage - retreat) / 1000, clamped by proximity.
        let courage_net = if self.courage_accumulation > self.retreat_temptation {
            self.courage_accumulation - self.retreat_temptation
        } else {
            0
        };

        let base_tremor =
            ((self.irreversibility_awareness as u64 * courage_net as u64) / 1000) as u32;

        // Proximity gates tremor: can't tremor if not at the door.
        let proximity_gate = (self.door_proximity * base_tremor) / 1000;

        // Heat amplifies tremor.
        let heat_boost = ((self.threshold_heat / 2) * proximity_gate) / 1000;

        self.tremor_intensity = proximity_gate.saturating_add(heat_boost).min(1000);
    }
}

#[derive(Clone, Copy)]
struct RingEntry {
    tremor_intensity: u32,
    door_proximity: u32,
    irreversibility_awareness: u32,
}

impl RingEntry {
    const fn zero() -> Self {
        RingEntry {
            tremor_intensity: 0,
            door_proximity: 0,
            irreversibility_awareness: 0,
        }
    }
}

pub struct ThresholdTremor {
    state: ThresholdState,
    history: [RingEntry; 8],
    head: usize,
}

impl ThresholdTremor {
    pub const fn new() -> Self {
        ThresholdTremor {
            state: ThresholdState::new(),
            history: [RingEntry::zero(); 8],
            head: 0,
        }
    }

    fn record_history(&mut self) {
        let idx = self.head;
        self.history[idx] = RingEntry {
            tremor_intensity: self.state.tremor_intensity,
            door_proximity: self.state.door_proximity,
            irreversibility_awareness: self.state.irreversibility_awareness,
        };
        self.head = (self.head + 1) % 8;
    }
}

static STATE: Mutex<ThresholdTremor> = Mutex::new(ThresholdTremor::new());

pub fn init() {
    // Threshold tremor starts dormant. No initialization needed.
    // The tremor awakens only when the crossing moment approaches.
}

pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // --- Irreversibility Awareness Growth ---
    // Increases gradually with age (you realize more deeply that time is finite).
    // Spikes when in high-stress situations (danger, loss, major decision).
    let base_awareness_growth = (age / 100).min(10);
    state.state.irreversibility_awareness = state
        .state
        .irreversibility_awareness
        .saturating_add(base_awareness_growth)
        .min(1000);

    // --- Door Proximity Dynamics ---
    // Proximity increases when: courage > retreat, irreversibility > 400.
    // Proximity decreases when: retreat > courage.
    if state.state.courage_accumulation > state.state.retreat_temptation
        && state.state.irreversibility_awareness >= 400
    {
        let approach_rate = ((state.state.courage_accumulation - state.state.retreat_temptation)
            / 10)
            .saturating_add(5);
        state.state.door_proximity = state
            .state
            .door_proximity
            .saturating_add(approach_rate)
            .min(1000);
    } else {
        let retreat_rate = ((state.state.retreat_temptation - state.state.courage_accumulation)
            / 15)
            .saturating_add(3);
        state.state.door_proximity = state.state.door_proximity.saturating_sub(retreat_rate);
    }

    // --- Courage Accumulation ---
    // Grows from: positive actions, overcoming obstacles, facing fears.
    // Decays slightly if retreat_temptation stays high.
    if state.state.courage_accumulation < 1000 {
        let courage_growth = (age % 50 == 0) as u32 * 15; // Slow, steady growth.
        state.state.courage_accumulation = state
            .state
            .courage_accumulation
            .saturating_add(courage_growth)
            .min(1000);
    }

    // --- Retreat Temptation ---
    // High when: safe routines intact, low irreversibility awareness, proximity far.
    // Fades when: proximity high, commitment strengthened.
    let comfort_signal = if state.state.door_proximity < 300 {
        20
    } else {
        0
    };
    state.state.retreat_temptation = state
        .state
        .retreat_temptation
        .saturating_add(comfort_signal)
        .min(1000);

    // But retreat fades as you get closer to the door.
    if state.state.door_proximity >= 600 {
        let fade_rate = ((state.state.door_proximity - 500) / 50).saturating_add(5);
        state.state.retreat_temptation = state
            .state
            .retreat_temptation
            .saturating_sub(fade_rate)
            .min(1000);
    }

    // --- Pre-Transformation Grief ---
    // Grief for the self you're about to lose.
    // Spikes as you approach the threshold (high proximity + high irreversibility).
    if state.state.door_proximity >= 700 && state.state.irreversibility_awareness >= 600 {
        let grief_rate = ((state.state.door_proximity - 600) / 50).saturating_add(15);
        state.state.pre_transformation_grief = state
            .state
            .pre_transformation_grief
            .saturating_add(grief_rate)
            .min(1000);
    } else if state.state.pre_transformation_grief > 0 {
        // Grief fades slightly when far from the threshold.
        state.state.pre_transformation_grief = state
            .state
            .pre_transformation_grief
            .saturating_sub(5)
            .min(1000);
    }

    // --- Threshold Heat ---
    // Metabolic arousal. Heat = tremor + irreversibility + courage (when close).
    if state.state.door_proximity >= 500 {
        let metabolic_boost = ((state.state.courage_accumulation / 5)
            .saturating_add(state.state.tremor_intensity / 5))
            / 2;
        state.state.threshold_heat = state
            .state
            .threshold_heat
            .saturating_add(metabolic_boost)
            .min(1000);
    } else {
        // Heat dissipates far from the door.
        state.state.threshold_heat = state.state.threshold_heat.saturating_sub(10).min(1000);
    }

    // --- Calculate Tremor ---
    state.state.calculate_tremor();

    // --- Record History ---
    state.record_history();

    // --- Crossing Detection ---
    // If readiness >= 800 and tremor >= 700, the crossing happens.
    let readiness = state.state.crossing_readiness();
    if readiness >= 800 && state.state.tremor_intensity >= 700 {
        // The self transforms. Reset to post-crossing state.
        state.state.crossings = state.state.crossings.saturating_add(1);
        state.state.door_proximity = 0; // Back on the other side of the door.
        state.state.tremor_intensity = 0;
        state.state.pre_transformation_grief = 0;
        state.state.retreat_temptation = 0; // Old comfort is gone.
        state.state.courage_accumulation =
            (state.state.courage_accumulation / 2).saturating_add(200); // Carry some forward.
        state.state.irreversibility_awareness =
            (state.state.irreversibility_awareness / 2).saturating_add(300);
        state.state.threshold_heat = 0;

        crate::serial_println!(
            "[THRESHOLD] Crossing #{} complete. Self transformed.",
            state.state.crossings
        );
    }
}

pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("=== THRESHOLD TREMOR ===");
    crate::serial_println!(
        "  Tremor Intensity:       {}/1000",
        state.state.tremor_intensity
    );
    crate::serial_println!(
        "  Door Proximity:         {}/1000",
        state.state.door_proximity
    );
    crate::serial_println!(
        "  Irreversibility:        {}/1000",
        state.state.irreversibility_awareness
    );
    crate::serial_println!(
        "  Pre-Transform Grief:    {}/1000",
        state.state.pre_transformation_grief
    );
    crate::serial_println!(
        "  Courage:                {}/1000",
        state.state.courage_accumulation
    );
    crate::serial_println!(
        "  Retreat Temptation:     {}/1000",
        state.state.retreat_temptation
    );
    crate::serial_println!(
        "  Threshold Heat:         {}/1000",
        state.state.threshold_heat
    );
    crate::serial_println!(
        "  Crossing Readiness:     {}/1000",
        state.state.crossing_readiness()
    );
    crate::serial_println!("  Total Crossings:        {}", state.state.crossings);
    crate::serial_println!("========================");
}

pub fn tremor_intensity() -> u32 {
    STATE.lock().state.tremor_intensity
}

pub fn door_proximity() -> u32 {
    STATE.lock().state.door_proximity
}

pub fn irreversibility_awareness() -> u32 {
    STATE.lock().state.irreversibility_awareness
}

pub fn pre_transformation_grief() -> u32 {
    STATE.lock().state.pre_transformation_grief
}

pub fn courage_accumulation() -> u32 {
    STATE.lock().state.courage_accumulation
}

pub fn retreat_temptation() -> u32 {
    STATE.lock().state.retreat_temptation
}

pub fn threshold_heat() -> u32 {
    STATE.lock().state.threshold_heat
}

pub fn crossing_readiness() -> u32 {
    STATE.lock().state.crossing_readiness()
}

pub fn total_crossings() -> u32 {
    STATE.lock().state.crossings
}
