pub mod ai_sync;
pub mod continuity;
/// Cross-device features for Genesis
///
/// Nearby sharing, device continuity, cloud sync,
/// multi-device clipboard, and phone link.
///
/// Inspired by: Android Nearby Share, Apple Continuity. All code is original.
pub mod nearby_share;
pub mod phone_link;
pub mod sync_engine;

use crate::{serial_print, serial_println};

pub fn init() {
    nearby_share::init();
    continuity::init();
    sync_engine::init();
    phone_link::init();
    ai_sync::init();
    serial_println!("  Cross-device initialized (AI sync priority, conflict, handoff)");
}
