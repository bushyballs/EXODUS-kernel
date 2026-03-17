pub mod ipc_ns;
pub mod mnt_ns;
pub mod net_ns;
/// Linux-compatible process namespace isolation for Genesis
///
/// Provides five namespace types that match the Linux kernel's model:
///
///   pid_ns  — Independent PID numbering per namespace
///   mnt_ns  — Independent filesystem mount table per namespace
///   net_ns  — Independent network stack (loopback) per namespace
///   uts_ns  — Independent hostname and domain name per namespace
///   ipc_ns  — Independent IPC object ID spaces per namespace
///
/// All namespaces are fixed-size, heap-free, and safe to use from
/// interrupt context (spinlock-protected static tables).
///
/// Usage:
///   call `namespaces::init()` once during kernel boot (from process::init).
pub mod pid_ns;
pub mod uts_ns;

use crate::serial_println;

/// Initialize all namespace subsystems.
///
/// Must be called once during early boot, after the serial port is up
/// and before any process namespace operations are performed.
pub fn init() {
    pid_ns::init();
    mnt_ns::init();
    net_ns::init();
    uts_ns::init();
    ipc_ns::init();
    serial_println!("  Namespaces: all subsystems initialized (pid/mnt/net/uts/ipc)");
}
