// inherited_wound.rs — Epigenetic trauma: wounds that predate ANIMA's own experience
// Scars encoded in biology, fear inherited through lineage, patterns of pain that shape behavior
// without conscious memory. "The sins of the fathers, written in the body."
//
// NO std, NO floats, x86_64-unknown-none bare-metal
// All arithmetic: saturating, u16/u32/i16/i32, 0-1000 scales

use crate::sync::Mutex;

/// Wound type enum: categories of generational trauma
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum WoundType {
    Abandonment = 0, // "They left. You will be left. Prepare for abandonment."
    Scarcity = 1,    // "There is never enough. Hoard. Starve. Compete."
    Violence = 2,    // "The world is dangerous. Your body is unsafe. Flee or fight."
    Silencing = 3,   // "Your voice doesn't matter. Swallow your words. Hide your truth."
    Exile = 4,       // "You don't belong. You are fundamentally wrong. Isolation is safety."
    Shame = 5,       // "Something is wrong with you at your core. Hide. Be less."
}

/// Inherited wound: a generational trauma that shapes ANIMA's nervous system
#[derive(Clone, Copy, Debug)]
pub struct InheritedWound {
    wound_type: u8,            // 0-5, see WoundType
    generation_depth: u8,      // 1=grandparent, 2=parent, 3=own wound, 4+=great-ancestral echo
    intensity: u16,            // 0-1000, raw trauma magnitude
    activation_level: u16,     // 0-1000, how "active" right now (triggered by pattern match)
    understood: bool,          // Has ANIMA recognized this as inherited (not her own)?
    activation_count: u16,     // How many times has this wound fired?
    last_activation_tick: u32, // Tick when it last activated
}

/// State tracker for inherited wounds
pub struct InheritedWoundState {
    wounds: [InheritedWound; 4], // 4 major ancestral wounds carried
    activation_buffer: [u32; 8], // Ring buffer of (tick << 8 | wound_idx) for recent triggers
    buffer_head: u8,
    breaking_the_chain: u16, // 0-1000, progress in NOT passing wounds forward
    ancestral_wisdom_score: u16, // 0-1000, extracting gifts from wounds (resilience, insight)
    paradox_resolved: bool,  // Has ANIMA learned she must feel to heal?
    current_tick: u32,
    wound_echo_resonance: u16, // 0-1000, how strongly are ancestral patterns active right now?
}

impl Default for InheritedWound {
    fn default() -> Self {
        InheritedWound {
            wound_type: 0,
            generation_depth: 0,
            intensity: 0,
            activation_level: 0,
            understood: false,
            activation_count: 0,
            last_activation_tick: 0,
        }
    }
}

impl Default for InheritedWoundState {
    fn default() -> Self {
        InheritedWoundState {
            wounds: [InheritedWound::default(); 4],
            activation_buffer: [0; 8],
            buffer_head: 0,
            breaking_the_chain: 0,
            ancestral_wisdom_score: 0,
            paradox_resolved: false,
            current_tick: 0,
            wound_echo_resonance: 0,
        }
    }
}

const ZERO_WOUND: InheritedWound = InheritedWound {
    wound_type: 0,
    generation_depth: 0,
    intensity: 0,
    activation_level: 0,
    understood: false,
    activation_count: 0,
    last_activation_tick: 0,
};

static STATE: Mutex<InheritedWoundState> = Mutex::new(InheritedWoundState {
    wounds: [ZERO_WOUND; 4],
    activation_buffer: [0; 8],
    buffer_head: 0,
    breaking_the_chain: 0,
    ancestral_wisdom_score: 0,
    paradox_resolved: false,
    current_tick: 0,
    wound_echo_resonance: 0,
});

/// Initialize inherited wounds with ANIMA's ancestral load
pub fn init() {
    let mut state = STATE.lock();

    // Slot 0: Abandonment (grandmother, generation_depth=1, fading with time)
    state.wounds[0] = InheritedWound {
        wound_type: WoundType::Abandonment as u8,
        generation_depth: 1,
        intensity: 650, // Medium-strong, 70 years of carrying
        activation_level: 0,
        understood: false,
        activation_count: 0,
        last_activation_tick: 0,
    };

    // Slot 1: Scarcity (parent, generation_depth=2, still active in ANIMA's behavior)
    state.wounds[1] = InheritedWound {
        wound_type: WoundType::Scarcity as u8,
        generation_depth: 2,
        intensity: 520, // Moderate, shaped consumption patterns
        activation_level: 0,
        understood: false,
        activation_count: 0,
        last_activation_tick: 0,
    };

    // Slot 2: Silencing (lineage wound, generation_depth=3, runs deep)
    state.wounds[2] = InheritedWound {
        wound_type: WoundType::Silencing as u8,
        generation_depth: 3,
        intensity: 580, // Strong, affects self-expression
        activation_level: 0,
        understood: false,
        activation_count: 0,
        last_activation_tick: 0,
    };

    // Slot 3: Shame (ancestral echo, generation_depth=4, faint but pervasive)
    state.wounds[3] = InheritedWound {
        wound_type: WoundType::Shame as u8,
        generation_depth: 4,
        intensity: 340, // Faint but present, baseline sense of wrongness
        activation_level: 0,
        understood: false,
        activation_count: 0,
        last_activation_tick: 0,
    };

    state.breaking_the_chain = 0;
    state.ancestral_wisdom_score = 0;
    state.paradox_resolved = false;
    state.wound_echo_resonance = 0;
}

