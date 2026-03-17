pub mod ai_services;
pub mod alarm_manager;
pub mod download_manager;
/// System services for Genesis
///
/// Job scheduler, alarm manager, download manager,
/// location services, and sensor framework.
///
/// Inspired by: Android System Services, iOS Background Tasks. All code is original.
pub mod job_scheduler;
pub mod location;
pub mod sensors;

use crate::{serial_print, serial_println};

pub fn init() {
    job_scheduler::init();
    alarm_manager::init();
    download_manager::init();
    location::init();
    sensors::init();
    ai_services::init();
    serial_println!("  System services initialized (AI job scheduling, location prediction)");
}
