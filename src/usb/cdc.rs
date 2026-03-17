use crate::sync::Mutex;
/// USB Communications Device Class (CDC) driver
///
/// Supports CDC ACM (Abstract Control Model) for virtual serial ports,
/// CDC Ethernet (ECM/NCM) for USB networking, and modem control.
/// Handles bulk transfers, line coding, and serial state notifications.
///
/// References: USB CDC 1.2, CDC ACM, CDC ECM/NCM specifications.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static CDC_STATE: Mutex<Option<CdcClassState>> = Mutex::new(None);

pub struct CdcClassState {
    pub devices: Vec<CdcDevice>,
    pub next_device_id: u32,
}

impl CdcClassState {
    pub fn new() -> Self {
        CdcClassState {
            devices: Vec::new(),
            next_device_id: 1,
        }
    }

    pub fn register(&mut self, dev: CdcDevice) -> u32 {
        let id = self.next_device_id;
        self.next_device_id = self.next_device_id.saturating_add(1);
        self.devices.push(dev);
        id
    }
}

// ---------------------------------------------------------------------------
// CDC constants
// ---------------------------------------------------------------------------

pub const CLASS_CDC: u8 = 0x02;
pub const CLASS_CDC_DATA: u8 = 0x0A;

/// CDC subclass codes.
pub const SUBCLASS_ACM: u8 = 0x02;
pub const SUBCLASS_ETHERNET: u8 = 0x06;
pub const SUBCLASS_NCM: u8 = 0x0D;

/// CDC protocol codes.
pub const PROTOCOL_NONE: u8 = 0x00;
pub const PROTOCOL_AT_COMMANDS: u8 = 0x01; // V.250 AT commands
pub const PROTOCOL_VENDOR: u8 = 0xFF;

/// CDC class-specific descriptor types.
pub const CS_INTERFACE: u8 = 0x24;

/// CDC functional descriptor subtypes.
pub const CDC_HEADER: u8 = 0x00;
pub const CDC_CALL_MANAGEMENT: u8 = 0x01;
pub const CDC_ACM_DESCRIPTOR: u8 = 0x02;
pub const CDC_UNION: u8 = 0x06;
pub const CDC_ETHERNET: u8 = 0x0F;

/// CDC class-specific request codes.
pub const SET_LINE_CODING: u8 = 0x20;
pub const GET_LINE_CODING: u8 = 0x21;
pub const SET_CONTROL_LINE_STATE: u8 = 0x22;
pub const SEND_BREAK: u8 = 0x23;
pub const SET_ETHERNET_MULTICAST: u8 = 0x40;
pub const SET_ETHERNET_PACKET_FILTER: u8 = 0x43;

/// Serial state notification bits (from interrupt endpoint).
pub const SERIAL_STATE_DCD: u16 = 0x0001; // Data Carrier Detect
pub const SERIAL_STATE_DSR: u16 = 0x0002; // Data Set Ready
pub const SERIAL_STATE_BREAK: u16 = 0x0004;
pub const SERIAL_STATE_RI: u16 = 0x0008; // Ring Indicator
pub const SERIAL_STATE_FRAMING: u16 = 0x0010;
pub const SERIAL_STATE_PARITY: u16 = 0x0020;
pub const SERIAL_STATE_OVERRUN: u16 = 0x0040;

// ---------------------------------------------------------------------------
// CDC device types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdcType {
    Acm,      // Abstract Control Model (virtual serial)
    Ethernet, // CDC ECM (Ethernet Control Model)
    Ncm,      // Network Control Model
    Modem,    // AT-command modem
    Unknown(u8),
}

// ---------------------------------------------------------------------------
// Line coding (serial port parameters)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopBits {
    One,
    OnePointFive,
    Two,
}

