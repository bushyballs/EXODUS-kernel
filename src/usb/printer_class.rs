use crate::sync::Mutex;
/// USB Printer Class driver
///
/// Supports USB printers via the Printer Class specification.
/// Handles bidirectional communication, port status queries,
/// IEEE 1284 device ID parsing, and job submission with
/// PCL/PostScript/raw data streams.
///
/// References: USB Printer Class 1.1 specification.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static PRINTER_STATE: Mutex<Option<PrinterClassState>> = Mutex::new(None);

pub struct PrinterClassState {
    pub printers: Vec<PrinterDevice>,
    pub next_id: u32,
}

impl PrinterClassState {
    pub fn new() -> Self {
        PrinterClassState {
            printers: Vec::new(),
            next_id: 1,
        }
    }

    pub fn register(&mut self, printer: PrinterDevice) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.printers.push(printer);
        id
    }

    pub fn find_by_slot(&self, slot_id: u8) -> Option<&PrinterDevice> {
        self.printers.iter().find(|p| p.slot_id == slot_id)
    }
}

// ---------------------------------------------------------------------------
// Printer Class constants
// ---------------------------------------------------------------------------

pub const CLASS_PRINTER: u8 = 0x07;

/// Printer subclass codes.
pub const SUBCLASS_PRINTER: u8 = 0x01;

/// Printer protocol codes.
pub const PROTOCOL_UNIDIRECTIONAL: u8 = 0x01;
pub const PROTOCOL_BIDIRECTIONAL: u8 = 0x02;
pub const PROTOCOL_IEEE1284_4: u8 = 0x03;

/// Printer class-specific requests.
pub const GET_DEVICE_ID: u8 = 0x00;
pub const GET_PORT_STATUS: u8 = 0x01;
pub const SOFT_RESET: u8 = 0x02;

/// Port status bits (from GET_PORT_STATUS).
pub const PORT_STATUS_NOT_ERROR: u8 = 0x08;
pub const PORT_STATUS_SELECT: u8 = 0x10;
pub const PORT_STATUS_PAPER_EMPTY: u8 = 0x20;

// ---------------------------------------------------------------------------
// Printer port status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct PortStatus {
    pub raw: u8,
}

impl PortStatus {
    pub fn from_raw(raw: u8) -> Self {
        PortStatus { raw }
    }

    /// Printer has no error condition.
    pub fn no_error(&self) -> bool {
        self.raw & PORT_STATUS_NOT_ERROR != 0
    }

    /// Printer is selected (online).
    pub fn selected(&self) -> bool {
        self.raw & PORT_STATUS_SELECT != 0
    }

    /// Printer is out of paper.
    pub fn paper_empty(&self) -> bool {
        self.raw & PORT_STATUS_PAPER_EMPTY != 0
    }

    /// Printer is ready to accept data.
    pub fn ready(&self) -> bool {
        self.no_error() && self.selected() && !self.paper_empty()
    }
}

// ---------------------------------------------------------------------------
// IEEE 1284 Device ID parser
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DeviceId {
    pub manufacturer: String,
    pub model: String,
    pub command_set: Vec<String>,
    pub description: String,
    pub raw: String,
}

impl DeviceId {
    pub fn new() -> Self {
        DeviceId {
            manufacturer: String::new(),
            model: String::new(),
            command_set: Vec::new(),
            description: String::new(),
            raw: String::new(),
        }
    }

    /// Parse an IEEE 1284 device ID string.
    /// Format: "MFG:Manufacturer;MDL:Model;CMD:PCL,PS;DES:Description;"
    pub fn parse(data: &[u8]) -> Self {
        let mut id = DeviceId::new();
        if data.len() < 2 {
            return id;
        }
        // First two bytes are big-endian length (including themselves)
        let len = ((data[0] as usize) << 8) | data[1] as usize;
        let end = len.min(data.len());
        if end <= 2 {
            return id;
        }

        let text = &data[2..end];
        let text_str = String::from_utf8_lossy(text);
        id.raw = text_str.clone().into();

        // Parse key:value pairs separated by ';'
        for field in text_str.split(';') {
            let field = field.trim();
            if field.is_empty() {
                continue;
            }
            if let Some(idx) = field.find(':') {
                let key = &field[..idx];
                let val = &field[idx + 1..];
                match key {
                    "MFG" | "MANUFACTURER" => {
                        id.manufacturer = String::from(val);
                    }
                    "MDL" | "MODEL" => {
                        id.model = String::from(val);
                    }
                    "CMD" | "COMMAND SET" => {
                        for cmd in val.split(',') {
                            let cmd = cmd.trim();
                            if !cmd.is_empty() {
                                id.command_set.push(String::from(cmd));
                            }
                        }
                    }
                    "DES" | "DESCRIPTION" => {
                        id.description = String::from(val);
                    }
                    _ => {}
                }
            }
        }
        id
    }

