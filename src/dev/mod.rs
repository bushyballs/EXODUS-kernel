/// Developer tools for Genesis — SDK, debugger, profiler, testing
///
/// Provides comprehensive development infrastructure:
/// debugger (breakpoints, step, inspect), profiler, test runner,
/// build system integration, and remote debugging.
///
/// Inspired by: GDB, Valgrind, JUnit, Xcode Instruments. All code is original.
pub mod debugger;
pub mod devtools;
pub mod profiler;
pub mod testing;

use crate::{serial_print, serial_println};

pub fn init() {
    debugger::init();
    profiler::init();
    testing::init();
    devtools::init();
    serial_println!("  Developer tools initialized");
}