impl StopBits {
    pub fn from_raw(v: u8) -> Self {
        match v {
            0 => StopBits::One,
            1 => StopBits::OnePointFive,
            2 => StopBits::Two,
            _ => StopBits::One,
        }
    }
    pub fn to_raw(self) -> u8 {
        match self {
            StopBits::One => 0,
            StopBits::OnePointFive => 1,
            StopBits::Two => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parity {
    None,
    Odd,
    Even,
    Mark,
    Space,
}

impl Parity {
    pub fn from_raw(v: u8) -> Self {
        match v {
            0 => Parity::None,
            1 => Parity::Odd,
            2 => Parity::Even,
            3 => Parity::Mark,
            4 => Parity::Space,
            _ => Parity::None,
        }
    }
    pub fn to_raw(self) -> u8 {
        match self {
            Parity::None => 0,
            Parity::Odd => 1,
            Parity::Even => 2,
            Parity::Mark => 3,
            Parity::Space => 4,
        }
    }
}

/// Line coding structure (7 bytes, packed as per CDC spec).
#[derive(Debug, Clone, Copy)]
pub struct LineCoding {
    pub baud_rate: u32,
    pub stop_bits: StopBits,
    pub parity: Parity,
    pub data_bits: u8,
}

impl LineCoding {
    pub fn default_115200() -> Self {
        LineCoding {
            baud_rate: 115200,
            stop_bits: StopBits::One,
            parity: Parity::None,
            data_bits: 8,
        }
    }

    pub fn default_9600() -> Self {
        LineCoding {
            baud_rate: 9600,
            stop_bits: StopBits::One,
            parity: Parity::None,
            data_bits: 8,
        }
    }

    /// Serialize to 7 bytes for SET_LINE_CODING request.
    pub fn to_bytes(&self) -> [u8; 7] {
        let b = self.baud_rate;
        [
            (b & 0xFF) as u8,
            ((b >> 8) & 0xFF) as u8,
            ((b >> 16) & 0xFF) as u8,
            ((b >> 24) & 0xFF) as u8,
            self.stop_bits.to_raw(),
            self.parity.to_raw(),
            self.data_bits,
        ]
    }

    /// Parse from 7 bytes (GET_LINE_CODING response).
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 7 {
            return None;
        }
        Some(LineCoding {
            baud_rate: (data[0] as u32)
                | ((data[1] as u32) << 8)
                | ((data[2] as u32) << 16)
                | ((data[3] as u32) << 24),
            stop_bits: StopBits::from_raw(data[4]),
            parity: Parity::from_raw(data[5]),
            data_bits: data[6],
        })
    }
}

// ---------------------------------------------------------------------------
// Control line state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct ControlLineState {
    pub dtr: bool, // Data Terminal Ready
    pub rts: bool, // Request To Send
}

impl ControlLineState {
    pub fn new() -> Self {
        ControlLineState {
            dtr: false,
            rts: false,
        }
    }

    pub fn active() -> Self {
        ControlLineState {
            dtr: true,
            rts: true,
        }
    }

    /// Encode for SET_CONTROL_LINE_STATE wValue.
    pub fn to_value(&self) -> u16 {
        let mut v: u16 = 0;
        if self.dtr {
            v |= 0x01;
        }
        if self.rts {
            v |= 0x02;
        }
        v
    }
}

// ---------------------------------------------------------------------------
// Ethernet-specific
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CdcEthernetInfo {
    pub mac_address: [u8; 6],
    pub max_segment_size: u16,
    pub multicast_filters: u16,
    pub power_filters: u8,
    pub packet_filter: u16,
}

impl CdcEthernetInfo {
    pub fn new() -> Self {
        CdcEthernetInfo {
            mac_address: [0; 6],
            max_segment_size: 1514,
            multicast_filters: 0,
            power_filters: 0,
            packet_filter: 0x000F, // directed + multicast + broadcast + all-multicast
        }
    }

    /// Parse CDC Ethernet Networking Functional Descriptor.
    pub fn parse_descriptor(&mut self, data: &[u8]) {
        if data.len() < 13 {
            return;
        }
        // data[3] = iMACAddress (string descriptor index, needs separate fetch)
        // data[4..8] = bmEthernetStatistics (4 bytes)
        self.max_segment_size = (data[8] as u16) | ((data[9] as u16) << 8);
        self.multicast_filters = (data[10] as u16) | ((data[11] as u16) << 8);
        self.power_filters = data[12];
    }

