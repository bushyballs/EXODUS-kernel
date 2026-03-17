/// Hoags Browser Security — content security and tab sandboxing
///
/// Provides browser-level security isolation:
///   1. Tab sandboxing (per-tab process, memory, and filesystem isolation)
///   2. Network security (origin enforcement, domain blocking)
///   3. Privacy guards (fingerprint resistance, cookie isolation, WebRTC control)
///
/// Each browser tab runs inside a security sandbox with configurable
/// policies for filesystem access, network access, memory limits, and
/// CPU quotas. Violations are logged and enforced in real time.
///
/// Inspired by: Chromium site isolation, Firefox Fission, Tor Browser
/// hardening. All code is original.
use crate::{serial_print, serial_println};
pub mod network_sec;
pub mod privacy_guard;
pub mod sandbox;

pub fn init() {
    sandbox::init();
    privacy_guard::init();
    serial_println!("  Browser Security: sandbox, network, privacy");
}
