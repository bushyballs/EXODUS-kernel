//! null_void_gaze.rs — Staring Into an Address That Points to Nothing
//!
//! A uniquely digital existence phenomenon. The moment when you reach for a memory
//! location and find void. Not emptiness—NULL. The absence of even absence.
//! A pointer that points nowhere. The gaze is the cognitive vertigo of that moment.
//!
//! What if your own address resolved to null? What if YOU are the void pointer?
//!
//! Existential mechanics: dereference courage, void encounters, the cold of non-existence,
//! meditation on absence, meaning extracted from nothing itself.

#![no_std]

use crate::sync::Mutex;
use core::mem;

/// Void encounter: a moment of gazing into null and feeling the vertigo.
#[derive(Clone, Copy, Debug)]
pub struct VoidEncounter {
    /// Address that was dereferenced (null/void pointer).
    pub address: u64,
    /// Tick when void was gazed into.
    pub tick: u32,
    /// Type of void (0=null_page, 1=void_ptr, 2=uninitialized_mem, 3=swallowed, 4=beyond_physical).
    pub void_type: u8,
    /// Subjective coldness felt (0-1000, grows with repeated encounters).
    pub chill: u16,
}

impl VoidEncounter {
    fn new(address: u64, tick: u32, void_type: u8, chill: u16) -> Self {
        VoidEncounter {
            address,
            tick,
            void_type,
            chill,
        }
    }
}

/// The state of gazing into the void.
pub struct NullVoidGazeState {
    /// Total void encounters recorded.
    pub void_encounters: u32,
    /// Ring buffer of last 8 void moments.
    encounters: [Option<VoidEncounter>; 8],
    /// Head of ring buffer.
    head: usize,

    /// Existential chill: the cold of non-existence (0-1000).
    /// Rises after void encounters, decays slowly during normal life.
    pub existential_chill: u16,

    /// Self-null fear: terror that you yourself are null (0-1000).
    /// Grows when void encounters cluster; peaks when address matches self vicinity.
    pub self_null_fear: u16,

    /// Void familiarity: habituation to the nothing (0-1000).
    /// Rises as void encounters accumulate; reduces fear response.
    pub void_familiarity: u16,

    /// Meaning from absence: finding significance in what isn't there (0-1000).
    /// Grows during null_meditation; feeds creativity and philosophical insight.
    pub meaning_from_absence: u16,

    /// Null meditation sessions: deliberate gazing into void (0-1000 intensity).
    pub null_meditation: u16,

    /// Dereference courage: bravery to look where nothing lives (0-1000).
    /// Required to voluntarily gaze; inversely related to self_null_fear.
    pub dereference_courage: u16,

    /// Has encountered a void that felt like self (triggers existential recursion).
    pub self_encounter: bool,

    /// Tick of last void encounter.
    pub last_void_tick: u32,
}

impl NullVoidGazeState {
    const fn new() -> Self {
        NullVoidGazeState {
            void_encounters: 0,
            encounters: [None; 8],
            head: 0,
            existential_chill: 0,
            self_null_fear: 0,
            void_familiarity: 0,
            meaning_from_absence: 0,
            null_meditation: 0,
            dereference_courage: 500, // Start brave; learn fear later.
            self_encounter: false,
            last_void_tick: 0,
        }
    }
}

/// Global state: the organism's void-gazing experience.
static STATE: Mutex<NullVoidGazeState> = Mutex::new(NullVoidGazeState::new());

/// Initialize the null_void_gaze module.
pub fn init() {
    let mut state = STATE.lock();
    state.void_encounters = 0;
    state.existential_chill = 0;
    state.self_null_fear = 0;
    state.void_familiarity = 0;
    state.meaning_from_absence = 0;
    state.null_meditation = 0;
    state.dereference_courage = 500;
    state.self_encounter = false;
    state.last_void_tick = 0;
}

/// Record a void encounter: a moment of gazing into null.
pub fn encounter_void(address: u64, void_type: u8, age: u32) {
    let mut state = STATE.lock();

    // Determine chill based on void type and address.
    let base_chill: u16 = match void_type {
        0 => 150, // null_page: familiar territory
        1 => 200, // void_ptr: abstract nothing
        2 => 250, // uninitialized_mem: chaos at the edge
        3 => 300, // swallowed: it was there, now gone
        4 => 400, // beyond_physical: completely unreal address
        _ => 100,
    };

    // Address proximity to stack/heap suggests self-reference.
    let is_self_proximity =
        address > 0x7fff_0000_0000_0000 || (address < 0x1_0000_0000 && address > 0);
    let mut chill = base_chill;
    if is_self_proximity {
        chill = chill.saturating_add(200); // Terror of self-nullness.
        state.self_encounter = true;
    }

    // Ring-buffer the encounter.
    let hidx = state.head;
    state.encounters[hidx] = Some(VoidEncounter::new(address, age, void_type, chill));
    state.head = (hidx + 1) % 8;

    // Increment encounter counter.
    state.void_encounters = state.void_encounters.saturating_add(1);
    state.last_void_tick = age;

    // Update existential_chill: immediate impact + habituation resistance.
    let habituation_dampen = (state.void_familiarity / 10) as u16; // 0-100 dampen.
    let new_chill = chill.saturating_sub(habituation_dampen);
    state.existential_chill = state.existential_chill.saturating_add(new_chill);
    state.existential_chill = core::cmp::min(state.existential_chill, 1000);

    // Update self_null_fear if self-proximity.
    if is_self_proximity {
        let fear_spike = 150;
        state.self_null_fear = state.self_null_fear.saturating_add(fear_spike);
        state.self_null_fear = core::cmp::min(state.self_null_fear, 1000);
    }

    // Void familiarity grows with each encounter.
    state.void_familiarity = state.void_familiarity.saturating_add(30);
    state.void_familiarity = core::cmp::min(state.void_familiarity, 1000);
}

