/// Network connection manager for Genesis OS
///
/// Manages WiFi scanning/connecting, Ethernet configuration, Bluetooth
/// pairing, VPN tunnels, and DNS settings. All addresses stored as u32
/// (IPv4) or hashes. Signal strength and transfer rates use Q16
/// fixed-point. Connections tracked with state machines.
///
/// Inspired by: NetworkManager, connman, Windows Network Settings. All code is original.

use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 helpers
// ---------------------------------------------------------------------------

/// 1.0 in Q16
const Q16_ONE: i32 = 65536;

/// Q16 multiplication: (a * b) >> 16
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Q16 division: (a << 16) / b
fn q16_div(a: i32, b: i32) -> Option<i32> {
    if b == 0 {
        return None;
    }
    Some((((a as i64) << 16) / (b as i64)) as i32)
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum network interfaces
const MAX_INTERFACES: usize = 32;
/// Maximum WiFi scan results
const MAX_WIFI_RESULTS: usize = 128;
/// Maximum Bluetooth devices
const MAX_BT_DEVICES: usize = 64;
/// Maximum VPN profiles
const MAX_VPN_PROFILES: usize = 32;
/// Maximum DNS servers
const MAX_DNS_SERVERS: usize = 8;
/// Maximum saved WiFi networks
const MAX_SAVED_NETWORKS: usize = 256;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Interface type
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InterfaceType {
    Ethernet,
    Wifi,
    Bluetooth,
    Loopback,
    Virtual,
    Unknown,
}

/// Connection state
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnState {
    Disconnected,
    Connecting,
    Connected,
    Authenticating,
    Failed,
    Limited,
}

/// WiFi security type
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WifiSecurity {
    Open,
    Wep,
    WpaPsk,
    Wpa2Psk,
    Wpa3Sae,
    Enterprise,
}

/// WiFi band
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WifiBand {
    Band2g,
    Band5g,
    Band6g,
    Unknown,
}

/// Bluetooth device class
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BtClass {
    Audio,
    Keyboard,
    Mouse,
    Phone,
    Computer,
    Headset,
    Gamepad,
    Other,
}

/// Bluetooth pairing state
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BtPairState {
    Discovered,
    Pairing,
    Paired,
    Connected,
    Failed,
}

/// VPN protocol
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VpnProtocol {
    WireGuard,
    OpenVpn,
    IpSec,
    L2tp,
}

/// VPN connection state
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VpnState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Failed,
}

/// IP configuration mode
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IpMode {
    Dhcp,
    Static,
    LinkLocal,
}

/// Network manager result
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NetResult {
    Success,
    NotFound,
    AlreadyConnected,
    AuthFailed,
    Timeout,
    LimitReached,
    InvalidConfig,
    InterfaceDown,
    IoError,
}

/// IPv4 configuration
#[derive(Debug, Clone, Copy)]
pub struct Ipv4Config {
    pub mode: IpMode,
    pub address: u32,
    pub subnet_mask: u32,
    pub gateway: u32,
    pub dns_primary: u32,
    pub dns_secondary: u32,
}

/// A network interface
#[derive(Debug, Clone)]
pub struct NetInterface {
    pub id: u64,
    pub name_hash: u64,
    pub iface_type: InterfaceType,
    pub mac_hash: u64,
    pub state: ConnState,
    pub ipv4: Ipv4Config,
    pub mtu: u32,
    pub speed_mbps: u32,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub tx_packets: u64,
    pub rx_packets: u64,
    pub up: bool,
    pub is_default: bool,
}

/// WiFi scan result
#[derive(Debug, Clone)]
pub struct WifiNetwork {
    pub ssid_hash: u64,
    pub bssid_hash: u64,
    pub signal_q16: i32,
    pub frequency_mhz: u32,
    pub band: WifiBand,
    pub security: WifiSecurity,
    pub channel: u8,
    pub saved: bool,
    pub hidden: bool,
}

/// Saved WiFi credential
#[derive(Debug, Clone, Copy)]
pub struct SavedNetwork {
    pub ssid_hash: u64,
    pub passphrase_hash: u64,
    pub security: WifiSecurity,
    pub auto_connect: bool,
    pub last_connected: u64,
    pub connect_count: u32,
}

/// A Bluetooth device
#[derive(Debug, Clone)]
pub struct BtDevice {
    pub id: u64,
    pub name_hash: u64,
    pub addr_hash: u64,
    pub class: BtClass,
    pub state: BtPairState,
    pub signal_q16: i32,
    pub battery_percent: u8,
    pub last_seen: u64,
    pub trusted: bool,
}

