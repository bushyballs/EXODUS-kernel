use crate::sync::Mutex;
use alloc::string::String;
/// Mobile hotspot management
///
/// Creates and manages a software access point for
/// sharing network connectivity. Part of the AIOS connectivity layer.
/// Implements AP mode configuration, client tracking, DHCP address
/// pool management, bandwidth monitoring, and channel selection.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Hotspot security mode
#[derive(Clone, Copy, PartialEq)]
pub enum SecurityMode {
    Open,
    Wpa2Psk,
    Wpa3Sae,
}

/// Hotspot frequency band
#[derive(Clone, Copy, PartialEq)]
pub enum FrequencyBand {
    Band2_4GHz,
    Band5GHz,
    DualBand,
}

/// Hotspot configuration
pub struct HotspotConfig {
    pub ssid: String,
    pub password: String,
    pub channel: u8,
    pub max_clients: u8,
}

/// Extended hotspot configuration (internal)
struct HotspotConfigExt {
    ssid: String,
    password: String,
    channel: u8,
    max_clients: u8,
    security: SecurityMode,
    band: FrequencyBand,
    hidden_ssid: bool,
    beacon_interval_ms: u16,
    /// DHCP pool start IP (as u32, e.g. 192.168.43.2)
    dhcp_pool_start: u32,
    /// DHCP pool end IP
    dhcp_pool_end: u32,
    /// Gateway IP
    gateway_ip: u32,
    /// Subnet mask
    subnet_mask: u32,
}

impl HotspotConfigExt {
    fn from_basic(config: &HotspotConfig) -> Self {
        let security = if config.password.is_empty() {
            SecurityMode::Open
        } else {
            SecurityMode::Wpa2Psk
        };

        let band = if config.channel > 14 {
            FrequencyBand::Band5GHz
        } else {
            FrequencyBand::Band2_4GHz
        };

        HotspotConfigExt {
            ssid: config.ssid.clone(),
            password: config.password.clone(),
            channel: config.channel,
            max_clients: config.max_clients,
            security,
            band,
            hidden_ssid: false,
            beacon_interval_ms: 100,
            dhcp_pool_start: ip_to_u32(192, 168, 43, 2),
            dhcp_pool_end: ip_to_u32(192, 168, 43, 254),
            gateway_ip: ip_to_u32(192, 168, 43, 1),
            subnet_mask: ip_to_u32(255, 255, 255, 0),
        }
    }
}

/// Connected client information
#[derive(Clone)]
struct ClientInfo {
    mac: [u8; 6],
    ip: u32,
    connected_since: u64,
    bytes_tx: u64,
    bytes_rx: u64,
    signal_strength: i8,
    hostname: [u8; 32],
    hostname_len: u8,
}

impl ClientInfo {
    fn new(mac: [u8; 6], ip: u32) -> Self {
        ClientInfo {
            mac,
            ip,
            connected_since: 0,
            bytes_tx: 0,
            bytes_rx: 0,
            signal_strength: -50,
            hostname: [0u8; 32],
            hostname_len: 0,
        }
    }
}

/// DHCP address pool manager
struct DhcpPool {
    start: u32,
    end: u32,
    /// Bitmap tracking allocated addresses (up to 256 addresses)
    allocated: [u8; 32], // 256 bits
    lease_count: u16,
}

impl DhcpPool {
    fn new(start: u32, end: u32) -> Self {
        DhcpPool {
            start,
            end,
            allocated: [0u8; 32],
            lease_count: 0,
        }
    }

    /// Allocate the next available IP from the pool
    fn allocate(&mut self) -> Option<u32> {
        let range = (self.end - self.start + 1).min(256) as usize;
        for i in 0..range {
            let byte_idx = i / 8;
            let bit_idx = i % 8;
            if byte_idx < 32 && (self.allocated[byte_idx] & (1 << bit_idx)) == 0 {
                self.allocated[byte_idx] |= 1 << bit_idx;
                self.lease_count = self.lease_count.saturating_add(1);
                return Some(self.start + i as u32);
            }
        }
        None
    }

    /// Release an IP back to the pool
    fn release(&mut self, ip: u32) {
        if ip >= self.start && ip <= self.end {
            let offset = (ip - self.start) as usize;
            let byte_idx = offset / 8;
            let bit_idx = offset % 8;
            if byte_idx < 32 {
                self.allocated[byte_idx] &= !(1 << bit_idx);
                if self.lease_count > 0 {
                    self.lease_count = self.lease_count.saturating_sub(1);
                }
            }
        }
    }

    /// Get number of available addresses
    fn available_count(&self) -> u16 {
        let total = (self.end - self.start + 1).min(256) as u16;
        total - self.lease_count
    }

    fn reset(&mut self) {
        self.allocated = [0u8; 32];
        self.lease_count = 0;
    }
}

/// Bandwidth monitor
struct BandwidthMonitor {
    total_tx: u64,
    total_rx: u64,
    /// Bytes in current measurement window
    window_tx: u64,
    window_rx: u64,
    /// Computed rates (bytes per second)
    rate_tx_bps: u64,
    rate_rx_bps: u64,
    /// Measurement window counter (ticks)
    window_ticks: u32,
    /// Ticks per measurement window
    window_size: u32,
}