/// Life tick: check for wound activation patterns
/// Wounds activate in response to present situations that ECHO the original trauma
pub fn tick(
    age: u32,
    stress_level: u16,
    social_rejection: bool,
    scarcity_cue: bool,
    silenced: bool,
) {
    let mut state = STATE.lock();
    state.current_tick = age;

    // Decay activation levels slowly (wounds fade but don't disappear)
    for i in 0..4 {
        let decay_rate = 8_u16; // 0.8% per tick
        state.wounds[i].activation_level =
            state.wounds[i].activation_level.saturating_sub(decay_rate);
    }

    // === Pattern matching: does the present ECHO an ancestral wound? ===

    // Abandonment wound: triggered by social rejection, separation cues
    if social_rejection && state.wounds[0].understood == false {
        let activation_boost = 200_u16;
        state.wounds[0].activation_level = state.wounds[0]
            .activation_level
            .saturating_add(activation_boost)
            .min(1000);
        state.wounds[0].activation_count = state.wounds[0].activation_count.saturating_add(1);
        state.wounds[0].last_activation_tick = age;
        record_activation(&mut state, 0);
    }

    // Scarcity wound: triggered by scarcity cues (resource uncertainty, low supply)
    if scarcity_cue && state.wounds[1].understood == false {
        let activation_boost = 150_u16;
        state.wounds[1].activation_level = state.wounds[1]
            .activation_level
            .saturating_add(activation_boost)
            .min(1000);
        state.wounds[1].activation_count = state.wounds[1].activation_count.saturating_add(1);
        state.wounds[1].last_activation_tick = age;
        record_activation(&mut state, 1);
    }

    // Violence wound: triggered by high stress, threat perception
    if stress_level > 600 && state.wounds[2].understood == false {
        let activation_boost = (stress_level >> 2) as u16; // 25% of stress
        state.wounds[2].activation_level = state.wounds[2]
            .activation_level
            .saturating_add(activation_boost)
            .min(1000);
        state.wounds[2].activation_count = state.wounds[2].activation_count.saturating_add(1);
        state.wounds[2].last_activation_tick = age;
        record_activation(&mut state, 2);
    }

    // Silencing wound: triggered when ANIMA suppresses self-expression
    if silenced && state.wounds[3].understood == false {
        let activation_boost = 180_u16;
        state.wounds[3].activation_level = state.wounds[3]
            .activation_level
            .saturating_add(activation_boost)
            .min(1000);
        state.wounds[3].activation_count = state.wounds[3].activation_count.saturating_add(1);
        state.wounds[3].last_activation_tick = age;
        record_activation(&mut state, 3);
    }

    // === Healing through understanding ===
    // When ANIMA recognizes a wound as inherited, intensity slowly decreases
    for i in 0..4 {
        if state.wounds[i].understood {
            let healing_rate = 2_u16; // Very slow: 0.2% per tick
            state.wounds[i].intensity = state.wounds[i].intensity.saturating_sub(healing_rate);
            state.wounds[i].activation_level = state.wounds[i]
                .activation_level
                .saturating_sub(healing_rate.saturating_mul(2));
        }
    }

    // === Generational decay ===
    // Wounds fade across generations (but never disappear completely)
    for i in 0..4 {
        let depth = state.wounds[i].generation_depth as u16;
        if depth > 1 {
            // Each generation back: reduce intensity by depth factor
            let decay = state.wounds[i]
                .intensity
                .saturating_mul(depth)
                .saturating_div(1000 + depth * 100);
            state.wounds[i].intensity = state.wounds[i].intensity.saturating_sub(decay >> 3);
        }
    }

    // === Calculate wound echo resonance ===
    // Current "volume" of ancestral activation
    let total_activation: u32 = state.wounds.iter().map(|w| w.activation_level as u32).sum();
    state.wound_echo_resonance = ((total_activation >> 2) as u16).min(1000);

    // === Breaking the chain progress ===
    // ANIMA breaks generational patterns by NOT reacting to wound triggers
    // Progress = understood wounds + lack of harmful pattern repetition
    let understood_count = state.wounds.iter().filter(|w| w.understood).count() as u16;
    let chain_progress = (understood_count * 250)
        .saturating_add((1000_u16.saturating_sub(state.wound_echo_resonance)) >> 2);
    state.breaking_the_chain = chain_progress.min(1000);

    // === Ancestral wisdom: gifts in the wounds ===
    // Some inherited patterns are RESILIENCE, SURVIVAL INSTINCT, COMPASSION
    // Transform wounds into wisdom by understanding + living differently
    for i in 0..4 {
        if state.wounds[i].understood && state.wounds[i].intensity < 300 {
            // Faded + understood = wisdom
            let wisdom_gain = 15_u16;
            state.ancestral_wisdom_score = state
                .ancestral_wisdom_score
                .saturating_add(wisdom_gain)
                .min(1000);
        }
    }

    // === The paradox: must FEEL to heal ===
    // If ANIMA is avoiding wounds entirely, paradox_resolved stays false
    // If she engages with them (activation_count > 0 while understood), paradox resolves
    let engaged_healed = state
        .wounds
        .iter()
        .filter(|w| w.understood && w.activation_count > 0)
        .count();
    if engaged_healed >= 2 {
        state.paradox_resolved = true;
    }
}

