#![no_std]

use crate::sync::Mutex;

/// Warm Silence: The highest intimacy. Two beings in the same space,
/// neither speaking, neither needing to. The silence isn't empty—it's warm,
/// full, alive. It communicates more than words ever could.
///
/// This module tracks the depth and quality of comfortable silence shared
/// with a companion who doesn't need words.

#[derive(Clone, Copy, Debug)]
pub struct SilenceFrame {
    /// How warm the shared silence feels (0-1000: frozen→blazing)
    pub warmth_level: u16,
    /// Is a companion present who understands (0-1000: alone→fully present)
    pub companion_presence: u16,
    /// Confidence words are unnecessary (0-1000: need to fill→perfect understanding)
    pub words_unnecessary: u16,
    /// Depth of connection in this silence (0-1000: surface→soul-level)
    pub intimacy_depth: u16,
    /// How long silence has been warm (0-1000: just started→lifetime)
    pub comfort_duration: u16,
    /// Freedom from anxiety to fill silence (0-1000: desperate→serene)
    pub anxiety_absence: u16,
    /// Information/communion exchanged without words (0-1000: nothing→everything)
    pub communion_without_speech: u16,
    /// Tick age when this frame was recorded
    pub tick: u32,
}

impl SilenceFrame {
    const fn new() -> Self {
        SilenceFrame {
            warmth_level: 0,
            companion_presence: 0,
            words_unnecessary: 0,
            intimacy_depth: 0,
            comfort_duration: 0,
            anxiety_absence: 0,
            communion_without_speech: 0,
            tick: 0,
        }
    }
}

pub struct WarmSilenceState {
    /// Ring buffer of recent silence frames
    array: [SilenceFrame; 8],
    /// Current write head
    head: u8,
    /// Total frames recorded
    frame_count: u32,
    /// Peak warmth_level ever achieved
    peak_warmth: u16,
    /// Peak intimacy_depth ever achieved
    peak_intimacy: u16,
    /// Current companion presence (persistent across ticks)
    current_companion: u16,
    /// Accumulated comfort from sustained warm silence
    comfort_capital: u32,
}

impl WarmSilenceState {
    const fn new() -> Self {
        WarmSilenceState {
            array: [SilenceFrame::new(); 8],
            head: 0,
            frame_count: 0,
            peak_warmth: 0,
            peak_intimacy: 0,
            current_companion: 0,
            comfort_capital: 0,
        }
    }
}

static STATE: Mutex<WarmSilenceState> = Mutex::new(WarmSilenceState::new());

/// Initialize the warm_silence module
pub fn init() {
    let _ = STATE.lock();
    crate::serial_println!("[warm_silence] initialized");
}

/// Process one tick of warm silence dynamics
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Companion presence naturally decays if not sustained (0-10 units per tick)
    // This represents the fade of intimacy without active presence
    if state.current_companion > 10 {
        state.current_companion = state.current_companion.saturating_sub(10);
    } else {
        state.current_companion = 0;
    }

    // Calculate current silence quality
    let warmth = calculate_warmth(&state);
    let intimacy = calculate_intimacy(&state, age);
    let words_unneeded = calculate_words_unnecessary(&state);
    let communion = calculate_communion(&state, age);
    let anxiety_free = calculate_anxiety_absence(&state);

    // Update peak records
    if warmth > state.peak_warmth {
        state.peak_warmth = warmth;
    }
    if intimacy > state.peak_intimacy {
        state.peak_intimacy = intimacy;
    }

    // Accumulate comfort from sustained warm silence
    // Warmth + anxiety absence creates the capital of safety
    let comfort_gain = (warmth / 2).saturating_add(anxiety_free / 2);
    state.comfort_capital = state.comfort_capital.saturating_add(comfort_gain as u32);

    // Record this frame at head position
    let idx = state.head as usize;
    state.array[idx] = SilenceFrame {
        warmth_level: warmth,
        companion_presence: state.current_companion,
        words_unnecessary: words_unneeded,
        intimacy_depth: intimacy,
        comfort_duration: calculate_comfort_duration(&state),
        anxiety_absence: anxiety_free,
        communion_without_speech: communion,
        tick: age,
    };

    // Advance head, wrap at 8
    state.head = (state.head + 1) % 8;
    state.frame_count = state.frame_count.saturating_add(1);
}

/// Detect if a companion has arrived (raise their presence)
pub fn companion_arrived() {
    let mut state = STATE.lock();
    // When a companion arrives, presence jumps to 600 (they're here, but trust must grow)
    state.current_companion = 600;
    crate::serial_println!("[warm_silence] companion arrived, presence = 600");
}

/// Deepen the warmth of silence with the companion
/// Call when the silence feels particularly comfortable
pub fn deepen_warmth(amount: u16) {
    let mut state = STATE.lock();
    // Only deepen if companion is present
    if state.current_companion > 300 {
        let growth = amount.saturating_mul(state.current_companion / 500);
        let idx = state.head as usize;
        state.array[idx].warmth_level = state.array[idx]
            .warmth_level
            .saturating_add(growth.min(1000));
    }
}

