/// Theming framework for Genesis
///
/// Dynamic color extraction, icon packs, wallpaper management.
pub mod dynamic_color;
pub mod icon_packs;
pub mod wallpaper;

use crate::{serial_print, serial_println};

pub fn init() {
    dynamic_color::init();
    icon_packs::init();
    wallpaper::init();
    serial_println!("  Theming initialized (dynamic color, icons, wallpaper)");
}
