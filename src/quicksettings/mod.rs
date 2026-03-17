/// Quick settings for Genesis
///
/// System toggles, tiles, customizable panel.
pub mod tiles;
pub mod toggles;

use crate::{serial_print, serial_println};

pub fn init() {
    tiles::init();
    toggles::init();
    serial_println!("  Quick settings initialized (tiles, toggles)");
}
