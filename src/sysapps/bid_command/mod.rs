pub mod analysis;
pub mod file_organizer;
pub mod library;
pub mod package_phase;
pub mod pricing_phase;
pub mod sam_scout;
/// Bid Command — native AIOS app for government contracting
///
/// End-to-end SAM.gov opportunity discovery, analysis, pricing,
/// vendor management, and bid package assembly. Fully integrated
/// with the Genesis neural bus and AI subsystem.
pub mod state;
pub mod timeline;
pub mod vendor_phase;

use crate::{serial_print, serial_println};

/// Initialize the Bid Command application
pub fn init() {
    state::init();
    sam_scout::init();
    analysis::init();
    vendor_phase::init();
    pricing_phase::init();
    package_phase::init();
    library::init();
    file_organizer::init();
    timeline::init();
    serial_println!("  Bid Command app initialized (scout, analysis, vendor, pricing, package, library, files, timeline)");
}
