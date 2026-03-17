pub mod ai_wellbeing;
pub mod focus_mode;
/// Digital wellbeing for Genesis
///
/// Screen time tracking, app usage limits, focus mode,
/// wind down, posture reminders, eye strain prevention,
/// bedtime mode, notification batching.
///
/// Original implementation for Hoags OS.
pub mod screen_time;
pub mod wind_down;

use crate::{serial_print, serial_println};

pub fn init() {
    screen_time::init();
    focus_mode::init();
    wind_down::init();
    ai_wellbeing::init();
    serial_println!("  Digital wellbeing initialized (screen time, focus, wind down, AI)");
}
