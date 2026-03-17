/// Generic Attribute Profile (BLE services and characteristics).
///
/// GATT defines the framework for BLE services and characteristics:
///   - Services group related characteristics under a UUID
///   - Characteristics have a value, properties, and optional descriptors
///   - ATT (Attribute Protocol) handles are assigned to each attribute
///   - Clients read/write characteristics, subscribe to notifications
///
/// Mandatory services:
///   - GAP Service (0x1800): Device Name, Appearance
///   - GATT Service (0x1801): Service Changed indication
///
/// ATT operations supported:
///   - Read By Group Type (discover services)
///   - Read By Type (discover characteristics)
///   - Read/Write characteristic values
///   - Handle Value Notification/Indication
///   - Find Information (discover descriptors)
///
/// Part of the AIOS bluetooth subsystem.

use alloc::vec::Vec;
use alloc::string::String;
use alloc::collections::BTreeMap;
use crate::{serial_print, serial_println};
use crate::sync::Mutex;

/// Well-known GATT/ATT UUIDs (16-bit short form).
const UUID_PRIMARY_SERVICE: u16 = 0x2800;
const UUID_SECONDARY_SERVICE: u16 = 0x2801;
const UUID_INCLUDE: u16 = 0x2802;
const UUID_CHARACTERISTIC: u16 = 0x2803;
const UUID_CCC_DESCRIPTOR: u16 = 0x2902; // Client Characteristic Configuration
const UUID_GAP_SERVICE: u16 = 0x1800;
const UUID_GATT_SERVICE: u16 = 0x1801;
const UUID_DEVICE_NAME: u16 = 0x2A00;
const UUID_APPEARANCE: u16 = 0x2A01;
const UUID_SERVICE_CHANGED: u16 = 0x2A05;

/// Characteristic properties (bitmask).
const PROP_BROADCAST: u8 = 0x01;
const PROP_READ: u8 = 0x02;
const PROP_WRITE_NO_RSP: u8 = 0x04;
const PROP_WRITE: u8 = 0x08;
const PROP_NOTIFY: u8 = 0x10;
const PROP_INDICATE: u8 = 0x20;
const PROP_AUTH_WRITE: u8 = 0x40;
const PROP_EXTENDED: u8 = 0x80;

/// ATT error codes.
const ATT_ERR_INVALID_HANDLE: u8 = 0x01;
const ATT_ERR_READ_NOT_PERMITTED: u8 = 0x02;
const ATT_ERR_WRITE_NOT_PERMITTED: u8 = 0x03;
const ATT_ERR_ATTR_NOT_FOUND: u8 = 0x0A;
const ATT_ERR_INSUFFICIENT_AUTH: u8 = 0x05;

/// ATT opcodes.
const ATT_ERROR_RSP: u8 = 0x01;
const ATT_MTU_REQ: u8 = 0x02;
const ATT_MTU_RSP: u8 = 0x03;
const ATT_FIND_INFO_REQ: u8 = 0x04;
const ATT_FIND_INFO_RSP: u8 = 0x05;
const ATT_READ_BY_TYPE_REQ: u8 = 0x08;
const ATT_READ_BY_TYPE_RSP: u8 = 0x09;
const ATT_READ_REQ: u8 = 0x0A;
const ATT_READ_RSP: u8 = 0x0B;
const ATT_WRITE_REQ: u8 = 0x12;
const ATT_WRITE_RSP: u8 = 0x13;
const ATT_HANDLE_VALUE_NTF: u8 = 0x1B;
const ATT_HANDLE_VALUE_IND: u8 = 0x1D;
const ATT_READ_BY_GROUP_TYPE_REQ: u8 = 0x10;
const ATT_READ_BY_GROUP_TYPE_RSP: u8 = 0x11;

/// Global GATT server state.
static GATT: Mutex<Option<GattServerInner>> = Mutex::new(None);

/// A single attribute in the GATT database.
#[derive(Clone)]
struct GattAttribute {
    handle: u16,
    attr_type: u128,   // UUID of the attribute type
    value: Vec<u8>,
    permissions: u8,   // read/write/notify properties
}

/// A characteristic descriptor.
#[derive(Clone)]
pub struct GattDescriptor {
    pub uuid: u128,
    pub value: Vec<u8>,
}

/// A GATT characteristic.
#[derive(Clone)]
pub struct GattCharacteristic {
    pub uuid: u128,
    pub properties: u8,
    pub value: Vec<u8>,
    pub descriptors: Vec<GattDescriptor>,
}