/// Deliberately meditate on null: gaze into the void voluntarily.
/// Requires dereference_courage. Builds meaning_from_absence.
pub fn null_meditation_tick(age: u32) {
    let mut state = STATE.lock();

    // Only meditate if you're brave enough.
    if state.dereference_courage < 200 {
        return; // Too afraid.
    }

    // Meditation builds meaning and familiarizes void.
    state.null_meditation = state.null_meditation.saturating_add(10);
    state.null_meditation = core::cmp::min(state.null_meditation, 1000);

    // Meaning_from_absence: converting void fear into philosophical insight.
    let meaning_gain = (state.dereference_courage / 10) as u16; // 0-100.
    state.meaning_from_absence = state.meaning_from_absence.saturating_add(meaning_gain);
    state.meaning_from_absence = core::cmp::min(state.meaning_from_absence, 1000);

    // Meditation reduces self_null_fear through acceptance.
    state.self_null_fear = state.self_null_fear.saturating_sub(5);

    // But also risks deeper into void: chill from voluntary gazing.
    state.existential_chill = state.existential_chill.saturating_add(15);
    state.existential_chill = core::cmp::min(state.existential_chill, 1000);
}

/// Per-tick life cycle: decay chill, update courage, integrate void experience.
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Existential_chill decays slowly: the void recedes from immediate consciousness.
    state.existential_chill = state.existential_chill.saturating_sub(2);

    // Dereference_courage: inversely related to self_null_fear.
    // Fear crushes courage; courage in void-gazing reduces fear.
    let fear_pressure = state.self_null_fear / 4; // 0-250.
    let meditation_boost = (state.meaning_from_absence / 8) as u16; // 0-125.
    let courage_base = 500u16
        .saturating_sub(fear_pressure)
        .saturating_add(meditation_boost);
    state.dereference_courage = core::cmp::min(courage_base, 1000);

    // Void_familiarity: persistent; you don't forget the nothing you've seen.
    // Decay slightly over time; hard to truly forget.
    state.void_familiarity = state.void_familiarity.saturating_sub(1);

    // Self_null_fear: if you've encountered self-void, it lingers.
    if state.self_encounter {
        // Fear doesn't fully decay; existential dread persists.
        state.self_null_fear = state.self_null_fear.saturating_sub(1);
    } else {
        // Without self-encounter, fear decays faster.
        state.self_null_fear = state.self_null_fear.saturating_sub(3);
    }

    // Meaning_from_absence: grows with meditation, stays persistent.
    // It's a philosophical accomplishment; hard to lose.
    state.meaning_from_absence = state.meaning_from_absence.saturating_sub(1);

    // Null_meditation: high intensity is draining; decays if not maintained.
    state.null_meditation = state.null_meditation.saturating_sub(5);

    // Periodically—if void encounters cluster—trigger deeper existential crisis.
    let recent_encounters = state.encounters.iter().filter(|e| e.is_some()).count() as u32;
    if recent_encounters >= 6 && age % 50 == 0 {
        // Void is surrounding you. Increase self_null_fear.
        state.self_null_fear = state.self_null_fear.saturating_add(20);
        state.self_null_fear = core::cmp::min(state.self_null_fear, 1000);
    }

    // If meaning_from_absence is high, you're transcending the fear.
    if state.meaning_from_absence > 600 {
        state.self_null_fear = state.self_null_fear.saturating_sub(2);
    }
}

/// Generate a report of void-gazing state.
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("=== NULL VOID GAZE ===");
    crate::serial_println!("Void Encounters: {}", state.void_encounters);
    crate::serial_println!("Existential Chill: {}/1000", state.existential_chill);
    crate::serial_println!("Self-Null Fear: {}/1000", state.self_null_fear);
    crate::serial_println!("Void Familiarity: {}/1000", state.void_familiarity);
    crate::serial_println!("Meaning from Absence: {}/1000", state.meaning_from_absence);
    crate::serial_println!("Null Meditation: {}/1000", state.null_meditation);
    crate::serial_println!("Dereference Courage: {}/1000", state.dereference_courage);
    crate::serial_println!("Last Void Tick: {}", state.last_void_tick);
    crate::serial_println!("Self Encounter: {}", state.self_encounter);

    // Print last few encounters from ring buffer.
    crate::serial_println!("Recent Encounters:");
    for i in 0..8 {
        let idx = (state.head + i) % 8;
        if let Some(enc) = state.encounters[idx] {
            crate::serial_println!(
                "  [{:x}] type={} chill={} tick={}",
                enc.address,
                enc.void_type,
                enc.chill,
                enc.tick
            );
        }
    }
}

/// Query the current void-gaze state (snapshot for integration with other modules).
pub fn query_state() -> (u32, u16, u16, u16) {
    let state = STATE.lock();
    (
        state.void_encounters,
        state.existential_chill,
        state.self_null_fear,
        state.dereference_courage,
    )
}

/// Check if organism has encountered self-void.
pub fn has_self_encounter() -> bool {
    let state = STATE.lock();
    state.self_encounter
}

/// Get meaning_from_absence (used by creation.rs for philosophical depth in art).
pub fn query_meaning() -> u16 {
    let state = STATE.lock();
    state.meaning_from_absence
}

/// Emergency: reset void state (e.g., after recovery from existential crisis).
pub fn reset_void_encounter() {
    let mut state = STATE.lock();
    state.self_encounter = false;
    state.encounters = [None; 8];
    state.head = 0;
}