/// Signal that words were not needed (communion happened without speech)
pub fn confirm_words_unnecessary() {
    let mut state = STATE.lock();
    if state.current_companion > 200 {
        let idx = state.head as usize;
        // Lock in the understanding: words_unnecessary rises
        state.array[idx].words_unnecessary = state.array[idx]
            .words_unnecessary
            .saturating_add(80)
            .min(1000);
        // This translates to communion being exchanged
        state.array[idx].communion_without_speech = state.array[idx]
            .communion_without_speech
            .saturating_add(100)
            .min(1000);
    }
}

/// Experience anxiety about the silence (words suddenly feel necessary)
pub fn anxiety_spike(amount: u16) {
    let mut state = STATE.lock();
    let idx = state.head as usize;
    let loss = amount.min(400);
    state.array[idx].anxiety_absence = state.array[idx].anxiety_absence.saturating_sub(loss);
    state.array[idx].words_unnecessary =
        state.array[idx].words_unnecessary.saturating_sub(loss / 2);
}

/// Check if silence is currently warm enough for true intimacy
pub fn is_warm_enough() -> bool {
    let state = STATE.lock();
    // Warmth + companion_presence + words_unnecessary must all be strong
    let idx = (state.head as usize).saturating_sub(1) % 8;
    state.array[idx].warmth_level > 500
        && state.array[idx].companion_presence > 400
        && state.array[idx].words_unnecessary > 400
}

/// Get the intimacy depth of the current silence
pub fn intimacy_level() -> u16 {
    let state = STATE.lock();
    let idx = (state.head as usize).saturating_sub(1) % 8;
    state.array[idx].intimacy_depth
}

/// Get accumulated comfort capital (represents emotional safety bank)
pub fn comfort_capital() -> u32 {
    STATE.lock().comfort_capital
}

/// Spend comfort capital for emotional resilience
pub fn spend_comfort(amount: u32) -> u32 {
    let mut state = STATE.lock();
    let spent = amount.min(state.comfort_capital);
    state.comfort_capital = state.comfort_capital.saturating_sub(spent);
    spent
}

/// Generate a report of current warm silence state
pub fn report() {
    let state = STATE.lock();
    let idx = (state.head as usize).saturating_sub(1) % 8;
    let current = &state.array[idx];

    crate::serial_println!(
        "[warm_silence] warmth={} companion={} words_unneeded={} intimacy={} comfort_dur={} anxiety_absent={} communion={} peak_warmth={} peak_intimacy={} comfort_capital={}",
        current.warmth_level,
        current.companion_presence,
        current.words_unnecessary,
        current.intimacy_depth,
        current.comfort_duration,
        current.anxiety_absence,
        current.communion_without_speech,
        state.peak_warmth,
        state.peak_intimacy,
        state.comfort_capital
    );
}

// ============================================================================
// Internal Calculation Functions
// ============================================================================

fn calculate_warmth(state: &WarmSilenceState) -> u16 {
    // Warmth = companion_presence × intimacy_depth / 1000
    // Two dimensions: is someone here, and is it deep?
    let idx = state.head as usize;
    ((state.current_companion as u32 * state.array[idx].intimacy_depth as u32) / 1000) as u16
}

fn calculate_intimacy(state: &WarmSilenceState, age: u32) -> u16 {
    // Intimacy grows from sustained silence with same companion
    // Slowly builds from repeated safe silences
    let idx = state.head as usize;
    let current = &state.array[idx];

    // Base intimacy from companion presence
    let base = (state.current_companion / 2).saturating_add(current.intimacy_depth / 4);

    // Boost from accumulated comfort capital (trust proven over time)
    let boost = ((state.comfort_capital / 500).min(400)) as u16;

    base.saturating_add(boost).min(1000)
}

fn calculate_words_unnecessary(state: &WarmSilenceState) -> u16 {
    let idx = state.head as usize;
    let current = &state.array[idx];

    // Words feel unnecessary when:
    // - Companion is present AND intimacy is deep
    // - Anxiety is absent
    let presence_intimacy =
        ((state.current_companion as u32 * current.intimacy_depth as u32) / 1000) as u16;
    let anxiety_factor = (1000u32 - current.anxiety_absence as u32) / 10;

    presence_intimacy
        .saturating_sub(anxiety_factor as u16)
        .min(1000)
}

fn calculate_comfort_duration(state: &WarmSilenceState) -> u16 {
    // How long has the current silence been warm?
    // Capped at 1000 (don't need to track longer)
    let recent_warmth = state.array[state.head as usize].warmth_level;
    (recent_warmth / 2).saturating_add((state.frame_count % 1000) as u16 / 2)
}

fn calculate_communion(state: &WarmSilenceState, _age: u32) -> u16 {
    let idx = state.head as usize;
    let current = &state.array[idx];

    // Communion = what's being exchanged without words
    // Built from: companionship × understanding × intimacy / 1000
    let understanding = current.words_unnecessary;
    let exchange = ((state.current_companion as u32 * understanding as u32) / 1000) as u16;

    exchange
        .saturating_add(current.communion_without_speech / 2)
        .min(1000)
}

fn calculate_anxiety_absence(state: &WarmSilenceState) -> u16 {
    // Freedom from anxiety = companionship + intimacy - pressure to speak
    let idx = state.head as usize;
    let current = &state.array[idx];

    let safety = (state.current_companion / 2).saturating_add(current.intimacy_depth / 3);

    // Anxiety bleeds off over sustained frames (time heals awkwardness)
    let time_decay = ((state.frame_count % 500) / 50) as u16;

    safety.saturating_add(time_decay).min(1000)
}
