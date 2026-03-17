/// Hoags Text — text processing and font subsystem for Genesis
///
/// Provides a complete text rendering, spell checking, and handwriting
/// recognition stack built from scratch. No external crates.
///
/// Subsystems:
///   - font_engine: TrueType/OpenType font parsing and glyph rasterization
///   - spell_check: Dictionary-based spell checker with Levenshtein distance
///   - handwriting: Stroke-based handwriting recognition with template matching
///
/// All numeric values use Q16 fixed-point (i32 * 65536) — no floating point.
/// Font metrics use integer math exclusively. Bezier curves are rasterized
/// with De Casteljau subdivision in fixed-point arithmetic.
pub mod font_engine;
pub mod handwriting;
pub mod spell_check;

use crate::{serial_print, serial_println};

pub fn init() {
    serial_println!("[TEXT] Initializing text subsystem...");

    font_engine::init();
    spell_check::init();
    handwriting::init();

    serial_println!("[TEXT] Text subsystem initialized");
}