/// A VPN profile
#[derive(Debug, Clone)]
pub struct VpnProfile {
    pub id: u64,
    pub name_hash: u64,
    pub protocol: VpnProtocol,
    pub server_ip: u32,
    pub server_port: u16,
    pub key_hash: u64,
    pub state: VpnState,
    pub auto_connect: bool,
    pub connected_since: u64,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
}

/// DNS server entry
#[derive(Debug, Clone, Copy)]
pub struct DnsServer {
    pub address: u32,
    pub name_hash: u64,
    pub priority: u8,
    pub is_custom: bool,
}

/// Network manager state
struct NetMgrState {
    interfaces: Vec<NetInterface>,
    wifi_scan: Vec<WifiNetwork>,
    saved_networks: Vec<SavedNetwork>,
    bt_devices: Vec<BtDevice>,
    vpn_profiles: Vec<VpnProfile>,
    dns_servers: Vec<DnsServer>,
    next_iface_id: u64,
    next_bt_id: u64,
    next_vpn_id: u64,
    timestamp: u64,
    wifi_scanning: bool,
    bt_scanning: bool,
    hostname_hash: u64,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static NET_MGR: Mutex<Option<NetMgrState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn next_timestamp(state: &mut NetMgrState) -> u64 {
    state.timestamp += 1;
    state.timestamp
}

fn default_ipv4() -> Ipv4Config {
    Ipv4Config {
        mode: IpMode::Dhcp,
        address: 0,
        subnet_mask: 0,
        gateway: 0,
        dns_primary: 0,
        dns_secondary: 0,
    }
}

fn default_state() -> NetMgrState {
    // Create loopback interface
    let lo = NetInterface {
        id: 1,
        name_hash: 0x6C6F_6F70_6261_636B, // "loopback" hash
        iface_type: InterfaceType::Loopback,
        mac_hash: 0,
        state: ConnState::Connected,
        ipv4: Ipv4Config {
            mode: IpMode::Static,
            address: 0x7F00_0001, // 127.0.0.1
            subnet_mask: 0xFF00_0000,
            gateway: 0,
            dns_primary: 0,
            dns_secondary: 0,
        },
        mtu: 65535,
        speed_mbps: 0,
        tx_bytes: 0,
        rx_bytes: 0,
        tx_packets: 0,
        rx_packets: 0,
        up: true,
        is_default: false,
    };

    NetMgrState {
        interfaces: vec![lo],
        wifi_scan: Vec::new(),
        saved_networks: Vec::new(),
        bt_devices: Vec::new(),
        vpn_profiles: Vec::new(),
        dns_servers: Vec::new(),
        next_iface_id: 2,
        next_bt_id: 1,
        next_vpn_id: 1,
        timestamp: 0,
        wifi_scanning: false,
        bt_scanning: false,
        hostname_hash: 0x67656E65_73697321, // "genesis!" hash
    }
}

// ---------------------------------------------------------------------------
// Public API -- Interfaces
// ---------------------------------------------------------------------------

/// Register a network interface
pub fn register_interface(
    name_hash: u64,
    iface_type: InterfaceType,
    mac_hash: u64,
    mtu: u32,
) -> Result<u64, NetResult> {
    let mut guard = NET_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return Err(NetResult::IoError),
    };
    if state.interfaces.len() >= MAX_INTERFACES {
        return Err(NetResult::LimitReached);
    }
    let id = state.next_iface_id;
    state.next_iface_id += 1;
    state.interfaces.push(NetInterface {
        id,
        name_hash,
        iface_type,
        mac_hash,
        state: ConnState::Disconnected,
        ipv4: default_ipv4(),
        mtu,
        speed_mbps: 0,
        tx_bytes: 0,
        rx_bytes: 0,
        tx_packets: 0,
        rx_packets: 0,
        up: false,
        is_default: false,
    });
    Ok(id)
}

/// Bring an interface up
pub fn interface_up(iface_id: u64) -> NetResult {
    let mut guard = NET_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NetResult::IoError,
    };
    let iface = match state.interfaces.iter_mut().find(|i| i.id == iface_id) {
        Some(i) => i,
        None => return NetResult::NotFound,
    };
    iface.up = true;
    if iface.state == ConnState::Disconnected {
        iface.state = ConnState::Connecting;
    }
    NetResult::Success
}

/// Bring an interface down
pub fn interface_down(iface_id: u64) -> NetResult {
    let mut guard = NET_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NetResult::IoError,
    };
    let iface = match state.interfaces.iter_mut().find(|i| i.id == iface_id) {
        Some(i) => i,
        None => return NetResult::NotFound,
    };
    iface.up = false;
    iface.state = ConnState::Disconnected;
    NetResult::Success
}

