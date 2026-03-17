pub mod bootloader_repair;
pub mod repair;
pub mod restore;
pub mod safe_mode;
/// Hoags Recovery — system recovery and restore for Genesis OS
///
/// Provides:
///   1. Snapshot — create, list, compare, and manage system state snapshots
///   2. Restore — rollback to snapshot, selective restore, integrity verification
///   3. Repair — filesystem check, registry repair, boot fix, service recovery
///   4. Safe Mode — minimal driver boot, diagnostic mode, network safe mode
///   5. Bootloader Repair — MBR/GPT fix, bootloader reinstall, boot menu management
///
/// All modules use Q16 fixed-point math (i32, 1.0 = 65536),
/// kernel-level Mutex synchronization, and no external crates.
///
/// All code is original. Built from scratch by Hoags Inc.
pub mod snapshot;

use crate::{serial_print, serial_println};

/// Initialize all recovery subsystems
pub fn init() {
    serial_println!("[RECOVERY] Initializing system recovery subsystem...");

    snapshot::init();
    restore::init();
    repair::init();
    safe_mode::init();
    bootloader_repair::init();

    serial_println!("[RECOVERY] System recovery subsystem initialized");
    serial_println!("  Recovery: snapshot, restore, repair, safe_mode, bootloader_repair");
}
