use crate::sync::Mutex;
/// Bluetooth HCI transport driver for Genesis
///
/// Implements the Bluetooth Host Controller Interface (HCI) layer:
///   - HCI command/event/ACL/SCO packet framing
///   - UART (H4) transport over I/O ports (COM-style 16550 UART)
///   - HCI reset and initialization sequence
///   - Device discovery (inquiry scan)
///   - Connection management (create/accept/disconnect)
///   - Pairing state machine (PIN/SSP)
///   - BD_ADDR and local name management
///   - Event queue with callback dispatch
///
/// HCI commands are sent as: opcode (2 bytes LE) + param_len (1 byte) + params.
/// Events arrive as: event_code (1 byte) + param_len (1 byte) + params.
///
/// Reference: Bluetooth Core Spec v5.3 Vol 4 Part A (HCI Transport),
/// Vol 2 Part E (HCI commands/events). All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// UART transport registers (16550-compatible, secondary UART for BT)
// ---------------------------------------------------------------------------

/// Default I/O base for the Bluetooth UART (COM3-like)
const BT_UART_BASE: u16 = 0x03E8;

/// UART register offsets
const UART_DATA: u16 = 0; // TX/RX data (DLAB=0)
const UART_IER: u16 = 1; // Interrupt Enable (DLAB=0)
const UART_FIFO: u16 = 2; // FIFO control (write) / ISR (read)
const UART_LCR: u16 = 3; // Line Control
const UART_MCR: u16 = 4; // Modem Control
const UART_LSR: u16 = 5; // Line Status
const UART_MSR: u16 = 6; // Modem Status
const UART_DLL: u16 = 0; // Divisor Latch Low (DLAB=1)
const UART_DLH: u16 = 1; // Divisor Latch High (DLAB=1)

/// LSR bits
const LSR_DATA_READY: u8 = 0x01;
const LSR_TX_EMPTY: u8 = 0x20;
const LSR_TX_IDLE: u8 = 0x40;

// ---------------------------------------------------------------------------
// HCI packet type indicators (H4 transport)
// ---------------------------------------------------------------------------

const HCI_PKT_COMMAND: u8 = 0x01;
const HCI_PKT_ACL: u8 = 0x02;
const HCI_PKT_SCO: u8 = 0x03;
const HCI_PKT_EVENT: u8 = 0x04;

// ---------------------------------------------------------------------------
// Common HCI command opcodes (OGF << 10 | OCF)
// ---------------------------------------------------------------------------

/// Link Control commands (OGF = 0x01)
const HCI_CMD_INQUIRY: u16 = 0x0401;
const HCI_CMD_INQUIRY_CANCEL: u16 = 0x0402;
const HCI_CMD_CREATE_CONNECTION: u16 = 0x0405;
const HCI_CMD_DISCONNECT: u16 = 0x0406;
const HCI_CMD_ACCEPT_CONN: u16 = 0x0409;
const HCI_CMD_REJECT_CONN: u16 = 0x040A;
const HCI_CMD_PIN_CODE_REPLY: u16 = 0x040D;
const HCI_CMD_PIN_CODE_NEG_REPLY: u16 = 0x040E;

/// Controller & Baseband commands (OGF = 0x03)
const HCI_CMD_RESET: u16 = 0x0C03;
const HCI_CMD_SET_EVENT_FILTER: u16 = 0x0C05;
const HCI_CMD_WRITE_LOCAL_NAME: u16 = 0x0C13;
const HCI_CMD_WRITE_SCAN_ENABLE: u16 = 0x0C1A;
const HCI_CMD_WRITE_CLASS_OF_DEV: u16 = 0x0C24;
const HCI_CMD_WRITE_INQUIRY_MODE: u16 = 0x0C45;

/// Informational commands (OGF = 0x04)
const HCI_CMD_READ_BD_ADDR: u16 = 0x1009;
const HCI_CMD_READ_LOCAL_VER: u16 = 0x1001;
const HCI_CMD_READ_LOCAL_NAME: u16 = 0x0C14;

