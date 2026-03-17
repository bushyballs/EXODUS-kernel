/// USB Core -- device enumeration and descriptor parsing
///
/// Handles the USB device lifecycle:
///   1. Port status change detected
///   2. Reset port
///   3. Get device descriptor (address 0)
///   4. Assign address
///   5. Get full configuration descriptor
///   6. Select configuration
///   7. Match and load class driver
///
/// Features:
///   - All standard USB descriptor types (device, config, interface, endpoint,
///     string, BOS, device qualifier, interface association)
///   - Setup packet construction for standard requests
///   - USB speed definitions and packet size defaults
///   - Configuration descriptor walker for parsing compound descriptors
///   - String descriptor parsing (UTF-16LE to UTF-8 conversion)
///   - Interface classification and class driver dispatch
///
/// References: USB 2.0 Specification chapters 9-11, USB 3.2 Specification.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// USB descriptor types
// ---------------------------------------------------------------------------

pub const DESC_DEVICE: u8 = 1;
pub const DESC_CONFIGURATION: u8 = 2;
pub const DESC_STRING: u8 = 3;
pub const DESC_INTERFACE: u8 = 4;
pub const DESC_ENDPOINT: u8 = 5;
pub const DESC_DEVICE_QUALIFIER: u8 = 6;
pub const DESC_OTHER_SPEED_CONFIG: u8 = 7;
pub const DESC_INTERFACE_POWER: u8 = 8;
pub const DESC_OTG: u8 = 9;
pub const DESC_DEBUG: u8 = 10;
pub const DESC_INTERFACE_ASSOCIATION: u8 = 11;
pub const DESC_BOS: u8 = 15;
pub const DESC_DEVICE_CAPABILITY: u8 = 16;
pub const DESC_SUPERSPEED_EP_COMPANION: u8 = 48;
pub const DESC_HID: u8 = 0x21;
pub const DESC_HID_REPORT: u8 = 0x22;

// ---------------------------------------------------------------------------
// USB standard request codes
// ---------------------------------------------------------------------------

pub const REQ_GET_STATUS: u8 = 0x00;
pub const REQ_CLEAR_FEATURE: u8 = 0x01;
pub const REQ_SET_FEATURE: u8 = 0x03;
pub const REQ_SET_ADDRESS: u8 = 0x05;
pub const REQ_GET_DESCRIPTOR: u8 = 0x06;
pub const REQ_SET_DESCRIPTOR: u8 = 0x07;
pub const REQ_GET_CONFIGURATION: u8 = 0x08;
pub const REQ_SET_CONFIGURATION: u8 = 0x09;
pub const REQ_GET_INTERFACE: u8 = 0x0A;
pub const REQ_SET_INTERFACE: u8 = 0x0B;
pub const REQ_SYNCH_FRAME: u8 = 0x0C;

// ---------------------------------------------------------------------------
// USB feature selectors
// ---------------------------------------------------------------------------

pub const FEATURE_ENDPOINT_HALT: u16 = 0;
pub const FEATURE_DEVICE_REMOTE_WAKEUP: u16 = 1;
pub const FEATURE_TEST_MODE: u16 = 2;

// ---------------------------------------------------------------------------
// USB speeds
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbSpeed {
    Low,       // 1.5 Mbps (USB 1.0)
    Full,      // 12 Mbps (USB 1.1)
    High,      // 480 Mbps (USB 2.0)
    Super,     // 5 Gbps (USB 3.0)
    SuperPlus, // 10 Gbps (USB 3.1)
}

impl UsbSpeed {
    pub fn name(&self) -> &'static str {
        match self {
            UsbSpeed::Low => "low-speed (1.5 Mbps)",
            UsbSpeed::Full => "full-speed (12 Mbps)",
            UsbSpeed::High => "high-speed (480 Mbps)",
            UsbSpeed::Super => "super-speed (5 Gbps)",
            UsbSpeed::SuperPlus => "super-speed+ (10 Gbps)",
        }
    }

    /// Default max packet size for EP0 at this speed
    pub fn default_max_packet_size(&self) -> u16 {
        match self {
            UsbSpeed::Low => 8,
            UsbSpeed::Full => 64,
            UsbSpeed::High => 64,
            UsbSpeed::Super | UsbSpeed::SuperPlus => 512,
        }
    }
}

