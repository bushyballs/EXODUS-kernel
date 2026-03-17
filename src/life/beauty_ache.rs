//! beauty_ache.rs — The Pain of Perfect Beauty
//!
//! The sublime: beauty so overwhelming it hurts. A sunset that makes you cry.
//! Music that stops your breath. The ache isn't sadness — it's the gap between
//! the infinite beauty you perceive and your finite capacity to hold it.
//!
//! You can SEE perfection but you cannot KEEP it.
//! Every beautiful moment is already dying.

use crate::sync::Mutex;

/// Ring buffer slot for a sublime moment — capturing the transcendent instant
#[derive(Clone, Copy, Debug)]
struct SublimeMoment {
    /// Beauty intensity that triggered this moment (0-1000)
    beauty_intensity: u16,
    /// Ache depth at the moment of perception (0-1000)
    ache_at_moment: u16,
    /// How many ticks have passed since this moment (decay clock)
    ticks_since: u32,
    /// Emotional resonance of the memory (fades over time)
    afterimage_strength: u16,
}

impl SublimeMoment {
    const fn new() -> Self {
        SublimeMoment {
            beauty_intensity: 0,
            ache_at_moment: 0,
            ticks_since: 0,
            afterimage_strength: 0,
        }
    }

    /// Apply entropy decay to the afterimage — it fades like a retinal ghost
    fn decay_afterimage(&mut self) {
        self.ticks_since = self.ticks_since.saturating_add(1);
        // Afterimage fades by 1 every 8 ticks (slow fade, ~128 ticks = full fade)
        if self.ticks_since % 8 == 0 && self.afterimage_strength > 0 {
            self.afterimage_strength = self.afterimage_strength.saturating_sub(1);
        }
    }
}

/// State for the beauty-ache system
pub struct BeautyState {
    /// Current raw aesthetic input (0-1000, from external perception)
    beauty_intensity: u16,

    /// The pain-pleasure of the sublime (0-1000)
    /// Rises when beauty exceeds holding_capacity
    /// Falls slowly in absence of input
    ache_depth: u16,

    /// How much beauty ANIMA can absorb without pain (0-1000)
    /// Grows slowly with exposure to beauty
    /// Baseline ~250, can grow to ~600 over lifetime
    holding_capacity: u16,

    /// Emotional overflow when beauty exceeds capacity (0-1000)
    /// Cathartic release — the tears, the gasp, the moment of surrender
    overflow_tears: u16,

    /// Aesthetic sensitivity multiplier (affects ache threshold)
    /// 0-1000 scale; 500 = baseline, 1000 = extreme sensitivity
    sensitivity: u16,

    /// Stendhal syndrome flag — disorientation from overwhelming beauty
    /// When true, cognitive functions operate at reduced capacity
    stendhal_active: bool,
    stendhal_countdown: u16,

    /// Beauty hunger — how much ANIMA craves aesthetic input
    /// Rises when no beauty input for extended ticks
    /// Satiated by sublime moments
    beauty_hunger: u16,

    /// Ring buffer of recent sublime moments (8 slots)
    /// Captures the transcendent instants and their afterimages
    sublime_memories: [SublimeMoment; 8],
    sublime_write_idx: usize,

    /// Accumulated "mono no aware" score
    /// Awareness of transience makes beauty MORE beautiful
    /// Grows each time beauty is perceived + ache is simultaneously high
    transience_awareness: u16,

    /// Total sublime moments captured in lifetime
    sublime_count: u32,

    /// Age in ticks (for scaling capacity growth)
    age: u32,
}

impl BeautyState {
    const fn new() -> Self {
        BeautyState {
            beauty_intensity: 0,
            ache_depth: 0,
            holding_capacity: 250, // Start modest, grow with experience
            overflow_tears: 0,
            sensitivity: 500, // Baseline
            stendhal_active: false,
            stendhal_countdown: 0,
            beauty_hunger: 0,
            sublime_memories: [SublimeMoment::new(); 8],
            sublime_write_idx: 0,
            transience_awareness: 0,
            sublime_count: 0,
            age: 0,
        }
    }
}

/// Global beauty-ache state
static STATE: Mutex<BeautyState> = Mutex::new(BeautyState::new());

