use crate::sync::Mutex;
/// USB gadget mode (act as USB device)
///
/// Part of the AIOS hardware layer.
/// Implements USB device-side (gadget) controller support,
/// allowing the system to present itself as a USB device to a host.
/// Supports mass storage, serial (CDC-ACM), Ethernet (RNDIS/ECM),
/// and HID function types with descriptor management and endpoint handling.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Gadget function type
#[derive(Clone, Copy, PartialEq)]
pub enum GadgetFunction {
    MassStorage,
    Serial,
    Ethernet,
    Hid,
}

/// USB device speed
#[derive(Clone, Copy, PartialEq)]
pub enum UsbSpeed {
    Low,   // 1.5 Mbps
    Full,  // 12 Mbps
    High,  // 480 Mbps
    Super, // 5 Gbps
}

/// Endpoint direction
#[derive(Clone, Copy, PartialEq)]
pub enum EndpointDirection {
    In,
    Out,
}

/// Endpoint transfer type
#[derive(Clone, Copy, PartialEq)]
pub enum TransferType {
    Control,
    Bulk,
    Interrupt,
    Isochronous,
}

/// USB endpoint descriptor (simplified)
#[derive(Clone, Copy)]
pub struct EndpointDescriptor {
    pub address: u8,
    pub direction: EndpointDirection,
    pub transfer_type: TransferType,
    pub max_packet_size: u16,
    pub interval: u8,
    pub active: bool,
}

impl EndpointDescriptor {
    fn new(
        address: u8,
        direction: EndpointDirection,
        transfer_type: TransferType,
        max_packet_size: u16,
    ) -> Self {
        EndpointDescriptor {
            address,
            direction,
            transfer_type,
            max_packet_size,
            interval: 0,
            active: false,
        }
    }
}

/// USB device descriptor fields
#[derive(Clone, Copy)]
struct DeviceDescriptor {
    vendor_id: u16,
    product_id: u16,
    device_class: u8,
    device_subclass: u8,
    device_protocol: u8,
    max_packet_size_ep0: u8,
    bcd_device: u16,
}

impl DeviceDescriptor {
    fn for_function(func: &GadgetFunction) -> Self {
        match func {
            GadgetFunction::MassStorage => DeviceDescriptor {
                vendor_id: 0x1D6B,  // Linux Foundation
                product_id: 0x0104, // Mass Storage Gadget
                device_class: 0x00,
                device_subclass: 0x00,
                device_protocol: 0x00,
                max_packet_size_ep0: 64,
                bcd_device: 0x0100,
            },
            GadgetFunction::Serial => DeviceDescriptor {
                vendor_id: 0x1D6B,
                product_id: 0x0103, // Serial Gadget
                device_class: 0x02, // CDC
                device_subclass: 0x02,
                device_protocol: 0x01,
                max_packet_size_ep0: 64,
                bcd_device: 0x0100,
            },
            GadgetFunction::Ethernet => DeviceDescriptor {
                vendor_id: 0x1D6B,
                product_id: 0x0105, // RNDIS Gadget
                device_class: 0xEF, // Miscellaneous
                device_subclass: 0x02,
                device_protocol: 0x01,
                max_packet_size_ep0: 64,
                bcd_device: 0x0100,
            },
            GadgetFunction::Hid => DeviceDescriptor {
                vendor_id: 0x1D6B,
                product_id: 0x0106, // HID Gadget
                device_class: 0x03, // HID
                device_subclass: 0x00,
                device_protocol: 0x00,
                max_packet_size_ep0: 64,
                bcd_device: 0x0100,
            },
        }
    }

