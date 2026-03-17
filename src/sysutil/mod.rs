pub mod backup_restore;
pub mod data_usage;
pub mod diagnostics;
/// Hoags System Utilities — essential tools for Genesis OS
///
/// Provides:
///   1. Password Manager — encrypted credential vault with auto-fill
///   2. Data Usage — network traffic monitoring with limits and alerts
///   3. Backup/Restore — full device backup, incremental, selective, encrypted
///   4. Diagnostics — system health tests, benchmarks, stress tests
///
/// All modules use Q16 fixed-point math (i32, 1.0 = 65536),
/// kernel-level Mutex synchronization, and no external crates.
///
/// Inspired by: Android Settings utilities, iOS system tools,
/// KDE System Monitor. All code is original.
pub mod password_mgr;

use crate::{serial_print, serial_println};

/// Initialize all system utility subsystems
pub fn init() {
    password_mgr::init();
    data_usage::init();
    backup_restore::init();
    diagnostics::init();
    serial_println!("  System utilities: password_mgr, data_usage, backup_restore, diagnostics");
}
