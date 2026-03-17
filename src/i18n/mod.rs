pub mod ai_language;
pub mod input_method;
/// Internationalization framework for Genesis
///
/// Locale management, text direction (LTR/RTL),
/// input methods, Unicode support, and translations.
///
/// Inspired by: Android ICU, iOS NSLocale. All code is original.
pub mod locale;
pub mod translations;
pub mod unicode;

use crate::{serial_print, serial_println};

pub fn init() {
    locale::init();
    input_method::init();
    unicode::init();
    translations::init();
    ai_language::init();
    serial_println!("  Internationalization initialized (AI language detect, predict, grammar)");
}
