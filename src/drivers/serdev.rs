/// serdev — serial device bus
///
/// Glue layer between UART host controllers and the protocol drivers
/// that attach to serial-connected peripherals (GPS modules, BT HCI UART,
/// GNSS chips, modem AT command ports, etc.).
///
/// Design:
///   - SerdevController: represents one UART host (name, base_port, baud)
///   - SerdevDevice: a protocol driver attached to a controller
///   - SerdevBus: up to 8 controllers, 16 devices
///   - Data path: serdev_send() → controller TX; serdev_push_rx() → device callback
///   - Baud-rate negotiation: device requests baud, controller applies it
///
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const SERDEV_MAX_CTRL: usize = 8;
pub const SERDEV_MAX_DEV: usize = 16;
pub const SERDEV_RX_BUF_LEN: usize = 256;
pub const SERDEV_TX_BUF_LEN: usize = 256;

// ---------------------------------------------------------------------------
// Parity / flow control
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SerdevParity {
    None,
    Odd,
    Even,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SerdevFlow {
    None,
    HwRtsCts,
    SwXonXoff,
}

// ---------------------------------------------------------------------------
// Controller (UART host adapter)
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct SerdevController {
    pub id: u32,
    pub name: [u8; 16],
    pub name_len: u8,
    pub base_port: u16, // I/O port base (e.g. 0x3F8 for COM1)
    pub baud: u32,
    pub parity: SerdevParity,
    pub data_bits: u8,
    pub stop_bits: u8,
    pub flow: SerdevFlow,
    pub valid: bool,
    // Simulated RX ring buffer
    pub rx_buf: [u8; SERDEV_RX_BUF_LEN],
    pub rx_head: u16,
    pub rx_tail: u16,
    // Simulated TX ring buffer
    pub tx_buf: [u8; SERDEV_TX_BUF_LEN],
    pub tx_head: u16,
    pub tx_tail: u16,
}

impl SerdevController {
    pub const fn empty() -> Self {
        SerdevController {
            id: 0,
            name: [0u8; 16],
            name_len: 0,
            base_port: 0,
            baud: 115200,
            parity: SerdevParity::None,
            data_bits: 8,
            stop_bits: 1,
            flow: SerdevFlow::None,
            valid: false,
            rx_buf: [0u8; SERDEV_RX_BUF_LEN],
            rx_head: 0,
            rx_tail: 0,
            tx_buf: [0u8; SERDEV_TX_BUF_LEN],
            tx_head: 0,
            tx_tail: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Device (protocol driver attached to one controller)
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct SerdevDevice {
    pub id: u32,
    pub ctrl_id: u32, // which controller this is attached to
    pub name: [u8; 24],
    pub name_len: u8,
    pub baud: u32, // requested baud (may differ from controller default)
    pub open: bool,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_overflow: u32,
    pub valid: bool,
}

impl SerdevDevice {
    pub const fn empty() -> Self {
        SerdevDevice {
            id: 0,
            ctrl_id: 0,
            name: [0u8; 24],
            name_len: 0,
            baud: 115200,
            open: false,
            rx_bytes: 0,
            tx_bytes: 0,
            rx_overflow: 0,
            valid: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Static tables
// ---------------------------------------------------------------------------

static CONTROLLERS: Mutex<[SerdevController; SERDEV_MAX_CTRL]> =
    Mutex::new([SerdevController::empty(); SERDEV_MAX_CTRL]);
static DEVICES: Mutex<[SerdevDevice; SERDEV_MAX_DEV]> =
    Mutex::new([SerdevDevice::empty(); SERDEV_MAX_DEV]);
static CTRL_NEXT_ID: AtomicU32 = AtomicU32::new(1);
static DEV_NEXT_ID: AtomicU32 = AtomicU32::new(1);

// ---------------------------------------------------------------------------
// Name copy helper
// ---------------------------------------------------------------------------

fn copy_name16(dst: &mut [u8; 16], src: &[u8]) -> u8 {
    let n = src.len().min(15);
    let mut i = 0usize;
    while i < n {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    n as u8
}

fn copy_name24(dst: &mut [u8; 24], src: &[u8]) -> u8 {
    let n = src.len().min(23);
    let mut i = 0usize;
    while i < n {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    n as u8
}

// ---------------------------------------------------------------------------
// Controller registration
// ---------------------------------------------------------------------------

/// Register a UART host controller. Returns controller id or 0 on failure.
pub fn serdev_register_ctrl(
    name: &[u8],
    base_port: u16,
    baud: u32,
    parity: SerdevParity,
    data_bits: u8,
    stop_bits: u8,
    flow: SerdevFlow,
) -> u32 {
    let mut table = CONTROLLERS.lock();
    let mut i = 0usize;
    while i < SERDEV_MAX_CTRL {
        if !table[i].valid {
            let id = CTRL_NEXT_ID.fetch_add(1, Ordering::Relaxed);
            table[i] = SerdevController::empty();
            table[i].id = id;
            table[i].name_len = copy_name16(&mut table[i].name, name);
            table[i].base_port = base_port;
            table[i].baud = baud;
            table[i].parity = parity;
            table[i].data_bits = data_bits;
            table[i].stop_bits = stop_bits;
            table[i].flow = flow;
            table[i].valid = true;
            return id;
        }
        i = i.saturating_add(1);
    }
    0
}

/// Unregister a controller (only if no devices attached).
pub fn serdev_unregister_ctrl(ctrl_id: u32) -> bool {
    // Check no devices attached
    {
        let devs = DEVICES.lock();
        let mut i = 0usize;
        while i < SERDEV_MAX_DEV {
            if devs[i].valid && devs[i].ctrl_id == ctrl_id {
                return false;
            }
            i = i.saturating_add(1);
        }
    }
    let mut table = CONTROLLERS.lock();
    let mut i = 0usize;
    while i < SERDEV_MAX_CTRL {
        if table[i].id == ctrl_id && table[i].valid {
            table[i] = SerdevController::empty();
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

// ---------------------------------------------------------------------------
// Device registration
// ---------------------------------------------------------------------------

/// Attach a protocol device to a controller. Returns device id or 0.
pub fn serdev_add_device(name: &[u8], ctrl_id: u32, baud: u32) -> u32 {
    // Verify controller exists
    {
        let ctrls = CONTROLLERS.lock();
        let mut found = false;
        let mut i = 0usize;
        while i < SERDEV_MAX_CTRL {
            if ctrls[i].id == ctrl_id && ctrls[i].valid {
                found = true;
                break;
            }
            i = i.saturating_add(1);
        }
        if !found {
            return 0;
        }
    }
    let mut table = DEVICES.lock();
    let mut i = 0usize;
    while i < SERDEV_MAX_DEV {
        if !table[i].valid {
            let id = DEV_NEXT_ID.fetch_add(1, Ordering::Relaxed);
            table[i] = SerdevDevice::empty();
            table[i].id = id;
            table[i].ctrl_id = ctrl_id;
            table[i].name_len = copy_name24(&mut table[i].name, name);
            table[i].baud = baud;
            table[i].valid = true;
            return id;
        }
        i = i.saturating_add(1);
    }
    0
}

/// Remove a device.
pub fn serdev_remove_device(dev_id: u32) -> bool {
    let mut table = DEVICES.lock();
    let mut i = 0usize;
    while i < SERDEV_MAX_DEV {
        if table[i].id == dev_id && table[i].valid {
            table[i] = SerdevDevice::empty();
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

// ---------------------------------------------------------------------------
// Open / close
// ---------------------------------------------------------------------------

pub fn serdev_open(dev_id: u32) -> bool {
    let mut table = DEVICES.lock();
    let mut i = 0usize;
    while i < SERDEV_MAX_DEV {
        if table[i].id == dev_id && table[i].valid && !table[i].open {
            table[i].open = true;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn serdev_close(dev_id: u32) -> bool {
    let mut table = DEVICES.lock();
    let mut i = 0usize;
    while i < SERDEV_MAX_DEV {
        if table[i].id == dev_id && table[i].valid && table[i].open {
            table[i].open = false;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

// ---------------------------------------------------------------------------
// TX: device sends to controller
// ---------------------------------------------------------------------------

/// Write bytes from a device to its controller's TX ring buffer.
/// Returns bytes actually enqueued.
pub fn serdev_send(dev_id: u32, data: &[u8]) -> usize {
    // Find device and its ctrl_id
    let ctrl_id = {
        let devs = DEVICES.lock();
        let mut cid = 0u32;
        let mut i = 0usize;
        while i < SERDEV_MAX_DEV {
            if devs[i].id == dev_id && devs[i].valid && devs[i].open {
                cid = devs[i].ctrl_id;
                break;
            }
            i = i.saturating_add(1);
        }
        cid
    };
    if ctrl_id == 0 {
        return 0;
    }

    let mut ctrls = CONTROLLERS.lock();
    let mut ci = 0usize;
    while ci < SERDEV_MAX_CTRL {
        if ctrls[ci].id == ctrl_id && ctrls[ci].valid {
            let mut written = 0usize;
            let mut k = 0usize;
            while k < data.len() {
                let head_idx = ctrls[ci].tx_head as usize;
                let next = (head_idx + 1) % SERDEV_TX_BUF_LEN;
                if next == ctrls[ci].tx_tail as usize {
                    break;
                } // full
                ctrls[ci].tx_buf[head_idx] = data[k];
                ctrls[ci].tx_head = next as u16;
                written = written.saturating_add(1);
                k = k.saturating_add(1);
            }
            // Update device TX stats
            drop(ctrls);
            let mut devs = DEVICES.lock();
            let mut i = 0usize;
            while i < SERDEV_MAX_DEV {
                if devs[i].id == dev_id {
                    devs[i].tx_bytes = devs[i].tx_bytes.wrapping_add(written as u64);
                    break;
                }
                i = i.saturating_add(1);
            }
            return written;
        }
        ci = ci.saturating_add(1);
    }
    0
}

// ---------------------------------------------------------------------------
// RX: push bytes from UART interrupt into controller's RX ring
// ---------------------------------------------------------------------------

/// Called by UART IRQ handler to push received bytes into controller ring.
pub fn serdev_push_rx(ctrl_id: u32, data: &[u8]) {
    let mut ctrls = CONTROLLERS.lock();
    let mut ci = 0usize;
    while ci < SERDEV_MAX_CTRL {
        if ctrls[ci].id == ctrl_id && ctrls[ci].valid {
            let mut k = 0usize;
            while k < data.len() {
                let head_idx = ctrls[ci].rx_head as usize;
                let next = (head_idx + 1) % SERDEV_RX_BUF_LEN;
                if next == ctrls[ci].rx_tail as usize {
                    // overflow: drop byte (in a real driver we'd notify the device)
                    break;
                }
                ctrls[ci].rx_buf[head_idx] = data[k];
                ctrls[ci].rx_head = next as u16;
                k = k.saturating_add(1);
            }
            return;
        }
        ci = ci.saturating_add(1);
    }
}

/// Drain up to `out.len()` bytes from a controller's RX ring into `out`.
/// Returns number of bytes drained.
pub fn serdev_drain_rx(ctrl_id: u32, out: &mut [u8]) -> usize {
    let mut ctrls = CONTROLLERS.lock();
    let mut ci = 0usize;
    while ci < SERDEV_MAX_CTRL {
        if ctrls[ci].id == ctrl_id && ctrls[ci].valid {
            let mut n = 0usize;
            while n < out.len() && ctrls[ci].rx_tail != ctrls[ci].rx_head {
                out[n] = ctrls[ci].rx_buf[ctrls[ci].rx_tail as usize];
                ctrls[ci].rx_tail = ((ctrls[ci].rx_tail as usize + 1) % SERDEV_RX_BUF_LEN) as u16;
                n = n.saturating_add(1);
            }
            return n;
        }
        ci = ci.saturating_add(1);
    }
    0
}

// ---------------------------------------------------------------------------
// Baud rate change
// ---------------------------------------------------------------------------

pub fn serdev_set_baud(ctrl_id: u32, baud: u32) -> bool {
    let mut ctrls = CONTROLLERS.lock();
    let mut i = 0usize;
    while i < SERDEV_MAX_CTRL {
        if ctrls[i].id == ctrl_id && ctrls[i].valid {
            ctrls[i].baud = baud;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

pub fn serdev_ctrl_count() -> usize {
    let table = CONTROLLERS.lock();
    let mut n = 0usize;
    let mut i = 0usize;
    while i < SERDEV_MAX_CTRL {
        if table[i].valid {
            n = n.saturating_add(1);
        }
        i = i.saturating_add(1);
    }
    n
}

pub fn serdev_dev_count() -> usize {
    let table = DEVICES.lock();
    let mut n = 0usize;
    let mut i = 0usize;
    while i < SERDEV_MAX_DEV {
        if table[i].valid {
            n = n.saturating_add(1);
        }
        i = i.saturating_add(1);
    }
    n
}

// ---------------------------------------------------------------------------
// Init: register standard platform serial controllers
// ---------------------------------------------------------------------------

pub fn init() {
    // COM1: 8250/16550 UART at 0x3F8
    let c1 = serdev_register_ctrl(
        b"uart8250-0",
        0x3F8,
        115200,
        SerdevParity::None,
        8,
        1,
        SerdevFlow::None,
    );
    // COM2
    let c2 = serdev_register_ctrl(
        b"uart8250-1",
        0x2F8,
        115200,
        SerdevParity::None,
        8,
        1,
        SerdevFlow::None,
    );
    // Attach simulated GPS device on COM1
    if c1 != 0 {
        serdev_add_device(b"gnss-nmea", c1, 9600);
    }
    // Attach simulated BT HCI UART on COM2
    if c2 != 0 {
        serdev_add_device(b"bt-hci-uart", c2, 115200);
    }
    serial_println!(
        "[serdev] serial device bus initialized ({} controllers, {} devices)",
        serdev_ctrl_count(),
        serdev_dev_count()
    );
}
