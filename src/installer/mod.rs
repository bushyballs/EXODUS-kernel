/// Hoags OS Installer + OTA Update System
///
/// Two roles:
///   1. Installer: Partitions disk, formats with HoagsFS, installs OS
///   2. OTA Updater: Downloads and applies system updates atomically
///
/// The installer runs from a live USB/ISO environment.
/// The OTA updater runs as a system service on an installed system.
pub mod disk;
pub mod iso;
pub mod ota;

/// Installer configuration
#[derive(Debug, Clone)]
pub struct InstallConfig {
    /// Target disk (e.g., "/dev/nvme0n1")
    pub target_disk: alloc::string::String,
    /// Whether to encrypt with LUKS
    pub encrypt: bool,
    /// Timezone
    pub timezone: alloc::string::String,
    /// Locale
    pub locale: alloc::string::String,
    /// Hostname
    pub hostname: alloc::string::String,
    /// Username for the primary user
    pub username: alloc::string::String,
}

impl InstallConfig {
    pub fn default() -> Self {
        InstallConfig {
            target_disk: alloc::string::String::from("/dev/nvme0n1"),
            encrypt: true,
            timezone: alloc::string::String::from("America/Los_Angeles"),
            locale: alloc::string::String::from("en_US.UTF-8"),
            hostname: alloc::string::String::from("hoags-os"),
            username: alloc::string::String::from("hoags"),
        }
    }
}
