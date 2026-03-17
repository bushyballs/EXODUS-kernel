/// Hoags Time — timekeeping and timers for Genesis
///
/// Sources:
///   - RTC (CMOS real-time clock) for wall clock time
///   - PIT/HPET/TSC for monotonic high-resolution timing
///   - Software timers for scheduling delayed work
///
/// All code is original.
use crate::{serial_print, serial_println};
pub mod clock;
pub mod hrtimer;
pub mod rtc;
pub mod timer;

pub use clock::uptime_ms;

pub fn init() {
    rtc::init();
    clock::init();
    timer::init();
    hrtimer::init();
    serial_println!("  Time: RTC, system clock, timers, hrtimers");
}
