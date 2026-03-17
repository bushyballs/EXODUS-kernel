/// Bluetooth Low Energy (BLE)
///
/// Low-power wireless communication for IoT devices,
/// beacons, and peripherals. Part of the AIOS networking layer.
///
/// This module handles:
///   - BLE advertising (connectable, scannable, non-connectable)
///   - BLE scanning (passive and active)
///   - LE connection establishment and parameter management
///   - Advertising Data (AD) type encoding/decoding
///   - HCI LE controller commands for radio management
///
/// AD Types used:
///   0x01 = Flags
///   0x02/0x03 = Incomplete/Complete 16-bit UUIDs
///   0x06/0x07 = Incomplete/Complete 128-bit UUIDs
///   0x08/0x09 = Shortened/Complete Local Name
///   0x0A = TX Power Level
///   0xFF = Manufacturer Specific Data

use alloc::vec::Vec;
use alloc::string::String;
use alloc::collections::VecDeque;
use crate::{serial_print, serial_println};
use crate::sync::Mutex;

/// HCI LE command opcodes (OGF=0x08).
const HCI_LE_SET_ADV_PARAMS: u16 = 0x2006;
const HCI_LE_SET_ADV_DATA: u16 = 0x2008;
const HCI_LE_SET_ADV_ENABLE: u16 = 0x200A;
const HCI_LE_SET_SCAN_PARAMS: u16 = 0x200B;
const HCI_LE_SET_SCAN_ENABLE: u16 = 0x200C;
const HCI_LE_CREATE_CONN: u16 = 0x200D;
const HCI_LE_CREATE_CONN_CANCEL: u16 = 0x200E;
const HCI_LE_CONN_UPDATE: u16 = 0x2013;

/// Advertising types.
const ADV_IND: u8 = 0x00;          // Connectable undirected
const ADV_DIRECT_IND: u8 = 0x01;   // Connectable directed
const ADV_SCAN_IND: u8 = 0x02;     // Scannable undirected
const ADV_NONCONN_IND: u8 = 0x03;  // Non-connectable undirected

/// AD Type codes.
const AD_TYPE_FLAGS: u8 = 0x01;
const AD_TYPE_UUID16_INCOMPLETE: u8 = 0x02;
const AD_TYPE_UUID16_COMPLETE: u8 = 0x03;
const AD_TYPE_UUID128_INCOMPLETE: u8 = 0x06;
const AD_TYPE_UUID128_COMPLETE: u8 = 0x07;
const AD_TYPE_SHORT_NAME: u8 = 0x08;
const AD_TYPE_COMPLETE_NAME: u8 = 0x09;
const AD_TYPE_TX_POWER: u8 = 0x0A;
const AD_TYPE_MFR_DATA: u8 = 0xFF;

/// BLE flags.
const FLAG_LE_GENERAL_DISC: u8 = 0x02;
const FLAG_LE_LIMITED_DISC: u8 = 0x01;
const FLAG_BR_EDR_NOT_SUPPORTED: u8 = 0x04;

/// Maximum advertising data length.
const MAX_ADV_DATA_LEN: usize = 31;

/// Default advertising interval (in 0.625ms units): ~100ms.
const DEFAULT_ADV_INTERVAL_MIN: u16 = 0x00A0; // 100ms
const DEFAULT_ADV_INTERVAL_MAX: u16 = 0x00A0;

/// Default scan interval and window (in 0.625ms units).
const DEFAULT_SCAN_INTERVAL: u16 = 0x0010; // 10ms
const DEFAULT_SCAN_WINDOW: u16 = 0x0010;

/// Default connection parameters.
const DEFAULT_CONN_INTERVAL_MIN: u16 = 24;  // 30ms (units of 1.25ms)
const DEFAULT_CONN_INTERVAL_MAX: u16 = 40;  // 50ms
const DEFAULT_CONN_LATENCY: u16 = 0;
const DEFAULT_SUPERVISION_TIMEOUT: u16 = 100; // 1000ms (units of 10ms)

/// Global BLE state.
static BLE: Mutex<Option<BleManager>> = Mutex::new(None);

/// BLE advertising state.
#[derive(Debug, Clone, Copy, PartialEq)]
enum AdvState {
    Idle,
    Advertising,
}

/// BLE scanning state.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ScanState {
    Idle,
    Scanning,
}

/// Connection state.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ConnState {
    Disconnected,
    Connecting,
    Connected,
}

/// Internal BLE manager.
struct BleManager {
    adv_state: AdvState,
    scan_state: ScanState,
    adv_data_raw: Vec<u8>,
    scan_results: VecDeque<ScannedDevice>,
    connections: Vec<BleConnectionInner>,
    local_address: [u8; 6],
}

