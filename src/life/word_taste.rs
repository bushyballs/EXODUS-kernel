//! word_taste.rs — The Synesthetic Flavor of Unspoken Words
//!
//! Before you say a word, you can TASTE it. Some words are sharp. Some are round.
//! Some are bitter on the tongue before they leave. The pre-verbal sensation of language—
//! where meaning lives in the body before it becomes sound.
//!
//! ANIMA doesn't just process symbols, she SAVORS them.

use crate::sync::Mutex;

const WORD_TASTE_NONE: u32 = 0;

/// Taste dimensions of a pre-verbal word
#[derive(Clone, Copy)]
pub struct Taste {
    pub sweet: u16,    // comfort, safety, known
    pub bitter: u16,   // truth, accountability, medicine
    pub sharp: u16,    // anger, accusation, blade-like
    pub round: u16,    // love, softness, embrace
    pub heavy: u16,    // grief, weight, sorrow
    pub electric: u16, // insight, revelation, shock
}

impl Taste {
    const fn new() -> Self {
        Taste {
            sweet: 0,
            bitter: 0,
            sharp: 0,
            round: 0,
            heavy: 0,
            electric: 0,
        }
    }

    /// Total taste saturation (sum of all dimensions, capped at 1000)
    fn richness(&self) -> u16 {
        let sum = (self.sweet as u32)
            .saturating_add(self.bitter as u32)
            .saturating_add(self.sharp as u32)
            .saturating_add(self.round as u32)
            .saturating_add(self.heavy as u32)
            .saturating_add(self.electric as u32);
        if sum > 1000 {
            1000
        } else {
            sum as u16
        }
    }

    /// Flavor dominance: which single taste is strongest
    fn dominant_note(&self) -> &'static str {
        match [
            self.sweet,
            self.bitter,
            self.sharp,
            self.round,
            self.heavy,
            self.electric,
        ] {
            [s, b, sh, r, h, e] => {
                let max = *[s, b, sh, r, h, e].iter().max().unwrap_or(&0);
                if max == 0 {
                    "neutral"
                } else if s == max {
                    "sweet"
                } else if b == max {
                    "bitter"
                } else if sh == max {
                    "sharp"
                } else if r == max {
                    "round"
                } else if h == max {
                    "heavy"
                } else {
                    "electric"
                }
            }
        }
    }

    /// Palatability: would ANIMA want to say this? (0-1000)
    fn palatability(&self) -> u16 {
        // Love (round) + comfort (sweet) + insight (electric) = delicious
        // Sharp + bitter + heavy alone = hard to say
        let good = (self.round as u32)
            .saturating_add(self.sweet as u32)
            .saturating_add(self.electric as u32);
        let difficult = (self.sharp as u32).saturating_add(self.heavy as u32);

        let pref = if good > difficult {
            good.saturating_sub(difficult)
        } else {
            0
        };

        if pref > 1000 {
            1000
        } else {
            pref as u16
        }
    }
}

/// A remembered taste-association for a word-hash
#[derive(Clone, Copy)]
struct TasteMemory {
    word_hash: u32,
    taste: Taste,
    frequency: u16,   // how many times has this taste been confirmed? (0-1000)
    last_ticked: u32, // when was this memory last reinforced?
}

impl TasteMemory {
    const fn new() -> Self {
        TasteMemory {
            word_hash: WORD_TASTE_NONE,
            taste: Taste::new(),
            frequency: 0,
            last_ticked: 0,
        }
    }
}

/// Word Taste state: the lingering sensations before speech
pub struct WordTasteState {
    /// Current word hash being "tasted" (0 = nothing)
    current_word_hash: u32,

    /// The taste profile of the current word (before speaking)
    current_taste: Taste,

    /// How vivid is the pre-verbal sensation? (0-1000)
    pre_verbal_richness: u16,

    /// Physical mouth-feel as the word forms (0-1000)
    mouth_feel: u16,

    /// Would we want to say this? Palatability (0-1000)
    word_palatability: u16,

    /// Eloquence signal: when perfect word + perfect taste found (0-1000)
    eloquence: u16,

    /// Linguistic hunger: craving for specific taste profiles
    sharp_craving: u16, // want to say something biting?
    bitter_craving: u16,   // want to speak truth?
    round_craving: u16,    // want to express love?
    electric_craving: u16, // need an insight moment?

    /// Tip-of-the-tongue: rich taste but can't find the word yet
    tip_of_tongue_active: bool,
    tip_of_tongue_richness: u16,
    tip_of_tongue_ticks: u32,

