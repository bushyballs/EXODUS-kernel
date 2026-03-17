/// Host Controller Interface -- low-level BT controller communication.
///
/// HCI is the standard interface between the Bluetooth host (software stack)
/// and the Bluetooth controller (radio hardware). This module handles:
///   - Encoding/decoding HCI command packets (OGF/OCF opcode format)
///   - Receiving HCI event packets from the controller
///   - ACL data transport for upper layers (L2CAP)
///   - Controller initialization (reset, read features, set event mask)
///
/// Transport is memory-mapped (USB HCI or SDIO), accessed via MMIO registers.
///
/// Part of the AIOS bluetooth subsystem.

use alloc::vec::Vec;
use alloc::collections::VecDeque;
use crate::{serial_print, serial_println};
use crate::sync::Mutex;

/// MMIO base address for the HCI transport registers (platform-dependent).
const HCI_MMIO_BASE: usize = 0xFEDC_0000;

/// MMIO register offsets.
const REG_COMMAND: usize = 0x00;     // Write: command packet data
const REG_EVENT: usize = 0x04;       // Read: event packet data
const REG_STATUS: usize = 0x08;      // Status register
const REG_CONTROL: usize = 0x0C;     // Control register
const REG_ACL_TX: usize = 0x10;      // ACL data transmit
const REG_ACL_RX: usize = 0x14;      // ACL data receive

/// Status register bits.
const STATUS_CMD_READY: u32 = 1 << 0;   // Controller ready for commands
const STATUS_EVENT_AVAIL: u32 = 1 << 1; // Event data available
const STATUS_ACL_AVAIL: u32 = 1 << 2;   // ACL data available

/// HCI packet indicator bytes.
const HCI_COMMAND_PKT: u8 = 0x01;
const HCI_ACL_DATA_PKT: u8 = 0x02;
const HCI_EVENT_PKT: u8 = 0x04;

/// OGF (Opcode Group Field) values.
const OGF_LINK_CONTROL: u16 = 0x01;
const OGF_LINK_POLICY: u16 = 0x02;
const OGF_CONTROLLER: u16 = 0x03;
const OGF_INFORMATIONAL: u16 = 0x04;
const OGF_LE_CONTROLLER: u16 = 0x08;

/// Common OCF (Opcode Command Field) values for OGF_CONTROLLER.
const OCF_RESET: u16 = 0x0003;
const OCF_SET_EVENT_MASK: u16 = 0x0001;
const OCF_READ_LOCAL_VERSION: u16 = 0x0001; // OGF_INFORMATIONAL

/// Encode OGF + OCF into a 16-bit opcode.
fn make_opcode(ogf: u16, ocf: u16) -> u16 {
    (ogf << 10) | ocf
}

/// Decode a 16-bit opcode into (OGF, OCF).
fn decode_opcode(opcode: u16) -> (u16, u16) {
    let ogf = (opcode >> 10) & 0x3F;
    let ocf = opcode & 0x03FF;
    (ogf, ocf)
}

/// HCI event codes.
const EVT_COMMAND_COMPLETE: u8 = 0x0E;
const EVT_COMMAND_STATUS: u8 = 0x0F;
const EVT_CONNECTION_COMPLETE: u8 = 0x03;
const EVT_DISCONNECTION_COMPLETE: u8 = 0x05;
const EVT_LE_META: u8 = 0x3E;

/// Global HCI transport state.
static HCI: Mutex<Option<HciTransportInner>> = Mutex::new(None);

/// Internal state for the HCI transport.
struct HciTransportInner {
    mmio_base: usize,
    initialized: bool,
    pending_events: VecDeque<Vec<u8>>,
    pending_acl: VecDeque<Vec<u8>>,
    command_credits: u8,
    local_version: u8,
    hci_revision: u16,
    lmp_version: u8,
    manufacturer: u16,
}

impl HciTransportInner {
    fn new(mmio_base: usize) -> Self {
        Self {
            mmio_base,
            initialized: false,
            pending_events: VecDeque::new(),
            pending_acl: VecDeque::new(),
            command_credits: 1,
            local_version: 0,
            hci_revision: 0,
            lmp_version: 0,
            manufacturer: 0,
        }
    }