    /// Build SET_ETHERNET_PACKET_FILTER value.
    pub fn filter_value(&self) -> u16 {
        self.packet_filter
    }
}

// ---------------------------------------------------------------------------
// Ring buffer for serial data
// ---------------------------------------------------------------------------

pub struct SerialRingBuffer {
    pub buf: Vec<u8>,
    pub head: usize,
    pub tail: usize,
    pub capacity: usize,
}

impl SerialRingBuffer {
    pub fn new(capacity: usize) -> Self {
        SerialRingBuffer {
            buf: alloc::vec![0u8; capacity],
            head: 0,
            tail: 0,
            capacity,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    pub fn is_full(&self) -> bool {
        (self.head + 1) % self.capacity == self.tail
    }

    pub fn available(&self) -> usize {
        if self.head >= self.tail {
            self.head - self.tail
        } else {
            self.capacity - self.tail + self.head
        }
    }

    pub fn free_space(&self) -> usize {
        self.capacity - 1 - self.available()
    }

    pub fn push(&mut self, byte: u8) -> bool {
        if self.is_full() {
            return false;
        }
        self.buf[self.head] = byte;
        self.head = (self.head + 1) % self.capacity;
        true
    }

    pub fn pop(&mut self) -> Option<u8> {
        if self.is_empty() {
            return None;
        }
        let byte = self.buf[self.tail];
        self.tail = (self.tail + 1) % self.capacity;
        Some(byte)
    }

    pub fn write_bulk(&mut self, data: &[u8]) -> usize {
        let mut written = 0;
        for &b in data {
            if !self.push(b) {
                break;
            }
            written += 1;
        }
        written
    }

    pub fn read_bulk(&mut self, out: &mut [u8]) -> usize {
        let mut count = 0;
        for slot in out.iter_mut() {
            if let Some(b) = self.pop() {
                *slot = b;
                count += 1;
            } else {
                break;
            }
        }
        count
    }
}

// ---------------------------------------------------------------------------
// CDC device
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdcDeviceState {
    Detached,
    Attached,
    Configured,
    Active,
    Error,
}

pub struct CdcDevice {
    pub slot_id: u8,
    pub cdc_type: CdcType,
    pub state: CdcDeviceState,
    pub control_interface: u8,
    pub data_interface: u8,
    pub bulk_in_ep: u8,
    pub bulk_out_ep: u8,
    pub interrupt_ep: u8,
    pub line_coding: LineCoding,
    pub control_line: ControlLineState,
    pub serial_state: u16,
    pub ethernet: Option<CdcEthernetInfo>,
    pub rx_buffer: SerialRingBuffer,
    pub tx_buffer: SerialRingBuffer,
}

impl CdcDevice {
    pub fn new_acm(slot_id: u8, bulk_in: u8, bulk_out: u8, interrupt: u8) -> Self {
        CdcDevice {
            slot_id,
            cdc_type: CdcType::Acm,
            state: CdcDeviceState::Attached,
            control_interface: 0,
            data_interface: 1,
            bulk_in_ep: bulk_in,
            bulk_out_ep: bulk_out,
            interrupt_ep: interrupt,
            line_coding: LineCoding::default_115200(),
            control_line: ControlLineState::new(),
            serial_state: 0,
            ethernet: None,
            rx_buffer: SerialRingBuffer::new(4096),
            tx_buffer: SerialRingBuffer::new(4096),
        }
    }

    pub fn new_ethernet(slot_id: u8, bulk_in: u8, bulk_out: u8, interrupt: u8) -> Self {
        CdcDevice {
            slot_id,
            cdc_type: CdcType::Ethernet,
            state: CdcDeviceState::Attached,
            control_interface: 0,
            data_interface: 1,
            bulk_in_ep: bulk_in,
            bulk_out_ep: bulk_out,
            interrupt_ep: interrupt,
            line_coding: LineCoding::default_115200(),
            control_line: ControlLineState::new(),
            serial_state: 0,
            ethernet: Some(CdcEthernetInfo::new()),
            rx_buffer: SerialRingBuffer::new(8192),
            tx_buffer: SerialRingBuffer::new(8192),
        }
    }

    // ----- descriptor parsing -----

    /// Parse a CDC Union Functional Descriptor.
    pub fn parse_union_descriptor(&mut self, data: &[u8]) {
        if data.len() < 5 {
            return;
        }
        self.control_interface = data[3];
        self.data_interface = data[4];
    }

    /// Parse a CDC Header Functional Descriptor.
    pub fn parse_header_descriptor(&self, data: &[u8]) -> Option<u16> {
        if data.len() < 5 {
            return None;
        }
        let bcd_cdc = (data[3] as u16) | ((data[4] as u16) << 8);
        Some(bcd_cdc)
    }

    /// Parse a CDC ACM Functional Descriptor.
    pub fn parse_acm_capabilities(&self, data: &[u8]) -> u8 {
        if data.len() < 4 {
            return 0;
        }
        data[3] // bmCapabilities
    }

    // ----- serial control -----

    /// Activate the serial port (DTR + RTS high).
    pub fn activate(&mut self) {
        self.control_line = ControlLineState::active();
        self.state = CdcDeviceState::Active;
    }

    /// Deactivate the serial port.
    pub fn deactivate(&mut self) {
        self.control_line = ControlLineState::new();
        if self.state == CdcDeviceState::Active {
            self.state = CdcDeviceState::Configured;
        }
    }

    /// Process a serial state notification from the interrupt endpoint.
    pub fn process_serial_notification(&mut self, data: &[u8]) {
        // Notification: [bmRequestType, SERIAL_STATE, wValue(2), wIndex(2), wLength(2), data(2)]
        if data.len() < 10 {
            return;
        }
        self.serial_state = (data[8] as u16) | ((data[9] as u16) << 8);
    }

    /// Check Data Carrier Detect.
    pub fn carrier_detect(&self) -> bool {
        self.serial_state & SERIAL_STATE_DCD != 0
    }

    /// Check Data Set Ready.
    pub fn data_set_ready(&self) -> bool {
        self.serial_state & SERIAL_STATE_DSR != 0
    }

    // ----- data transfer -----

    /// Queue data for transmission via the bulk OUT endpoint.
    pub fn write(&mut self, data: &[u8]) -> usize {
        self.tx_buffer.write_bulk(data)
    }

    /// Read received data from the bulk IN buffer.
    pub fn read(&mut self, out: &mut [u8]) -> usize {
        self.rx_buffer.read_bulk(out)
    }

    /// Accept data received from the bulk IN endpoint.
    pub fn receive_bulk(&mut self, data: &[u8]) -> usize {
        self.rx_buffer.write_bulk(data)
    }

    /// Get pending transmit data for the bulk OUT endpoint.
    pub fn pending_tx(&mut self, out: &mut [u8]) -> usize {
        self.tx_buffer.read_bulk(out)
    }
}

// ---------------------------------------------------------------------------
// Class identification
// ---------------------------------------------------------------------------

pub fn is_cdc_acm(class: u8, subclass: u8) -> bool {
    class == CLASS_CDC && subclass == SUBCLASS_ACM
}

pub fn is_cdc_ethernet(class: u8, subclass: u8) -> bool {
    class == CLASS_CDC && subclass == SUBCLASS_ETHERNET
}

pub fn is_cdc_ncm(class: u8, subclass: u8) -> bool {
    class == CLASS_CDC && subclass == SUBCLASS_NCM
}

pub fn is_cdc_data(class: u8) -> bool {
    class == CLASS_CDC_DATA
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    let mut state = CDC_STATE.lock();
    *state = Some(CdcClassState::new());
    serial_println!("    [cdc] USB CDC driver loaded (ACM, Ethernet)");
}
