use crate::sync::Mutex;
use alloc::string::String;
/// WiFi Direct -- peer-to-peer wireless connections
///
/// Enables device-to-device communication without
/// an access point. Part of the AIOS connectivity layer.
/// Implements WiFi Direct (Wi-Fi P2P) peer discovery,
/// group formation, group owner negotiation, service
/// discovery, and session management.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// WiFi Direct device state
#[derive(Clone, Copy, PartialEq)]
pub enum DeviceState {
    Idle,
    Discovering,
    GroupForming,
    Connected,
    GroupOwner,
    GroupClient,
}

/// WiFi Direct group owner intent (0-15, higher = more likely to be GO)
#[derive(Clone, Copy)]
pub struct GoIntent {
    pub value: u8,
    pub tie_breaker: bool,
}

impl GoIntent {
    fn new(value: u8) -> Self {
        GoIntent {
            value: value.min(15),
            tie_breaker: false,
        }
    }
}

/// WiFi Direct peer
pub struct WifiDirectPeer {
    pub name: String,
    pub mac: [u8; 6],
    pub signal_strength: i8,
    /// Device capability bitmap
    device_capability: u8,
    /// Group capability bitmap
    group_capability: u8,
    /// Config methods supported (bitmap)
    config_methods: u16,
    /// WPS device type
    device_type: u16,
    /// Whether peer is a group owner
    is_go: bool,
    /// Group owner intent value
    go_intent: u8,
    /// Operating channel (if known)
    operating_channel: u8,
    /// Last seen timestamp (tick counter)
    last_seen: u64,
    /// Connection state with this peer
    connected: bool,
}

impl WifiDirectPeer {
    fn new(name: String, mac: [u8; 6], signal: i8) -> Self {
        WifiDirectPeer {
            name,
            mac,
            signal_strength: signal,
            device_capability: 0x01, // Service discovery capable
            group_capability: 0,
            config_methods: 0x0188, // PBC + Display + Keypad
            device_type: 0x000A,    // Phone
            is_go: false,
            go_intent: 7, // default mid-range intent
            operating_channel: 0,
            last_seen: 0,
            connected: false,
        }
    }
}

/// Service advertisement for service discovery
#[derive(Clone)]
struct ServiceAdvertisement {
    service_name: String,
    service_info: String,
    protocol: u8, // 0=all, 1=bonjour, 2=upnp, 3=wsd
}

/// Group session information
struct GroupSession {
    ssid: String,
    passphrase: [u8; 64],
    passphrase_len: u8,
    channel: u8,
    go_mac: [u8; 6],
    is_persistent: bool,
    frequency_mhz: u16,
}

impl GroupSession {
    fn new(channel: u8, go_mac: [u8; 6]) -> Self {
        // Generate a simple passphrase
        let passphrase = b"GenesisP2P-Default";
        let mut pp = [0u8; 64];
        let len = passphrase.len().min(64);
        pp[..len].copy_from_slice(&passphrase[..len]);

        GroupSession {
            ssid: String::from("DIRECT-Genesis"),
            passphrase: pp,
            passphrase_len: len as u8,
            channel,
            go_mac,
            is_persistent: false,
            frequency_mhz: channel_to_freq(channel),
        }
    }
}

/// WiFi Direct manager
pub struct WifiDirectManager {
    peers: Vec<WifiDirectPeer>,
    group_owner: bool,
    /// Current device state
    state: DeviceState,
    /// Our device name
    device_name: String,
    /// Our MAC address
    our_mac: [u8; 6],
    /// GO intent for negotiation
    our_go_intent: GoIntent,
    /// Listen channel (social channel: 1, 6, or 11)
    listen_channel: u8,
    /// Operating channel for group
    operating_channel: u8,
    /// Active group session
    session: Option<GroupSession>,
    /// Service advertisements
    services: Vec<ServiceAdvertisement>,
    /// Discovery tick counter
    discovery_ticks: u64,
    /// Discovery timeout (ticks)
    discovery_timeout: u64,
    /// Current tick counter
    tick_counter: u64,
    /// Peer expiry timeout (ticks since last seen)
    peer_expiry: u64,
    /// Total discoveries performed
    total_discoveries: u32,
    /// Total connections
    total_connections: u32,
}

static MANAGER: Mutex<Option<WifiDirectManager>> = Mutex::new(None);

