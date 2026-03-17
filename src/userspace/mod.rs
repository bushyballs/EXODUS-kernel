pub mod ai_shell;
pub mod coreutils;
pub mod cron;
pub mod dmesg;
pub mod dynlink;
pub mod flatpak;
/// Hoags OS userspace services
///
/// These are the core system services that run as processes:
///   - hoags-init: Service supervisor (PID 1)
///   - hoags-shell: Command-line shell
///   - hoags-pkg: Package manager
///
/// In the final OS, these will be separate ELF binaries.
/// For now, they're kernel threads that demonstrate the architecture.
pub mod init_service;
pub mod libc;
pub mod login_mgr;
pub mod pkg;
pub mod pkg_build;
pub mod pkg_repo;
pub mod pthread;
pub mod service;
pub mod shell;
pub mod sudo;
pub mod syslog;
