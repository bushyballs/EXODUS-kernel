/// VPN management for Genesis
///
/// VPN profiles, connection management, split tunneling,
/// always-on VPN, and per-app VPN.
///
/// Inspired by: Android VpnManager, iOS NEVPNManager. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// VPN protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VpnProtocol {
    WireGuard,
    OpenVpn,
    IpSec,
    L2tp,
    Pptp,
    Custom,
}

/// VPN state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VpnState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Disconnecting,
    Error,
}

/// VPN profile
pub struct VpnProfile {
    pub id: u32,
    pub name: String,
    pub protocol: VpnProtocol,
    pub server: String,
    pub port: u16,
    pub username: String,
    pub dns_servers: Vec<u32>,
    pub split_tunnel: bool,
    pub allowed_apps: Vec<String>,
    pub disallowed_apps: Vec<String>,
    pub mtu: u16,
    pub always_on: bool,
    pub block_without_vpn: bool,
}

/// Active VPN connection
pub struct VpnConnection {
    pub profile_id: u32,
    pub state: VpnState,
    pub local_ip: u32,
    pub remote_ip: u32,
    pub connected_at: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub latency_ms: u32,
}

/// VPN manager
pub struct VpnManager {
    pub profiles: Vec<VpnProfile>,
    pub connection: Option<VpnConnection>,
    pub next_id: u32,
    pub lockdown_mode: bool,
}

impl VpnManager {
    const fn new() -> Self {
        VpnManager {
            profiles: Vec::new(),
            connection: None,
            next_id: 1,
            lockdown_mode: false,
        }
    }

    pub fn add_profile(
        &mut self,
        name: &str,
        protocol: VpnProtocol,
        server: &str,
        port: u16,
    ) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.profiles.push(VpnProfile {
            id,
            name: String::from(name),
            protocol,
            server: String::from(server),
            port,
            username: String::new(),
            dns_servers: Vec::new(),
            split_tunnel: false,
            allowed_apps: Vec::new(),
            disallowed_apps: Vec::new(),
            mtu: 1400,
            always_on: false,
            block_without_vpn: false,
        });
        id
    }

    pub fn remove_profile(&mut self, id: u32) {
        self.profiles.retain(|p| p.id != id);
    }

    pub fn connect(&mut self, profile_id: u32) -> bool {
        if self.connection.is_some() {
            return false;
        }
        if !self.profiles.iter().any(|p| p.id == profile_id) {
            return false;
        }

        self.connection = Some(VpnConnection {
            profile_id,
            state: VpnState::Connecting,
            local_ip: 0x0A000001, // 10.0.0.1
            remote_ip: 0,
            connected_at: crate::time::clock::unix_time(),
            rx_bytes: 0,
            tx_bytes: 0,
            latency_ms: 0,
        });
        true
    }

    pub fn disconnect(&mut self) {
        if let Some(ref mut conn) = self.connection {
            conn.state = VpnState::Disconnecting;
        }
        self.connection = None;
    }

    pub fn is_connected(&self) -> bool {
        self.connection
            .as_ref()
            .map(|c| c.state == VpnState::Connected)
            .unwrap_or(false)
    }

    pub fn set_always_on(&mut self, profile_id: u32, lockdown: bool) {
        if let Some(profile) = self.profiles.iter_mut().find(|p| p.id == profile_id) {
            profile.always_on = true;
            profile.block_without_vpn = lockdown;
        }
        self.lockdown_mode = lockdown;
    }

    pub fn should_allow_traffic(&self, _app_id: &str) -> bool {
        if !self.lockdown_mode {
            return true;
        }
        self.is_connected()
    }
}

static VPN: Mutex<VpnManager> = Mutex::new(VpnManager::new());

pub fn init() {
    crate::serial_println!("  [connectivity] VPN manager initialized");
}