impl WifiDirectManager {
    pub fn new() -> Self {
        WifiDirectManager {
            peers: Vec::new(),
            group_owner: false,
            state: DeviceState::Idle,
            device_name: String::from("Genesis-Device"),
            our_mac: [0x02, 0x00, 0xDE, 0xAD, 0xBE, 0xEF],
            our_go_intent: GoIntent::new(7),
            listen_channel: 6,
            operating_channel: 6,
            session: None,
            services: Vec::new(),
            discovery_ticks: 0,
            discovery_timeout: 30_000, // 30 seconds
            tick_counter: 0,
            peer_expiry: 60_000, // 60 seconds
            total_discoveries: 0,
            total_connections: 0,
        }
    }

    /// Start peer discovery (scan for Wi-Fi Direct devices)
    pub fn discover_peers(&mut self) -> Vec<&WifiDirectPeer> {
        if self.state != DeviceState::Idle && self.state != DeviceState::Discovering {
            serial_println!(
                "    [wifi-direct] cannot discover: busy (state={:?})",
                state_name(&self.state)
            );
            return Vec::new();
        }

        self.state = DeviceState::Discovering;
        self.discovery_ticks = self.tick_counter;
        self.total_discoveries = self.total_discoveries.saturating_add(1);

        serial_println!(
            "    [wifi-direct] discovery started on listen channel {}",
            self.listen_channel
        );

        // Expire old peers
        self.expire_stale_peers();

        // Return references to discovered peers
        self.peers.iter().collect()
    }

    /// Connect to a peer by MAC address
    pub fn connect(&mut self, peer_mac: &[u8; 6]) -> Result<(), ()> {
        if self.state == DeviceState::Connected
            || self.state == DeviceState::GroupOwner
            || self.state == DeviceState::GroupClient
        {
            serial_println!("    [wifi-direct] already in a group, disconnect first");
            return Err(());
        }

        // Find the peer
        let peer_idx = self.peers.iter().position(|p| &p.mac == peer_mac);
        if peer_idx.is_none() {
            serial_println!(
                "    [wifi-direct] peer not found: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                peer_mac[0],
                peer_mac[1],
                peer_mac[2],
                peer_mac[3],
                peer_mac[4],
                peer_mac[5]
            );
            return Err(());
        }
        let peer_idx = peer_idx.unwrap();

        // Group Owner Negotiation
        self.state = DeviceState::GroupForming;
        serial_println!(
            "    [wifi-direct] GO negotiation with '{}'",
            self.peers[peer_idx].name
        );

        let peer_intent = self.peers[peer_idx].go_intent;
        let our_intent = self.our_go_intent.value;

        // Determine GO
        let we_are_go = if our_intent > peer_intent {
            true
        } else if our_intent < peer_intent {
            false
        } else {
            // Tie: use tie breaker bit
            self.our_go_intent.tie_breaker
        };

        // Create group session
        let go_mac = if we_are_go { self.our_mac } else { *peer_mac };
        let channel = if we_are_go {
            self.operating_channel
        } else {
            self.peers[peer_idx].operating_channel.max(1)
        };
        let session = GroupSession::new(channel, go_mac);

        if we_are_go {
            self.group_owner = true;
            self.state = DeviceState::GroupOwner;
            serial_println!("    [wifi-direct] we are Group Owner on ch {}", channel);
        } else {
            self.group_owner = false;
            self.state = DeviceState::GroupClient;
            serial_println!(
                "    [wifi-direct] we are Group Client of '{}'",
                self.peers[peer_idx].name
            );
        }

        self.session = Some(session);
        self.peers[peer_idx].connected = true;
        self.total_connections = self.total_connections.saturating_add(1);

        serial_println!("    [wifi-direct] P2P group formed successfully");
        Ok(())
    }

    /// Disconnect from current group
    pub fn disconnect(&mut self) {
        match self.state {
            DeviceState::GroupOwner | DeviceState::GroupClient | DeviceState::Connected => {
                // Mark all peers as disconnected
                for peer in &mut self.peers {
                    peer.connected = false;
                }
                self.session = None;
                self.group_owner = false;
                self.state = DeviceState::Idle;
                serial_println!("    [wifi-direct] disconnected from P2P group");
            }
            _ => {
                serial_println!("    [wifi-direct] not in a group");
            }
        }
    }

