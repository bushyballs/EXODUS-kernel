//! Genesis OS - System Updates and OTA Module
//!
//! Provides over-the-air update management, delta patching, and rollback capabilities.

#![allow(unused_imports)]

pub mod delta;
pub mod ota_manager;
pub mod rollback;

use crate::{serial_print, serial_println};

/// Initialize all update subsystems
pub fn init() {
    serial_println!("[UPDATES] Initializing system update subsystem...");

    ota_manager::init();
    delta::init();
    rollback::init();

    serial_println!("[UPDATES] System update subsystem initialized");
}