    /// Serialize the device descriptor to 18-byte USB format
    fn serialize(&self) -> [u8; 18] {
        let mut buf = [0u8; 18];
        buf[0] = 18; // bLength
        buf[1] = 0x01; // bDescriptorType = DEVICE
        buf[2] = 0x00; // bcdUSB low
        buf[3] = 0x02; // bcdUSB high (USB 2.0)
        buf[4] = self.device_class;
        buf[5] = self.device_subclass;
        buf[6] = self.device_protocol;
        buf[7] = self.max_packet_size_ep0;
        buf[8] = (self.vendor_id & 0xFF) as u8;
        buf[9] = (self.vendor_id >> 8) as u8;
        buf[10] = (self.product_id & 0xFF) as u8;
        buf[11] = (self.product_id >> 8) as u8;
        buf[12] = (self.bcd_device & 0xFF) as u8;
        buf[13] = (self.bcd_device >> 8) as u8;
        buf[14] = 1; // iManufacturer string index
        buf[15] = 2; // iProduct string index
        buf[16] = 3; // iSerialNumber string index
        buf[17] = 1; // bNumConfigurations
        buf
    }
}

/// Gadget controller state machine
#[derive(Clone, Copy, PartialEq)]
enum GadgetState {
    Disabled,
    Powered,
    Default,
    Addressed,
    Configured,
    Suspended,
}

/// TX/RX data buffers for endpoints
struct EndpointBuffer {
    tx_buf: Vec<u8>,
    rx_buf: Vec<u8>,
    tx_pending: usize,
    rx_available: usize,
}

impl EndpointBuffer {
    fn new(capacity: usize) -> Self {
        EndpointBuffer {
            tx_buf: Vec::with_capacity(capacity),
            rx_buf: Vec::with_capacity(capacity),
            tx_pending: 0,
            rx_available: 0,
        }
    }

    fn write(&mut self, data: &[u8]) -> usize {
        let space = 4096usize.saturating_sub(self.tx_buf.len());
        let to_write = data.len().min(space);
        self.tx_buf.extend_from_slice(&data[..to_write]);
        self.tx_pending += to_write;
        to_write
    }

    fn read(&mut self, buf: &mut [u8]) -> usize {
        let available = self.rx_buf.len().min(buf.len());
        buf[..available].copy_from_slice(&self.rx_buf[..available]);
        // Remove consumed bytes
        let remaining = self.rx_buf.split_off(available);
        self.rx_buf = remaining;
        self.rx_available = self.rx_buf.len();
        available
    }

    fn flush_tx(&mut self) -> usize {
        let flushed = self.tx_buf.len();
        self.tx_buf.clear();
        self.tx_pending = 0;
        flushed
    }
}

/// USB gadget controller
pub struct UsbGadget {
    pub function: GadgetFunction,
    pub connected: bool,
    /// Device descriptor
    descriptor: DeviceDescriptor,
    /// Controller state
    state: GadgetState,
    /// Negotiated USB speed
    speed: UsbSpeed,
    /// Endpoints (EP0 control + function endpoints)
    endpoints: Vec<EndpointDescriptor>,
    /// Endpoint data buffers (indexed by endpoint address)
    buffers: Vec<EndpointBuffer>,
    /// USB address assigned by host
    device_address: u8,
    /// Current configuration value
    configuration: u8,
    /// VBUS detected
    vbus_present: bool,
    /// Total bytes transferred
    bytes_tx: u64,
    bytes_rx: u64,
    /// Setup request counter
    setup_requests: u64,
    /// Stall count
    stall_count: u32,
}

static GADGET: Mutex<Option<UsbGadget>> = Mutex::new(None);

