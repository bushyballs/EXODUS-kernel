pub mod ai_health;
pub mod fitness;
pub mod medical;
pub mod sleep;
/// Health framework for Genesis
///
/// Health data aggregation, fitness tracking, heart rate,
/// sleep analysis, nutrition, medical records, health alerts,
/// and AI-powered health insights.
///
/// Original implementation for Hoags OS.
pub mod vitals;

use crate::{serial_print, serial_println};

pub fn init() {
    vitals::init();
    fitness::init();
    sleep::init();
    medical::init();
    ai_health::init();
    serial_println!("  Health framework initialized (vitals, fitness, sleep, AI insights)");
}