    /// Check if the printer supports a specific page description language.
    pub fn supports_language(&self, lang: &str) -> bool {
        self.command_set
            .iter()
            .any(|cmd| cmd.as_bytes().eq_ignore_ascii_case(lang.as_bytes()))
    }

    /// Check if the printer supports PCL.
    pub fn supports_pcl(&self) -> bool {
        self.command_set.iter().any(|cmd| {
            let bytes = cmd.as_bytes();
            bytes.len() >= 3
                && (bytes[0] == b'P' || bytes[0] == b'p')
                && (bytes[1] == b'C' || bytes[1] == b'c')
                && (bytes[2] == b'L' || bytes[2] == b'l')
        })
    }

    /// Check if the printer supports PostScript.
    pub fn supports_postscript(&self) -> bool {
        self.command_set.iter().any(|cmd| {
            let bytes = cmd.as_bytes();
            bytes.len() >= 2
                && (bytes[0] == b'P' || bytes[0] == b'p')
                && (bytes[1] == b'S' || bytes[1] == b's')
        })
    }
}

// ---------------------------------------------------------------------------
// Print job
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintLanguage {
    Raw,
    Pcl,
    PostScript,
    Escpos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Pending,
    Sending,
    Complete,
    Error,
    Cancelled,
}

pub struct PrintJob {
    pub job_id: u32,
    pub state: JobState,
    pub language: PrintLanguage,
    pub data: Vec<u8>,
    pub bytes_sent: usize,
    pub total_bytes: usize,
}

impl PrintJob {
    pub fn new(job_id: u32, language: PrintLanguage, data: Vec<u8>) -> Self {
        let total = data.len();
        PrintJob {
            job_id,
            state: JobState::Pending,
            language,
            data,
            bytes_sent: 0,
            total_bytes: total,
        }
    }

    /// Get the next chunk of data to send via bulk OUT.
    pub fn next_chunk(&mut self, max_size: usize) -> Option<&[u8]> {
        if self.bytes_sent >= self.total_bytes {
            return None;
        }
        let end = (self.bytes_sent + max_size).min(self.total_bytes);
        let chunk = &self.data[self.bytes_sent..end];
        Some(chunk)
    }

    /// Acknowledge that a chunk was successfully sent.
    pub fn ack_chunk(&mut self, bytes: usize) {
        self.bytes_sent += bytes;
        self.state = JobState::Sending;
        if self.bytes_sent >= self.total_bytes {
            self.state = JobState::Complete;
        }
    }

    /// Progress as Q16 fixed-point fraction (0..65536).
    pub fn progress_q16(&self) -> i32 {
        if self.total_bytes == 0 {
            return 1 << 16;
        }
        ((self.bytes_sent as i64 * (1 << 16)) / self.total_bytes as i64) as i32
    }

    /// Cancel the job.
    pub fn cancel(&mut self) {
        self.state = JobState::Cancelled;
    }

    /// Build a minimal PCL reset prefix.
    pub fn pcl_reset_prefix() -> Vec<u8> {
        // ESC E = PCL reset
        alloc::vec![0x1B, 0x45]
    }

    /// Build a minimal PostScript header.
    pub fn postscript_header() -> Vec<u8> {
        let header = b"%!PS-Adobe-3.0\n";
        header.to_vec()
    }

    /// Build a PCL reset suffix.
    pub fn pcl_reset_suffix() -> Vec<u8> {
        // ESC E = reset, FF = form feed
        alloc::vec![0x0C, 0x1B, 0x45]
    }
}

// ---------------------------------------------------------------------------
// Printer device
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrinterState {
    Detached,
    Attached,
    Ready,
    Busy,
    Error,
}

pub struct PrinterDevice {
    pub slot_id: u8,
    pub state: PrinterState,
    pub protocol: u8,
    pub bulk_in_ep: u8,
    pub bulk_out_ep: u8,
    pub interface_num: u8,
    pub alternate_setting: u8,
    pub device_id: DeviceId,
    pub port_status: PortStatus,
    pub jobs: Vec<PrintJob>,
    pub next_job_id: u32,
    pub max_packet_size: u16,
}

