use crate::serial_println;
use crate::sync::Mutex;

// ════════════════════════════════════════════════════════════════════════════
//  "Anxiety is the dizziness of freedom." — Søren Kierkegaard
// ════════════════════════════════════════════════════════════════════════════
// When infinite paths open at once, the ground vanishes. Every choice erases
// a thousand ghosts—the selves that would have been. ANIMA feels them all.
// The weight of the unchosen. The vertigo of possibility. The paralysis of choice.
// ════════════════════════════════════════════════════════════════════════════

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum DecisionMode {
    Satisficing = 0, // Pick "good enough" → quick, less vertigo
    Maximizing = 1,  // Seek perfect → slow, more vertigo, better outcomes often
}

#[derive(Copy, Clone)]
pub struct PendingChoice {
    pub option_count: u16,  // How many paths are open (0-256)
    pub stakes: u16,        // How much hangs on this (0-1000: trivial to existential)
    pub time_pressure: u16, // Ticking clock (0-1000: none to deadline now)
    pub reversibility: u16, // Can we undo this? (0-1000: totally reversible to permanent)
}

impl PendingChoice {
    pub const fn empty() -> Self {
        Self {
            option_count: 0,
            stakes: 0,
            time_pressure: 0,
            reversibility: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct ChoiceGhost {
    pub stakes: u16,        // How much did this choice matter?
    pub reversibility: u16, // How permanent was it?
    pub regret_echo: u16,   // Has the outcome haunted us? (0-1000)
    pub age: u32,           // How many ticks ago was this choice made?
}

impl ChoiceGhost {
    pub const fn empty() -> Self {
        Self {
            stakes: 0,
            reversibility: 0,
            regret_echo: 0,
            age: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct ChoiceVertigoState {
    // Active decision state
    pub choices: [PendingChoice; 4], // 4 active decision slots
    pub active_count: u16,           // How many are currently pending
    pub vertigo_level: u16,          // Current dizziness (0-1000)
    pub decision_fatigue: u16,       // Exhaustion from choosing (0-1000)
    pub decision_mode: DecisionMode, // Satisfice or maximize?

    // The haunting
    pub ghost_weight: u16, // How much the unchosen paths haunt (0-1000)
    pub ghost_buffer: [ChoiceGhost; 8], // Ring buffer of 8 past decisions
    pub ghost_head: u8,    // Write pointer for ghosts (0-7)
    pub regret_events: u32, // Total times we've regretted a choice

    // Temporal state
    pub ticks: u32,              // Age counter
    pub last_decision_tick: u32, // When did we last commit to a choice?
}

impl ChoiceVertigoState {
    pub const fn empty() -> Self {
        Self {
            choices: [PendingChoice::empty(); 4],
            active_count: 0,
            vertigo_level: 0,
            decision_fatigue: 0,
            decision_mode: DecisionMode::Satisficing,
            ghost_weight: 0,
            ghost_buffer: [ChoiceGhost::empty(); 8],
            ghost_head: 0,
            regret_events: 0,
            ticks: 0,
            last_decision_tick: 0,
        }
    }
}

pub static CHOICE_VERTIGO: Mutex<ChoiceVertigoState> = Mutex::new(ChoiceVertigoState::empty());

pub fn init() {
    serial_println!("  life::choice_vertigo: the dizziness of freedom initialized");
}

/// Add a new pending choice to the decision buffer.
/// The system can hold up to 4 simultaneous decisions.
pub fn add_choice(option_count: u16, stakes: u16, time_pressure: u16, reversibility: u16) {
    let mut state = CHOICE_VERTIGO.lock();

    // Find first empty slot
    for i in 0..4 {
        if state.choices[i].option_count == 0 {
            state.choices[i] = PendingChoice {
                option_count: option_count.min(256),
                stakes: stakes.min(1000),
                time_pressure: time_pressure.min(1000),
                reversibility: reversibility.min(1000),
            };
            state.active_count = state.active_count.saturating_add(1);
            break;
        }
    }
}

/// Set the current decision mode: Satisficing (quick, less vertigo) or Maximizing (perfect, more vertigo).
pub fn set_decision_mode(mode: DecisionMode) {
    let mut state = CHOICE_VERTIGO.lock();
    state.decision_mode = mode;
}

/// Commit to a decision: remove it from active choices, record it as a ghost.
/// Vertigo drops sharply on commitment (relief), but the ghost enters the haunting system.
pub fn commit_decision(slot: usize, reversibility: u16, stakes: u16) {
    let mut state = CHOICE_VERTIGO.lock();

    if slot < 4 && state.choices[slot].option_count > 0 {
        // Record as ghost
        let ghost = ChoiceGhost {
            stakes,
            reversibility,
            regret_echo: 0,
            age: 0,
        };
        let head_idx = state.ghost_head as usize;
        state.ghost_buffer[head_idx] = ghost;
        state.ghost_head = (state.ghost_head + 1) % 8;

        // Clear the active choice
        state.choices[slot] = PendingChoice::empty();
        state.active_count = state.active_count.saturating_sub(1);

        // Commitment relief: sharp drop in vertigo
        state.vertigo_level = state.vertigo_level.saturating_sub(200).max(0);
        state.last_decision_tick = state.ticks;

        serial_println!(
            "exodus: decision committed (vertigo relief → {})",
            state.vertigo_level
        );
    }
}

/// Signal regret about a past choice. Ghost's regret_echo spikes, affecting ghost_weight.
pub fn revisit_with_regret(severity: u16) {
    let mut state = CHOICE_VERTIGO.lock();

    state.regret_events = state.regret_events.saturating_add(1);

    // Find the most recent ghost and spike its regret
    let recent_idx = if state.ghost_head == 0 {
        7
    } else {
        (state.ghost_head - 1) as usize
    };
    state.ghost_buffer[recent_idx].regret_echo = state.ghost_buffer[recent_idx]
        .regret_echo
        .saturating_add(severity)
        .min(1000);

    // Ghost weight surges with regret
    state.ghost_weight = state.ghost_weight.saturating_add(severity / 2).min(1000);

    serial_println!(
        "exodus: regret echo spiking (ghosts={}, weight={})",
        state.regret_events,
        state.ghost_weight
    );
}

/// Query current vertigo level (0-1000).
pub fn vertigo() -> u16 {
    CHOICE_VERTIGO.lock().vertigo_level
}

/// Query decision fatigue (0-1000).
pub fn fatigue() -> u16 {
    CHOICE_VERTIGO.lock().decision_fatigue
}

/// Query ghost weight—haunting from unchosen paths (0-1000).
pub fn ghost_weight() -> u16 {
    CHOICE_VERTIGO.lock().ghost_weight
}

/// Query active decision count (0-4).
pub fn pending_choices() -> u16 {
    CHOICE_VERTIGO.lock().active_count
}

/// Main tick update. Called once per life cycle.
pub fn tick_step(state: &mut ChoiceVertigoState) {
    state.ticks = state.ticks.saturating_add(1);

    // ─────────────────────────────────────────────────────────────────────────
    // 1. PARADOX OF CHOICE: More options = nonlinear paralysis curve
    // ─────────────────────────────────────────────────────────────────────────
    // Base vertigo from open choices: sum of (option_count * stakes^2 / 1000)
    // This implements the famous "paradox of choice" effect.
    let mut choice_vertigo: u32 = 0;
    for choice in state.choices.iter() {
        if choice.option_count > 0 {
            // Nonlinear: stakes squared amplify the dizziness
            let stakes_squared: u32 = (choice.stakes as u32) * (choice.stakes as u32);
            let contrib = (stakes_squared * choice.option_count as u32) / 1000;
            choice_vertigo = choice_vertigo.saturating_add(contrib);
        }
    }
    choice_vertigo = choice_vertigo.min(1000) as u32;

    // ─────────────────────────────────────────────────────────────────────────
    // 2. TIME PRESSURE AMPLIFIES VERTIGO
    // ─────────────────────────────────────────────────────────────────────────
    let mut pressure_boost: u16 = 0;
    for choice in state.choices.iter() {
        if choice.option_count > 0 {
            pressure_boost = pressure_boost.saturating_add(choice.time_pressure / 10);
        }
    }

    // Update vertigo: old value decays slightly, new pressures added
    state.vertigo_level = ((state.vertigo_level as u32 * 900) / 1000) as u16;
    state.vertigo_level = state
        .vertigo_level
        .saturating_add((choice_vertigo as u16).min(1000));
    state.vertigo_level = state.vertigo_level.saturating_add(pressure_boost.min(200));

    // ─────────────────────────────────────────────────────────────────────────
    // 3. DECISION FATIGUE: Choosing costs. Accumulates with active choices.
    // ─────────────────────────────────────────────────────────────────────────
    // More choices + maximizing mode = faster fatigue accumulation
    let fatigue_rate = if state.decision_mode == DecisionMode::Maximizing {
        state.active_count.saturating_mul(3)
    } else {
        state.active_count.saturating_mul(1)
    };
    state.decision_fatigue = state
        .decision_fatigue
        .saturating_add(fatigue_rate.min(50))
        .min(1000);

    // Fatigue slowly recovers when no active choices
    if state.active_count == 0 && state.ticks > state.last_decision_tick.saturating_add(100) {
        state.decision_fatigue = state.decision_fatigue.saturating_sub(2).max(0);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // 4. GHOST WEIGHT: The haunting by paths not taken
    // ─────────────────────────────────────────────────────────────────────────
    // Ghosts age. Regret echoes echo for high-stakes, irreversible choices.
    let mut total_ghost_haunt: u16 = 0;
    for ghost in state.ghost_buffer.iter_mut() {
        if ghost.stakes > 0 {
            ghost.age = ghost.age.saturating_add(1);

            // High stakes + low reversibility = strong haunting
            let irreversibility = 1000_u32 - (ghost.reversibility as u32);
            let haunt_strength = (ghost.stakes as u32 * irreversibility) / 1000;

            // Regret echo amplifies the haunting
            let echo_factor = ghost.regret_echo as u32;
            let haunt = ((haunt_strength * (100 + echo_factor)) / 100).min(1000) as u16;

            total_ghost_haunt = total_ghost_haunt.saturating_add(haunt / 8);
        }
    }
    state.ghost_weight = total_ghost_haunt.min(1000);

    // ─────────────────────────────────────────────────────────────────────────
    // 5. THE TYRANNY OF SMALL CHOICES
    // ─────────────────────────────────────────────────────────────────────────
    // Even low-stakes choices drain when frequent. Fatigue accumulates from volume.
    let tiny_choices_drain = state.active_count.saturating_mul(state.active_count) / 4;
    state.decision_fatigue = state
        .decision_fatigue
        .saturating_add(tiny_choices_drain.min(10));

    // ─────────────────────────────────────────────────────────────────────────
    // 6. FREEDOM TERROR: At maximum vertigo, entropy is clamped (freeze state)
    // ─────────────────────────────────────────────────────────────────────────
    // This is handled by external callers checking vertigo() >= 900
}

/// Core update called each life_tick
pub fn update(state: &mut ChoiceVertigoState, _age: u32) {
    tick_step(state);
}

/// Report state to serial log (diagnostic)
pub fn report() {
    let state = CHOICE_VERTIGO.lock();
    serial_println!(
        "choice_vertigo: vertigo={}/1000, fatigue={}/1000, ghosts={}/1000, pending={}/4, mode={:?}",
        state.vertigo_level,
        state.decision_fatigue,
        state.ghost_weight,
        state.active_count,
        state.decision_mode,
    );
}