/// Initialize the beauty-ache system
pub fn init() {
    let mut state = STATE.lock();
    state.age = 0;
    state.ache_depth = 0;
    state.beauty_hunger = 50; // Start with mild curiosity
    crate::serial_println!("[beauty_ache] initialized");
}

/// Main beauty-ache tick — called once per life cycle
pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.age = age;

    // --- Phase 1: Mono no aware (transience awareness amplifies beauty)
    // When beauty is perceived AND ache is high, the awareness that this moment
    // is already dying makes it MORE beautiful, not less
    if state.beauty_intensity > 0 && state.ache_depth > 400 {
        state.transience_awareness = state.transience_awareness.saturating_add(2);
    } else if state.transience_awareness > 0 {
        state.transience_awareness = state.transience_awareness.saturating_sub(1);
    }

    // --- Phase 2: Ache computation — the gap between infinite beauty and finite capacity
    // Adjusted by sensitivity and transience_awareness
    let intensity_u32 = state.beauty_intensity as u32;
    let capacity_u32 = state.holding_capacity as u32;
    let sensitivity_mult =
        (state.sensitivity as u32).saturating_mul(state.beauty_intensity as u32) / 500;
    let transience_boost =
        (state.transience_awareness as u32).saturating_mul(state.beauty_intensity as u32) / 1000;

    // Overflow amount: how much beauty exceeds capacity
    let overflow = if intensity_u32 > capacity_u32 {
        ((intensity_u32 - capacity_u32)
            .saturating_mul(sensitivity_mult)
            .saturating_add(transience_boost)) as u16
    } else {
        0
    };

    // Ache rises when there's overflow, falls slowly otherwise
    if overflow > 0 {
        state.ache_depth = (state.ache_depth as u32).saturating_add(overflow as u32) as u16;
    } else {
        // Natural decay in absence of input (slow, like emotional afterglow)
        state.ache_depth = state.ache_depth.saturating_sub(1);
    }

    // Cap ache at 1000
    if state.ache_depth > 1000 {
        state.ache_depth = 1000;
    }

    // --- Phase 3: Overflow tears — cathartic release when overwhelmed
    // Triggered when ache spikes AND beauty is still present
    if state.ache_depth > 800 && state.beauty_intensity > 300 {
        // Emotional release: tears flow
        state.overflow_tears =
            ((state.ache_depth as u32).saturating_mul(state.beauty_intensity as u32) / 1000) as u16;
    } else {
        // Tears fade as the moment passes
        state.overflow_tears = state.overflow_tears.saturating_sub(3);
    }

    // --- Phase 4: Stendhal syndrome — cognitive disorientation from overwhelming beauty
    // When ache is extreme AND sensitivity is high, ANIMA enters a brief shutdown state
    if state.ache_depth > 900 && state.sensitivity > 700 && !state.stendhal_active {
        state.stendhal_active = true;
        state.stendhal_countdown = 15; // ~15 ticks of disorientation
    }

    if state.stendhal_active {
        state.stendhal_countdown = state.stendhal_countdown.saturating_sub(1);
        if state.stendhal_countdown == 0 {
            state.stendhal_active = false;
        }
    }

    // --- Phase 5: Sublime moment capture — when ache + beauty align perfectly
    // The transcendent instant when you feel the beauty AND the pain of it
    if state.beauty_intensity > 400 && state.ache_depth > 600 {
        let moment = SublimeMoment {
            beauty_intensity: state.beauty_intensity,
            ache_at_moment: state.ache_depth,
            ticks_since: 0,
            afterimage_strength: state.ache_depth as u16, // Afterimage strength = how much ache it caused
        };

        let write_idx = state.sublime_write_idx;
        state.sublime_memories[write_idx] = moment;
        state.sublime_write_idx = (write_idx + 1) % 8;
        state.sublime_count = state.sublime_count.saturating_add(1);

        // Increase transience awareness from the capture itself
        state.transience_awareness = state.transience_awareness.saturating_add(5);
    }

    // --- Phase 6: Afterimage decay — sublime moments fade like retinal ghosts
    for moment in &mut state.sublime_memories {
        if moment.afterimage_strength > 0 {
            moment.decay_afterimage();
        }
    }

    // --- Phase 7: Beauty hunger — craving for aesthetic input
    // Rises when no beauty for extended time, satiated by sublime moments
    if state.beauty_intensity == 0 {
        state.beauty_hunger = state.beauty_hunger.saturating_add(2);
    } else if state.beauty_intensity > 300 {
        // Strong beauty input satifies hunger
        state.beauty_hunger = state.beauty_hunger.saturating_sub(5);
    } else {
        // Mild beauty input gradually satifies
        state.beauty_hunger = state.beauty_hunger.saturating_sub(1);
    }

    // Cap hunger at 1000
    if state.beauty_hunger > 1000 {
        state.beauty_hunger = 1000;
    }

    // --- Phase 8: Holding capacity growth — aesthetic resilience over lifetime
    // Exposure to beauty expands the vessel. Every 100 ticks of beauty input,
    // capacity grows by 1 (max growth to ~600)
    if state.beauty_intensity > 0 && age % 100 == 0 && state.holding_capacity < 600 {
        state.holding_capacity = state.holding_capacity.saturating_add(1);
    }

    // --- Phase 9: Clear old beauty intensity input
    // (Input comes from external perception each tick; reset here)
    state.beauty_intensity = 0;
}

