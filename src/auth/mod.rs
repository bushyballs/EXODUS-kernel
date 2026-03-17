/// Hoags Auth — login, session management, and authentication
///
/// Handles:
///   1. Login flow (username + password verification)
///   2. Session tokens (capability-based, time-limited)
///   3. Screen lock/unlock
///   4. PAM-like pluggable authentication
///   5. Biometric hooks (fingerprint, face — when hardware available)
///
/// Uses our crypto subsystem: SHA-256 for password hashing,
/// HMAC for session tokens, CSPRNG for salt generation.
///
/// All code is original.
use crate::{serial_print, serial_println};
pub mod login;
pub mod session;

pub fn init() {
    login::init_user_table();
    session::init();
    serial_println!("  Auth: login, sessions, screen lock");
}