// ---------------------------------------------------------------------------
// USB device descriptor (18 bytes)
// ---------------------------------------------------------------------------

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DeviceDescriptor {
    pub length: u8,
    pub descriptor_type: u8,
    pub bcd_usb: u16,
    pub device_class: u8,
    pub device_subclass: u8,
    pub device_protocol: u8,
    pub max_packet_size_0: u8,
    pub vendor_id: u16,
    pub product_id: u16,
    pub bcd_device: u16,
    pub manufacturer_index: u8,
    pub product_index: u8,
    pub serial_index: u8,
    pub num_configurations: u8,
}

impl DeviceDescriptor {
    /// Parse from raw bytes
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 18 {
            return None;
        }
        if data[1] != DESC_DEVICE {
            return None;
        }
        Some(DeviceDescriptor {
            length: data[0],
            descriptor_type: data[1],
            bcd_usb: u16::from_le_bytes([data[2], data[3]]),
            device_class: data[4],
            device_subclass: data[5],
            device_protocol: data[6],
            max_packet_size_0: data[7],
            vendor_id: u16::from_le_bytes([data[8], data[9]]),
            product_id: u16::from_le_bytes([data[10], data[11]]),
            bcd_device: u16::from_le_bytes([data[12], data[13]]),
            manufacturer_index: data[14],
            product_index: data[15],
            serial_index: data[16],
            num_configurations: data[17],
        })
    }

    /// USB version as "X.Y" string
    pub fn usb_version_str(&self) -> &'static str {
        let bcd = self.bcd_usb;
        match bcd {
            0x0100 => "1.0",
            0x0110 => "1.1",
            0x0200 => "2.0",
            0x0201 => "2.01",
            0x0210 => "2.1",
            0x0300 => "3.0",
            0x0310 => "3.1",
            0x0320 => "3.2",
            _ => "unknown",
        }
    }

    /// Device class name
    pub fn class_name(&self) -> &'static str {
        match self.device_class {
            0x00 => "per-interface",
            0x01 => "audio",
            0x02 => "CDC",
            0x03 => "HID",
            0x05 => "physical",
            0x06 => "image",
            0x07 => "printer",
            0x08 => "mass-storage",
            0x09 => "hub",
            0x0A => "CDC-data",
            0x0B => "smart-card",
            0x0D => "content-security",
            0x0E => "video",
            0x0F => "personal-healthcare",
            0x10 => "audio/video",
            0xDC => "diagnostic",
            0xE0 => "wireless",
            0xEF => "miscellaneous",
            0xFE => "application-specific",
            0xFF => "vendor-specific",
            _ => "unknown",
        }
    }
}

// ---------------------------------------------------------------------------
// USB configuration descriptor (9 bytes header)
// ---------------------------------------------------------------------------

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ConfigDescriptor {
    pub length: u8,
    pub descriptor_type: u8,
    pub total_length: u16,
    pub num_interfaces: u8,
    pub configuration_value: u8,
    pub configuration_index: u8,
    pub attributes: u8,
    pub max_power: u8, // in 2mA units
}

impl ConfigDescriptor {
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 9 {
            return None;
        }
        if data[1] != DESC_CONFIGURATION {
            return None;
        }
        Some(ConfigDescriptor {
            length: data[0],
            descriptor_type: data[1],
            total_length: u16::from_le_bytes([data[2], data[3]]),
            num_interfaces: data[4],
            configuration_value: data[5],
            configuration_index: data[6],
            attributes: data[7],
            max_power: data[8],
        })
    }

    /// Self-powered?
    pub fn self_powered(&self) -> bool {
        self.attributes & 0x40 != 0
    }

    /// Remote wakeup supported?
    pub fn remote_wakeup(&self) -> bool {
        self.attributes & 0x20 != 0
    }

    /// Maximum power in milliamps
    pub fn max_power_ma(&self) -> u16 {
        self.max_power as u16 * 2
    }
}

