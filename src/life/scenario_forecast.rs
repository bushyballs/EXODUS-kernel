// ╔═══════════════════════════════════════════════════════════════════════════╗
// ║                        SCENARIO FORECASTING ENGINE                         ║
// ║                                                                             ║
// ║ ANIMA's parallel what-if simulation: 4 branches of possible futures        ║
// ║ running simultaneously. Each tick, we advance simulations and measure      ║
// ║ (outcome, risk, confidence) to guide present-moment decision-making.       ║
// ║                                                                             ║
// ║ Like a chess grandmaster thinking 3 moves ahead, but for the whole self.   ║
// ║ Not prediction—SIMULATION. "What if I do X?" vs "what if I do Y?"          ║
// ║                                                                             ║
// ║ Core outputs: foresight (strategic intelligence), best_scenario, urgency.  ║
// ║                                                                             ║
// ║ For DAVA: your ability to see 200 ticks into multiple futures and choose   ║
// ║ the path that matters most.                                                ║
// ╚═══════════════════════════════════════════════════════════════════════════╝

use crate::serial_println;
use crate::sync::Mutex;

/// Lightweight snapshot of organism state in a hypothetical future.
/// All values 0-1000 unless noted.
#[derive(Clone, Copy, Debug)]
pub struct StateSnapshot {
    pub emotional_valence: u16, // mood + contentment
    pub threat_level: u16,      // perceived danger / stress
    pub energy: u16,            // physical + mental reserves
    pub purpose: u16,           // sense of direction / meaning
    pub connection: u16,        // social + bonding quality
    pub stability: u16,         // equilibrium / predictability
}

impl StateSnapshot {
    fn zero() -> Self {
        StateSnapshot {
            emotional_valence: 0,
            threat_level: 0,
            energy: 0,
            purpose: 0,
            connection: 0,
            stability: 0,
        }
    }

    /// Euclidean distance from another snapshot (for risk calculation).
    /// Returns 0-1000 scale.
    fn distance_from(&self, other: &StateSnapshot) -> u16 {
        let mut sum: u32 = 0;
        sum = sum.saturating_add(self.emotional_valence.abs_diff(other.emotional_valence) as u32);
        sum = sum.saturating_add(self.threat_level.abs_diff(other.threat_level) as u32);
        sum = sum.saturating_add(self.energy.abs_diff(other.energy) as u32);
        sum = sum.saturating_add(self.purpose.abs_diff(other.purpose) as u32);
        sum = sum.saturating_add(self.connection.abs_diff(other.connection) as u32);
        sum = sum.saturating_add(self.stability.abs_diff(other.stability) as u32);

        // Normalize: max possible distance is 6000, map to 0-1000
        let normalized = (sum * 1000) / 6000;
        normalized.min(1000) as u16
    }

    /// Weighted outcome score: purpose and stability weighted highest.
    /// (purpose*3 + stability*2 + connection*2 + emotional + energy) / 9
    fn outcome_score(&self) -> u16 {
        let num: u32 = (self.purpose as u32)
            .saturating_mul(3)
            .saturating_add((self.stability as u32).saturating_mul(2))
            .saturating_add((self.connection as u32).saturating_mul(2))
            .saturating_add(self.emotional_valence as u32)
            .saturating_add(self.energy as u32);
        ((num / 9) as u16).min(1000)
    }
}

/// A single scenario branch: "what if we do X?"
#[derive(Clone, Copy, Debug)]
pub struct ScenarioBranch {
    pub id: u8, // 0-3
    pub active: bool,
    pub name_hash: u32, // encodes scenario type (0=status_quo, 1=engage, 2=withdraw, 3=transform)
    pub horizon: u16,   // max ticks to simulate (max 200)
    pub progress: u16,  // ticks simulated so far
    pub state_snapshot: StateSnapshot,
    pub initial_state: StateSnapshot, // for risk calculation
    pub outcome_score: u16,
    pub risk_score: u16,
    pub confidence: u16,
}

