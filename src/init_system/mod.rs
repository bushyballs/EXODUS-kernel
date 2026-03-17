/// Service manager (systemd equivalent)
///
/// Part of the AIOS.
///
/// Subsystems:
///   - service / service_mgr: Service lifecycle and orchestration
///   - unit / unit_file: Unit state machine and INI-style file parsing
///   - target: Boot target grouping (rescue, multi-user, graphical)
///   - dependency: Topological sort for service ordering
///   - cgroup_mgr: Per-service resource limits (CPU, memory, I/O)
///   - journal: Ring-buffer structured logging with severity levels
///   - timer / timer_unit: Periodic and oneshot timers
///   - watchdog / watchdog_mgr: Heartbeat monitoring and auto-restart
///   - socket_activation: Lazy service start on socket activity

pub mod service;
pub mod service_mgr;
pub mod unit;
pub mod unit_file;
pub mod target;
pub mod socket_activation;
pub mod cgroup_mgr;
pub mod journal;
pub mod timer;
pub mod timer_unit;
pub mod watchdog;
pub mod watchdog_mgr;
pub mod dependency;

use crate::{serial_print, serial_println};

/// Initialize all init_system subsystems in dependency order.
pub fn init() {
    serial_println!("[init_system] initializing service manager subsystems...");

    journal::init();
    dependency::init();
    unit::init();
    unit_file::init();
    cgroup_mgr::init();
    service::init();
    service_mgr::init();
    target::init();
    timer::init();
    timer_unit::init();
    socket_activation::init();
    watchdog::init();
    watchdog_mgr::init();

    serial_println!("[init_system] all subsystems initialized");
}