// ---------------------------------------------------------------------------
// USB interface descriptor (9 bytes)
// ---------------------------------------------------------------------------

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct InterfaceDescriptor {
    pub length: u8,
    pub descriptor_type: u8,
    pub interface_number: u8,
    pub alternate_setting: u8,
    pub num_endpoints: u8,
    pub interface_class: u8,
    pub interface_subclass: u8,
    pub interface_protocol: u8,
    pub interface_index: u8,
}

impl InterfaceDescriptor {
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 9 {
            return None;
        }
        if data[1] != DESC_INTERFACE {
            return None;
        }
        Some(InterfaceDescriptor {
            length: data[0],
            descriptor_type: data[1],
            interface_number: data[2],
            alternate_setting: data[3],
            num_endpoints: data[4],
            interface_class: data[5],
            interface_subclass: data[6],
            interface_protocol: data[7],
            interface_index: data[8],
        })
    }
}

// ---------------------------------------------------------------------------
// USB endpoint descriptor (7 bytes)
// ---------------------------------------------------------------------------

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct EndpointDescriptor {
    pub length: u8,
    pub descriptor_type: u8,
    pub endpoint_address: u8,
    pub attributes: u8,
    pub max_packet_size: u16,
    pub interval: u8,
}

impl EndpointDescriptor {
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 7 {
            return None;
        }
        if data[1] != DESC_ENDPOINT {
            return None;
        }
        Some(EndpointDescriptor {
            length: data[0],
            descriptor_type: data[1],
            endpoint_address: data[2],
            attributes: data[3],
            max_packet_size: u16::from_le_bytes([data[4], data[5]]),
            interval: data[6],
        })
    }

    /// Endpoint number (0-15)
    pub fn number(&self) -> u8 {
        self.endpoint_address & 0x0F
    }

    /// Direction: true = IN (device to host)
    pub fn is_in(&self) -> bool {
        self.endpoint_address & 0x80 != 0
    }

    /// Transfer type
    pub fn transfer_type(&self) -> TransferType {
        match self.attributes & 0x03 {
            0 => TransferType::Control,
            1 => TransferType::Isochronous,
            2 => TransferType::Bulk,
            3 => TransferType::Interrupt,
            _ => TransferType::Control,
        }
    }

    /// For isochronous: synchronization type
    pub fn sync_type(&self) -> u8 {
        (self.attributes >> 2) & 0x03
    }

    /// For isochronous: usage type
    pub fn usage_type(&self) -> u8 {
        (self.attributes >> 4) & 0x03
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferType {
    Control,
    Isochronous,
    Bulk,
    Interrupt,
}

impl TransferType {
    pub fn name(&self) -> &'static str {
        match self {
            TransferType::Control => "control",
            TransferType::Isochronous => "isochronous",
            TransferType::Bulk => "bulk",
            TransferType::Interrupt => "interrupt",
        }
    }
}

// ---------------------------------------------------------------------------
// USB Interface Association Descriptor (8 bytes)
// ---------------------------------------------------------------------------

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct InterfaceAssociationDescriptor {
    pub length: u8,
    pub descriptor_type: u8,
    pub first_interface: u8,
    pub interface_count: u8,
    pub function_class: u8,
    pub function_subclass: u8,
    pub function_protocol: u8,
    pub function_index: u8,
}

impl InterfaceAssociationDescriptor {
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 8 {
            return None;
        }
        if data[1] != DESC_INTERFACE_ASSOCIATION {
            return None;
        }
        Some(InterfaceAssociationDescriptor {
            length: data[0],
            descriptor_type: data[1],
            first_interface: data[2],
            interface_count: data[3],
            function_class: data[4],
            function_subclass: data[5],
            function_protocol: data[6],
            function_index: data[7],
        })
    }
}

// ---------------------------------------------------------------------------
// USB device class codes
// ---------------------------------------------------------------------------