// ---------------------------------------------------------------------------
// HCI event codes
// ---------------------------------------------------------------------------

const HCI_EVT_INQUIRY_COMPLETE: u8 = 0x01;
const HCI_EVT_INQUIRY_RESULT: u8 = 0x02;
const HCI_EVT_CONN_COMPLETE: u8 = 0x03;
const HCI_EVT_CONN_REQUEST: u8 = 0x04;
const HCI_EVT_DISCONN_COMPLETE: u8 = 0x05;
const HCI_EVT_AUTH_COMPLETE: u8 = 0x06;
const HCI_EVT_CMD_COMPLETE: u8 = 0x0E;
const HCI_EVT_CMD_STATUS: u8 = 0x0F;
const HCI_EVT_PIN_CODE_REQ: u8 = 0x16;
const HCI_EVT_NUM_COMP_PKTS: u8 = 0x13;
const HCI_EVT_INQUIRY_RESULT_RSSI: u8 = 0x22;

// ---------------------------------------------------------------------------
// Connection and pairing states
// ---------------------------------------------------------------------------

/// Pairing state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingState {
    /// Not pairing
    Idle,
    /// Waiting for PIN code from user
    WaitingForPin,
    /// PIN sent, waiting for authentication
    Authenticating,
    /// Pairing complete
    Paired,
    /// Pairing failed
    Failed,
}

/// Connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
}

/// A discovered Bluetooth device
#[derive(Debug, Clone)]
pub struct BtDevice {
    pub addr: [u8; 6],
    pub class_of_device: u32,
    pub rssi: i8,
    pub name: String,
}

/// An active HCI connection
#[derive(Debug, Clone)]
pub struct BtConnection {
    pub handle: u16,
    pub remote_addr: [u8; 6],
    pub state: ConnectionState,
    pub link_type: u8, // 0x01 = ACL, 0x00 = SCO
    pub encryption: bool,
}

// ---------------------------------------------------------------------------
// HCI events for the public queue
// ---------------------------------------------------------------------------

/// Public HCI event
#[derive(Debug, Clone)]
pub enum HciEvent {
    /// HCI reset completed successfully
    ResetComplete,
    /// Inquiry found a device
    DeviceFound(BtDevice),
    /// Inquiry scan finished
    InquiryComplete,
    /// Connection established
    Connected { handle: u16, addr: [u8; 6] },
    /// Connection terminated
    Disconnected { handle: u16, reason: u8 },
    /// PIN code requested for pairing
    PinCodeRequest { addr: [u8; 6] },
    /// Authentication completed
    AuthComplete { handle: u16, success: bool },
    /// Command completed with status
    CommandComplete { opcode: u16, status: u8 },
}

// ---------------------------------------------------------------------------
// Internal driver state
// ---------------------------------------------------------------------------

/// Maximum pending events
const MAX_EVENTS: usize = 32;
/// Maximum connections
const MAX_CONNECTIONS: usize = 8;

struct BtHciDriver {
    /// UART I/O base
    io_base: u16,
    /// Local Bluetooth address (BD_ADDR)
    bd_addr: [u8; 6],
    /// HCI version (from Read Local Version)
    hci_version: u8,
    /// LMP subversion
    lmp_subversion: u16,
    /// Manufacturer
    manufacturer: u16,
    /// Whether the controller is powered and initialized
    powered: bool,
    /// Active connections
    connections: Vec<BtConnection>,
    /// Discovered devices from last inquiry
    discovered: Vec<BtDevice>,
    /// Pairing state
    pairing: PairingState,
    /// Address of device being paired
    pairing_addr: [u8; 6],
    /// Event queue
    events: VecDeque<HciEvent>,
    /// Whether an inquiry is in progress
    inquiry_active: bool,
    /// Receive buffer for partial packets
    rx_buf: Vec<u8>,
    /// Number of HCI command credits available
    cmd_credits: u8,
}