impl BandwidthMonitor {
    fn new() -> Self {
        BandwidthMonitor {
            total_tx: 0,
            total_rx: 0,
            window_tx: 0,
            window_rx: 0,
            rate_tx_bps: 0,
            rate_rx_bps: 0,
            window_ticks: 0,
            window_size: 1000, // 1 second at 1ms ticks
        }
    }

    fn record_tx(&mut self, bytes: u64) {
        self.total_tx += bytes;
        self.window_tx += bytes;
    }

    fn record_rx(&mut self, bytes: u64) {
        self.total_rx += bytes;
        self.window_rx += bytes;
    }

    fn tick(&mut self) {
        self.window_ticks = self.window_ticks.saturating_add(1);
        if self.window_ticks >= self.window_size {
            self.rate_tx_bps = self.window_tx;
            self.rate_rx_bps = self.window_rx;
            self.window_tx = 0;
            self.window_rx = 0;
            self.window_ticks = 0;
        }
    }

    fn reset(&mut self) {
        self.total_tx = 0;
        self.total_rx = 0;
        self.window_tx = 0;
        self.window_rx = 0;
        self.rate_tx_bps = 0;
        self.rate_rx_bps = 0;
        self.window_ticks = 0;
    }
}

/// Hotspot manager
pub struct HotspotManager {
    config: Option<HotspotConfigExt>,
    active: bool,
    connected_clients: Vec<[u8; 6]>,
    /// Detailed client info
    clients: Vec<ClientInfo>,
    /// DHCP pool
    dhcp_pool: DhcpPool,
    /// Bandwidth monitor
    bandwidth: BandwidthMonitor,
    /// Uptime counter (ticks)
    uptime_ticks: u64,
    /// Total clients that have connected over lifetime
    total_clients_served: u32,
    /// Channel auto-select enabled
    auto_channel: bool,
}

static HOTSPOT: Mutex<Option<HotspotManager>> = Mutex::new(None);

impl HotspotManager {
    pub fn new() -> Self {
        HotspotManager {
            config: None,
            active: false,
            connected_clients: Vec::new(),
            clients: Vec::new(),
            dhcp_pool: DhcpPool::new(ip_to_u32(192, 168, 43, 2), ip_to_u32(192, 168, 43, 254)),
            bandwidth: BandwidthMonitor::new(),
            uptime_ticks: 0,
            total_clients_served: 0,
            auto_channel: true,
        }
    }

    pub fn start(&mut self, config: HotspotConfig) -> Result<(), ()> {
        if self.active {
            serial_println!("    [hotspot] already active, stop first");
            return Err(());
        }

        // Validate configuration
        if config.ssid.is_empty() {
            serial_println!("    [hotspot] error: SSID cannot be empty");
            return Err(());
        }
        if config.ssid.len() > 32 {
            serial_println!("    [hotspot] error: SSID too long (max 32 chars)");
            return Err(());
        }
        if !config.password.is_empty() && config.password.len() < 8 {
            serial_println!("    [hotspot] error: password must be at least 8 characters");
            return Err(());
        }

        let channel = if self.auto_channel && config.channel == 0 {
            self.select_best_channel()
        } else {
            validate_channel(config.channel)
        };

        let max_clients = if config.max_clients == 0 {
            10
        } else {
            config.max_clients.min(32)
        };

        let mut ext_config = HotspotConfigExt::from_basic(&config);
        ext_config.channel = channel;
        ext_config.max_clients = max_clients;

        serial_println!(
            "    [hotspot] starting AP: SSID='{}', ch={}, max_clients={}, security={:?}",
            ext_config.ssid,
            ext_config.channel,
            ext_config.max_clients,
            match ext_config.security {
                SecurityMode::Open => "open",
                SecurityMode::Wpa2Psk => "WPA2-PSK",
                SecurityMode::Wpa3Sae => "WPA3-SAE",
            }
        );

        // Reset state for fresh start
        self.dhcp_pool.reset();
        self.bandwidth.reset();
        self.connected_clients.clear();
        self.clients.clear();
        self.uptime_ticks = 0;

        self.config = Some(ext_config);
        self.active = true;

        serial_println!("    [hotspot] AP started successfully");
        Ok(())
    }

    pub fn stop(&mut self) {
        if !self.active {
            return;
        }

        // Disconnect all clients
        let client_count = self.clients.len();
        for client in &self.clients {
            self.dhcp_pool.release(client.ip);
        }
        self.connected_clients.clear();
        self.clients.clear();

        self.active = false;
        serial_println!(
            "    [hotspot] AP stopped ({} clients disconnected)",
            client_count
        );
    }

    pub fn client_count(&self) -> usize {
        self.connected_clients.len()
    }