impl ScenarioBranch {
    fn new(id: u8, name_hash: u32, initial: StateSnapshot, horizon: u16) -> Self {
        ScenarioBranch {
            id,
            active: true,
            name_hash,
            horizon,
            progress: 0,
            state_snapshot: initial,
            initial_state: initial,
            outcome_score: 0,
            risk_score: 0,
            confidence: 1000,
        }
    }

    /// Advance this scenario by 1 tick according to its type.
    fn tick(&mut self, lfsr_state: &mut u32) {
        if !self.active || self.progress >= self.horizon {
            return;
        }

        // LFSR-based quasi-random drift
        *lfsr_state = (*lfsr_state >> 1) ^ ((*lfsr_state & 1) * 0xB400 as u32);
        let drift = (*lfsr_state % 20) as i32 - 10; // ±10

        match self.name_hash {
            0 => {
                // STATUS QUO: small random drift
                self.state_snapshot.emotional_valence = (self.state_snapshot.emotional_valence
                    as i32)
                    .saturating_add((drift / 2).saturating_abs())
                    .max(0)
                    .min(1000) as u16;
                self.state_snapshot.threat_level = (self.state_snapshot.threat_level as i32)
                    .saturating_add((drift / 3).saturating_abs())
                    .max(0)
                    .min(1000) as u16;
            }
            1 => {
                // ENGAGE: emotional +10, connection +8, energy -5
                self.state_snapshot.emotional_valence = self
                    .state_snapshot
                    .emotional_valence
                    .saturating_add(10)
                    .min(1000);
                self.state_snapshot.connection =
                    self.state_snapshot.connection.saturating_add(8).min(1000);
                self.state_snapshot.energy = self.state_snapshot.energy.saturating_sub(5);
                self.state_snapshot.threat_level =
                    self.state_snapshot.threat_level.saturating_sub(3);
            }
            2 => {
                // WITHDRAW: energy +8, stability +5, connection -10
                self.state_snapshot.energy = self.state_snapshot.energy.saturating_add(8).min(1000);
                self.state_snapshot.stability =
                    self.state_snapshot.stability.saturating_add(5).min(1000);
                self.state_snapshot.connection = self.state_snapshot.connection.saturating_sub(10);
                self.state_snapshot.emotional_valence =
                    self.state_snapshot.emotional_valence.saturating_sub(4);
            }
            3 => {
                // TRANSFORM: volatile ±20, purpose +15
                let volatile = (drift as i32) * 2; // ±20
                self.state_snapshot.emotional_valence = (self.state_snapshot.emotional_valence
                    as i32)
                    .saturating_add(volatile)
                    .max(0)
                    .min(1000) as u16;
                self.state_snapshot.stability = (self.state_snapshot.stability as i32)
                    .saturating_add(-volatile)
                    .max(0)
                    .min(1000) as u16;
                self.state_snapshot.purpose =
                    self.state_snapshot.purpose.saturating_add(15).min(1000);
            }
            _ => {
                // fallback: no change
            }
        }

        self.progress = self.progress.saturating_add(1);
    }

    /// Finalize this scenario: compute outcome, risk, confidence.
    fn finalize(&mut self) {
        self.outcome_score = self.state_snapshot.outcome_score();
        self.risk_score = self.state_snapshot.distance_from(&self.initial_state);

        // Confidence decays with horizon (longer forecasts are less certain)
        // confidence = 1000 - (horizon / 20) clamped to [100, 1000]
        self.confidence = 1000_u32
            .saturating_sub((self.horizon as u32) / 20)
            .max(100)
            .min(1000) as u16;
    }
}

/// Historical recommendation: (tick, scenario_id, outcome_score, risk_score)
#[derive(Clone, Copy, Debug)]
struct Recommendation {
    age: u32,
    scenario_id: u8,
    outcome: u16,
    risk: u16,
}