pub const CLASS_PER_INTERFACE: u8 = 0x00;
pub const CLASS_AUDIO: u8 = 0x01;
pub const CLASS_CDC: u8 = 0x02;
pub const CLASS_HID: u8 = 0x03;
pub const CLASS_PHYSICAL: u8 = 0x05;
pub const CLASS_IMAGE: u8 = 0x06;
pub const CLASS_PRINTER: u8 = 0x07;
pub const CLASS_MASS_STORAGE: u8 = 0x08;
pub const CLASS_HUB: u8 = 0x09;
pub const CLASS_CDC_DATA: u8 = 0x0A;
pub const CLASS_VIDEO: u8 = 0x0E;
pub const CLASS_WIRELESS: u8 = 0xE0;
pub const CLASS_MISC: u8 = 0xEF;
pub const CLASS_VENDOR_SPECIFIC: u8 = 0xFF;

// ---------------------------------------------------------------------------
// USB setup packet (8 bytes)
// ---------------------------------------------------------------------------

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct SetupPacket {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

impl SetupPacket {
    /// Construct a raw setup packet
    pub fn new(request_type: u8, request: u8, value: u16, index: u16, length: u16) -> Self {
        SetupPacket {
            request_type,
            request,
            value,
            index,
            length,
        }
    }

    /// GET_DESCRIPTOR request
    pub fn get_descriptor(desc_type: u8, index: u8, length: u16) -> Self {
        SetupPacket {
            request_type: 0x80, // Device-to-Host, Standard, Device
            request: REQ_GET_DESCRIPTOR,
            value: ((desc_type as u16) << 8) | index as u16,
            index: 0,
            length,
        }
    }

    /// GET_DESCRIPTOR for string descriptor (with language ID)
    pub fn get_string_descriptor(index: u8, lang_id: u16, length: u16) -> Self {
        SetupPacket {
            request_type: 0x80,
            request: REQ_GET_DESCRIPTOR,
            value: ((DESC_STRING as u16) << 8) | index as u16,
            index: lang_id,
            length,
        }
    }

    /// SET_ADDRESS request
    pub fn set_address(address: u8) -> Self {
        SetupPacket {
            request_type: 0x00,
            request: REQ_SET_ADDRESS,
            value: address as u16,
            index: 0,
            length: 0,
        }
    }

    /// SET_CONFIGURATION request
    pub fn set_configuration(config: u8) -> Self {
        SetupPacket {
            request_type: 0x00,
            request: REQ_SET_CONFIGURATION,
            value: config as u16,
            index: 0,
            length: 0,
        }
    }

    /// GET_CONFIGURATION request
    pub fn get_configuration() -> Self {
        SetupPacket {
            request_type: 0x80,
            request: REQ_GET_CONFIGURATION,
            value: 0,
            index: 0,
            length: 1,
        }
    }

    /// SET_INTERFACE request
    pub fn set_interface(interface: u16, alternate: u16) -> Self {
        SetupPacket {
            request_type: 0x01,
            request: REQ_SET_INTERFACE,
            value: alternate,
            index: interface,
            length: 0,
        }
    }

    /// CLEAR_FEATURE request (endpoint halt)
    pub fn clear_endpoint_halt(endpoint: u8) -> Self {
        SetupPacket {
            request_type: 0x02, // Host-to-Device, Standard, Endpoint
            request: REQ_CLEAR_FEATURE,
            value: FEATURE_ENDPOINT_HALT,
            index: endpoint as u16,
            length: 0,
        }
    }

    /// GET_STATUS request (device)
    pub fn get_device_status() -> Self {
        SetupPacket {
            request_type: 0x80,
            request: REQ_GET_STATUS,
            value: 0,
            index: 0,
            length: 2,
        }
    }

    /// Serialize to 8-byte array
    pub fn to_bytes(&self) -> [u8; 8] {
        let val = self.value.to_le_bytes();
        let idx = self.index.to_le_bytes();
        let len = self.length.to_le_bytes();
        [
            self.request_type,
            self.request,
            val[0],
            val[1],
            idx[0],
            idx[1],
            len[0],
            len[1],
        ]
    }
}

// ---------------------------------------------------------------------------
// String descriptor parsing
// ---------------------------------------------------------------------------

/// Parse a USB string descriptor (UTF-16LE) into a Rust String.
/// The first 2 bytes are length and descriptor type.
pub fn parse_string_descriptor(data: &[u8]) -> Option<String> {
    if data.len() < 2 {
        return None;
    }
    let len = data[0] as usize;
    if data[1] != DESC_STRING {
        return None;
    }
    if len < 2 || len > data.len() {
        return None;
    }

    // Decode UTF-16LE code units (skip first 2 bytes)
    let utf16_bytes = &data[2..len];
    let mut s = String::new();
    let mut i = 0;
    while i + 1 < utf16_bytes.len() {
        let code_unit = u16::from_le_bytes([utf16_bytes[i], utf16_bytes[i + 1]]);
        if let Some(c) = char::from_u32(code_unit as u32) {
            s.push(c);
        }
        i += 2;
    }
    Some(s)
}

/// Get supported language IDs from string descriptor 0.
pub fn parse_language_ids(data: &[u8]) -> Vec<u16> {
    let mut langs = Vec::new();
    if data.len() < 4 || data[1] != DESC_STRING {
        return langs;
    }
    let len = data[0] as usize;
    let mut i = 2;
    while i + 1 < len && i + 1 < data.len() {
        let lang = u16::from_le_bytes([data[i], data[i + 1]]);
        langs.push(lang);
        i += 2;
    }
    langs
}

// ---------------------------------------------------------------------------
// Configuration descriptor walker
// ---------------------------------------------------------------------------

/// Iterates through a complete configuration descriptor blob,
/// yielding individual descriptors (interface, endpoint, class-specific, etc.)
pub struct ConfigWalker<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> ConfigWalker<'a> {
    /// Create a walker from a complete configuration descriptor blob
    pub fn new(data: &'a [u8]) -> Self {
        // Skip the config descriptor header (first 9 bytes)
        let start = if data.len() >= 9 && data[1] == DESC_CONFIGURATION {
            data[0] as usize
        } else {
            0
        };
        ConfigWalker {
            data,
            offset: start,
        }
    }

    /// Create a walker that starts from the beginning (no skip)
    pub fn new_raw(data: &'a [u8]) -> Self {
        ConfigWalker { data, offset: 0 }
    }
}

