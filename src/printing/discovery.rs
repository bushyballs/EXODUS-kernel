use crate::sync::Mutex;
/// Printer discovery for Genesis
///
/// mDNS/DNS-SD discovery, IPP, USB printers,
/// cloud print services.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum PrinterType {
    Ipp,
    Usb,
    Bluetooth,
    CloudPrint,
}

#[derive(Clone, Copy, PartialEq)]
pub enum PrinterState {
    Idle,
    Printing,
    PaperJam,
    OutOfPaper,
    OutOfInk,
    Offline,
    Error,
}

struct Printer {
    id: u32,
    printer_type: PrinterType,
    name: [u8; 48],
    name_len: usize,
    state: PrinterState,
    supports_color: bool,
    supports_duplex: bool,
    dpi_max: u16,
    ip_addr: [u8; 4],
}

struct PrinterDiscovery {
    printers: Vec<Printer>,
    next_id: u32,
    scanning: bool,
    last_scan: u64,
}

static DISCOVERY: Mutex<Option<PrinterDiscovery>> = Mutex::new(None);

impl PrinterDiscovery {
    fn new() -> Self {
        PrinterDiscovery {
            printers: Vec::new(),
            next_id: 1,
            scanning: false,
            last_scan: 0,
        }
    }

    fn start_scan(&mut self, timestamp: u64) {
        self.scanning = true;
        self.last_scan = timestamp;
        // In real implementation: send mDNS queries, check USB, Bluetooth
    }

    fn add_printer(
        &mut self,
        name: &[u8],
        ptype: PrinterType,
        color: bool,
        duplex: bool,
        dpi: u16,
    ) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut n = [0u8; 48];
        let nlen = name.len().min(48);
        n[..nlen].copy_from_slice(&name[..nlen]);
        self.printers.push(Printer {
            id,
            printer_type: ptype,
            name: n,
            name_len: nlen,
            state: PrinterState::Idle,
            supports_color: color,
            supports_duplex: duplex,
            dpi_max: dpi,
            ip_addr: [0; 4],
        });
        id
    }

    fn get_available(&self) -> Vec<u32> {
        self.printers
            .iter()
            .filter(|p| p.state == PrinterState::Idle)
            .map(|p| p.id)
            .collect()
    }
}

pub fn init() {
    let mut d = DISCOVERY.lock();
    *d = Some(PrinterDiscovery::new());
    serial_println!("    Printing: printer discovery (IPP, USB, BT) ready");
}
