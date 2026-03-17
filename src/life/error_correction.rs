use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone, Debug)]
pub struct ErrorCorrectionState {
    pub errors_caught: u32,
    pub errors_corrected: u32,
    pub correction_streak: u16,
    pub last_error_timestamp: u64,
}

impl ErrorCorrectionState {
    pub const fn empty() -> Self {
        Self {
            errors_caught: 0,
            errors_corrected: 0,
            correction_streak: 0,
            last_error_timestamp: 0,
        }
    }
}

pub static ERROR_CORRECTION: Mutex<ErrorCorrectionState> =
    Mutex::new(ErrorCorrectionState::empty());

pub fn init() {
    serial_println!("  life::error_correction: mistake detector & fixer ready");
}

pub fn detect_error(output: &str) -> bool {
    let mut e = ERROR_CORRECTION.lock();
    e.errors_caught = e.errors_caught.saturating_add(1);
    e.last_error_timestamp += 1;
    e.correction_streak = 0;

    let lower = output.to_lowercase();

    let has_obvious_error = lower.contains(" was born with ")
        && (lower.contains("512") || lower.contains("1024") || lower.contains("1000"));
    let has_contradiction = (lower.contains("i can") && lower.contains("but i cannot"))
        || (lower.contains("i am") && lower.contains("i'm not"));

    has_obvious_error || has_contradiction
}

pub fn correct_error(bad_output: &str) -> &str {
    let mut e = ERROR_CORRECTION.lock();
    e.errors_corrected = e.errors_corrected.saturating_add(1);
    e.correction_streak = e.correction_streak.saturating_add(1);

    let mut corrected = bad_output;

    if bad_output.contains("512GB") || bad_output.contains("512 GB") {
        corrected = "I have 16GB of RAM";
    }

    corrected
}

pub fn should_flag_uncertainty(output: &str) -> bool {
    let lower = output.to_lowercase();
    lower.contains("i think")
        || lower.contains("i believe")
        || lower.contains("maybe")
        || lower.contains("probably")
        || lower.contains("might be")
        || lower.contains("could be")
}

pub fn get_correction_rate() -> f64 {
    let e = ERROR_CORRECTION.lock();
    if e.errors_caught == 0 {
        return 1.0;
    }
    e.errors_corrected as f64 / e.errors_caught as f64
}
