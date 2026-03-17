pub mod audio_cast;
pub mod dlna;
/// Cast / screen sharing for Genesis
///
/// Miracast, DLNA, audio multi-room.
pub mod screen_cast;
pub mod session;

use crate::{serial_print, serial_println};

pub fn init() {
    screen_cast::init();
    dlna::init();
    audio_cast::init();
    session::init();
    serial_println!("  Cast initialized (Miracast, DLNA, audio multi-room)");
}