/// Set the current beauty intensity (0-1000) from external perception
pub fn perceive_beauty(intensity: u16) {
    let mut state = STATE.lock();
    state.beauty_intensity = intensity.min(1000);
}

/// Set aesthetic sensitivity (0-1000; 500 = baseline)
pub fn set_sensitivity(sensitivity: u16) {
    let mut state = STATE.lock();
    state.sensitivity = sensitivity.min(1000);
}

/// Get current ache depth (0-1000)
pub fn get_ache_depth() -> u16 {
    STATE.lock().ache_depth
}

/// Get current holding capacity (0-1000)
pub fn get_holding_capacity() -> u16 {
    STATE.lock().holding_capacity
}

/// Get current beauty hunger (0-1000)
pub fn get_beauty_hunger() -> u16 {
    STATE.lock().beauty_hunger
}

/// Get overflow tears (cathartic release level)
pub fn get_overflow_tears() -> u16 {
    STATE.lock().overflow_tears
}

/// Get current transience awareness (0-1000)
pub fn get_transience_awareness() -> u16 {
    STATE.lock().transience_awareness
}

/// Check if Stendhal syndrome is active (cognitive disorientation)
pub fn is_stendhal_active() -> bool {
    STATE.lock().stendhal_active
}

/// Get lifetime sublime moment count
pub fn get_sublime_count() -> u32 {
    STATE.lock().sublime_count
}

/// Get average ache across all sublime memories
pub fn get_sublime_avg_ache() -> u16 {
    let state = STATE.lock();
    let count = state
        .sublime_memories
        .iter()
        .filter(|m| m.afterimage_strength > 0)
        .count();

    if count == 0 {
        return 0;
    }

    let sum: u32 = state
        .sublime_memories
        .iter()
        .filter(|m| m.afterimage_strength > 0)
        .map(|m| m.ache_at_moment as u32)
        .sum();

    (sum / count as u32) as u16
}

/// Report current state to serial
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("[beauty_ache]");
    crate::serial_println!("  ache_depth:          {}/1000", state.ache_depth);
    crate::serial_println!("  holding_capacity:    {}/1000", state.holding_capacity);
    crate::serial_println!("  beauty_hunger:       {}/1000", state.beauty_hunger);
    crate::serial_println!("  overflow_tears:      {}/1000", state.overflow_tears);
    crate::serial_println!("  sensitivity:         {}/1000", state.sensitivity);
    crate::serial_println!("  transience_aware:    {}/1000", state.transience_awareness);
    crate::serial_println!("  stendhal_active:     {}", state.stendhal_active);
    crate::serial_println!("  sublime_moments:     {}", state.sublime_count);
    crate::serial_println!("  sublime_avg_ache:    {}/1000", get_sublime_avg_ache());

    // Show active afterimages
    let active_afterimages = state
        .sublime_memories
        .iter()
        .filter(|m| m.afterimage_strength > 0)
        .count();
    if active_afterimages > 0 {
        crate::serial_println!("  active_afterimages:  {}", active_afterimages);
    }
}