    /// Add a peer (from discovery scan results)
    fn add_or_update_peer(&mut self, name: String, mac: [u8; 6], signal: i8) {
        // Update existing peer or add new one
        for peer in &mut self.peers {
            if peer.mac == mac {
                peer.signal_strength = signal;
                peer.last_seen = self.tick_counter;
                peer.name = name;
                return;
            }
        }
        let mut peer = WifiDirectPeer::new(name, mac, signal);
        peer.last_seen = self.tick_counter;
        self.peers.push(peer);
    }

    /// Remove peers not seen recently
    fn expire_stale_peers(&mut self) {
        let now = self.tick_counter;
        let expiry = self.peer_expiry;
        self.peers
            .retain(|p| !p.connected && (now.saturating_sub(p.last_seen) < expiry) || p.connected);
    }

    /// Register a service for advertisement
    fn advertise_service(&mut self, name: String, info: String, protocol: u8) {
        self.services.push(ServiceAdvertisement {
            service_name: name.clone(),
            service_info: info,
            protocol,
        });
        serial_println!("    [wifi-direct] advertising service: {}", name);
    }

    /// Set GO intent (0-15)
    fn set_go_intent(&mut self, intent: u8) {
        self.our_go_intent = GoIntent::new(intent);
        serial_println!(
            "    [wifi-direct] GO intent set to {}",
            self.our_go_intent.value
        );
    }

    /// Set listen channel (must be 1, 6, or 11)
    fn set_listen_channel(&mut self, channel: u8) {
        self.listen_channel = match channel {
            1 => 1,
            6 => 6,
            11 => 11,
            _ => 6, // default social channel
        };
    }

    /// Check if we are group owner
    fn is_group_owner(&self) -> bool {
        self.group_owner
    }

    /// Get current device state
    fn get_state(&self) -> DeviceState {
        self.state
    }

    /// Get number of discovered peers
    fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// Periodic tick
    fn tick(&mut self) {
        self.tick_counter = self.tick_counter.saturating_add(1);

        // Check discovery timeout
        if self.state == DeviceState::Discovering {
            if self.tick_counter.saturating_sub(self.discovery_ticks) > self.discovery_timeout {
                self.state = DeviceState::Idle;
                serial_println!("    [wifi-direct] discovery timed out");
            }
        }
    }
}

fn state_name(state: &DeviceState) -> &'static str {
    match state {
        DeviceState::Idle => "idle",
        DeviceState::Discovering => "discovering",
        DeviceState::GroupForming => "forming",
        DeviceState::Connected => "connected",
        DeviceState::GroupOwner => "GO",
        DeviceState::GroupClient => "client",
    }
}

/// Convert WiFi channel number to frequency in MHz
fn channel_to_freq(channel: u8) -> u16 {
    match channel {
        1..=13 => 2407 + (channel as u16) * 5,
        14 => 2484,
        36 => 5180,
        40 => 5200,
        44 => 5220,
        48 => 5240,
        52 => 5260,
        56 => 5280,
        60 => 5300,
        64 => 5320,
        149 => 5745,
        153 => 5765,
        157 => 5785,
        161 => 5805,
        165 => 5825,
        _ => 2437, // default to ch 6
    }
}

/// Discover peers (public API)
pub fn discover_peers() -> usize {
    let mut guard = MANAGER.lock();
    match guard.as_mut() {
        Some(mgr) => {
            let peers = mgr.discover_peers();
            peers.len()
        }
        None => 0,
    }
}

/// Connect to a peer (public API)
pub fn connect(peer_mac: &[u8; 6]) -> Result<(), ()> {
    let mut guard = MANAGER.lock();
    match guard.as_mut() {
        Some(mgr) => mgr.connect(peer_mac),
        None => Err(()),
    }
}

/// Disconnect (public API)
pub fn disconnect() {
    let mut guard = MANAGER.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.disconnect();
    }
}

/// Get peer count
pub fn peer_count() -> usize {
    let guard = MANAGER.lock();
    match guard.as_ref() {
        Some(mgr) => mgr.peer_count(),
        None => 0,
    }
}

/// Check if we are group owner
pub fn is_group_owner() -> bool {
    let guard = MANAGER.lock();
    match guard.as_ref() {
        Some(mgr) => mgr.is_group_owner(),
        None => false,
    }
}

/// Initialize the WiFi Direct subsystem
pub fn init() {
    let mut guard = MANAGER.lock();
    *guard = Some(WifiDirectManager::new());
    serial_println!("    [wifi-direct] WiFi Direct manager initialized (P2P, GO negotiation, service discovery)");
}
