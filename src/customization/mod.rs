/// OS customization framework for Genesis
///
/// Desktop layout, keyboard shortcuts, gesture mapping,
/// user profiles, and startup customization.
pub mod desktop_layout;
pub mod gestures;
pub mod profiles;
pub mod settings;
pub mod shortcuts;
pub mod startup;

use crate::{serial_print, serial_println};

pub fn init() {
    desktop_layout::init();
    shortcuts::init();
    gestures::init();
    profiles::init();
    startup::init();
    settings::init();
    serial_println!(
        "  Customization initialized (desktop, shortcuts, gestures, profiles, startup)"
    );
}
