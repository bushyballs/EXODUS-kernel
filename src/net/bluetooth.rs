use crate::sync::Mutex;
/// Hoags Bluetooth — Bluetooth Classic + BLE stack
///
/// Layers:
///   1. HCI (Host Controller Interface) — talk to BT hardware
///   2. L2CAP (Logical Link Control) — multiplexing
///   3. SDP (Service Discovery) — find services
///   4. RFCOMM — serial port emulation
///   5. GATT/ATT — BLE attribute protocol
///
/// Inspired by: BlueZ (Linux), CoreBluetooth (Apple).
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

static BT_STATE: Mutex<Option<BluetoothManager>> = Mutex::new(None);

/// Bluetooth device address (6 bytes, like MAC)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BtAddr(pub [u8; 6]);

impl BtAddr {
    pub fn to_string(&self) -> String {
        alloc::format!(
            "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            self.0[0],
            self.0[1],
            self.0[2],
            self.0[3],
            self.0[4],
            self.0[5]
        )
    }
}

/// Bluetooth device class
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtDeviceClass {
    Computer,
    Phone,
    AudioVideo,
    Peripheral, // keyboard, mouse
    Imaging,
    Wearable,
    Unknown(u32),
}

/// Bluetooth device info (discovered or paired)
#[derive(Debug, Clone)]
pub struct BtDevice {
    pub addr: BtAddr,
    pub name: String,
    pub class: BtDeviceClass,
    pub rssi: i8,
    pub paired: bool,
    pub connected: bool,
    pub ble: bool,
    pub services: Vec<BtService>,
}

/// A Bluetooth service (from SDP)
#[derive(Debug, Clone)]
pub struct BtService {
    pub uuid: u16,
    pub name: String,
    pub channel: u8,
}

/// BLE GATT characteristic
#[derive(Debug, Clone)]
pub struct GattCharacteristic {
    pub uuid: u16,
    pub handle: u16,
    pub properties: u8, // read, write, notify, indicate
    pub value: Vec<u8>,
}

/// BLE GATT service
#[derive(Debug, Clone)]
pub struct GattService {
    pub uuid: u16,
    pub start_handle: u16,
    pub end_handle: u16,
    pub characteristics: Vec<GattCharacteristic>,
}

/// HCI command opcodes
pub const HCI_INQUIRY: u16 = 0x0401;
pub const HCI_CREATE_CONNECTION: u16 = 0x0405;
pub const HCI_DISCONNECT: u16 = 0x0406;
pub const HCI_ACCEPT_CONN: u16 = 0x0409;
pub const HCI_LINK_KEY_REPLY: u16 = 0x040B;
pub const HCI_PIN_CODE_REPLY: u16 = 0x040D;
pub const HCI_READ_LOCAL_NAME: u16 = 0x0C14;
pub const HCI_WRITE_LOCAL_NAME: u16 = 0x0C13;
pub const HCI_READ_BD_ADDR: u16 = 0x1009;
pub const HCI_LE_SET_SCAN_ENABLE: u16 = 0x200C;
pub const HCI_LE_SET_SCAN_PARAMS: u16 = 0x200B;

/// HCI event codes
pub const HCI_EVENT_INQUIRY_RESULT: u8 = 0x02;
pub const HCI_EVENT_CONN_COMPLETE: u8 = 0x03;
pub const HCI_EVENT_DISCONN_COMPLETE: u8 = 0x05;
pub const HCI_EVENT_CMD_COMPLETE: u8 = 0x0E;
pub const HCI_EVENT_CMD_STATUS: u8 = 0x0F;
pub const HCI_EVENT_LE_META: u8 = 0x3E;

/// HCI command packet
#[derive(Debug, Clone)]
pub struct HciCommand {
    pub opcode: u16,
    pub params: Vec<u8>,
}

impl HciCommand {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.push(0x01); // HCI command packet type
        buf.extend_from_slice(&self.opcode.to_le_bytes());
        buf.push(self.params.len() as u8);
        buf.extend_from_slice(&self.params);
        buf
    }
}

/// Bluetooth manager
pub struct BluetoothManager {
    pub local_addr: BtAddr,
    pub local_name: String,
    pub devices: BTreeMap<BtAddr, BtDevice>,
    pub scanning: bool,
    pub discoverable: bool,
    pub powered: bool,
}

impl BluetoothManager {
    pub fn new() -> Self {
        BluetoothManager {
            local_addr: BtAddr([0; 6]),
            local_name: String::from("Hoags OS"),
            devices: BTreeMap::new(),
            scanning: false,
            discoverable: false,
            powered: false,
        }
    }

    /// Start scanning for nearby devices
    pub fn start_scan(&mut self) -> HciCommand {
        self.scanning = true;
        serial_println!("    [bt] Starting device scan...");

        // BLE scan parameters
        HciCommand {
            opcode: HCI_LE_SET_SCAN_ENABLE,
            params: alloc::vec![0x01, 0x00], // enable, no filter duplicates
        }
    }

    /// Stop scanning
    pub fn stop_scan(&mut self) -> HciCommand {
        self.scanning = false;
        HciCommand {
            opcode: HCI_LE_SET_SCAN_ENABLE,
            params: alloc::vec![0x00, 0x00],
        }
    }

    /// Process a discovered device
    pub fn on_device_found(&mut self, addr: BtAddr, name: String, rssi: i8, ble: bool) {
        let device = self.devices.entry(addr).or_insert_with(|| BtDevice {
            addr,
            name: name.clone(),
            class: BtDeviceClass::Unknown(0),
            rssi,
            paired: false,
            connected: false,
            ble,
            services: Vec::new(),
        });
        device.rssi = rssi;
        if !name.is_empty() {
            device.name = name;
        }
        serial_println!(
            "    [bt] Found: {} ({}) RSSI: {}dBm",
            device.name,
            addr.to_string(),
            rssi
        );
    }

    /// Initiate pairing with a device
    pub fn pair(&mut self, addr: &BtAddr) -> Result<HciCommand, &'static str> {
        let device = self.devices.get_mut(addr).ok_or("device not found")?;
        serial_println!("    [bt] Pairing with: {}", device.name);

        Ok(HciCommand {
            opcode: HCI_CREATE_CONNECTION,
            params: {
                let mut p = Vec::new();
                p.extend_from_slice(&addr.0);
                p.extend_from_slice(&[0x18, 0xCC, 0x01, 0x00, 0x00, 0x00, 0x01]);
                p
            },
        })
    }

    /// Mark device as paired
    pub fn on_paired(&mut self, addr: &BtAddr) {
        if let Some(device) = self.devices.get_mut(addr) {
            device.paired = true;
            serial_println!("    [bt] Paired with: {}", device.name);
        }
    }

    /// Disconnect from a device
    pub fn disconnect(&mut self, addr: &BtAddr) -> Result<(), &'static str> {
        let device = self.devices.get_mut(addr).ok_or("device not found")?;
        device.connected = false;
        serial_println!("    [bt] Disconnected: {}", device.name);
        Ok(())
    }

    /// List paired devices
    pub fn paired_devices(&self) -> Vec<&BtDevice> {
        self.devices.values().filter(|d| d.paired).collect()
    }

    /// List nearby devices
    pub fn nearby_devices(&self) -> Vec<&BtDevice> {
        self.devices.values().collect()
    }
}

pub fn init() {
    *BT_STATE.lock() = Some(BluetoothManager::new());
    serial_println!("    [bt] Bluetooth stack initialized (Classic + BLE)");
}