/// Set static IP configuration on an interface
pub fn set_static_ip(
    iface_id: u64,
    address: u32,
    subnet: u32,
    gateway: u32,
) -> NetResult {
    let mut guard = NET_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NetResult::IoError,
    };
    let iface = match state.interfaces.iter_mut().find(|i| i.id == iface_id) {
        Some(i) => i,
        None => return NetResult::NotFound,
    };
    iface.ipv4.mode = IpMode::Static;
    iface.ipv4.address = address;
    iface.ipv4.subnet_mask = subnet;
    iface.ipv4.gateway = gateway;
    if iface.up {
        iface.state = ConnState::Connected;
    }
    NetResult::Success
}

/// Set DHCP on an interface
pub fn set_dhcp(iface_id: u64) -> NetResult {
    let mut guard = NET_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NetResult::IoError,
    };
    let iface = match state.interfaces.iter_mut().find(|i| i.id == iface_id) {
        Some(i) => i,
        None => return NetResult::NotFound,
    };
    iface.ipv4.mode = IpMode::Dhcp;
    NetResult::Success
}

/// Set default interface for routing
pub fn set_default_interface(iface_id: u64) -> NetResult {
    let mut guard = NET_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NetResult::IoError,
    };
    if !state.interfaces.iter().any(|i| i.id == iface_id) {
        return NetResult::NotFound;
    }
    for iface in state.interfaces.iter_mut() {
        iface.is_default = iface.id == iface_id;
    }
    NetResult::Success
}

/// List all interfaces
pub fn list_interfaces() -> Vec<NetInterface> {
    let guard = NET_MGR.lock();
    match guard.as_ref() {
        Some(state) => state.interfaces.clone(),
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Public API -- WiFi
// ---------------------------------------------------------------------------

/// Start a WiFi scan
pub fn wifi_scan_start() -> NetResult {
    let mut guard = NET_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NetResult::IoError,
    };
    let has_wifi = state.interfaces.iter().any(|i| i.iface_type == InterfaceType::Wifi && i.up);
    if !has_wifi {
        return NetResult::InterfaceDown;
    }
    state.wifi_scanning = true;
    state.wifi_scan.clear();
    NetResult::Success
}

/// Report a WiFi network found during scan
pub fn wifi_scan_result(
    ssid_hash: u64,
    bssid_hash: u64,
    signal_q16: i32,
    frequency_mhz: u32,
    security: WifiSecurity,
    channel: u8,
) -> NetResult {
    let mut guard = NET_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NetResult::IoError,
    };
    if state.wifi_scan.len() >= MAX_WIFI_RESULTS {
        return NetResult::LimitReached;
    }
    let band = if frequency_mhz < 3000 {
        WifiBand::Band2g
    } else if frequency_mhz < 5900 {
        WifiBand::Band5g
    } else {
        WifiBand::Band6g
    };
    let saved = state.saved_networks.iter().any(|s| s.ssid_hash == ssid_hash);
    state.wifi_scan.push(WifiNetwork {
        ssid_hash,
        bssid_hash,
        signal_q16,
        frequency_mhz,
        band,
        security,
        channel,
        saved,
        hidden: false,
    });
    NetResult::Success
}

/// Complete a WiFi scan
pub fn wifi_scan_complete() -> usize {
    let mut guard = NET_MGR.lock();
    match guard.as_mut() {
        Some(state) => {
            state.wifi_scanning = false;
            // Sort by signal strength (strongest first)
            state.wifi_scan.sort_by(|a, b| b.signal_q16.cmp(&a.signal_q16));
            state.wifi_scan.len()
        }
        None => 0,
    }
}

/// Get WiFi scan results
pub fn wifi_scan_results() -> Vec<WifiNetwork> {
    let guard = NET_MGR.lock();
    match guard.as_ref() {
        Some(state) => state.wifi_scan.clone(),
        None => Vec::new(),
    }
}