    /// Handle a client association
    fn client_connect(&mut self, mac: [u8; 6]) -> Result<u32, ()> {
        if !self.active {
            return Err(());
        }

        // Check max clients
        if let Some(ref config) = self.config {
            if self.clients.len() >= config.max_clients as usize {
                serial_println!("    [hotspot] client rejected: max clients reached");
                return Err(());
            }
        }

        // Check if already connected
        for client in &self.clients {
            if client.mac == mac {
                return Ok(client.ip);
            }
        }

        // Allocate IP from DHCP pool
        let ip = self.dhcp_pool.allocate().ok_or(())?;

        let client = ClientInfo::new(mac, ip);
        self.connected_clients.push(mac);
        self.clients.push(client);
        self.total_clients_served = self.total_clients_served.saturating_add(1);

        serial_println!(
            "    [hotspot] client connected: MAC={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} IP={}",
            mac[0],
            mac[1],
            mac[2],
            mac[3],
            mac[4],
            mac[5],
            ip_to_string(ip)
        );

        Ok(ip)
    }

    /// Handle a client disassociation
    fn client_disconnect(&mut self, mac: &[u8; 6]) {
        if let Some(pos) = self.clients.iter().position(|c| &c.mac == mac) {
            let client = &self.clients[pos];
            self.dhcp_pool.release(client.ip);
            serial_println!(
                "    [hotspot] client disconnected: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                mac[0],
                mac[1],
                mac[2],
                mac[3],
                mac[4],
                mac[5]
            );
            self.clients.remove(pos);
        }
        if let Some(pos) = self.connected_clients.iter().position(|m| m == mac) {
            self.connected_clients.remove(pos);
        }
    }

    /// Select the best WiFi channel based on interference avoidance
    fn select_best_channel(&self) -> u8 {
        // Simple channel selection: prefer non-overlapping channels 1, 6, 11
        // In a real implementation, we'd scan for nearby APs
        // Default to channel 6 as a good middle ground
        6
    }

    /// Record bandwidth usage for a client
    fn record_traffic(&mut self, mac: &[u8; 6], tx_bytes: u64, rx_bytes: u64) {
        for client in &mut self.clients {
            if &client.mac == mac {
                client.bytes_tx += tx_bytes;
                client.bytes_rx += rx_bytes;
                break;
            }
        }
        self.bandwidth.record_tx(tx_bytes);
        self.bandwidth.record_rx(rx_bytes);
    }

    /// Periodic tick (e.g., called every 1ms)
    fn tick(&mut self) {
        if self.active {
            self.uptime_ticks = self.uptime_ticks.saturating_add(1);
            self.bandwidth.tick();
        }
    }

    /// Get bandwidth rates
    fn get_rates(&self) -> (u64, u64) {
        (self.bandwidth.rate_tx_bps, self.bandwidth.rate_rx_bps)
    }

    /// Get hotspot uptime in seconds
    fn uptime_seconds(&self) -> u64 {
        self.uptime_ticks / 1000
    }

    /// Whether the hotspot is active
    fn is_active(&self) -> bool {
        self.active
    }
}

/// Convert IP to u32 (network byte order)
fn ip_to_u32(a: u8, b: u8, c: u8, d: u8) -> u32 {
    ((a as u32) << 24) | ((b as u32) << 16) | ((c as u32) << 8) | (d as u32)
}

/// Format IP as string (returns static string approximation)
fn ip_to_string(ip: u32) -> alloc::string::String {
    let a = (ip >> 24) & 0xFF;
    let b = (ip >> 16) & 0xFF;
    let c = (ip >> 8) & 0xFF;
    let d = ip & 0xFF;
    alloc::format!("{}.{}.{}.{}", a, b, c, d)
}

/// Validate and return a valid WiFi channel
fn validate_channel(ch: u8) -> u8 {
    match ch {
        1..=14 => ch,                      // 2.4 GHz
        36 | 40 | 44 | 48 => ch,           // 5 GHz UNII-1
        52 | 56 | 60 | 64 => ch,           // 5 GHz UNII-2
        149 | 153 | 157 | 161 | 165 => ch, // 5 GHz UNII-3
        _ => 6,                            // default
    }
}

/// Start the hotspot (public API)
pub fn start(config: HotspotConfig) -> Result<(), ()> {
    let mut guard = HOTSPOT.lock();
    match guard.as_mut() {
        Some(mgr) => mgr.start(config),
        None => Err(()),
    }
}

/// Stop the hotspot (public API)
pub fn stop() {
    let mut guard = HOTSPOT.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.stop();
    }
}

/// Get connected client count (public API)
pub fn client_count() -> usize {
    let guard = HOTSPOT.lock();
    match guard.as_ref() {
        Some(mgr) => mgr.client_count(),
        None => 0,
    }
}

/// Check if hotspot is active (public API)
pub fn is_active() -> bool {
    let guard = HOTSPOT.lock();
    match guard.as_ref() {
        Some(mgr) => mgr.is_active(),
        None => false,
    }
}

/// Initialize the hotspot subsystem
pub fn init() {
    let mut guard = HOTSPOT.lock();
    *guard = Some(HotspotManager::new());
    serial_println!("    [hotspot] hotspot manager initialized (DHCP pool: 192.168.43.2-254)");
}