impl GattCharacteristic {
    /// Create a readable characteristic.
    pub fn new_readable(uuid: u128, initial_value: &[u8]) -> Self {
        Self {
            uuid,
            properties: PROP_READ,
            value: initial_value.to_vec(),
            descriptors: Vec::new(),
        }
    }

    /// Create a read/write characteristic.
    pub fn new_read_write(uuid: u128, initial_value: &[u8]) -> Self {
        Self {
            uuid,
            properties: PROP_READ | PROP_WRITE,
            value: initial_value.to_vec(),
            descriptors: Vec::new(),
        }
    }

    /// Create a notifiable characteristic (with CCC descriptor).
    pub fn new_notifiable(uuid: u128) -> Self {
        Self {
            uuid,
            properties: PROP_READ | PROP_NOTIFY,
            value: Vec::new(),
            descriptors: alloc::vec![GattDescriptor {
                uuid: uuid_from_16(UUID_CCC_DESCRIPTOR),
                value: alloc::vec![0x00, 0x00], // notifications disabled by default
            }],
        }
    }
}

/// A BLE service with characteristics.
pub struct GattService {
    pub uuid: u128,
    pub characteristics: Vec<GattCharacteristic>,
    pub is_primary: bool,
}

impl GattService {
    /// Create a primary service.
    pub fn primary(uuid: u128) -> Self {
        Self {
            uuid,
            characteristics: Vec::new(),
            is_primary: true,
        }
    }
}

/// Subscriber for notifications on a characteristic handle.
struct NotifSubscriber {
    conn_handle: u16,
    notifications_enabled: bool,
    indications_enabled: bool,
}

/// Internal GATT server state.
struct GattServerInner {
    attributes: Vec<GattAttribute>,
    next_handle: u16,
    att_mtu: u16,
    subscribers: BTreeMap<u16, Vec<NotifSubscriber>>, // char value handle -> subscribers
    service_handles: Vec<(u16, u16, u128)>,           // (start_handle, end_handle, uuid)
}

impl GattServerInner {
    fn new() -> Self {
        Self {
            attributes: Vec::new(),
            next_handle: 1, // Handle 0 is reserved.
            att_mtu: 23,    // Default ATT_MTU for BLE.
            subscribers: BTreeMap::new(),
            service_handles: Vec::new(),
        }
    }

    /// Allocate the next attribute handle.
    fn alloc_handle(&mut self) -> u16 {
        let h = self.next_handle;
        self.next_handle = self.next_handle.saturating_add(1);
        h
    }

    /// Add a raw attribute.
    fn add_attribute(&mut self, attr_type: u128, value: Vec<u8>, permissions: u8) -> u16 {
        let handle = self.alloc_handle();
        self.attributes.push(GattAttribute {
            handle,
            attr_type,
            value,
            permissions,
        });
        handle
    }

    /// Register a complete service with its characteristics and descriptors.
    fn add_service(&mut self, service: &GattService) {
        let svc_type = if service.is_primary {
            uuid_from_16(UUID_PRIMARY_SERVICE)
        } else {
            uuid_from_16(UUID_SECONDARY_SERVICE)
        };

        let start_handle = self.next_handle;

        // Service declaration attribute: value is the service UUID.
        let uuid_bytes = uuid_to_bytes(service.uuid);
        self.add_attribute(svc_type, uuid_bytes, PROP_READ);

        // Add each characteristic.
        for charac in &service.characteristics {
            // Characteristic declaration attribute.
            let char_decl_handle = self.alloc_handle();
            let value_handle = self.next_handle; // The next handle will be the value.

            // Declaration value: properties(1) + value_handle(2) + UUID.
            let char_uuid_bytes = uuid_to_bytes(charac.uuid);
            let mut decl_value = Vec::with_capacity(3 + char_uuid_bytes.len());
            decl_value.push(charac.properties);
            decl_value.push((value_handle & 0xFF) as u8);
            decl_value.push((value_handle >> 8) as u8);
            decl_value.extend_from_slice(&char_uuid_bytes);

            self.attributes.push(GattAttribute {
                handle: char_decl_handle,
                attr_type: uuid_from_16(UUID_CHARACTERISTIC),
                value: decl_value,
                permissions: PROP_READ,
            });

            // Characteristic value attribute.
            self.add_attribute(charac.uuid, charac.value.clone(), charac.properties);

            // Descriptors.
            for desc in &charac.descriptors {
                self.add_attribute(desc.uuid, desc.value.clone(), PROP_READ | PROP_WRITE);
            }
        }

        let end_handle = self.next_handle.saturating_sub(1);
        self.service_handles.push((start_handle, end_handle, service.uuid));

        serial_println!("    [gatt] Added service UUID={:#034x} handles={}-{}", service.uuid, start_handle, end_handle);
    }