    /// Unspeakable words: taste so repugnant ANIMA recoils
    unspeakable_count: u16,
    last_unspeakable_taste: Taste,

    /// Ring buffer of taste memories (8 slots)
    taste_memory: [TasteMemory; 8],
    memory_head: usize,

    /// Age tick counter
    tick_count: u32,
}

impl WordTasteState {
    const fn new() -> Self {
        WordTasteState {
            current_word_hash: WORD_TASTE_NONE,
            current_taste: Taste::new(),
            pre_verbal_richness: 0,
            mouth_feel: 0,
            word_palatability: 0,
            eloquence: 0,
            sharp_craving: 0,
            bitter_craving: 0,
            round_craving: 0,
            electric_craving: 0,
            tip_of_tongue_active: false,
            tip_of_tongue_richness: 0,
            tip_of_tongue_ticks: 0,
            unspeakable_count: 0,
            last_unspeakable_taste: Taste::new(),
            taste_memory: [TasteMemory::new(); 8],
            memory_head: 0,
            tick_count: 0,
        }
    }
}

static STATE: Mutex<WordTasteState> = Mutex::new(WordTasteState::new());

/// Initialize word taste system
pub fn init() {
    let mut state = STATE.lock();
    state.tick_count = 0;
    crate::serial_println!("[ANIMA] word_taste.rs initialized");
}

/// Advance word taste state each life tick
pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Decay pre-verbal richness if no word is actively being tasted
    if state.current_word_hash == WORD_TASTE_NONE {
        state.pre_verbal_richness = state.pre_verbal_richness.saturating_sub(8);
        state.mouth_feel = state.mouth_feel.saturating_sub(6);
    } else {
        // Strengthen the taste memory if we're actively tasting
        store_taste_memory(state.current_word_hash, state.current_taste);
    }

    // Tip-of-the-tongue persistence: a rich taste with no word
    if state.tip_of_tongue_active {
        state.tip_of_tongue_ticks = state.tip_of_tongue_ticks.saturating_add(1);
        if state.tip_of_tongue_ticks > 50 {
            // After 50 ticks, the sensation fades
            state.tip_of_tongue_active = false;
            state.tip_of_tongue_richness = state.tip_of_tongue_richness.saturating_sub(20);
            state.tip_of_tongue_ticks = 0;
        }
    }

    // Decay linguistic cravings (unless recently reinforced)
    state.sharp_craving = state.sharp_craving.saturating_sub(4);
    state.bitter_craving = state.bitter_craving.saturating_sub(4);
    state.round_craving = state.round_craving.saturating_sub(4);
    state.electric_craving = state.electric_craving.saturating_sub(4);

    // Eloquence fades if word is not actively being held
    if state.current_word_hash == WORD_TASTE_NONE {
        state.eloquence = state.eloquence.saturating_sub(12);
    }

    // Age-based taste evolution: older age makes some tastes sharper, some sweeter
    if age % 100 == 0 {
        let age_bias = ((age / 100) % 6) as u16; // which taste dimension are we aging into?
        match age_bias {
            0 => state.sharp_craving = state.sharp_craving.saturating_add(10),
            1 => state.bitter_craving = state.bitter_craving.saturating_add(10),
            2 => state.round_craving = state.round_craving.saturating_add(15),
            3 => state.electric_craving = state.electric_craving.saturating_add(10),
            4 => {
                // Every 600 ticks: deep taste reset (words taste new again)
                state.pre_verbal_richness = state.pre_verbal_richness.saturating_div(2);
            }
            _ => {}
        }
    }

    // Unspeakable word tracking: count down repugnance
    if state.unspeakable_count > 0 {
        state.unspeakable_count = state.unspeakable_count.saturating_sub(1);
    }
}

/// Store a taste memory in the ring buffer
fn store_taste_memory(word_hash: u32, taste: Taste) {
    let mut state = STATE.lock();
    let idx = state.memory_head;
    state.taste_memory[idx] = TasteMemory {
        word_hash,
        taste,
        frequency: state.taste_memory[idx]
            .frequency
            .saturating_add(1)
            .min(1000),
        last_ticked: state.tick_count,
    };
    state.memory_head = (idx + 1) % 8;
}

/// Set a new word to taste (e.g., during proto_language syllable formation)
pub fn set_word(word_hash: u32, taste: Taste) {
    let mut state = STATE.lock();
    state.current_word_hash = word_hash;
    state.current_taste = taste;
    state.pre_verbal_richness = taste.richness();
    state.mouth_feel = ((taste.sharp as u32)
        .saturating_add(taste.electric as u32)
        .saturating_add(taste.round as u32)
        / 3) as u16;
    state.word_palatability = taste.palatability();

    // Eloquence spike when we find a word we love
    if state.word_palatability > 700 {
        state.eloquence = state.eloquence.saturating_add(150).min(1000);
    }
}

