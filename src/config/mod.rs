/// Hoags Config — system configuration for Genesis
///
/// Configuration files use a simple key=value format:
///   [section]
///   key = value
///   # comments
///
/// System config lives at /etc/hoags.conf
/// User config lives at ~/.config/hoags/
use crate::{serial_print, serial_println};
pub mod parser;
pub mod sysconf;

pub fn init() {
    sysconf::init();
    serial_println!("  Config: system configuration loaded");
}