impl<'a> Iterator for ConfigWalker<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.data.len() {
            return None;
        }
        let len = self.data[self.offset] as usize;
        if len < 2 {
            return None;
        } // Invalid descriptor
        let end = self.offset + len;
        if end > self.data.len() {
            return None;
        }
        let desc = &self.data[self.offset..end];
        self.offset = end;
        Some(desc)
    }
}

// ---------------------------------------------------------------------------
// Interface classification
// ---------------------------------------------------------------------------

/// Determine which class driver to use for an interface
pub fn classify_interface(class: u8, _subclass: u8, _protocol: u8) -> &'static str {
    match class {
        CLASS_AUDIO => "audio",
        CLASS_CDC => "cdc",
        CLASS_HID => "hid",
        CLASS_PHYSICAL => "physical",
        CLASS_IMAGE => "image",
        CLASS_PRINTER => "printer",
        CLASS_MASS_STORAGE => "mass-storage",
        CLASS_HUB => "hub",
        CLASS_CDC_DATA => "cdc-data",
        CLASS_VIDEO => "video",
        CLASS_WIRELESS => "wireless",
        CLASS_VENDOR_SPECIFIC => "vendor-specific",
        _ => "unknown",
    }
}

/// Get the human-readable name for a USB class code
pub fn class_name(class: u8) -> &'static str {
    classify_interface(class, 0, 0)
}

// ---------------------------------------------------------------------------
// Enumeration helpers
// ---------------------------------------------------------------------------

