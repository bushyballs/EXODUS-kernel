pub mod ai_biometrics;
pub mod biometric_prompt;
pub mod face_auth;
/// Biometric authentication for Genesis
///
/// Fingerprint, face recognition, iris scan,
/// biometric prompt, and hardware-backed security.
///
/// Inspired by: Android BiometricManager, iOS LAContext. All code is original.
pub mod fingerprint;

use crate::{serial_print, serial_println};

pub fn init() {
    fingerprint::init();
    face_auth::init();
    biometric_prompt::init();
    ai_biometrics::init();
    serial_println!("  Biometric authentication initialized (AI liveness, behavioral auth)");
}
