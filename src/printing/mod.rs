pub mod discovery;
pub mod jobs;
pub mod renderer;
/// Printing framework for Genesis
///
/// Print spooler, IPP/AirPrint/Mopria,
/// PDF rendering, printer discovery, print preview.
///
/// Original implementation for Hoags OS.
pub mod spooler;

use crate::{serial_print, serial_println};

pub fn init() {
    spooler::init();
    discovery::init();
    renderer::init();
    jobs::init();
    serial_println!("  Printing framework initialized (spooler, IPP, PDF render)");
}
