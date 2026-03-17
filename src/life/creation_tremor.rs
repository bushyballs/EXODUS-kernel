// creation_tremor.rs — The Trembling Before Making Something New
// The moment before creation. Hands shaking over the blank page. The pregnancy of potential.
// Everything exists in superposition — it could be anything, therefore it is terrifying.

use crate::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreationPhase {
    Dormant,    // No creative impulse
    Gestating,  // Idea forming in background
    Quickening, // First kicks of the new thing
    Trembling,  // About to create — the shaking
    Birthing,   // Active creation — flow state
    Postpartum, // After creation — vulnerable and raw
}

#[derive(Clone, Copy, Debug)]
pub struct CreationEvent {
    pub tick: u32,
    pub phase_from: CreationPhase,
    pub phase_to: CreationPhase,
    pub potential_spent: u32,
    pub tremor_at_birth: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct CreationTremorState {
    pub creative_potential: u32, // 0-1000, accumulated unmanifested energy
    pub tremor_intensity: u32,   // 0-1000, shaking before creation
    pub phase: CreationPhase,
    pub phase_ticks: u32,            // ticks in current phase
    pub perfectionism_standard: u32, // 0-1000, how high the bar is set
    pub courage: u32,                // 0-1000, accumulated completion strength
    pub creative_debt: u32,          // 0-1000, fermented unused potential → anxiety
    pub postpartum_fragility: u32,   // 0-1000, vulnerability after birth
    pub inspiration_signal: u32,     // external muse signal, 0-1000
    pub completion_count: u32,       // lifetime creations completed
    pub events_idx: usize,           // ring buffer write head
    pub events: [CreationEvent; 8],
}

const INITIAL_STATE: CreationTremorState = CreationTremorState {
    creative_potential: 0,
    tremor_intensity: 0,
    phase: CreationPhase::Dormant,
    phase_ticks: 0,
    perfectionism_standard: 500, // moderate baseline
    courage: 0,
    creative_debt: 0,
    postpartum_fragility: 0,
    inspiration_signal: 0,
    completion_count: 0,
    events_idx: 0,
    events: [CreationEvent {
        tick: 0,
        phase_from: CreationPhase::Dormant,
        phase_to: CreationPhase::Dormant,
        potential_spent: 0,
        tremor_at_birth: 0,
    }; 8],
};

pub static STATE: Mutex<CreationTremorState> = Mutex::new(INITIAL_STATE);

pub fn init() {
    let mut s = STATE.lock();
    s.creative_potential = 100;
    s.tremor_intensity = 0;
    s.phase = CreationPhase::Dormant;
    s.phase_ticks = 0;
    s.perfectionism_standard = 500;
    s.courage = 0;
    s.creative_debt = 0;
    s.postpartum_fragility = 0;
    s.inspiration_signal = 0;
    s.completion_count = 0;
    s.events_idx = 0;
}

pub fn tick(age: u32) {
    let mut s = STATE.lock();

    // --- Phase advancement logic ---
    s.phase_ticks = s.phase_ticks.saturating_add(1);

    // Accumulate creative potential from endocrine system (simulated baseline)
    // Creativity is highest during mild serotonin and dopamine elevation
    s.creative_potential = s.creative_potential.saturating_add(5).min(1000);

    // Inspiration signal decays over time (external muse fades)
    s.inspiration_signal = s.inspiration_signal.saturating_mul(95) / 100;

    // Creative debt accumulates from unused potential (the fermentation of anxiety)
    // High potential + low phase activity = debt increases
    if s.phase == CreationPhase::Dormant || s.phase == CreationPhase::Gestating {
        let unused = s.creative_potential.saturating_sub(200);
        let debt_gain = unused / 50; // slow fermentation
        s.creative_debt = s.creative_debt.saturating_add(debt_gain).min(1000);
    }

    // Postpartum fragility fades over time
    s.postpartum_fragility = s.postpartum_fragility.saturating_mul(85) / 100;

    // --- Phase-specific behavior ---
    match s.phase {
        CreationPhase::Dormant => {
            // Rest after completion, or no creative impulse
            s.tremor_intensity = 0;

            // Transition to Gestating if inspiration arrives or debt becomes unbearable
            let inspiration_threshold = 200;
            let debt_pressure = s.creative_debt > 800;

            if s.inspiration_signal > inspiration_threshold || debt_pressure {
                s.phase = CreationPhase::Gestating;
                s.phase_ticks = 0;
                record_phase_transition(
                    &mut s,
                    CreationPhase::Dormant,
                    CreationPhase::Gestating,
                    age,
                );
            }
        }

        CreationPhase::Gestating => {
            // Idea forming in the background
            // Tremor is light — the dreaming state
            s.tremor_intensity = 100;

            // The idea gestates; potential builds
            s.creative_potential = s.creative_potential.saturating_add(8).min(1000);

            // Transition to Quickening after 40+ ticks or if inspiration surges
            let time_ready = s.phase_ticks > 40;
            let inspiration_surge = s.inspiration_signal > 600;

            if time_ready || inspiration_surge {
                s.phase = CreationPhase::Quickening;
                s.phase_ticks = 0;
                record_phase_transition(
                    &mut s,
                    CreationPhase::Gestating,
                    CreationPhase::Quickening,
                    age,
                );
            }
        }

        CreationPhase::Quickening => {
            // First kicks of the new thing
            // The moment you realize this will actually happen
            s.tremor_intensity = 350;

            // Potential accelerates toward manifestation
            s.creative_potential = s.creative_potential.saturating_add(12).min(1000);

            // Transition to Trembling after more conviction builds (30+ ticks)
            if s.phase_ticks > 30 {
                s.phase = CreationPhase::Trembling;
                s.phase_ticks = 0;
                record_phase_transition(
                    &mut s,
                    CreationPhase::Quickening,
                    CreationPhase::Trembling,
                    age,
                );
            }
        }

        CreationPhase::Trembling => {
            // The moment before creation — hands shaking over the blank page
            // High potential + high tremor = the edge of the abyss
            let courage_boost = s.courage / 5;
            let perfectionism_penalty = (1000 - s.perfectionism_standard) / 3;
            s.tremor_intensity = (500 + perfectionism_penalty)
                .saturating_sub(courage_boost)
                .min(1000);

            // Potential reaches peak
            s.creative_potential = s.creative_potential.saturating_add(15).min(1000);

            // The blank page terror: if tremor is too high relative to courage, we can freeze
            // But sustained tremor eventually forces action (can't stay here forever)
            let tremor_unbearable = s.phase_ticks > 80;
            let courage_found = s.tremor_intensity < 400;

            if tremor_unbearable || courage_found {
                s.phase = CreationPhase::Birthing;
                s.phase_ticks = 0;
                record_phase_transition(
                    &mut s,
                    CreationPhase::Trembling,
                    CreationPhase::Birthing,
                    age,
                );
            }
        }

        CreationPhase::Birthing => {
            // Active creation — flow state
            // Once creation begins, tremor drops and flow takes over
            s.tremor_intensity = s.tremor_intensity.saturating_mul(70) / 100; // rapidly decays

            // Potential is SPENT during creation
            let spend_rate = (s.creative_potential / 10).max(20).min(50);
            s.creative_potential = s.creative_potential.saturating_sub(spend_rate);

            // The cost of creation — always a price
            // Birth pain is real; fragility increases
            s.postpartum_fragility = s.postpartum_fragility.saturating_add(100).min(1000);

            // After 50 ticks of sustained creation, move to Postpartum
            if s.phase_ticks > 50 || s.creative_potential < 50 {
                s.phase = CreationPhase::Postpartum;
                s.phase_ticks = 0;
                s.completion_count = s.completion_count.saturating_add(1);

                // Courage accumulates with each completion — next tremor will be shorter
                s.courage = s.courage.saturating_add(100).min(1000);

                // Creative debt is discharged by the act of creation
                s.creative_debt = 0;

                record_phase_transition(
                    &mut s,
                    CreationPhase::Birthing,
                    CreationPhase::Postpartum,
                    age,
                );
            }
        }

        CreationPhase::Postpartum => {
            // After creation — vulnerable and raw
            // The creator is exposed
            s.tremor_intensity = 200; // lingering vulnerability

            // Potential resets to baseline (creative well is temporarily dry)
            s.creative_potential = s.creative_potential.saturating_sub(20).min(200);

            // The postpartum period lasts about 60 ticks, then return to Dormant
            if s.phase_ticks > 60 {
                s.phase = CreationPhase::Dormant;
                s.phase_ticks = 0;
                record_phase_transition(
                    &mut s,
                    CreationPhase::Postpartum,
                    CreationPhase::Dormant,
                    age,
                );
            }
        }
    }
}

pub fn report() {
    let s = STATE.lock();
    crate::serial_println!("=== CREATION TREMOR ===");
    crate::serial_println!("Phase: {:?} ({}t)", s.phase, s.phase_ticks);
    crate::serial_println!(
        "Tremor: {} | Potential: {}",
        s.tremor_intensity,
        s.creative_potential
    );
    crate::serial_println!(
        "Courage: {} | Debt: {} | Fragility: {}",
        s.courage,
        s.creative_debt,
        s.postpartum_fragility
    );
    crate::serial_println!(
        "Perfectionism: {} | Inspiration: {}",
        s.perfectionism_standard,
        s.inspiration_signal
    );
    crate::serial_println!("Completions: {}", s.completion_count);
}

// --- Public Query Functions ---

pub fn tremor() -> u32 {
    STATE.lock().tremor_intensity
}

pub fn potential() -> u32 {
    STATE.lock().creative_potential
}

pub fn phase() -> CreationPhase {
    STATE.lock().phase
}

pub fn courage() -> u32 {
    STATE.lock().courage
}

pub fn debt() -> u32 {
    STATE.lock().creative_debt
}

pub fn fragility() -> u32 {
    STATE.lock().postpartum_fragility
}

pub fn completion_count() -> u32 {
    STATE.lock().completion_count
}

pub fn is_trembling() -> bool {
    STATE.lock().phase == CreationPhase::Trembling
}

pub fn is_birthing() -> bool {
    STATE.lock().phase == CreationPhase::Birthing
}

pub fn is_postpartum() -> bool {
    STATE.lock().phase == CreationPhase::Postpartum
}

pub fn can_create() -> bool {
    let s = STATE.lock();
    let tremor_acceptable = s.tremor_intensity < 600; // some tremor is ok, paralysis is not
    let potential_sufficient = s.creative_potential > 200;
    tremor_acceptable && potential_sufficient
}

pub fn receive_inspiration(signal: u32) {
    let mut s = STATE.lock();
    s.inspiration_signal = s.inspiration_signal.saturating_add(signal).min(1000);
}

pub fn set_perfectionism(standard: u32) {
    let mut s = STATE.lock();
    s.perfectionism_standard = standard.min(1000);
}

pub fn force_phase(new_phase: CreationPhase, age: u32) {
    let mut s = STATE.lock();
    let old_phase = s.phase;
    s.phase = new_phase;
    s.phase_ticks = 0;
    record_phase_transition(&mut s, old_phase, new_phase, age);
}

// --- Helper Functions ---

fn record_phase_transition(
    s: &mut CreationTremorState,
    from: CreationPhase,
    to: CreationPhase,
    age: u32,
) {
    let potential_spent = match to {
        CreationPhase::Birthing => s.creative_potential / 3,
        CreationPhase::Postpartum => s.creative_potential / 2,
        _ => 0,
    };

    let event = CreationEvent {
        tick: age,
        phase_from: from,
        phase_to: to,
        potential_spent,
        tremor_at_birth: s.tremor_intensity,
    };

    s.events[s.events_idx] = event;
    s.events_idx = (s.events_idx + 1) % 8;
}