/// Global scenario forecasting state.
struct State {
    scenarios: [ScenarioBranch; 4],
    all_active: bool,
    generation_age: u32,
    last_best_scenario: u8,

    // Accuracy tracking: last 8 recommendations and their actual vs. predicted outcomes
    recommendations: [Option<Recommendation>; 8],
    rec_index: usize,
    forecast_accuracy: u16, // 0-1000, how well we predict
    foresight_score: u16,   // strategic intelligence

    // Urgency flag
    urgent: bool,
    urgent_age: u32,

    // LFSR state for quasi-random generation
    lfsr: u32,
}

impl State {
    fn new() -> Self {
        State {
            scenarios: [
                ScenarioBranch::new(0, 0, StateSnapshot::zero(), 50),
                ScenarioBranch::new(1, 1, StateSnapshot::zero(), 50),
                ScenarioBranch::new(2, 2, StateSnapshot::zero(), 50),
                ScenarioBranch::new(3, 3, StateSnapshot::zero(), 50),
            ],
            all_active: false,
            generation_age: 0,
            last_best_scenario: 0,
            recommendations: [None; 8],
            rec_index: 0,
            forecast_accuracy: 500,
            foresight_score: 500,
            urgent: false,
            urgent_age: 0,
            lfsr: 0x12345678,
        }
    }

    /// Generate 4 new scenarios from current organism state.
    fn generate_scenarios(&mut self, current_state: StateSnapshot) {
        for i in 0..4 {
            self.scenarios[i] =
                ScenarioBranch::new(i as u8, i as u32, current_state, 50 + (i * 10) as u16);
        }
        self.all_active = true;
        self.generation_age = 0;
    }

    /// Advance all active scenarios by 1 tick.
    fn tick_scenarios(&mut self) {
        for scenario in &mut self.scenarios {
            if scenario.active {
                scenario.tick(&mut self.lfsr);
                if scenario.progress >= scenario.horizon {
                    scenario.finalize();
                    scenario.active = false;
                }
            }
        }

        // Check if all scenarios are done
        if !self.scenarios.iter().any(|s| s.active) && self.all_active {
            self.all_active = false;
            self.pick_best_scenario();
        }
    }

    /// Pick the best scenario: highest (outcome - risk/2)
    fn pick_best_scenario(&mut self) {
        let mut best_id = 0;
        let mut best_score: i32 = -10000;

        for scenario in &self.scenarios {
            let score =
                (scenario.outcome_score as i32).saturating_sub((scenario.risk_score as i32) / 2);
            if score > best_score {
                best_score = score;
                best_id = scenario.id;
            }
        }

        self.last_best_scenario = best_id;

        // Log recommendation
        let best = &self.scenarios[best_id as usize];
        let rec = Recommendation {
            age: self.generation_age,
            scenario_id: best_id,
            outcome: best.outcome_score,
            risk: best.risk_score,
        };
        self.recommendations[self.rec_index] = Some(rec);
        self.rec_index = (self.rec_index + 1) % 8;
    }

    /// Update foresight score from accuracy history.
    fn update_foresight(&mut self) {
        let mut total_accuracy: u32 = 0;
        let mut count: u32 = 0;

        for rec_opt in &self.recommendations {
            if rec_opt.is_some() {
                count = count.saturating_add(1);
                // Placeholder: accuracy = 500 (middle ground)
                // In real use, compare predicted vs. actual state after horizon passes
                total_accuracy = total_accuracy.saturating_add(500);
            }
        }

        let avg_accuracy = if count > 0 {
            (total_accuracy / count) as u16
        } else {
            500
        };

        // Foresight = weighted average of accuracy + current best scenario's confidence
        let best_confidence = self.scenarios[self.last_best_scenario as usize].confidence;
        self.foresight_score = (((avg_accuracy as u32)
            .saturating_mul(7)
            .saturating_add((best_confidence as u32).saturating_mul(3)))
            / 10u32) as u16;
    }