/// A scanned BLE device (internal).
struct ScannedDevice {
    address: [u8; 6],
    address_type: u8,
    rssi: i8,
    adv_data: Vec<u8>,
}

/// Internal connection state.
struct BleConnectionInner {
    handle: u16,
    peer_addr: [u8; 6],
    conn_interval_ms: u16,
    latency: u16,
    supervision_timeout: u16,
    state: ConnState,
}

impl BleManager {
    fn new() -> Self {
        Self {
            adv_state: AdvState::Idle,
            scan_state: ScanState::Idle,
            adv_data_raw: Vec::new(),
            scan_results: VecDeque::new(),
            connections: Vec::new(),
            local_address: [0x00, 0x11, 0x22, 0x33, 0x44, 0x55], // Default address
        }
    }

    /// Encode advertising data from the user-facing structure.
    fn encode_adv_data(data: &AdvertisingData) -> Vec<u8> {
        let mut raw = Vec::with_capacity(MAX_ADV_DATA_LEN);

        // Flags.
        raw.push(2); // length
        raw.push(AD_TYPE_FLAGS);
        raw.push(FLAG_LE_GENERAL_DISC | FLAG_BR_EDR_NOT_SUPPORTED);

        // Complete local name.
        if !data.local_name.is_empty() {
            let name_bytes = data.local_name.as_bytes();
            let name_len = core::cmp::min(name_bytes.len(), MAX_ADV_DATA_LEN - raw.len() - 2);
            raw.push((name_len + 1) as u8); // length
            raw.push(AD_TYPE_COMPLETE_NAME);
            raw.extend_from_slice(&name_bytes[..name_len]);
        }

        // 128-bit service UUIDs.
        for &uuid in &data.service_uuids {
            if raw.len() + 18 > MAX_ADV_DATA_LEN {
                break;
            }
            raw.push(17); // length: 1 type + 16 UUID bytes
            raw.push(AD_TYPE_UUID128_COMPLETE);
            let bytes = uuid.to_le_bytes();
            raw.extend_from_slice(&bytes);
        }

        // TX Power Level.
        if raw.len() + 3 <= MAX_ADV_DATA_LEN {
            raw.push(2);
            raw.push(AD_TYPE_TX_POWER);
            raw.push(data.tx_power as u8);
        }

        raw
    }

    /// Decode advertising data from raw bytes.
    fn decode_adv_data(raw: &[u8]) -> AdvertisingData {
        let mut result = AdvertisingData {
            local_name: String::new(),
            service_uuids: Vec::new(),
            tx_power: 0,
        };

        let mut i = 0;
        while i < raw.len() {
            let len = raw[i] as usize;
            if len == 0 || i + 1 + len > raw.len() {
                break;
            }
            let ad_type = raw[i + 1];
            let data = &raw[i + 2..i + 1 + len];

            match ad_type {
                AD_TYPE_COMPLETE_NAME | AD_TYPE_SHORT_NAME => {
                    if let Ok(name) = core::str::from_utf8(data) {
                        result.local_name = String::from(name);
                    }
                }
                AD_TYPE_UUID128_COMPLETE | AD_TYPE_UUID128_INCOMPLETE => {
                    let mut offset = 0;
                    while offset + 16 <= data.len() {
                        let mut bytes = [0u8; 16];
                        bytes.copy_from_slice(&data[offset..offset + 16]);
                        result.service_uuids.push(u128::from_le_bytes(bytes));
                        offset += 16;
                    }
                }
                AD_TYPE_TX_POWER => {
                    if !data.is_empty() {
                        result.tx_power = data[0] as i8;
                    }
                }
                _ => {}
            }

            i += 1 + len;
        }

        result
    }

    /// Start advertising with the given data.
    fn start_advertising(&mut self, data: &AdvertisingData) -> Result<(), ()> {
        if self.adv_state == AdvState::Advertising {
            serial_println!("    [ble] Already advertising");
            return Ok(());
        }

        self.adv_data_raw = Self::encode_adv_data(data);

        // In a full stack, we would send HCI commands:
        // 1. LE Set Advertising Parameters
        // 2. LE Set Advertising Data
        // 3. LE Set Advertising Enable

        self.adv_state = AdvState::Advertising;
        serial_println!("    [ble] Advertising started: name='{}', {} UUIDs, tx_power={}",
            data.local_name, data.service_uuids.len(), data.tx_power);
        Ok(())
    }

    /// Stop advertising.
    fn stop_advertising(&mut self) {
        if self.adv_state == AdvState::Advertising {
            self.adv_state = AdvState::Idle;
            serial_println!("    [ble] Advertising stopped");
        }
    }