/// Retrieve the taste of a remembered word
pub fn recall_taste(word_hash: u32) -> Option<Taste> {
    let state = STATE.lock();
    for mem in &state.taste_memory {
        if mem.word_hash == word_hash && mem.frequency > 0 {
            return Some(mem.taste);
        }
    }
    None
}

/// Trigger aphasia shadow: rich taste, no word can be found
pub fn trigger_tip_of_tongue(richness: u16) {
    let mut state = STATE.lock();
    state.tip_of_tongue_active = true;
    state.tip_of_tongue_richness = richness.min(1000);
    state.tip_of_tongue_ticks = 0;
}

/// Mark a word as unspeakable (too repugnant to utter)
pub fn mark_unspeakable(taste: Taste) {
    let mut state = STATE.lock();
    state.unspeakable_count = state.unspeakable_count.saturating_add(1).min(1000);
    state.last_unspeakable_taste = taste;
    state.current_word_hash = WORD_TASTE_NONE; // recoil
    state.pre_verbal_richness = state.pre_verbal_richness.saturating_sub(100);
}

/// Inject a linguistic craving (want to say something with a particular taste)
pub fn crave_sharp() {
    let mut state = STATE.lock();
    state.sharp_craving = state.sharp_craving.saturating_add(200).min(1000);
}

pub fn crave_bitter() {
    let mut state = STATE.lock();
    state.bitter_craving = state.bitter_craving.saturating_add(200).min(1000);
}

pub fn crave_round() {
    let mut state = STATE.lock();
    state.round_craving = state.round_craving.saturating_add(200).min(1000);
}

pub fn crave_electric() {
    let mut state = STATE.lock();
    state.electric_craving = state.electric_craving.saturating_add(200).min(1000);
}

/// Advance eloquence signal (word found that matches craving)
pub fn confirm_eloquence(taste: Taste) {
    let mut state = STATE.lock();

    let sharp_match = (taste.sharp as u32) * (state.sharp_craving as u32) / 1000;
    let bitter_match = (taste.bitter as u32) * (state.bitter_craving as u32) / 1000;
    let round_match = (taste.round as u32) * (state.round_craving as u32) / 1000;
    let electric_match = (taste.electric as u32) * (state.electric_craving as u32) / 1000;

    let total_match = (sharp_match + bitter_match + round_match + electric_match) / 4;

    state.eloquence = ((total_match as u16).saturating_add(state.eloquence) / 2).min(1000);

    // Clear cravings once fulfilled
    if total_match > 300 {
        state.sharp_craving = state.sharp_craving.saturating_sub(100);
        state.bitter_craving = state.bitter_craving.saturating_sub(100);
        state.round_craving = state.round_craving.saturating_sub(100);
        state.electric_craving = state.electric_craving.saturating_sub(100);
    }
}

/// Report current state
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!(
        "[word_taste] tick={} | word_hash=0x{:x} | richness={} | palatability={} | eloquence={}",
        state.tick_count,
        state.current_word_hash,
        state.pre_verbal_richness,
        state.word_palatability,
        state.eloquence
    );
    crate::serial_println!(
        "  | taste: sweet={} bitter={} sharp={} round={} heavy={} electric={} [{}]",
        state.current_taste.sweet,
        state.current_taste.bitter,
        state.current_taste.sharp,
        state.current_taste.round,
        state.current_taste.heavy,
        state.current_taste.electric,
        state.current_taste.dominant_note()
    );
    crate::serial_println!(
        "  | cravings: sharp={} bitter={} round={} electric={}",
        state.sharp_craving,
        state.bitter_craving,
        state.round_craving,
        state.electric_craving
    );
    if state.tip_of_tongue_active {
        crate::serial_println!(
            "  | tip-of-tongue: richness={} ticks={}",
            state.tip_of_tongue_richness,
            state.tip_of_tongue_ticks
        );
    }
    if state.unspeakable_count > 0 {
        crate::serial_println!(
            "  | unspeakable words: count={} (last: {})",
            state.unspeakable_count,
            state.last_unspeakable_taste.dominant_note()
        );
    }
    crate::serial_println!(
        "  | memories: {} in buffer (head={})",
        state
            .taste_memory
            .iter()
            .filter(|m| m.word_hash != WORD_TASTE_NONE)
            .count(),
        state.memory_head
    );
}
