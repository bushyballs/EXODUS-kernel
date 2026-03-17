/// Preferences framework for Genesis
///
/// A comprehensive user preferences system providing:
///   1. Typed key-value storage with namespaces
///   2. Schema-driven validation with defaults, ranges, and descriptions
///   3. Change observers with batch notifications
///   4. Import/export, migration, and versioning
///   5. Built-in system preferences (display, sound, network, privacy, security)
///
/// All values stored as Q16 fixed-point or string representations.
/// No floating-point arithmetic (f32/f64) anywhere.
use crate::{serial_print, serial_println};

pub mod observer;
pub mod schema;
pub mod store;
pub mod sync_prefs;
pub mod system_prefs;

/// Initialize the entire preferences subsystem
pub fn init() {
    store::init();
    schema::init();
    observer::init();
    sync_prefs::init();
    system_prefs::init();
    serial_println!("  Preferences: framework initialized (store, schema, observer, sync, system)");
}
