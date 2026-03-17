/// Tethering / Mobile Hotspot for Genesis
///
/// WiFi hotspot, USB tethering, Bluetooth tethering,
/// client management, and bandwidth controls.
///
/// Inspired by: Android Tethering, iOS Personal Hotspot. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Tethering mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TetheringMode {
    WifiHotspot,
    UsbTether,
    BluetoothTether,
    EthernetTether,
}

/// Hotspot security
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotspotSecurity {
    Open,
    Wpa2,
    Wpa3,
}

/// Connected client
pub struct TetherClient {
    pub mac: [u8; 6],
    pub ip: u32,
    pub hostname: String,
    pub connected_at: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub blocked: bool,
}

/// Hotspot configuration
pub struct HotspotConfig {
    pub ssid: String,
    pub password: String,
    pub security: HotspotSecurity,
    pub band: u8, // 2 = 2.4GHz, 5 = 5GHz, 6 = 6GHz
    pub channel: u8,
    pub hidden: bool,
    pub max_clients: u8,
    pub auto_shutoff_min: u16,
}

/// Tethering manager
pub struct TetheringManager {
    pub active_modes: Vec<TetheringMode>,
    pub config: HotspotConfig,
    pub clients: Vec<TetherClient>,
    pub total_rx: u64,
    pub total_tx: u64,
    pub data_limit_bytes: Option<u64>,
    pub data_used: u64,
}

impl TetheringManager {
    const fn new() -> Self {
        TetheringManager {
            active_modes: Vec::new(),
            config: HotspotConfig {
                ssid: String::new(),
                password: String::new(),
                security: HotspotSecurity::Wpa3,
                band: 5,
                channel: 36,
                hidden: false,
                max_clients: 10,
                auto_shutoff_min: 0,
            },
            clients: Vec::new(),
            total_rx: 0,
            total_tx: 0,
            data_limit_bytes: None,
            data_used: 0,
        }
    }

    pub fn start(&mut self, mode: TetheringMode) -> bool {
        if self.active_modes.iter().any(|m| *m == mode) {
            return false;
        }
        self.active_modes.push(mode);
        crate::serial_println!("  [tether] Started {:?} tethering", mode);
        true
    }

    pub fn stop(&mut self, mode: TetheringMode) {
        self.active_modes.retain(|m| *m != mode);
        // Disconnect clients for this mode
    }

    pub fn stop_all(&mut self) {
        self.active_modes.clear();
        self.clients.clear();
    }

    pub fn on_client_connect(&mut self, mac: [u8; 6], ip: u32, hostname: &str) {
        if self.clients.len() >= self.config.max_clients as usize {
            return;
        }
        self.clients.push(TetherClient {
            mac,
            ip,
            hostname: String::from(hostname),
            connected_at: crate::time::clock::unix_time(),
            rx_bytes: 0,
            tx_bytes: 0,
            blocked: false,
        });
    }

    pub fn block_client(&mut self, mac: &[u8; 6]) {
        if let Some(client) = self.clients.iter_mut().find(|c| &c.mac == mac) {
            client.blocked = true;
        }
    }

    pub fn client_count(&self) -> usize {
        self.clients.iter().filter(|c| !c.blocked).count()
    }

    pub fn is_active(&self) -> bool {
        !self.active_modes.is_empty()
    }

    pub fn set_config(&mut self, ssid: &str, password: &str, security: HotspotSecurity) {
        self.config.ssid = String::from(ssid);
        self.config.password = String::from(password);
        self.config.security = security;
    }
}

static TETHER: Mutex<TetheringManager> = Mutex::new(TetheringManager::new());

pub fn init() {
    let mut t = TETHER.lock();
    t.config.ssid = String::from("HoagsOS-Hotspot");
    t.config.password = String::from("hoags2026");
    crate::serial_println!("  [connectivity] Tethering manager initialized");
}