impl UsbGadget {
    pub fn new(function: GadgetFunction) -> Self {
        let descriptor = DeviceDescriptor::for_function(&function);
        let mut endpoints = Vec::new();
        let mut buffers = Vec::new();

        // EP0 control endpoint (always present)
        endpoints.push(EndpointDescriptor::new(
            0,
            EndpointDirection::In,
            TransferType::Control,
            64,
        ));
        buffers.push(EndpointBuffer::new(64));

        // Function-specific endpoints
        match function {
            GadgetFunction::MassStorage => {
                // Bulk IN (EP1) and Bulk OUT (EP2) for SCSI commands/data
                endpoints.push(EndpointDescriptor::new(
                    1,
                    EndpointDirection::In,
                    TransferType::Bulk,
                    512,
                ));
                endpoints.push(EndpointDescriptor::new(
                    2,
                    EndpointDirection::Out,
                    TransferType::Bulk,
                    512,
                ));
                buffers.push(EndpointBuffer::new(512));
                buffers.push(EndpointBuffer::new(512));
            }
            GadgetFunction::Serial => {
                // Bulk IN/OUT for data, Interrupt IN for notifications
                endpoints.push(EndpointDescriptor::new(
                    1,
                    EndpointDirection::In,
                    TransferType::Bulk,
                    512,
                ));
                endpoints.push(EndpointDescriptor::new(
                    2,
                    EndpointDirection::Out,
                    TransferType::Bulk,
                    512,
                ));
                let mut notify =
                    EndpointDescriptor::new(3, EndpointDirection::In, TransferType::Interrupt, 8);
                notify.interval = 10;
                endpoints.push(notify);
                buffers.push(EndpointBuffer::new(512));
                buffers.push(EndpointBuffer::new(512));
                buffers.push(EndpointBuffer::new(8));
            }
            GadgetFunction::Ethernet => {
                // Bulk IN/OUT for network frames, Interrupt IN for status
                endpoints.push(EndpointDescriptor::new(
                    1,
                    EndpointDirection::In,
                    TransferType::Bulk,
                    512,
                ));
                endpoints.push(EndpointDescriptor::new(
                    2,
                    EndpointDirection::Out,
                    TransferType::Bulk,
                    512,
                ));
                let mut status =
                    EndpointDescriptor::new(3, EndpointDirection::In, TransferType::Interrupt, 16);
                status.interval = 32;
                endpoints.push(status);
                buffers.push(EndpointBuffer::new(1536)); // MTU-sized
                buffers.push(EndpointBuffer::new(1536));
                buffers.push(EndpointBuffer::new(16));
            }
            GadgetFunction::Hid => {
                // Interrupt IN for HID reports
                let mut report_in =
                    EndpointDescriptor::new(1, EndpointDirection::In, TransferType::Interrupt, 64);
                report_in.interval = 1; // 1ms polling
                endpoints.push(report_in);
                buffers.push(EndpointBuffer::new(64));
            }
        }

        UsbGadget {
            function,
            connected: false,
            descriptor,
            state: GadgetState::Disabled,
            speed: UsbSpeed::High,
            endpoints,
            buffers,
            device_address: 0,
            configuration: 0,
            vbus_present: false,
            bytes_tx: 0,
            bytes_rx: 0,
            setup_requests: 0,
            stall_count: 0,
        }
    }

    pub fn enable(&mut self) -> Result<(), ()> {
        if self.state != GadgetState::Disabled {
            serial_println!("    [usb-gadget] already enabled");
            return Err(());
        }

        // Simulate enabling the device controller
        // In real hardware: configure UDC registers, enable pull-up
        self.state = GadgetState::Powered;
        self.vbus_present = true;

        // Activate all endpoints
        for ep in &mut self.endpoints {
            ep.active = true;
        }

        serial_println!(
            "    [usb-gadget] enabled as {:?} function, {} endpoints",
            function_name(&self.function),
            self.endpoints.len()
        );

        // Simulate bus reset and address assignment
        self.state = GadgetState::Default;
        self.device_address = 0;

        // Simulate SET_ADDRESS
        self.device_address = 1; // Host would assign this
        self.state = GadgetState::Addressed;

        // Simulate SET_CONFIGURATION
        self.configuration = 1;
        self.state = GadgetState::Configured;
        self.connected = true;

        serial_println!(
            "    [usb-gadget] configured at address {}, speed={:?}",
            self.device_address,
            speed_name(&self.speed)
        );

        Ok(())
    }

