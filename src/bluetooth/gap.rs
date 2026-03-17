/// Generic Access Profile -- discovery and connection management.
///
/// GAP defines how Bluetooth devices discover each other, establish
/// connections, and manage security. This module handles:
///   - Device discovery (inquiry for BR/EDR, scanning for LE)
///   - Name discovery and resolution
///   - Connection establishment and management
///   - Device roles (central/peripheral, master/slave)
///   - Discoverable and connectable modes
///   - Extended Inquiry Response (EIR) data parsing
///
/// GAP modes:
///   - Non-discoverable / Limited / General discoverable
///   - Non-connectable / Connectable
///   - Non-bondable / Bondable
///
/// Part of the AIOS bluetooth subsystem.

use alloc::vec::Vec;
use alloc::string::String;
use alloc::collections::BTreeMap;
use crate::{serial_print, serial_println};
use crate::sync::Mutex;

/// HCI command opcodes for GAP operations.
const HCI_INQUIRY: u16 = 0x0401;
const HCI_INQUIRY_CANCEL: u16 = 0x0402;
const HCI_CREATE_CONNECTION: u16 = 0x0405;
const HCI_DISCONNECT: u16 = 0x0406;
const HCI_REMOTE_NAME_REQ: u16 = 0x0419;
const HCI_WRITE_SCAN_ENABLE: u16 = 0x0C1A;
const HCI_WRITE_LOCAL_NAME: u16 = 0x0C13;
const HCI_WRITE_CLASS_OF_DEVICE: u16 = 0x0C24;

/// Inquiry Access Codes.
const GIAC: u32 = 0x9E8B33; // General Inquiry Access Code
const LIAC: u32 = 0x9E8B00; // Limited Inquiry Access Code

/// Scan enable bits.
const SCAN_DISABLED: u8 = 0x00;
const SCAN_INQUIRY_ENABLED: u8 = 0x01;
const SCAN_PAGE_ENABLED: u8 = 0x02;
const SCAN_BOTH_ENABLED: u8 = 0x03;

/// Default inquiry duration (1.28s * N): 10.24 seconds.
const DEFAULT_INQUIRY_LENGTH: u8 = 0x08;

/// Maximum inquiry responses.
const MAX_INQUIRY_RESPONSES: u8 = 0x00; // 0 = unlimited

/// Device major class codes.
const MAJOR_COMPUTER: u32 = 0x01 << 8;
const MAJOR_PHONE: u32 = 0x02 << 8;
const MAJOR_AUDIO_VIDEO: u32 = 0x04 << 8;
const MAJOR_PERIPHERAL: u32 = 0x05 << 8;

/// Global GAP state.
static GAP: Mutex<Option<GapControllerInner>> = Mutex::new(None);

/// GAP role.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GapRole {
    Central,
    Peripheral,
    Observer,
    Broadcaster,
}

/// Discoverable mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiscoverableMode {
    NonDiscoverable,
    LimitedDiscoverable,
    GeneralDiscoverable,
}

/// Connectable mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnectableMode {
    NonConnectable,
    DirectConnectable,
    UndirectedConnectable,
}

/// Connection state for a remote device.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
}

/// Information about a connected device.
struct ConnectedDevice {
    address: [u8; 6],
    handle: u16,
    name: String,
    state: ConnectionState,
    role: GapRole,
    encrypted: bool,
}

/// A discovered Bluetooth device.
pub struct DiscoveredDevice {
    pub address: [u8; 6],
    pub rssi: i8,
    pub name: String,
    pub device_class: u32,
    pub eir_data: Vec<u8>,
}

/// Internal GAP controller state.
struct GapControllerInner {
    local_name: String,
    local_address: [u8; 6],
    device_class: u32,
    discoverable: DiscoverableMode,
    connectable: ConnectableMode,
    role: GapRole,
    discovering: bool,
    discovered_devices: Vec<DiscoveredDevice>,
    connections: BTreeMap<u16, ConnectedDevice>,
    next_handle: u16,
}

impl GapControllerInner {
    fn new() -> Self {
        Self {
            local_name: String::from("AIOS-BT"),
            local_address: [0x00, 0x11, 0x22, 0x33, 0x44, 0x55],
            device_class: MAJOR_COMPUTER,
            discoverable: DiscoverableMode::GeneralDiscoverable,
            connectable: ConnectableMode::UndirectedConnectable,
            role: GapRole::Central,
            discovering: false,
            discovered_devices: Vec::new(),
            connections: BTreeMap::new(),
            next_handle: 1,
        }
    }

    /// Set the local device name.
    fn set_local_name(&mut self, name: &str) {
        self.local_name = String::from(name);
        serial_println!("    [gap] Local name set to '{}'", name);
    }

    /// Set discoverable mode.
    fn set_discoverable(&mut self, mode: DiscoverableMode) {
        self.discoverable = mode;
        let scan_enable = match (&self.discoverable, &self.connectable) {
            (DiscoverableMode::NonDiscoverable, ConnectableMode::NonConnectable) => SCAN_DISABLED,
            (DiscoverableMode::NonDiscoverable, _) => SCAN_PAGE_ENABLED,
            (_, ConnectableMode::NonConnectable) => SCAN_INQUIRY_ENABLED,
            _ => SCAN_BOTH_ENABLED,
        };
        serial_println!("    [gap] Discoverable mode: {:?}, scan_enable={:#04x}", mode, scan_enable);
    }