    /// Read a 32-bit MMIO register.
    fn read_reg(&self, offset: usize) -> u32 {
        let addr = (self.mmio_base + offset) as *const u32;
        unsafe { core::ptr::read_volatile(addr) }
    }

    /// Write a 32-bit MMIO register.
    fn write_reg(&self, offset: usize, value: u32) {
        let addr = (self.mmio_base + offset) as *mut u32;
        unsafe { core::ptr::write_volatile(addr, value); }
    }

    /// Check if the controller is ready to accept commands.
    fn is_cmd_ready(&self) -> bool {
        (self.read_reg(REG_STATUS) & STATUS_CMD_READY) != 0
    }

    /// Check if an event is available.
    fn is_event_available(&self) -> bool {
        (self.read_reg(REG_STATUS) & STATUS_EVENT_AVAIL) != 0
    }

    /// Send a raw HCI command packet to the controller.
    fn send_command_raw(&mut self, opcode: u16, params: &[u8]) {
        if self.command_credits == 0 {
            serial_println!("    [hci] No command credits, queuing");
            return;
        }

        // Wait for controller ready (bounded spin).
        let mut retries = 10000u32;
        while !self.is_cmd_ready() && retries > 0 {
            core::hint::spin_loop();
            retries -= 1;
        }
        if retries == 0 {
            serial_println!("    [hci] Controller not ready, timeout");
            return;
        }

        // Build command packet: indicator | opcode_lo | opcode_hi | param_len | params
        let lo = (opcode & 0xFF) as u32;
        let hi = ((opcode >> 8) & 0xFF) as u32;
        let plen = params.len() as u32;

        // Write the command header as a packed 32-bit word.
        let header = (HCI_COMMAND_PKT as u32) | (lo << 8) | (hi << 16) | (plen << 24);
        self.write_reg(REG_COMMAND, header);

        // Write parameter bytes in 32-bit chunks.
        let mut i = 0;
        while i < params.len() {
            let mut word: u32 = 0;
            for j in 0..4 {
                if i + j < params.len() {
                    word |= (params[i + j] as u32) << (j * 8);
                }
            }
            self.write_reg(REG_COMMAND, word);
            i += 4;
        }

        self.command_credits = self.command_credits.saturating_sub(1);

        let (ogf, ocf) = decode_opcode(opcode);
        serial_println!("    [hci] Sent command OGF={:#04x} OCF={:#06x} params_len={}", ogf, ocf, params.len());
    }

    /// Poll the controller for events and queue them.
    fn poll_events(&mut self) {
        while self.is_event_available() {
            // Read event header: event_code | param_len packed in first word
            let header = self.read_reg(REG_EVENT);
            let _indicator = (header & 0xFF) as u8;
            let event_code = ((header >> 8) & 0xFF) as u8;
            let param_len = ((header >> 16) & 0xFF) as usize;

            let mut event_data = Vec::with_capacity(2 + param_len);
            event_data.push(event_code);
            event_data.push(param_len as u8);

            // Read parameter bytes.
            let mut remaining = param_len;
            while remaining > 0 {
                let word = self.read_reg(REG_EVENT);
                let bytes_in_word = core::cmp::min(remaining, 4);
                for j in 0..bytes_in_word {
                    event_data.push(((word >> (j * 8)) & 0xFF) as u8);
                }
                remaining -= bytes_in_word;
            }

            // Process command-complete events to restore credits.
            if event_code == EVT_COMMAND_COMPLETE && param_len >= 3 {
                self.command_credits = event_data[2]; // num_hci_command_packets
            } else if event_code == EVT_COMMAND_STATUS && param_len >= 3 {
                self.command_credits = event_data[3]; // num_hci_command_packets
            }

            self.pending_events.push_back(event_data);
        }
    }

    /// Read the next queued event, polling hardware first.
    fn read_event(&mut self) -> Vec<u8> {
        self.poll_events();
        self.pending_events.pop_front().unwrap_or_default()
    }

