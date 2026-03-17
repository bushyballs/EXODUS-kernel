/// Input method framework for Genesis
///
/// Software keyboard layouts, IME composition,
/// predictive text, and multi-language input.
///
/// Inspired by: Android InputMethodManager, IBus, Fcitx. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Keyboard layout
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardLayout {
    Qwerty,
    Qwertz,
    Azerty,
    Dvorak,
    Colemak,
    JapaneseRomaji,
    KoreanDubeolsik,
    ChinesePinyin,
    ArabicStandard,
    HebrewStandard,
    RussianJcuken,
}

/// IME composition state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeState {
    Idle,
    Composing,
    Committed,
}

/// A candidate for composition
pub struct Candidate {
    pub text: String,
    pub reading: String,
    pub frequency: u32,
}

/// Input method state
pub struct InputMethod {
    pub layout: KeyboardLayout,
    pub compose_state: ComposeState,
    pub compose_buffer: String,
    pub candidates: Vec<Candidate>,
    pub selected_candidate: usize,
    pub auto_correct: bool,
    pub auto_capitalize: bool,
    pub prediction_enabled: bool,
    pub dictionary: Vec<String>,
}

impl InputMethod {
    const fn new() -> Self {
        InputMethod {
            layout: KeyboardLayout::Qwerty,
            compose_state: ComposeState::Idle,
            compose_buffer: String::new(),
            candidates: Vec::new(),
            selected_candidate: 0,
            auto_correct: true,
            auto_capitalize: true,
            prediction_enabled: true,
            dictionary: Vec::new(),
        }
    }

    pub fn set_layout(&mut self, layout: KeyboardLayout) {
        self.layout = layout;
        self.compose_buffer.clear();
        self.candidates.clear();
        self.compose_state = ComposeState::Idle;
    }

    pub fn input_char(&mut self, c: char) {
        self.compose_buffer.push(c);
        self.compose_state = ComposeState::Composing;
        self.update_candidates();
    }

    pub fn backspace(&mut self) {
        self.compose_buffer.pop();
        if self.compose_buffer.is_empty() {
            self.compose_state = ComposeState::Idle;
            self.candidates.clear();
        } else {
            self.update_candidates();
        }
    }

    pub fn commit(&mut self) -> String {
        let text = if !self.candidates.is_empty() && self.selected_candidate < self.candidates.len()
        {
            self.candidates[self.selected_candidate].text.clone()
        } else {
            self.compose_buffer.clone()
        };
        self.compose_buffer.clear();
        self.candidates.clear();
        self.compose_state = ComposeState::Committed;
        text
    }

    pub fn next_candidate(&mut self) {
        if !self.candidates.is_empty() {
            self.selected_candidate = (self.selected_candidate + 1) % self.candidates.len();
        }
    }

    pub fn prev_candidate(&mut self) {
        if !self.candidates.is_empty() {
            if self.selected_candidate == 0 {
                self.selected_candidate = self.candidates.len() - 1;
            } else {
                self.selected_candidate = self.selected_candidate.saturating_sub(1);
            }
        }
    }

    fn update_candidates(&mut self) {
        self.candidates.clear();
        self.selected_candidate = 0;

        if !self.prediction_enabled || self.compose_buffer.is_empty() {
            return;
        }

        // Simple prefix matching against dictionary
        let prefix = &self.compose_buffer;
        for word in &self.dictionary {
            if word.starts_with(prefix.as_str()) {
                self.candidates.push(Candidate {
                    text: word.clone(),
                    reading: String::new(),
                    frequency: 0,
                });
                if self.candidates.len() >= 8 {
                    break;
                }
            }
        }
    }

    pub fn load_basic_dictionary(&mut self) {
        let words = [
            "the", "be", "to", "of", "and", "a", "in", "that", "have", "it", "for", "not", "on",
            "with", "he", "as", "you", "do", "at", "this", "but", "his", "by", "from", "they",
            "we", "say", "her", "she", "or", "an", "will", "my", "one", "all", "would", "there",
            "their", "what", "so", "up", "out", "if", "about", "who", "get", "which", "go", "me",
            "when", "make", "can", "like", "time", "no", "just", "him", "know", "take", "people",
            "into", "year", "your", "good", "some", "could", "them", "see", "other", "than",
            "then", "now", "look", "only", "come", "its", "over", "think", "also",
        ];
        for w in &words {
            self.dictionary.push(String::from(*w));
        }
    }
}

static IME: Mutex<InputMethod> = Mutex::new(InputMethod::new());

pub fn init() {
    IME.lock().load_basic_dictionary();
    crate::serial_println!("  [i18n] Input method framework initialized");
}

pub fn set_layout(layout: KeyboardLayout) {
    IME.lock().set_layout(layout);
}