    /// Start device discovery (inquiry for BR/EDR).
    fn start_discovery(&mut self) {
        if self.discovering {
            serial_println!("    [gap] Already discovering");
            return;
        }

        self.discovered_devices.clear();
        self.discovering = true;

        // HCI Inquiry command: LAP(3) + inquiry_length(1) + num_responses(1)
        let _lap = GIAC;
        serial_println!("    [gap] Discovery started (GIAC inquiry, {}x1.28s)", DEFAULT_INQUIRY_LENGTH);
    }

    /// Stop device discovery.
    fn stop_discovery(&mut self) {
        if self.discovering {
            self.discovering = false;
            serial_println!("    [gap] Discovery stopped, {} devices found", self.discovered_devices.len());
        }
    }

    /// Handle an inquiry result from HCI.
    fn handle_inquiry_result(&mut self, address: [u8; 6], rssi: i8, class: u32, eir: &[u8]) {
        if !self.discovering {
            return;
        }

        // Parse name from EIR data if present.
        let name = Self::parse_eir_name(eir);

        // Avoid duplicates.
        for dev in &self.discovered_devices {
            if dev.address == address {
                return;
            }
        }

        serial_println!("    [gap] Found device {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} rssi={} name='{}'",
            address[0], address[1], address[2], address[3], address[4], address[5],
            rssi, name);

        self.discovered_devices.push(DiscoveredDevice {
            address,
            rssi,
            name,
            device_class: class,
            eir_data: eir.to_vec(),
        });
    }

    /// Parse a device name from Extended Inquiry Response data.
    fn parse_eir_name(eir: &[u8]) -> String {
        let mut i = 0;
        while i < eir.len() {
            let len = eir[i] as usize;
            if len == 0 || i + 1 + len > eir.len() {
                break;
            }
            let eir_type = eir[i + 1];
            // 0x09 = Complete Local Name, 0x08 = Shortened Local Name.
            if eir_type == 0x09 || eir_type == 0x08 {
                let name_data = &eir[i + 2..i + 1 + len];
                if let Ok(name) = core::str::from_utf8(name_data) {
                    return String::from(name);
                }
            }
            i += 1 + len;
        }
        String::new()
    }

    /// Initiate a connection to a remote device.
    fn connect(&mut self, address: &[u8; 6]) {
        // Check if already connected.
        for (_, conn) in &self.connections {
            if conn.address == *address && conn.state == ConnectionState::Connected {
                serial_println!("    [gap] Already connected to {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    address[0], address[1], address[2], address[3], address[4], address[5]);
                return;
            }
        }

        let handle = self.next_handle;
        self.next_handle = self.next_handle.saturating_add(1);

        let conn = ConnectedDevice {
            address: *address,
            handle,
            name: String::new(),
            state: ConnectionState::Connected,
            role: GapRole::Central,
            encrypted: false,
        };

        serial_println!("    [gap] Connecting to {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} handle={}",
            address[0], address[1], address[2], address[3], address[4], address[5], handle);

        self.connections.insert(handle, conn);
    }

    /// Disconnect from a remote device by address.
    fn disconnect(&mut self, address: &[u8; 6]) {
        let mut handle_to_remove = None;
        for (&h, conn) in &mut self.connections {
            if conn.address == *address {
                conn.state = ConnectionState::Disconnected;
                handle_to_remove = Some(h);
                serial_println!("    [gap] Disconnected from {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    address[0], address[1], address[2], address[3], address[4], address[5]);
                break;
            }
        }
        if let Some(h) = handle_to_remove {
            self.connections.remove(&h);
        }
    }

    /// Get the list of currently connected devices.
    fn connected_count(&self) -> usize {
        self.connections.values()
            .filter(|c| c.state == ConnectionState::Connected)
            .count()
    }
}

/// GAP controller for device discovery and connection.
pub struct GapController {
    _private: (),
}

impl GapController {
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Start scanning for nearby Bluetooth devices.
    pub fn start_discovery(&mut self) {
        if let Some(inner) = GAP.lock().as_mut() {
            inner.start_discovery();
        }
    }

    /// Initiate a connection to a remote device.
    pub fn connect(&mut self, address: &[u8; 6]) {
        if let Some(inner) = GAP.lock().as_mut() {
            inner.connect(address);
        }
    }
}

/// Set the local device name.
pub fn set_local_name(name: &str) {
    if let Some(inner) = GAP.lock().as_mut() {
        inner.set_local_name(name);
    }
}

/// Set the discoverable mode.
pub fn set_discoverable(mode: DiscoverableMode) {
    if let Some(inner) = GAP.lock().as_mut() {
        inner.set_discoverable(mode);
    }
}

/// Get the number of currently connected devices.
pub fn connected_count() -> usize {
    if let Some(inner) = GAP.lock().as_ref() {
        inner.connected_count()
    } else {
        0
    }
}

pub fn init() {
    let mut inner = GapControllerInner::new();

    serial_println!("    [gap] Initializing Generic Access Profile");

    // Set default local name.
    inner.set_local_name("AIOS-BT");

    // Set default discoverable + connectable.
    inner.set_discoverable(DiscoverableMode::GeneralDiscoverable);

    *GAP.lock() = Some(inner);
    serial_println!("    [gap] GAP initialized (role=Central, discoverable, connectable)");
}