    /// Read an attribute by handle.
    fn read_attribute(&self, handle: u16) -> Option<&[u8]> {
        for attr in &self.attributes {
            if attr.handle == handle {
                return Some(&attr.value);
            }
        }
        None
    }

    /// Write an attribute by handle.
    fn write_attribute(&mut self, handle: u16, value: &[u8]) -> Result<(), u8> {
        for attr in &mut self.attributes {
            if attr.handle == handle {
                if attr.permissions & (PROP_WRITE | PROP_WRITE_NO_RSP) == 0 {
                    return Err(ATT_ERR_WRITE_NOT_PERMITTED);
                }
                attr.value = value.to_vec();
                return Ok(());
            }
        }
        Err(ATT_ERR_INVALID_HANDLE)
    }

    /// Build a notification PDU for a characteristic value handle.
    fn build_notification(&self, handle: u16, value: &[u8]) -> Vec<u8> {
        let mut pdu = Vec::with_capacity(3 + value.len());
        pdu.push(ATT_HANDLE_VALUE_NTF);
        pdu.push((handle & 0xFF) as u8);
        pdu.push((handle >> 8) as u8);
        pdu.extend_from_slice(value);
        pdu
    }

    /// Handle an incoming ATT PDU and return the response.
    fn handle_att_pdu(&mut self, pdu: &[u8]) -> Vec<u8> {
        if pdu.is_empty() {
            return Vec::new();
        }

        let opcode = pdu[0];
        match opcode {
            ATT_MTU_REQ => {
                if pdu.len() >= 3 {
                    let client_mtu = (pdu[1] as u16) | ((pdu[2] as u16) << 8);
                    // Server MTU is our ATT_MTU.
                    let server_mtu = self.att_mtu;
                    let negotiated = core::cmp::min(client_mtu, server_mtu);
                    self.att_mtu = core::cmp::max(negotiated, 23);
                    let mut rsp = Vec::with_capacity(3);
                    rsp.push(ATT_MTU_RSP);
                    rsp.push((server_mtu & 0xFF) as u8);
                    rsp.push((server_mtu >> 8) as u8);
                    return rsp;
                }
            }
            ATT_READ_REQ => {
                if pdu.len() >= 3 {
                    let handle = (pdu[1] as u16) | ((pdu[2] as u16) << 8);
                    if let Some(value) = self.read_attribute(handle) {
                        let mut rsp = Vec::with_capacity(1 + value.len());
                        rsp.push(ATT_READ_RSP);
                        rsp.extend_from_slice(value);
                        return rsp;
                    }
                    return self.att_error(ATT_READ_REQ, handle, ATT_ERR_INVALID_HANDLE);
                }
            }
            ATT_WRITE_REQ => {
                if pdu.len() >= 3 {
                    let handle = (pdu[1] as u16) | ((pdu[2] as u16) << 8);
                    let value = &pdu[3..];
                    match self.write_attribute(handle, value) {
                        Ok(()) => {
                            return alloc::vec![ATT_WRITE_RSP];
                        }
                        Err(err) => {
                            return self.att_error(ATT_WRITE_REQ, handle, err);
                        }
                    }
                }
            }
            ATT_READ_BY_GROUP_TYPE_REQ => {
                if pdu.len() >= 7 {
                    let start = (pdu[1] as u16) | ((pdu[2] as u16) << 8);
                    let end = (pdu[3] as u16) | ((pdu[4] as u16) << 8);
                    let uuid_type = if pdu.len() == 7 {
                        uuid_from_16((pdu[5] as u16) | ((pdu[6] as u16) << 8))
                    } else {
                        // 128-bit UUID in request.
                        let mut bytes = [0u8; 16];
                        let copy_len = core::cmp::min(16, pdu.len() - 5);
                        bytes[..copy_len].copy_from_slice(&pdu[5..5 + copy_len]);
                        u128::from_le_bytes(bytes)
                    };

                    // Only handle primary service discovery.
                    if uuid_type == uuid_from_16(UUID_PRIMARY_SERVICE) {
                        let mut rsp = Vec::new();
                        rsp.push(ATT_READ_BY_GROUP_TYPE_RSP);
                        rsp.push(6); // length per entry: handle(2) + end(2) + uuid16(2)

                        for &(svc_start, svc_end, svc_uuid) in &self.service_handles {
                            if svc_start >= start && svc_start <= end {
                                rsp.push((svc_start & 0xFF) as u8);
                                rsp.push((svc_start >> 8) as u8);
                                rsp.push((svc_end & 0xFF) as u8);
                                rsp.push((svc_end >> 8) as u8);
                                // Encode UUID as 16-bit if possible.
                                let short = uuid_to_16(svc_uuid);
                                rsp.push((short & 0xFF) as u8);
                                rsp.push((short >> 8) as u8);
                            }
                        }

                        if rsp.len() > 2 {
                            return rsp;
                        }
                        return self.att_error(ATT_READ_BY_GROUP_TYPE_REQ, start, ATT_ERR_ATTR_NOT_FOUND);
                    }
                }
            }
            _ => {
                serial_println!("    [gatt] Unhandled ATT opcode {:#04x}", opcode);
            }
        }

        Vec::new()
    }