    /// Send an HCI Reset command and wait for the command-complete event.
    fn reset(&mut self) {
        let opcode = make_opcode(OGF_CONTROLLER, OCF_RESET);
        self.send_command_raw(opcode, &[]);
        serial_println!("    [hci] Reset command sent");

        // Poll for the command-complete event.
        let mut attempts = 100u32;
        while attempts > 0 {
            self.poll_events();
            if let Some(evt) = self.pending_events.front() {
                if evt.len() >= 2 && evt[0] == EVT_COMMAND_COMPLETE {
                    let _ = self.pending_events.pop_front();
                    serial_println!("    [hci] Reset complete");
                    return;
                }
            }
            core::hint::spin_loop();
            attempts -= 1;
        }
        serial_println!("    [hci] Reset: no command-complete received (controller may be absent)");
    }

    /// Set the HCI event mask to enable all standard events.
    fn set_event_mask(&mut self) {
        let opcode = make_opcode(OGF_CONTROLLER, OCF_SET_EVENT_MASK);
        // Enable all events: 0xFFFFFFFFFFFFFFFF
        let mask: [u8; 8] = [0xFF; 8];
        self.send_command_raw(opcode, &mask);
    }

    /// Read local version information.
    fn read_local_version(&mut self) {
        let opcode = make_opcode(OGF_INFORMATIONAL, OCF_READ_LOCAL_VERSION);
        self.send_command_raw(opcode, &[]);

        // Poll for command-complete with version info.
        let mut attempts = 100u32;
        while attempts > 0 {
            self.poll_events();
            if let Some(evt) = self.pending_events.front() {
                if evt.len() >= 2 && evt[0] == EVT_COMMAND_COMPLETE {
                    let evt = self.pending_events.pop_front().unwrap();
                    // Parse: [event_code, plen, num_cmds, opcode_lo, opcode_hi, status,
                    //         hci_version, hci_revision_lo, hci_revision_hi,
                    //         lmp_version, manufacturer_lo, manufacturer_hi, ...]
                    if evt.len() >= 12 {
                        self.local_version = evt[7];
                        self.hci_revision = evt[8] as u16 | ((evt[9] as u16) << 8);
                        self.lmp_version = evt[10];
                        self.manufacturer = evt[11] as u16 | (if evt.len() > 12 { (evt[12] as u16) << 8 } else { 0 });
                    }
                    serial_println!("    [hci] Local version: HCI={}, LMP={}, manufacturer={:#06x}",
                        self.local_version, self.lmp_version, self.manufacturer);
                    return;
                }
            }
            core::hint::spin_loop();
            attempts -= 1;
        }
    }
}

/// HCI transport layer for communicating with the Bluetooth controller.
pub struct HciTransport {
    _private: (), // use the global singleton
}

impl HciTransport {
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Send an HCI command to the controller.
    pub fn send_command(&mut self, opcode: u16, params: &[u8]) {
        if let Some(inner) = HCI.lock().as_mut() {
            inner.send_command_raw(opcode, params);
        }
    }

    /// Read the next HCI event from the controller.
    pub fn read_event(&mut self) -> Vec<u8> {
        if let Some(inner) = HCI.lock().as_mut() {
            inner.read_event()
        } else {
            Vec::new()
        }
    }
}

/// Encode an OGF/OCF pair into an HCI opcode.
pub fn opcode(ogf: u16, ocf: u16) -> u16 {
    make_opcode(ogf, ocf)
}

pub fn init() {
    let mut inner = HciTransportInner::new(HCI_MMIO_BASE);

    serial_println!("    [hci] Initializing Bluetooth HCI transport");

    // Reset the controller.
    inner.reset();

    // Set event mask to receive all events.
    inner.set_event_mask();

    // Read local version info.
    inner.read_local_version();

    inner.initialized = true;
    inner.command_credits = 1;

    *HCI.lock() = Some(inner);
    serial_println!("    [hci] HCI transport initialized");
}