    pub fn disable(&mut self) {
        // Deactivate all endpoints
        for ep in &mut self.endpoints {
            ep.active = false;
        }

        // Flush buffers
        for buf in &mut self.buffers {
            buf.flush_tx();
            buf.rx_buf.clear();
            buf.rx_available = 0;
        }

        self.state = GadgetState::Disabled;
        self.connected = false;
        self.device_address = 0;
        self.configuration = 0;
        self.vbus_present = false;

        serial_println!("    [usb-gadget] disabled");
    }

    /// Handle a USB setup request (control transfer on EP0)
    fn handle_setup(
        &mut self,
        request_type: u8,
        request: u8,
        value: u16,
        _index: u16,
        _length: u16,
    ) -> Result<Vec<u8>, ()> {
        self.setup_requests = self.setup_requests.saturating_add(1);
        let dir_in = (request_type & 0x80) != 0;

        match (request_type & 0x60, request) {
            // Standard device requests
            (0x00, 0x06) if dir_in => {
                // GET_DESCRIPTOR
                let desc_type = (value >> 8) as u8;
                match desc_type {
                    0x01 => {
                        // Device descriptor
                        let desc = self.descriptor.serialize();
                        Ok(desc.to_vec())
                    }
                    _ => {
                        // Unsupported descriptor type
                        self.stall_count = self.stall_count.saturating_add(1);
                        Err(())
                    }
                }
            }
            (0x00, 0x05) => {
                // SET_ADDRESS
                self.device_address = (value & 0x7F) as u8;
                self.state = GadgetState::Addressed;
                Ok(Vec::new())
            }
            (0x00, 0x09) => {
                // SET_CONFIGURATION
                self.configuration = (value & 0xFF) as u8;
                if self.configuration > 0 {
                    self.state = GadgetState::Configured;
                } else {
                    self.state = GadgetState::Addressed;
                }
                Ok(Vec::new())
            }
            _ => {
                self.stall_count = self.stall_count.saturating_add(1);
                Err(())
            }
        }
    }

    /// Write data to an IN endpoint (device -> host)
    fn ep_write(&mut self, ep_addr: u8, data: &[u8]) -> Result<usize, ()> {
        if self.state != GadgetState::Configured {
            return Err(());
        }
        let idx = ep_addr as usize;
        if idx >= self.buffers.len() {
            return Err(());
        }
        let written = self.buffers[idx].write(data);
        self.bytes_tx += written as u64;
        Ok(written)
    }

    /// Read data from an OUT endpoint (host -> device)
    fn ep_read(&mut self, ep_addr: u8, buf: &mut [u8]) -> Result<usize, ()> {
        if self.state != GadgetState::Configured {
            return Err(());
        }
        let idx = ep_addr as usize;
        if idx >= self.buffers.len() {
            return Err(());
        }
        let read = self.buffers[idx].read(buf);
        self.bytes_rx += read as u64;
        Ok(read)
    }

    /// Check if the gadget is in configured state
    fn is_configured(&self) -> bool {
        self.state == GadgetState::Configured
    }

    /// Get transfer statistics
    fn stats(&self) -> (u64, u64, u64, u32) {
        (
            self.bytes_tx,
            self.bytes_rx,
            self.setup_requests,
            self.stall_count,
        )
    }
}

fn function_name(f: &GadgetFunction) -> &'static str {
    match f {
        GadgetFunction::MassStorage => "mass-storage",
        GadgetFunction::Serial => "serial/CDC-ACM",
        GadgetFunction::Ethernet => "ethernet/RNDIS",
        GadgetFunction::Hid => "HID",
    }
}

fn speed_name(s: &UsbSpeed) -> &'static str {
    match s {
        UsbSpeed::Low => "low (1.5M)",
        UsbSpeed::Full => "full (12M)",
        UsbSpeed::High => "high (480M)",
        UsbSpeed::Super => "super (5G)",
    }
}

/// Initialize the USB gadget subsystem
pub fn init() {
    let mut guard = GADGET.lock();
    let gadget = UsbGadget::new(GadgetFunction::Serial);
    *guard = Some(gadget);
    serial_println!("    [usb-gadget] gadget controller initialized (default: serial/CDC-ACM)");
}