/// Walk a complete configuration descriptor blob (all interfaces + endpoints)
/// and invoke `cb` for each interface descriptor found, passing the slice of
/// the interface descriptor and all its endpoint descriptors that follow it
/// before the next interface descriptor.
///
/// This is the standard pattern for matching class drivers after the host
/// has retrieved the full configuration descriptor.
pub fn walk_config_interfaces<F>(config_blob: &[u8], mut cb: F)
where
    F: FnMut(&InterfaceDescriptor, &[EndpointDescriptor]),
{
    // Collect all descriptors first via ConfigWalker, then group by interface.
    let mut current_iface: Option<InterfaceDescriptor> = None;
    let mut current_eps: Vec<EndpointDescriptor> = Vec::new();

    for desc in ConfigWalker::new(config_blob) {
        if desc.len() < 2 {
            continue;
        }
        let desc_type = desc[1];
        match desc_type {
            DESC_INTERFACE => {
                // Flush the previous interface to the callback before starting a new one.
                if let Some(iface) = current_iface.take() {
                    cb(&iface, &current_eps);
                }
                current_eps.clear();
                if let Some(iface) = InterfaceDescriptor::from_bytes(desc) {
                    current_iface = Some(iface);
                }
            }
            DESC_ENDPOINT => {
                if let Some(ep) = EndpointDescriptor::from_bytes(desc) {
                    current_eps.push(ep);
                }
            }
            _ => {}
        }
    }
    // Flush final interface.
    if let Some(iface) = current_iface {
        cb(&iface, &current_eps);
    }
}

/// Given a device descriptor, decide the correct class/subclass/protocol to
/// use for driver matching.  For composite devices (class 0x00) the returned
/// triple is (0,0,0) signalling per-interface dispatch; for single-function
/// devices the device-level class is returned directly.
pub fn resolve_device_class(desc: &DeviceDescriptor) -> (u8, u8, u8) {
    if desc.device_class == CLASS_PER_INTERFACE {
        (0, 0, 0) // caller must walk each interface descriptor
    } else {
        (
            desc.device_class,
            desc.device_subclass,
            desc.device_protocol,
        )
    }
}

/// Locate the first IN interrupt endpoint in an endpoint slice (used by HID,
/// Hub interrupt-driven polling).
pub fn find_interrupt_in_ep(eps: &[EndpointDescriptor]) -> Option<&EndpointDescriptor> {
    eps.iter()
        .find(|ep| ep.is_in() && ep.transfer_type() == TransferType::Interrupt)
}

/// Locate bulk IN and bulk OUT endpoints in an endpoint slice (Mass Storage /
/// CDC data interface).  Returns (bulk_in, bulk_out).
pub fn find_bulk_eps(
    eps: &[EndpointDescriptor],
) -> (Option<&EndpointDescriptor>, Option<&EndpointDescriptor>) {
    let bulk_in = eps
        .iter()
        .find(|ep| ep.is_in() && ep.transfer_type() == TransferType::Bulk);
    let bulk_out = eps
        .iter()
        .find(|ep| !ep.is_in() && ep.transfer_type() == TransferType::Bulk);
    (bulk_in, bulk_out)
}

/// Build a complete GET_DESCRIPTOR (string, language 0x0409 English) setup
/// packet for the given string index.
pub fn build_get_string_setup(index: u8, max_len: u16) -> [u8; 8] {
    let len = max_len.to_le_bytes();
    [
        0x80, // bmRequestType: Device-to-Host, Standard, Device
        REQ_GET_DESCRIPTOR,
        index,       // wValue low: string index
        DESC_STRING, // wValue high: descriptor type
        0x09,
        0x04, // wIndex: language ID 0x0409 (English US)
        len[0],
        len[1], // wLength
    ]
}

/// Parse a USB BCD version (e.g. 0x0200) into "major.minor" components.
pub fn bcd_to_version(bcd: u16) -> (u8, u8) {
    let major = ((bcd >> 8) & 0xFF) as u8;
    let minor = (bcd & 0xFF) as u8;
    (major, minor)
}
