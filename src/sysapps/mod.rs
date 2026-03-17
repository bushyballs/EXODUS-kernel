pub mod bid_command;
pub mod calculator;
pub mod clock;
/// Built-in system applications for Genesis OS
///
/// Provides core productivity and media apps that ship with the OS:
/// file manager, photo gallery, music player, calculator, clock,
/// PDF reader, and notes. All apps use fixed-point Q16 math,
/// kernel-level Mutex synchronization, and no external crates.
///
/// Inspired by: GNOME core apps, KDE apps, Android AOSP apps.
/// All code is original.
pub mod file_manager;
pub mod gallery;
pub mod music_player;
pub mod notes;
pub mod pdf_reader;

use crate::{serial_print, serial_println};

/// Initialize all built-in system applications
pub fn init() {
    file_manager::init();
    gallery::init();
    music_player::init();
    calculator::init();
    clock::init();
    pdf_reader::init();
    notes::init();
    bid_command::init();
    serial_println!("  System apps initialized (file_manager, gallery, music_player, calculator, clock, pdf_reader, notes, bid_command)");
}