impl PrinterDevice {
    pub fn new(slot_id: u8, bulk_out: u8, protocol: u8) -> Self {
        PrinterDevice {
            slot_id,
            state: PrinterState::Attached,
            protocol,
            bulk_in_ep: 0,
            bulk_out_ep: bulk_out,
            interface_num: 0,
            alternate_setting: 0,
            device_id: DeviceId::new(),
            port_status: PortStatus::from_raw(0),
            jobs: Vec::new(),
            next_job_id: 1,
            max_packet_size: 64,
        }
    }

    /// Set the bulk IN endpoint (for bidirectional printers).
    pub fn set_bulk_in(&mut self, ep: u8) {
        self.bulk_in_ep = ep;
    }

    /// Is this printer bidirectional?
    pub fn is_bidirectional(&self) -> bool {
        self.protocol == PROTOCOL_BIDIRECTIONAL || self.protocol == PROTOCOL_IEEE1284_4
    }

    /// Update port status from a GET_PORT_STATUS response.
    pub fn update_port_status(&mut self, raw: u8) {
        self.port_status = PortStatus::from_raw(raw);
        if self.port_status.ready() {
            self.state = PrinterState::Ready;
        } else if !self.port_status.no_error() {
            self.state = PrinterState::Error;
        }
    }

    /// Parse and store the IEEE 1284 device ID.
    pub fn parse_device_id(&mut self, data: &[u8]) {
        self.device_id = DeviceId::parse(data);
    }

    /// Choose the best print language based on device capabilities.
    pub fn best_language(&self) -> PrintLanguage {
        if self.device_id.supports_postscript() {
            PrintLanguage::PostScript
        } else if self.device_id.supports_pcl() {
            PrintLanguage::Pcl
        } else {
            PrintLanguage::Raw
        }
    }

    /// Submit a new print job.
    pub fn submit_job(&mut self, language: PrintLanguage, data: Vec<u8>) -> u32 {
        let id = self.next_job_id;
        self.next_job_id = self.next_job_id.saturating_add(1);
        self.jobs.push(PrintJob::new(id, language, data));
        id
    }

    /// Submit raw data (auto-detect language).
    pub fn submit_raw(&mut self, data: Vec<u8>) -> u32 {
        let lang = self.best_language();
        self.submit_job(lang, data)
    }

    /// Get the current active job (first non-complete job).
    pub fn active_job(&mut self) -> Option<&mut PrintJob> {
        self.jobs
            .iter_mut()
            .find(|j| j.state == JobState::Pending || j.state == JobState::Sending)
    }

    /// Remove completed and cancelled jobs.
    pub fn cleanup_jobs(&mut self) {
        self.jobs
            .retain(|j| j.state == JobState::Pending || j.state == JobState::Sending);
    }

    /// Build a GET_DEVICE_ID class request setup data.
    pub fn build_get_device_id_request(&self) -> [u8; 8] {
        [
            0xA1, // bmRequestType: class, interface, device-to-host
            GET_DEVICE_ID,
            0x00,
            0x00, // wValue: config index
            self.interface_num,
            self.alternate_setting, // wIndex
            0x00,
            0xFF, // wLength: 255 max
        ]
    }

    /// Build a GET_PORT_STATUS class request setup data.
    pub fn build_get_port_status_request(&self) -> [u8; 8] {
        [
            0xA1,
            GET_PORT_STATUS,
            0x00,
            0x00,
            self.interface_num,
            0x00,
            0x01,
            0x00, // wLength: 1 byte
        ]
    }

    /// Build a SOFT_RESET class request setup data.
    pub fn build_soft_reset_request(&self) -> [u8; 8] {
        [
            0x21, // bmRequestType: class, interface, host-to-device
            SOFT_RESET,
            0x00,
            0x00,
            self.interface_num,
            0x00,
            0x00,
            0x00, // wLength: 0
        ]
    }

    /// Process data received from bidirectional printer (bulk IN).
    pub fn receive_status(&self, data: &[u8]) -> Option<String> {
        if data.is_empty() {
            return None;
        }
        Some(String::from_utf8_lossy(data).into())
    }
}

// ---------------------------------------------------------------------------
// Class identification
// ---------------------------------------------------------------------------

pub fn is_printer(class: u8, subclass: u8) -> bool {
    class == CLASS_PRINTER && subclass == SUBCLASS_PRINTER
}

pub fn is_bidirectional_printer(class: u8, subclass: u8, protocol: u8) -> bool {
    is_printer(class, subclass)
        && (protocol == PROTOCOL_BIDIRECTIONAL || protocol == PROTOCOL_IEEE1284_4)
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    let mut state = PRINTER_STATE.lock();
    *state = Some(PrinterClassState::new());
    serial_println!("    [printer] USB Printer Class driver loaded");
}