/// Connect to a WiFi network
pub fn wifi_connect(ssid_hash: u64, passphrase_hash: u64) -> NetResult {
    let mut guard = NET_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NetResult::IoError,
    };
    let wifi_iface = match state.interfaces.iter_mut().find(|i| i.iface_type == InterfaceType::Wifi) {
        Some(i) => i,
        None => return NetResult::NotFound,
    };
    if !wifi_iface.up {
        return NetResult::InterfaceDown;
    }
    let network = state.wifi_scan.iter().find(|n| n.ssid_hash == ssid_hash);
    let security = match network {
        Some(n) => n.security,
        None => WifiSecurity::Wpa2Psk,
    };

    wifi_iface.state = ConnState::Authenticating;

    // Simulate auth success (in real kernel, driver callback would update)
    wifi_iface.state = ConnState::Connected;

    // Save network
    let now = next_timestamp(state);
    if let Some(saved) = state.saved_networks.iter_mut().find(|s| s.ssid_hash == ssid_hash) {
        saved.passphrase_hash = passphrase_hash;
        saved.last_connected = now;
        saved.connect_count += 1;
    } else if state.saved_networks.len() < MAX_SAVED_NETWORKS {
        state.saved_networks.push(SavedNetwork {
            ssid_hash,
            passphrase_hash,
            security,
            auto_connect: true,
            last_connected: now,
            connect_count: 1,
        });
    }
    NetResult::Success
}

/// Disconnect WiFi
pub fn wifi_disconnect() -> NetResult {
    let mut guard = NET_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NetResult::IoError,
    };
    let wifi_iface = match state.interfaces.iter_mut().find(|i| i.iface_type == InterfaceType::Wifi) {
        Some(i) => i,
        None => return NetResult::NotFound,
    };
    wifi_iface.state = ConnState::Disconnected;
    NetResult::Success
}

/// Forget a saved WiFi network
pub fn wifi_forget(ssid_hash: u64) -> NetResult {
    let mut guard = NET_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NetResult::IoError,
    };
    let before = state.saved_networks.len();
    state.saved_networks.retain(|s| s.ssid_hash != ssid_hash);
    if state.saved_networks.len() < before {
        NetResult::Success
    } else {
        NetResult::NotFound
    }
}

// ---------------------------------------------------------------------------
// Public API -- Bluetooth
// ---------------------------------------------------------------------------

/// Start Bluetooth scanning
pub fn bt_scan_start() -> NetResult {
    let mut guard = NET_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NetResult::IoError,
    };
    state.bt_scanning = true;
    NetResult::Success
}

/// Report a discovered Bluetooth device
pub fn bt_device_found(
    name_hash: u64,
    addr_hash: u64,
    class: BtClass,
    signal_q16: i32,
) -> Result<u64, NetResult> {
    let mut guard = NET_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return Err(NetResult::IoError),
    };
    if state.bt_devices.len() >= MAX_BT_DEVICES {
        return Err(NetResult::LimitReached);
    }
    // Check if already discovered
    if let Some(dev) = state.bt_devices.iter_mut().find(|d| d.addr_hash == addr_hash) {
        let now = next_timestamp(state);
        dev.signal_q16 = signal_q16;
        dev.last_seen = now;
        return Ok(dev.id);
    }
    let id = state.next_bt_id;
    state.next_bt_id += 1;
    let now = next_timestamp(state);
    state.bt_devices.push(BtDevice {
        id,
        name_hash,
        addr_hash,
        class,
        state: BtPairState::Discovered,
        signal_q16,
        battery_percent: 0,
        last_seen: now,
        trusted: false,
    });
    Ok(id)
}

/// Stop Bluetooth scanning
pub fn bt_scan_stop() {
    let mut guard = NET_MGR.lock();
    if let Some(state) = guard.as_mut() {
        state.bt_scanning = false;
    }
}

/// Pair with a Bluetooth device
pub fn bt_pair(device_id: u64) -> NetResult {
    let mut guard = NET_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NetResult::IoError,
    };
    let dev = match state.bt_devices.iter_mut().find(|d| d.id == device_id) {
        Some(d) => d,
        None => return NetResult::NotFound,
    };
    dev.state = BtPairState::Pairing;
    // Simulate successful pairing
    dev.state = BtPairState::Paired;
    dev.trusted = true;
    NetResult::Success
}

/// Connect to a paired Bluetooth device
pub fn bt_connect(device_id: u64) -> NetResult {
    let mut guard = NET_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NetResult::IoError,
    };
    let dev = match state.bt_devices.iter_mut().find(|d| d.id == device_id) {
        Some(d) => d,
        None => return NetResult::NotFound,
    };
    if dev.state != BtPairState::Paired {
        return NetResult::AuthFailed;
    }
    dev.state = BtPairState::Connected;
    NetResult::Success
}

/// Disconnect a Bluetooth device
pub fn bt_disconnect(device_id: u64) -> NetResult {
    let mut guard = NET_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NetResult::IoError,
    };
    let dev = match state.bt_devices.iter_mut().find(|d| d.id == device_id) {
        Some(d) => d,
        None => return NetResult::NotFound,
    };
    dev.state = BtPairState::Paired;
    NetResult::Success
}

