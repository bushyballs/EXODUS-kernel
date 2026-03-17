/// Structured logging framework
///
/// Part of the AIOS.

pub mod logger;
pub mod sink;
pub mod format;
pub mod filter;
pub mod rotate;
pub mod remote;

pub fn init() {
    filter::init();
    format::init();
    rotate::init();
    sink::init();
    remote::init();
    logger::init();
    crate::serial_println!("  Log system initialized (logger, sinks, filter, format, rotate, remote)");
}