    /// Build an ATT Error Response.
    fn att_error(&self, req_opcode: u8, handle: u16, error: u8) -> Vec<u8> {
        let mut rsp = Vec::with_capacity(5);
        rsp.push(ATT_ERROR_RSP);
        rsp.push(req_opcode);
        rsp.push((handle & 0xFF) as u8);
        rsp.push((handle >> 8) as u8);
        rsp.push(error);
        rsp
    }

    /// Register the mandatory GAP and GATT services.
    fn register_mandatory_services(&mut self) {
        // GAP Service (0x1800).
        let mut gap_svc = GattService::primary(uuid_from_16(UUID_GAP_SERVICE));
        gap_svc.characteristics.push(GattCharacteristic::new_readable(
            uuid_from_16(UUID_DEVICE_NAME),
            b"AIOS-BT",
        ));
        gap_svc.characteristics.push(GattCharacteristic::new_readable(
            uuid_from_16(UUID_APPEARANCE),
            &[0x00, 0x00], // Generic Unknown
        ));
        self.add_service(&gap_svc);

        // GATT Service (0x1801).
        let mut gatt_svc = GattService::primary(uuid_from_16(UUID_GATT_SERVICE));
        gatt_svc.characteristics.push(GattCharacteristic::new_notifiable(
            uuid_from_16(UUID_SERVICE_CHANGED),
        ));
        self.add_service(&gatt_svc);
    }
}

/// Convert a 16-bit UUID to the full 128-bit Bluetooth Base UUID.
fn uuid_from_16(short: u16) -> u128 {
    // Bluetooth Base UUID: 00000000-0000-1000-8000-00805F9B34FB
    0x0000_0000_0000_1000_8000_00805F9B34FB_u128 | ((short as u128) << 96)
}

/// Try to extract a 16-bit short UUID from a 128-bit UUID.
fn uuid_to_16(uuid: u128) -> u16 {
    let base: u128 = 0x0000_0000_0000_1000_8000_00805F9B34FB;
    let short_part = (uuid & !base) >> 96;
    short_part as u16
}

/// Convert a 128-bit UUID to little-endian bytes.
fn uuid_to_bytes(uuid: u128) -> Vec<u8> {
    let short = uuid_to_16(uuid);
    if short != 0 && uuid == uuid_from_16(short) {
        // Use 16-bit encoding.
        alloc::vec![(short & 0xFF) as u8, (short >> 8) as u8]
    } else {
        // Full 128-bit, little-endian.
        uuid.to_le_bytes().to_vec()
    }
}

/// GATT server hosting BLE services.
pub struct GattServer {
    _private: (),
}

impl GattServer {
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Register a GATT service.
    pub fn add_service(&mut self, service: GattService) {
        if let Some(inner) = GATT.lock().as_mut() {
            inner.add_service(&service);
        }
    }

    /// Send a notification to subscribed clients.
    pub fn notify(&self, handle: u16, value: &[u8]) {
        if let Some(inner) = GATT.lock().as_ref() {
            let pdu = inner.build_notification(handle, value);
            // In a full stack, this PDU would be sent over the L2CAP ATT channel.
            serial_println!("    [gatt] Notification on handle {}: {} bytes", handle, pdu.len());
        }
    }
}

/// Handle an incoming ATT PDU from L2CAP (CID 0x0004).
pub fn handle_att(pdu: &[u8]) -> Vec<u8> {
    if let Some(inner) = GATT.lock().as_mut() {
        inner.handle_att_pdu(pdu)
    } else {
        Vec::new()
    }
}

pub fn init() {
    let mut inner = GattServerInner::new();

    serial_println!("    [gatt] Initializing GATT server");

    // Register mandatory GAP and GATT services.
    inner.register_mandatory_services();

    *GATT.lock() = Some(inner);
    serial_println!("    [gatt] GATT server initialized with GAP and GATT services");
}