/// List Bluetooth devices
pub fn list_bt_devices() -> Vec<BtDevice> {
    let guard = NET_MGR.lock();
    match guard.as_ref() {
        Some(state) => state.bt_devices.clone(),
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Public API -- VPN
// ---------------------------------------------------------------------------

/// Create a VPN profile
pub fn vpn_create(
    name_hash: u64,
    protocol: VpnProtocol,
    server_ip: u32,
    server_port: u16,
    key_hash: u64,
) -> Result<u64, NetResult> {
    let mut guard = NET_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return Err(NetResult::IoError),
    };
    if state.vpn_profiles.len() >= MAX_VPN_PROFILES {
        return Err(NetResult::LimitReached);
    }
    let id = state.next_vpn_id;
    state.next_vpn_id += 1;
    state.vpn_profiles.push(VpnProfile {
        id,
        name_hash,
        protocol,
        server_ip,
        server_port,
        key_hash,
        state: VpnState::Disconnected,
        auto_connect: false,
        connected_since: 0,
        tx_bytes: 0,
        rx_bytes: 0,
    });
    Ok(id)
}

/// Connect a VPN
pub fn vpn_connect(vpn_id: u64) -> NetResult {
    let mut guard = NET_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NetResult::IoError,
    };
    let now = next_timestamp(state);
    let vpn = match state.vpn_profiles.iter_mut().find(|v| v.id == vpn_id) {
        Some(v) => v,
        None => return NetResult::NotFound,
    };
    vpn.state = VpnState::Connecting;
    vpn.state = VpnState::Connected;
    vpn.connected_since = now;
    NetResult::Success
}

/// Disconnect a VPN
pub fn vpn_disconnect(vpn_id: u64) -> NetResult {
    let mut guard = NET_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NetResult::IoError,
    };
    let vpn = match state.vpn_profiles.iter_mut().find(|v| v.id == vpn_id) {
        Some(v) => v,
        None => return NetResult::NotFound,
    };
    vpn.state = VpnState::Disconnected;
    vpn.connected_since = 0;
    NetResult::Success
}

/// Delete a VPN profile (must be disconnected)
pub fn vpn_delete(vpn_id: u64) -> NetResult {
    let mut guard = NET_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NetResult::IoError,
    };
    if let Some(vpn) = state.vpn_profiles.iter().find(|v| v.id == vpn_id) {
        if vpn.state == VpnState::Connected {
            return NetResult::AlreadyConnected;
        }
    } else {
        return NetResult::NotFound;
    }
    state.vpn_profiles.retain(|v| v.id != vpn_id);
    NetResult::Success
}

/// List VPN profiles
pub fn list_vpns() -> Vec<VpnProfile> {
    let guard = NET_MGR.lock();
    match guard.as_ref() {
        Some(state) => state.vpn_profiles.clone(),
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Public API -- DNS
// ---------------------------------------------------------------------------

/// Add a DNS server
pub fn dns_add(address: u32, name_hash: u64, priority: u8) -> NetResult {
    let mut guard = NET_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NetResult::IoError,
    };
    if state.dns_servers.len() >= MAX_DNS_SERVERS {
        return NetResult::LimitReached;
    }
    if state.dns_servers.iter().any(|d| d.address == address) {
        return NetResult::InvalidConfig;
    }
    state.dns_servers.push(DnsServer {
        address,
        name_hash,
        priority,
        is_custom: true,
    });
    state.dns_servers.sort_by(|a, b| a.priority.cmp(&b.priority));
    NetResult::Success
}

/// Remove a DNS server
pub fn dns_remove(address: u32) -> NetResult {
    let mut guard = NET_MGR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NetResult::IoError,
    };
    let before = state.dns_servers.len();
    state.dns_servers.retain(|d| d.address != address);
    if state.dns_servers.len() < before {
        NetResult::Success
    } else {
        NetResult::NotFound
    }
}

/// List DNS servers
pub fn list_dns() -> Vec<DnsServer> {
    let guard = NET_MGR.lock();
    match guard.as_ref() {
        Some(state) => state.dns_servers.clone(),
        None => Vec::new(),
    }
}

/// Set the system hostname
pub fn set_hostname(hash: u64) {
    let mut guard = NET_MGR.lock();
    if let Some(state) = guard.as_mut() {
        state.hostname_hash = hash;
    }
}

/// Get interface count
pub fn interface_count() -> usize {
    let guard = NET_MGR.lock();
    match guard.as_ref() {
        Some(state) => state.interfaces.len(),
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the network manager subsystem
pub fn init() {
    let mut guard = NET_MGR.lock();
    *guard = Some(default_state());
    serial_println!("    Network manager ready (loopback up)");
}