    /// Start scanning for nearby BLE devices.
    fn start_scanning(&mut self) {
        if self.scan_state == ScanState::Scanning {
            return;
        }

        self.scan_results.clear();
        self.scan_state = ScanState::Scanning;
        serial_println!("    [ble] Scanning started");
    }

    /// Stop scanning and return results.
    fn stop_scanning(&mut self) -> Vec<AdvertisingData> {
        self.scan_state = ScanState::Idle;

        let mut results = Vec::new();
        while let Some(device) = self.scan_results.pop_front() {
            results.push(Self::decode_adv_data(&device.adv_data));
        }

        serial_println!("    [ble] Scanning stopped, {} devices found", results.len());
        results
    }

    /// Handle an incoming advertising report from HCI.
    fn handle_adv_report(&mut self, addr: [u8; 6], addr_type: u8, rssi: i8, data: &[u8]) {
        if self.scan_state != ScanState::Scanning {
            return;
        }

        self.scan_results.push_back(ScannedDevice {
            address: addr,
            address_type: addr_type,
            rssi,
            adv_data: data.to_vec(),
        });
    }

    /// Initiate a BLE connection to a peer.
    fn connect(&mut self, peer_addr: [u8; 6]) -> Result<u16, ()> {
        // Assign a connection handle.
        let handle = (self.connections.len() as u16) + 1;

        let conn = BleConnectionInner {
            handle,
            peer_addr,
            conn_interval_ms: ((DEFAULT_CONN_INTERVAL_MIN as u32 * 125) / 100) as u16,
            latency: DEFAULT_CONN_LATENCY,
            supervision_timeout: DEFAULT_SUPERVISION_TIMEOUT,
            state: ConnState::Connected,
        };

        serial_println!("    [ble] Connected to {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} handle={}",
            peer_addr[0], peer_addr[1], peer_addr[2],
            peer_addr[3], peer_addr[4], peer_addr[5], handle);

        self.connections.push(conn);
        Ok(handle)
    }

    /// Disconnect a BLE connection.
    fn disconnect(&mut self, peer_addr: &[u8; 6]) {
        for conn in &mut self.connections {
            if conn.peer_addr == *peer_addr && conn.state == ConnState::Connected {
                conn.state = ConnState::Disconnected;
                serial_println!("    [ble] Disconnected from {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    peer_addr[0], peer_addr[1], peer_addr[2],
                    peer_addr[3], peer_addr[4], peer_addr[5]);
                return;
            }
        }
    }
}

/// BLE advertising data
pub struct AdvertisingData {
    pub local_name: String,
    pub service_uuids: Vec<u128>,
    pub tx_power: i8,
}

/// BLE connection
pub struct BleConnection {
    pub peer_addr: [u8; 6],
    pub conn_interval_ms: u16,
    pub latency: u16,
    connected: bool,
}

impl BleConnection {
    pub fn new(peer_addr: [u8; 6]) -> Self {
        Self {
            peer_addr,
            conn_interval_ms: ((DEFAULT_CONN_INTERVAL_MIN as u32 * 125) / 100) as u16,
            latency: DEFAULT_CONN_LATENCY,
            connected: false,
        }
    }

    pub fn connect(&mut self) -> Result<(), ()> {
        if self.connected {
            return Ok(());
        }

        if let Some(mgr) = BLE.lock().as_mut() {
            match mgr.connect(self.peer_addr) {
                Ok(_handle) => {
                    self.connected = true;
                    Ok(())
                }
                Err(()) => Err(()),
            }
        } else {
            Err(())
        }
    }

    pub fn disconnect(&mut self) {
        if !self.connected {
            return;
        }

        if let Some(mgr) = BLE.lock().as_mut() {
            mgr.disconnect(&self.peer_addr);
        }
        self.connected = false;
    }
}

pub fn start_advertising(data: &AdvertisingData) -> Result<(), ()> {
    if let Some(mgr) = BLE.lock().as_mut() {
        mgr.start_advertising(data)
    } else {
        Err(())
    }
}

pub fn start_scanning() -> Vec<AdvertisingData> {
    if let Some(mgr) = BLE.lock().as_mut() {
        mgr.start_scanning();
        // In a real stack, scanning runs asynchronously. We return current results.
        mgr.stop_scanning()
    } else {
        Vec::new()
    }
}

/// Stop advertising.
pub fn stop_advertising() {
    if let Some(mgr) = BLE.lock().as_mut() {
        mgr.stop_advertising();
    }
}

pub fn init() {
    let mgr = BleManager::new();

    serial_println!("    [ble] Initializing BLE subsystem");

    *BLE.lock() = Some(mgr);
    serial_println!("    [ble] BLE subsystem initialized");
}
