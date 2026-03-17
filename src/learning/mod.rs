pub mod adaptation;
pub mod feedback;
pub mod knowledge;
/// Persistent AI Learning Module for Genesis
///
/// The OS learns from user behavior over time, building a model
/// of habits, preferences, and usage patterns. This enables:
///   1. Pattern detection — app usage, time-of-day habits, sequences
///   2. Adaptive behavior — preload predictions, layout optimization
///   3. User profiles — learning rate, confidence decay, habit scoring
///   4. Feedback loops — explicit corrections, implicit signals
///   5. Knowledge store — facts, preferences, associative recall
///
/// All learning is on-device. No data leaves the machine.
/// Uses Q16 fixed-point math (i32, 16 fractional bits, 65536 = 1.0).
///
/// Inspired by: Apple on-device learning, Android Adaptive Battery,
/// predictive text systems. All code is original.
pub mod patterns;
pub mod profile;

use crate::{serial_print, serial_println};

pub fn init() {
    patterns::init();
    profile::init();
    feedback::init();
    knowledge::init();
    adaptation::init();
    serial_println!("  Learning: patterns, profiles, feedback, knowledge, adaptation");
}
