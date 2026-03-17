use super::parser::Config;
use crate::sync::Mutex;
/// System configuration — default settings for Hoags OS
use crate::{serial_print, serial_println};

static SYSTEM_CONFIG: Mutex<Option<Config>> = Mutex::new(None);

/// Load the default system configuration
pub fn default_config() -> Config {
    Config::parse(
        r#"
[system]
hostname = hoags-os
timezone = America/Los_Angeles
locale = en_US.UTF-8
version = 0.4.0
codename = Genesis

[display]
compositor = hoags-compositor
theme = dark
font_size = 14
dpi = 96
vsync = true
refresh_rate = 60

[network]
hostname = hoags-os
dns = 1.1.1.1, 8.8.8.8
wireguard = false
firewall = true

[audio]
master_volume = 80
output = auto
input = auto
sample_rate = 48000

[security]
mac_enforcing = true
audit = true
auto_updates = true
update_channel = stable

[power]
sleep_timeout = 600
screen_timeout = 300
suspend_on_lid_close = true

[pkg]
repos = https://pkg.hoagsinc.com/genesis/stable, https://pkg.hoagsinc.com/genesis/community
auto_update = true
check_interval = 86400

[ai]
enabled = true
model = local
privacy = strict
offline_capable = true
"#,
    )
}

pub fn init() {
    *SYSTEM_CONFIG.lock() = Some(default_config());
    serial_println!("    [sysconf] System configuration loaded");
}

/// Get a system config value
pub fn get(section: &str, key: &str) -> Option<alloc::string::String> {
    SYSTEM_CONFIG
        .lock()
        .as_ref()?
        .get(section, key)
        .map(|s| alloc::string::String::from(s))
}

/// Set a system config value
pub fn set(section: &str, key: &str, value: &str) {
    if let Some(config) = SYSTEM_CONFIG.lock().as_mut() {
        config.set(section, key, value);
    }
}
