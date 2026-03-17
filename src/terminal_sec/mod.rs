/// Hoags Terminal Security — command filtering, privilege guarding, and audit logging
///
/// Secures the Genesis terminal/shell against:
///   1. Dangerous commands (rm -rf /, dd to devices, fork bombs, etc.)
///   2. Injection attacks (backticks, path traversal, null bytes, variable expansion)
///   3. Unauthorized privilege escalation (sudo, su, kernel module loading)
///   4. Data exfiltration (reverse shells, encoded piping, archive tunneling)
///
/// Every command typed into the terminal passes through cmd_filter before
/// execution. priv_guard enforces least-privilege per session. audit_log
/// records all security-relevant events for forensic review.
///
/// Inspired by: AppArmor command profiles, SELinux transition rules,
/// PowerShell Constrained Language Mode. All code is original.
use crate::{serial_print, serial_println};
pub mod audit_log;
pub mod cmd_filter;
pub mod priv_guard;

/// Initialize all terminal security subsystems
pub fn init() {
    cmd_filter::init();
    priv_guard::init();
    audit_log::init();
    serial_println!("  Terminal Security: filter, privilege guard, audit log");
}
