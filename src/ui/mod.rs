pub mod accessibility;
pub mod animation;
pub mod app_switcher;
pub mod clipboard;
pub mod drag_drop;
pub mod gesture;
pub mod launcher;
pub mod lock_screen;
pub mod navigation;
pub mod notification_shade;
pub mod popup;
pub mod power_menu;
pub mod quick_settings;
pub mod screenshot;
pub mod settings;
/// System UI for Genesis — the user-facing shell
///
/// Implements: status bar, navigation, launcher, lock screen,
/// quick settings, settings app, and the app switcher.
///
/// Inspired by: Android SystemUI, GNOME Shell, iOS SpringBoard. All code is original.
pub mod status_bar;
pub mod themes;
pub mod toast;
pub mod transition;
pub mod volume;

use crate::{serial_print, serial_println};

pub fn init() {
    themes::init();
    status_bar::init();
    navigation::init();
    quick_settings::init();
    lock_screen::init();
    launcher::init();
    app_switcher::init();
    settings::init();
    gesture::init();
    animation::init();
    transition::init();
    accessibility::init();
    clipboard::init();
    drag_drop::init();
    popup::init();
    toast::init();
    notification_shade::init();
    power_menu::init();
    volume::init();
    screenshot::init();
    serial_println!("  System UI initialized (status_bar, navigation, quick_settings, lock_screen, launcher, app_switcher, settings, gesture, animation, transition, accessibility, clipboard, drag_drop, popup, toast, notification_shade, power_menu, volume, screenshot)");
}