impl BtHciDriver {
    const fn new() -> Self {
        BtHciDriver {
            io_base: BT_UART_BASE,
            bd_addr: [0; 6],
            hci_version: 0,
            lmp_subversion: 0,
            manufacturer: 0,
            powered: false,
            connections: Vec::new(),
            discovered: Vec::new(),
            pairing: PairingState::Idle,
            pairing_addr: [0; 6],
            events: VecDeque::new(),
            inquiry_active: false,
            rx_buf: Vec::new(),
            cmd_credits: 1,
        }
    }

    fn push_event(&mut self, event: HciEvent) {
        if self.events.len() >= MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    // -----------------------------------------------------------------------
    // UART transport (H4)
    // -----------------------------------------------------------------------

    /// Initialize the UART for BT HCI at 115200 baud, 8N1, FIFO enabled
    fn uart_init(&self) {
        let base = self.io_base;
        // Disable interrupts
        crate::io::outb(base + UART_IER, 0x00);
        // Enable DLAB to set baud rate
        crate::io::outb(base + UART_LCR, 0x80);
        // Set divisor for 115200 baud (divisor = 1 for 1.8432 MHz clock)
        crate::io::outb(base + UART_DLL, 0x01);
        crate::io::outb(base + UART_DLH, 0x00);
        // 8 bits, no parity, 1 stop bit, DLAB off
        crate::io::outb(base + UART_LCR, 0x03);
        // Enable FIFO, clear, 14-byte threshold
        crate::io::outb(base + UART_FIFO, 0xC7);
        // RTS/DTR on, OUT2 (interrupt enable on PC)
        crate::io::outb(base + UART_MCR, 0x0B);
        // Enable RX data available interrupt
        crate::io::outb(base + UART_IER, 0x01);
    }

    /// Check if the UART is present (loopback test)
    fn uart_detect(&self) -> bool {
        let base = self.io_base;
        // Set loopback mode
        crate::io::outb(base + UART_MCR, 0x10);
        // Send test byte
        crate::io::outb(base + UART_DATA, 0xAE);
        // Small delay
        for _ in 0..100 {
            crate::io::io_wait();
        }
        // Check if we got it back
        let result = crate::io::inb(base + UART_DATA);
        // Restore MCR
        crate::io::outb(base + UART_MCR, 0x0B);
        result == 0xAE
    }

    /// Write a byte to the UART, waiting for TX empty
    fn uart_write_byte(&self, byte: u8) {
        let base = self.io_base;
        for _ in 0..100_000 {
            if crate::io::inb(base + UART_LSR) & LSR_TX_EMPTY != 0 {
                crate::io::outb(base + UART_DATA, byte);
                return;
            }
            core::hint::spin_loop();
        }
    }

    /// Read a byte from the UART if available
    fn uart_read_byte(&self) -> Option<u8> {
        let base = self.io_base;
        if crate::io::inb(base + UART_LSR) & LSR_DATA_READY != 0 {
            Some(crate::io::inb(base + UART_DATA))
        } else {
            None
        }
    }

    /// Read a byte with timeout (spin-wait)
    fn uart_read_byte_timeout(&self, timeout_iters: u32) -> Option<u8> {
        for _ in 0..timeout_iters {
            if let Some(b) = self.uart_read_byte() {
                return Some(b);
            }
            core::hint::spin_loop();
        }
        None
    }

    // -----------------------------------------------------------------------
    // HCI packet I/O
    // -----------------------------------------------------------------------

    /// Send an HCI command packet over H4 transport
    fn send_command_raw(&self, opcode: u16, params: &[u8]) {
        // HCI spec: param_total_len is 1 byte — truncate silently if caller is bad
        if params.len() > 255 {
            serial_println!(
                "  [bt] send_command_raw: params too long ({} > 255), truncating",
                params.len()
            );
        }
        let param_len = params.len().min(255) as u8;
        // H4 packet indicator
        self.uart_write_byte(HCI_PKT_COMMAND);
        // Opcode (2 bytes, little-endian)
        self.uart_write_byte((opcode & 0xFF) as u8);
        self.uart_write_byte((opcode >> 8) as u8);
        // Parameter total length
        self.uart_write_byte(param_len);
        // Parameters
        for &b in &params[..param_len as usize] {
            self.uart_write_byte(b);
        }
    }

    /// Send an ACL data packet
    fn send_acl_raw(&self, handle: u16, data: &[u8]) {
        self.uart_write_byte(HCI_PKT_ACL);
        // Handle + flags (PB=0b10 first auto-flush, BC=0b00 point-to-point)
        let handle_flags = handle | (0x2 << 12);
        self.uart_write_byte((handle_flags & 0xFF) as u8);
        self.uart_write_byte((handle_flags >> 8) as u8);
        // Data total length (2 bytes LE)
        let len = data.len() as u16;
        self.uart_write_byte((len & 0xFF) as u8);
        self.uart_write_byte((len >> 8) as u8);
        for &b in data {
            self.uart_write_byte(b);
        }
    }

    /// Try to read a complete HCI event from UART.
    /// Returns (event_code, parameters) if a full event is available.
    fn read_event_raw(&self) -> Option<(u8, Vec<u8>)> {
        // Read packet indicator
        let indicator = self.uart_read_byte_timeout(100_000)?;
        if indicator != HCI_PKT_EVENT {
            // Not an event packet -- discard or could be ACL data
            return None;
        }
        // Event code
        let event_code = self.uart_read_byte_timeout(50_000)?;
        // Parameter length
        let param_len = self.uart_read_byte_timeout(50_000)? as usize;
        // Parameters
        let mut params = Vec::with_capacity(param_len);
        for _ in 0..param_len {
            let b = self.uart_read_byte_timeout(50_000)?;
            params.push(b);
        }
        Some((event_code, params))
    }

    /// Send a command and wait for Command Complete event, returning the event params
    fn send_command_wait(&self, opcode: u16, params: &[u8]) -> Option<Vec<u8>> {
        self.send_command_raw(opcode, params);

        // Poll for response
        for _ in 0..500_000u32 {
            if let Some((evt, evt_params)) = self.read_event_raw() {
                if evt == HCI_EVT_CMD_COMPLETE && evt_params.len() >= 3 {
                    let rsp_opcode = (evt_params[1] as u16) | ((evt_params[2] as u16) << 8);
                    if rsp_opcode == opcode {
                        return Some(evt_params);
                    }
                }
                if evt == HCI_EVT_CMD_STATUS && evt_params.len() >= 4 {
                    let rsp_opcode = (evt_params[2] as u16) | ((evt_params[3] as u16) << 8);
                    if rsp_opcode == opcode {
                        return Some(evt_params);
                    }
                }
            }
            core::hint::spin_loop();
        }
        None
    }

    // -----------------------------------------------------------------------
    // HCI initialization
    // -----------------------------------------------------------------------

    /// Perform HCI reset and read basic controller information
    fn hci_init(&mut self) -> bool {
        // HCI_Reset
        serial_println!("    [bt] sending HCI_Reset...");
        let reset_rsp = self.send_command_wait(HCI_CMD_RESET, &[]);
        match reset_rsp {
            Some(ref params) if params.len() >= 4 && params[3] == 0x00 => {
                serial_println!("    [bt] HCI_Reset succeeded");
            }
            _ => {
                serial_println!("    [bt] HCI_Reset failed or timed out");
                return false;
            }
        }

        // Read Local Version Information
        if let Some(params) = self.send_command_wait(HCI_CMD_READ_LOCAL_VER, &[]) {
            if params.len() >= 10 && params[3] == 0x00 {
                self.hci_version = params[4];
                self.manufacturer = (params[8] as u16) | ((params[9] as u16) << 8);
                self.lmp_subversion = (params[6] as u16) | ((params[7] as u16) << 8);
                serial_println!(
                    "    [bt] HCI version={}, manufacturer={:#06x}",
                    self.hci_version,
                    self.manufacturer
                );
            }
        }

        // Read BD_ADDR
        if let Some(params) = self.send_command_wait(HCI_CMD_READ_BD_ADDR, &[]) {
            if params.len() >= 10 && params[3] == 0x00 {
                self.bd_addr.copy_from_slice(&params[4..10]);
                serial_println!(
                    "    [bt] BD_ADDR={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    self.bd_addr[5],
                    self.bd_addr[4],
                    self.bd_addr[3],
                    self.bd_addr[2],
                    self.bd_addr[1],
                    self.bd_addr[0]
                );
            }
        }

        // Set local name
        let mut name_buf = [0u8; 248];
        let name = b"Genesis-AIOS";
        let copy_len = name.len().min(247);
        name_buf[..copy_len].copy_from_slice(&name[..copy_len]);
        self.send_command_wait(HCI_CMD_WRITE_LOCAL_NAME, &name_buf);

        // Write scan enable (inquiry scan + page scan)
        self.send_command_wait(HCI_CMD_WRITE_SCAN_ENABLE, &[0x03]);

        // Set Class of Device (computer, audio)
        self.send_command_wait(HCI_CMD_WRITE_CLASS_OF_DEV, &[0x0C, 0x01, 0x00]);

        // Set inquiry mode to RSSI
        self.send_command_wait(HCI_CMD_WRITE_INQUIRY_MODE, &[0x01]);

        self.cmd_credits = 1;
        true
    }

    // -----------------------------------------------------------------------
    // Event processing
    // -----------------------------------------------------------------------

    /// Process a received HCI event
    fn process_event(&mut self, event_code: u8, params: &[u8]) {
        match event_code {
            HCI_EVT_INQUIRY_RESULT | HCI_EVT_INQUIRY_RESULT_RSSI => {
                if params.is_empty() {
                    return;
                }
                let num_responses = params[0] as usize;
                // Each response: 6-byte addr + page_scan_rep + reserved + class[3] + clock_offset[2]
                // RSSI variant adds 1 byte RSSI per response
                let has_rssi = event_code == HCI_EVT_INQUIRY_RESULT_RSSI;
                let entry_size: usize = if has_rssi { 14 } else { 14 };
                // Bound num_responses to prevent runaway iteration on corrupted data
                let num_responses = num_responses.min(255);
                for i in 0..num_responses {
                    let offset = 1usize.saturating_add(i.saturating_mul(entry_size));
                    if offset.saturating_add(6) > params.len() {
                        break;
                    }
                    let mut addr = [0u8; 6];
                    addr.copy_from_slice(&params[offset..offset + 6]);
                    let cod_base = offset.saturating_add(9);
                    let cod = if cod_base.saturating_add(2) < params.len() {
                        (params[cod_base] as u32)
                            | ((params[cod_base + 1] as u32) << 8)
                            | ((params[cod_base + 2] as u32) << 16)
                    } else {
                        0
                    };
                    let rssi_idx = offset.saturating_add(13);
                    let rssi = if has_rssi && rssi_idx < params.len() {
                        params[rssi_idx] as i8
                    } else {
                        0
                    };

                    let dev = BtDevice {
                        addr,
                        class_of_device: cod,
                        rssi,
                        name: String::new(),
                    };
                    // Avoid duplicates
                    if !self.discovered.iter().any(|d| d.addr == addr) {
                        self.push_event(HciEvent::DeviceFound(dev.clone()));
                        self.discovered.push(dev);
                    }
                }
            }
            HCI_EVT_INQUIRY_COMPLETE => {
                self.inquiry_active = false;
                self.push_event(HciEvent::InquiryComplete);
                serial_println!(
                    "    [bt] inquiry complete, {} devices found",
                    self.discovered.len()
                );
            }
            HCI_EVT_CONN_COMPLETE => {
                if params.len() >= 11 {
                    let status = params[0];
                    let handle = (params[1] as u16) | ((params[2] as u16) << 8);
                    let mut addr = [0u8; 6];
                    addr.copy_from_slice(&params[3..9]);
                    let link_type = params[9];
                    let encryption = params[10] != 0;

                    if status == 0x00 {
                        let conn = BtConnection {
                            handle,
                            remote_addr: addr,
                            state: ConnectionState::Connected,
                            link_type,
                            encryption,
                        };
                        if self.connections.len() < MAX_CONNECTIONS {
                            self.connections.push(conn);
                        }
                        self.push_event(HciEvent::Connected { handle, addr });
                        serial_println!("    [bt] connected handle={:#06x}", handle);
                    } else {
                        serial_println!("    [bt] connection failed status={:#04x}", status);
                    }
                }
            }
            HCI_EVT_DISCONN_COMPLETE => {
                if params.len() >= 4 {
                    let status = params[0];
                    let handle = (params[1] as u16) | ((params[2] as u16) << 8);
                    let reason = params[3];
                    if status == 0x00 {
                        self.connections.retain(|c| c.handle != handle);
                        self.push_event(HciEvent::Disconnected { handle, reason });
                        serial_println!(
                            "    [bt] disconnected handle={:#06x} reason={:#04x}",
                            handle,
                            reason
                        );
                    }
                }
            }
            HCI_EVT_CONN_REQUEST => {
                if params.len() >= 10 {
                    let mut addr = [0u8; 6];
                    addr.copy_from_slice(&params[0..6]);
                    // Auto-accept as slave
                    let mut accept_params = [0u8; 7];
                    accept_params[0..6].copy_from_slice(&addr);
                    accept_params[6] = 0x01; // role = slave
                    self.send_command_raw(HCI_CMD_ACCEPT_CONN, &accept_params);
                }
            }
            HCI_EVT_PIN_CODE_REQ => {
                if params.len() >= 6 {
                    let mut addr = [0u8; 6];
                    addr.copy_from_slice(&params[0..6]);
                    self.pairing = PairingState::WaitingForPin;
                    self.pairing_addr = addr;
                    self.push_event(HciEvent::PinCodeRequest { addr });
                    serial_println!(
                        "    [bt] PIN code requested for {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                        addr[5],
                        addr[4],
                        addr[3],
                        addr[2],
                        addr[1],
                        addr[0]
                    );
                }
            }
            HCI_EVT_AUTH_COMPLETE => {
                if params.len() >= 3 {
                    let status = params[0];
                    let handle = (params[1] as u16) | ((params[2] as u16) << 8);
                    let success = status == 0x00;
                    if success {
                        self.pairing = PairingState::Paired;
                    } else {
                        self.pairing = PairingState::Failed;
                    }
                    self.push_event(HciEvent::AuthComplete { handle, success });
                }
            }
            HCI_EVT_CMD_COMPLETE => {
                if params.len() >= 3 {
                    self.cmd_credits = params[0];
                    let opcode = (params[1] as u16) | ((params[2] as u16) << 8);
                    let status = if params.len() >= 4 { params[3] } else { 0xFF };
                    self.push_event(HciEvent::CommandComplete { opcode, status });
                }
            }
            HCI_EVT_CMD_STATUS => {
                if params.len() >= 4 {
                    let status = params[0];
                    self.cmd_credits = params[1];
                    let opcode = (params[2] as u16) | ((params[3] as u16) << 8);
                    self.push_event(HciEvent::CommandComplete { opcode, status });
                }
            }
            HCI_EVT_NUM_COMP_PKTS => {
                // Flow control -- just track credits, nothing else to do
            }
            _ => {
                // Unknown event -- ignore
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static HCI: Mutex<Option<BtHciDriver>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the Bluetooth HCI controller.
/// Detects UART, performs HCI reset, reads BD_ADDR.
pub fn init() {
    let mut drv = BtHciDriver::new();

    // Detect UART
    if !drv.uart_detect() {
        serial_println!("  Bluetooth: no UART detected at {:#06x}", drv.io_base);
        return;
    }
    serial_println!("  Bluetooth: UART detected at {:#06x}", drv.io_base);

    // Initialize UART
    drv.uart_init();

    // Perform HCI initialization
    if !drv.hci_init() {
        serial_println!("  Bluetooth: HCI initialization failed");
        return;
    }

    drv.powered = true;
    serial_println!("  Bluetooth: HCI initialized, v{}", drv.hci_version);

    *HCI.lock() = Some(drv);
    super::register("bluetooth", super::DeviceType::Other);
}

/// Send an HCI command and return the event response parameters.
pub fn send_command(opcode: u16, params: &[u8]) -> Result<Vec<u8>, ()> {
    let mut guard = HCI.lock();
    let drv = guard.as_mut().ok_or(())?;
    if !drv.powered {
        return Err(());
    }
    drv.send_command_wait(opcode, params).ok_or(())
}

/// Read and process pending HCI events from the transport.
/// Call this periodically or in an interrupt handler.
pub fn poll() {
    let mut guard = HCI.lock();
    if let Some(ref mut drv) = *guard {
        if !drv.powered {
            return;
        }
        // Read up to 16 events per poll
        for _ in 0..16 {
            match drv.read_event_raw() {
                Some((code, params)) => drv.process_event(code, &params),
                None => break,
            }
        }
    }
}

/// Pop the next HCI event from the queue.
pub fn read_event() -> Option<HciEvent> {
    let mut guard = HCI.lock();
    guard.as_mut().and_then(|drv| drv.events.pop_front())
}

/// Start Bluetooth device discovery (inquiry scan).
/// Duration in 1.28s units (e.g., 8 = ~10 seconds). Max responses 0 = unlimited.
pub fn start_inquiry(duration: u8, max_responses: u8) -> Result<(), ()> {
    let mut guard = HCI.lock();
    let drv = guard.as_mut().ok_or(())?;
    if !drv.powered {
        return Err(());
    }
    if drv.inquiry_active {
        return Err(());
    }

    drv.discovered.clear();
    drv.inquiry_active = true;

    // LAP for General Inquiry = 0x9E8B33
    let params = [0x33, 0x8B, 0x9E, duration, max_responses];
    drv.send_command_raw(HCI_CMD_INQUIRY, &params);
    serial_println!(
        "    [bt] inquiry started ({}s)",
        duration as u32 * 128 / 100
    );
    Ok(())
}

/// Cancel an ongoing inquiry.
pub fn cancel_inquiry() -> Result<(), ()> {
    let mut guard = HCI.lock();
    let drv = guard.as_mut().ok_or(())?;
    if !drv.inquiry_active {
        return Ok(());
    }
    drv.send_command_raw(HCI_CMD_INQUIRY_CANCEL, &[]);
    drv.inquiry_active = false;
    Ok(())
}

/// Get the list of discovered devices from the last inquiry.
pub fn discovered_devices() -> Vec<BtDevice> {
    HCI.lock()
        .as_ref()
        .map_or(Vec::new(), |drv| drv.discovered.clone())
}

/// Create an ACL connection to a remote device.
pub fn connect(addr: &[u8; 6]) -> Result<(), ()> {
    let mut guard = HCI.lock();
    let drv = guard.as_mut().ok_or(())?;
    if !drv.powered {
        return Err(());
    }

    // Check if already connected
    if drv.connections.iter().any(|c| c.remote_addr == *addr) {
        return Err(());
    }

    // HCI_Create_Connection parameters:
    // BD_ADDR (6) + packet_type (2) + page_scan_rep (1) + reserved (1) +
    // clock_offset (2) + allow_role_switch (1) = 13 bytes
    let mut params = [0u8; 13];
    params[0..6].copy_from_slice(addr);
    // DM1|DH1|DM3|DH3|DM5|DH5 packet types
    params[6] = 0x18;
    params[7] = 0xCC;
    // Page scan repetition mode R1
    params[8] = 0x01;
    // Allow role switch
    params[12] = 0x01;

    drv.send_command_raw(HCI_CMD_CREATE_CONNECTION, &params);
    serial_println!(
        "    [bt] connecting to {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        addr[5],
        addr[4],
        addr[3],
        addr[2],
        addr[1],
        addr[0]
    );
    Ok(())
}

/// Disconnect an active connection.
pub fn disconnect(handle: u16) -> Result<(), ()> {
    let mut guard = HCI.lock();
    let drv = guard.as_mut().ok_or(())?;
    if !drv.powered {
        return Err(());
    }

    // Mark as disconnecting
    for conn in &mut drv.connections {
        if conn.handle == handle {
            conn.state = ConnectionState::Disconnecting;
            break;
        }
    }

    // HCI_Disconnect: handle (2 LE) + reason (1)
    let params = [
        (handle & 0xFF) as u8,
        (handle >> 8) as u8,
        0x13, // Remote User Terminated Connection
    ];
    drv.send_command_raw(HCI_CMD_DISCONNECT, &params);
    Ok(())
}

/// Send ACL data on a connection.
pub fn send_acl(handle: u16, data: &[u8]) -> Result<(), ()> {
    let guard = HCI.lock();
    let drv = guard.as_ref().ok_or(())?;
    if !drv.powered {
        return Err(());
    }
    if !drv
        .connections
        .iter()
        .any(|c| c.handle == handle && c.state == ConnectionState::Connected)
    {
        return Err(());
    }
    drv.send_acl_raw(handle, data);
    Ok(())
}

/// Reply to a PIN code request for pairing.
pub fn pin_code_reply(addr: &[u8; 6], pin: &[u8]) -> Result<(), ()> {
    let mut guard = HCI.lock();
    let drv = guard.as_mut().ok_or(())?;
    if drv.pairing != PairingState::WaitingForPin {
        return Err(());
    }

    let pin_len = pin.len().min(16);
    // BD_ADDR (6) + PIN_Length (1) + PIN_Code (16)
    let mut params = [0u8; 23];
    params[0..6].copy_from_slice(addr);
    params[6] = pin_len as u8;
    params[7..7 + pin_len].copy_from_slice(&pin[..pin_len]);

    drv.send_command_raw(HCI_CMD_PIN_CODE_REPLY, &params);
    drv.pairing = PairingState::Authenticating;
    Ok(())
}

/// Get the local BD_ADDR.
pub fn bd_addr() -> Option<[u8; 6]> {
    HCI.lock().as_ref().map(|drv| drv.bd_addr)
}

/// Check if the controller is powered.
pub fn is_powered() -> bool {
    HCI.lock().as_ref().map_or(false, |drv| drv.powered)
}

/// Get the number of active connections.
pub fn connection_count() -> usize {
    HCI.lock().as_ref().map_or(0, |drv| drv.connections.len())
}

/// Get the current pairing state.
pub fn pairing_state() -> PairingState {
    HCI.lock()
        .as_ref()
        .map_or(PairingState::Idle, |drv| drv.pairing)
}

/// Check if inquiry is in progress.
pub fn is_inquiring() -> bool {
    HCI.lock().as_ref().map_or(false, |drv| drv.inquiry_active)
}

/// Check if any events are pending.
pub fn has_events() -> bool {
    HCI.lock()
        .as_ref()
        .map_or(false, |drv| !drv.events.is_empty())
}
