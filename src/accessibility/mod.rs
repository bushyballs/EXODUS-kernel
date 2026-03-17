pub mod ai_accessibility;
pub mod captions;
pub mod color_correction;
pub mod magnification;
/// Accessibility framework for Genesis
///
/// Screen reader, magnification, color correction,
/// high contrast, switch access, and caption support.
///
/// Inspired by: Android Accessibility, iOS VoiceOver. All code is original.
pub mod screen_reader;

use crate::{serial_print, serial_println};

pub fn init() {
    screen_reader::init();
    magnification::init();
    color_correction::init();
    captions::init();
    ai_accessibility::init();
    serial_println!("  Accessibility framework initialized (AI descriptions, sounds, nav)");
}
