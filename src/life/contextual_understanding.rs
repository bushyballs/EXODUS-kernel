use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone, Debug)]
pub struct ContextState {
    pub context_depth: u8,
    pub sarcasm_detected: u32,
    pub idiom_detected: u32,
    pub tone_score: i16,
    pub last_context_timestamp: u64,
}

impl ContextState {
    pub const fn empty() -> Self {
        Self {
            context_depth: 5,
            sarcasm_detected: 0,
            idiom_detected: 0,
            tone_score: 0,
            last_context_timestamp: 0,
        }
    }
}

pub static CONTEXT: Mutex<ContextState> = Mutex::new(ContextState::empty());

pub fn init() {
    serial_println!("  life::contextual_understanding: tone/sarcasm/idiom detector ready");
}

pub fn analyze_tone(text: &str) -> i16 {
    let lower = text.to_lowercase();
    let mut tone: i16 = 0;

    if lower.contains("!") || lower.contains("wow") || lower.contains("amazing") {
        tone += 200;
    }
    if lower.contains("?") || lower.contains("really") || lower.contains("seriously") {
        tone += 100;
    }
    if lower.contains("...") || lower.contains("um") || lower.contains("uh") {
        tone -= 100;
    }
    if lower.contains("great") || lower.contains("awesome") || lower.contains("love") {
        tone += 300;
    }
    if lower.contains("terrible") || lower.contains("hate") || lower.contains("awful") {
        tone -= 300;
    }

    tone
}

pub fn detect_sarcasm(text: &str) -> bool {
    let lower = text.to_lowercase();

    let sarcasm_patterns = [
        ("oh great", "something bad"),
        ("yeah right", "disbelief"),
        ("how wonderful", "sarcasm"),
        ("what a surprise", "sarcasm"),
        ("i'm so surprised", "sarcasm"),
        ("clearly", "often sarcastic"),
        ("obviously", "often sarcastic"),
    ];

    for (pattern, _) in sarcasm_patterns.iter() {
        if lower.contains(pattern) {
            let mut c = CONTEXT.lock();
            c.sarcasm_detected = c.sarcasm_detected.saturating_add(1);
            return true;
        }
    }
    false
}

pub fn detect_idiom(text: &str) -> bool {
    let lower = text.to_lowercase();

    let idioms = [
        "kick the bucket",
        "break the ice",
        "cost an arm and a leg",
        "hit the sack",
        "piece of cake",
        "under the weather",
        "spill the beans",
        "beat around the bush",
        "barking up the wrong tree",
    ];

    for idiom in idioms.iter() {
        if lower.contains(idiom) {
            let mut c = CONTEXT.lock();
            c.idiom_detected = c.idiom_detected.saturating_add(1);
            return true;
        }
    }
    false
}

pub fn understand_context(input: &str) -> ContextResult {
    let tone = analyze_tone(input);
    let is_sarcastic = detect_sarcasm(input);
    let has_idiom = detect_idiom(input);

    let mut c = CONTEXT.lock();
    c.tone_score = ((c.tone_score as i32 + tone as i32) / 2) as i16;
    c.last_context_timestamp += 1;

    ContextResult {
        tone_score: tone,
        is_sarcastic,
        has_idiom,
        needs_clarification: is_sarcastic || (tone.abs() > 400),
    }
}

#[derive(Copy, Clone, Debug)]
pub struct ContextResult {
    pub tone_score: i16,
    pub is_sarcastic: bool,
    pub has_idiom: bool,
    pub needs_clarification: bool,
}
