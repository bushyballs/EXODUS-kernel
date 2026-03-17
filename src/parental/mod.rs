pub mod activity_report;
pub mod ai_parental;
/// Parental controls for Genesis
///
/// Child profiles, content filtering, app restrictions,
/// location tracking, screen time limits, activity reports,
/// and AI-powered content safety.
///
/// Original implementation for Hoags OS.
pub mod child_profile;
pub mod content_filter;

use crate::{serial_print, serial_println};

pub fn init() {
    child_profile::init();
    content_filter::init();
    activity_report::init();
    ai_parental::init();
    serial_println!("  Parental controls initialized (profiles, filtering, AI safety)");
}
