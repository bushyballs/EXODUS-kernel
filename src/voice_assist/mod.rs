pub mod action_dispatch;
pub mod command_parser;
pub mod tts_response;
/// Voice Assistant subsystem for Genesis OS
///
/// Provides a complete on-device voice assistant pipeline:
///   - Wake word detection (energy + pattern matching)
///   - Command parsing with intent/slot extraction
///   - Action dispatch to OS subsystems
///   - Text-to-speech response generation
///
/// All processing is local. No external services.
/// Uses Q16 fixed-point arithmetic (no floating point).
pub mod wake_word;

use crate::sync::Mutex;
use crate::{serial_print, serial_println};

/// Q16 fixed-point: value * 65536 represents the real number.
/// For example, 1.0 = 65536, 0.5 = 32768, 0.25 = 16384.
pub type Q16 = i32;

/// Q16 constants
pub const Q16_ONE: Q16 = 65536;
pub const Q16_HALF: Q16 = 32768;
pub const Q16_ZERO: Q16 = 0;

/// Multiply two Q16 values: (a * b) >> 16
pub fn q16_mul(a: Q16, b: Q16) -> Q16 {
    ((a as i64 * b as i64) >> 16) as Q16
}

/// Divide two Q16 values: (a << 16) / b
pub fn q16_div(a: Q16, b: Q16) -> Q16 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << 16) / (b as i64)) as Q16
}

/// Convert integer to Q16
pub fn q16_from_int(v: i32) -> Q16 {
    v << 16
}

/// Convert Q16 to integer (truncated)
pub fn q16_to_int(v: Q16) -> i32 {
    v >> 16
}

/// Overall voice assistant state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssistantState {
    /// Idle, listening for wake word
    Idle,
    /// Wake word detected, capturing command
    Listening,
    /// Parsing and processing the command
    Processing,
    /// Generating and speaking the response
    Responding,
    /// Error state requiring reset
    Error,
}

/// Global voice assistant state
static ASSISTANT_STATE: Mutex<AssistantState> = Mutex::new(AssistantState::Idle);

/// Get the current assistant state
pub fn get_state() -> AssistantState {
    *ASSISTANT_STATE.lock()
}

/// Set the assistant state
pub fn set_state(state: AssistantState) {
    *ASSISTANT_STATE.lock() = state;
}

/// Process one full voice interaction cycle:
///   1. Check wake word engine for trigger
///   2. Parse recognized tokens into intent
///   3. Dispatch the intent as an OS action
///   4. Generate and queue TTS response
pub fn process_cycle(audio_samples: &[i16], sample_rate: u32, timestamp: u64) {
    let state = get_state();

    match state {
        AssistantState::Idle => {
            // Feed audio to wake word engine
            wake_word::feed_audio(audio_samples, sample_rate, timestamp);
            if wake_word::is_triggered() {
                set_state(AssistantState::Listening);
                serial_println!("    [voice_assist] Wake word detected");
                wake_word::reset();
            }
        }
        AssistantState::Listening => {
            // In a real system we would run STT here.
            // For now we transition to Processing when silence is detected.
            let energy = wake_word::compute_energy(audio_samples);
            let silence_threshold: Q16 = 3276; // ~0.05 in Q16
            if energy < silence_threshold {
                set_state(AssistantState::Processing);
            }
        }
        AssistantState::Processing => {
            // Placeholder token stream — real STT would provide these
            // Process whatever is queued in the command parser
            set_state(AssistantState::Responding);
        }
        AssistantState::Responding => {
            if !tts_response::is_speaking() {
                set_state(AssistantState::Idle);
            }
        }
        AssistantState::Error => {
            // Reset everything
            wake_word::reset();
            tts_response::stop();
            set_state(AssistantState::Idle);
            serial_println!("    [voice_assist] Recovered from error state");
        }
    }
}

/// Initialize the entire voice assistant subsystem
pub fn init() {
    wake_word::init();
    command_parser::init();
    action_dispatch::init();
    tts_response::init();
    set_state(AssistantState::Idle);
    serial_println!("    [voice_assist] Voice assistant subsystem initialized");
}