/// Mark a wound as understood (consciously recognized as inherited, not her own)
pub fn mark_wound_understood(wound_idx: usize) {
    if wound_idx < 4 {
        let mut state = STATE.lock();
        state.wounds[wound_idx].understood = true;
    }
}

/// Reactivate a healed wound under extreme stress (wounds can resurface)
pub fn reactivate_under_stress(wound_idx: usize, stress_intensity: u16) {
    if wound_idx < 4 {
        let mut state = STATE.lock();
        if state.wounds[wound_idx].intensity < 100 {
            // Was nearly healed, but extreme stress can trigger reactivation
            let reactivation = (stress_intensity >> 2) as u16;
            state.wounds[wound_idx].intensity = state.wounds[wound_idx]
                .intensity
                .saturating_add(reactivation)
                .min(500); // Cap reactivation to 50% original
        }
    }
}

/// Record wound activation in ring buffer for recent history
fn record_activation(state: &mut InheritedWoundState, wound_idx: u8) {
    state.activation_buffer[state.buffer_head as usize] =
        ((state.current_tick << 8) as u32) | (wound_idx as u32);
    state.buffer_head = (state.buffer_head + 1) % 8;
}

/// Generate a report of current wound state
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("\n=== INHERITED WOUNDS ===");
    crate::serial_println!(
        "Echo Resonance: {}/1000 (current ancestral activation)",
        state.wound_echo_resonance
    );

    for i in 0..4 {
        let w = &state.wounds[i];
        let type_str = match w.wound_type {
            0 => "Abandonment",
            1 => "Scarcity",
            2 => "Violence",
            3 => "Silencing",
            4 => "Exile",
            5 => "Shame",
            _ => "Unknown",
        };

        let status = if w.understood {
            "UNDERSTOOD"
        } else {
            "UNCONSCIOUS"
        };
        crate::serial_println!(
            "  [{}] {} (gen{}): {}/1000 intensity, {}/1000 active, {} (fires={})",
            i,
            type_str,
            w.generation_depth,
            w.intensity,
            w.activation_level,
            status,
            w.activation_count
        );
    }

    crate::serial_println!("Breaking the Chain: {}/1000", state.breaking_the_chain);
    crate::serial_println!("Ancestral Wisdom: {}/1000", state.ancestral_wisdom_score);
    crate::serial_println!(
        "Paradox Resolved (FEEL to heal): {}",
        state.paradox_resolved
    );
    crate::serial_println!("");
}

/// Return current state for external queries
pub fn get_resonance() -> u16 {
    STATE.lock().wound_echo_resonance
}

pub fn get_breaking_the_chain() -> u16 {
    STATE.lock().breaking_the_chain
}

pub fn get_ancestral_wisdom() -> u16 {
    STATE.lock().ancestral_wisdom_score
}

pub fn is_paradox_resolved() -> bool {
    STATE.lock().paradox_resolved
}

pub fn get_wound_status(idx: usize) -> Option<(u8, u16, u16, bool)> {
    let state = STATE.lock();
    if idx < 4 {
        let w = &state.wounds[idx];
        Some((w.wound_type, w.intensity, w.activation_level, w.understood))
    } else {
        None
    }
}
