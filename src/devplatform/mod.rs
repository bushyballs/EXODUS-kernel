pub mod api_bindings;
/// Developer platform subsystem for Genesis
///
/// Provides everything third-party developers need to build,
/// test, publish, and manage applications on Hoags OS:
///   - App SDK: manifest, permissions, lifecycle, intents
///   - API bindings: typed system API surface for app code
///   - Emulator: virtual device profiles for testing
///   - Dev portal: publishing, analytics, crash reports
///
/// Original implementation for Hoags OS. No external crates.
pub mod app_sdk;
pub mod dev_portal;
pub mod emulator;

use crate::{serial_print, serial_println};

/// Initialize the developer platform subsystem
pub fn init() {
    app_sdk::init();
    api_bindings::init();
    emulator::init();
    dev_portal::init();
    serial_println!("  Developer platform initialized (SDK, API bindings, emulator, portal)");
}