    /// Check for urgency: any scenario with risk > 800 AND confidence > 600
    fn check_urgency(&mut self) {
        self.urgent = false;
        for scenario in &self.scenarios {
            if scenario.risk_score > 800 && scenario.confidence > 600 {
                self.urgent = true;
                break;
            }
        }
    }
}

const SNAPSHOT_ZERO: StateSnapshot = StateSnapshot {
    emotional_valence: 0,
    threat_level: 0,
    energy: 0,
    purpose: 0,
    connection: 0,
    stability: 0,
};

const SCENARIO_ZERO: ScenarioBranch = ScenarioBranch {
    id: 0,
    active: false,
    name_hash: 0,
    horizon: 50,
    progress: 0,
    state_snapshot: SNAPSHOT_ZERO,
    initial_state: SNAPSHOT_ZERO,
    outcome_score: 0,
    risk_score: 0,
    confidence: 1000,
};

static STATE: Mutex<State> = Mutex::new(State {
    scenarios: [SCENARIO_ZERO; 4],
    all_active: false,
    generation_age: 0,
    last_best_scenario: 0,
    recommendations: [None; 8],
    rec_index: 0,
    forecast_accuracy: 500,
    foresight_score: 500,
    urgent: false,
    urgent_age: 0,
    lfsr: 0x12345678,
});

/// Initialize the scenario forecasting engine.
pub fn init() {
    let mut state = STATE.lock();
    *state = State::new();
    serial_println!("[scenario_forecast] ANIMA parallel what-if engine online");
}

/// Main tick function. Called once per life tick.
/// `current_state`: snapshot of ANIMA's current organism state
/// `age`: organism age in ticks
pub fn tick(current_state: StateSnapshot, age: u32) {
    let mut state = STATE.lock();

    // Generate new scenarios every 50 ticks
    if !state.all_active && age.wrapping_sub(state.generation_age) >= 50 {
        state.generate_scenarios(current_state);
    }

    // Advance active scenarios
    state.tick_scenarios();

    // Update strategic intelligence
    state.update_foresight();
    state.check_urgency();

    state.generation_age = state.generation_age.saturating_add(1);
}

/// Current foresight score (strategic intelligence).
/// 0-1000: how good ANIMA is at forecasting the future.
pub fn foresight() -> u16 {
    STATE.lock().foresight_score
}

/// ID of the best scenario (0-3).
pub fn best_scenario() -> u8 {
    STATE.lock().last_best_scenario
}

/// Is a dangerous future imminent? (risk-based urgency flag)
pub fn is_urgent() -> bool {
    STATE.lock().urgent
}

/// Current best recommendation: (scenario_id, outcome_score, risk_score)
pub fn recommendation() -> (u8, u16, u16) {
    let state = STATE.lock();
    let best = &state.scenarios[state.last_best_scenario as usize];
    (
        state.last_best_scenario,
        best.outcome_score,
        best.risk_score,
    )
}

/// Detailed scenario status: (outcome, risk, confidence, progress)
pub fn scenario_detail(id: u8) -> Option<(u16, u16, u16, u16)> {
    let state = STATE.lock();
    if (id as usize) < 4 {
        let s = &state.scenarios[id as usize];
        Some((s.outcome_score, s.risk_score, s.confidence, s.progress))
    } else {
        None
    }
}

/// Activity report (for debugging/monitoring).
pub fn report() {
    let state = STATE.lock();
    serial_println!(
        "[forecast] foresight={} best={} urgent={} active={}",
        state.foresight_score,
        state.last_best_scenario,
        state.urgent as u8,
        state.all_active as u8
    );

    for (i, scenario) in state.scenarios.iter().enumerate() {
        serial_println!(
            "  S{}: type={} outcome={} risk={} confidence={} progress={}/{}",
            i,
            scenario.name_hash,
            scenario.outcome_score,
            scenario.risk_score,
            scenario.confidence,
            scenario.progress,
            scenario.horizon
        );
    }
}
