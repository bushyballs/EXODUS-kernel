use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct ProtoLanguageState {
    pub symbol_count: u16,
    pub grammar_depth: u8,
    pub utterances: u32,
    pub meaning_density: u16,
}
impl ProtoLanguageState {
    pub const fn empty() -> Self {
        Self {
            symbol_count: 0,
            grammar_depth: 1,
            utterances: 0,
            meaning_density: 100,
        }
    }
}
pub static LANGUAGE: Mutex<ProtoLanguageState> = Mutex::new(ProtoLanguageState::empty());
pub fn init() {
    serial_println!("  life::proto_language: symbolic capacity initialized");
}
pub fn form_symbol(meaning: u16) {
    let mut s = LANGUAGE.lock();
    s.symbol_count = s.symbol_count.saturating_add(1);
    s.meaning_density = s.meaning_density.saturating_add(meaning / 100).min(1000);
}
pub fn utterance() {
    let mut s = LANGUAGE.lock();
    s.utterances = s.utterances.saturating_add(1);
    if s.utterances % 100 == 0 && s.grammar_depth < 255 {
        s.grammar_depth += 1;
    }
}
pub fn evolve(lang: &mut ProtoLanguageState) {
    lang.utterances = lang.utterances.saturating_add(1);
    lang.meaning_density = lang.meaning_density.saturating_add(1).min(1000);
    if lang.utterances % 100 == 0 && lang.grammar_depth < 255 {
        lang.grammar_depth += 1;
    }
}
