/// USB CDC-ACM Serial Driver — Abstract Control Model (virtual COM port)
///
/// CDC-ACM (USB Communications Device Class, Abstract Control Model) presents
/// a virtual serial port to the host OS.  The host sees a standard COM/tty
/// device; the USB device manages RX/TX queues and control-line state (DTR/RTS).
///
/// This driver manages up to MAX_ACM_PORTS concurrent virtual COM ports using
/// fixed-size static ring buffers.  No Vec, Box, String, or alloc calls
/// anywhere in this module.
///
/// Ring buffer convention: head = write cursor, tail = read cursor.
///   - Empty: head == tail
///   - Full:  (head.wrapping_sub(tail) & (BUF_SIZE - 1)) == BUF_SIZE - 1
///             (equivalently, (head + 1) % BUF_SIZE == tail % BUF_SIZE)
///   - Index: ptr % BUF_SIZE
///   - Advance: ptr.wrapping_add(1)
///
/// Rules enforced:
///   - No heap (no Vec, Box, String, alloc::*)
///   - No floats (no as f32 / as f64)
///   - No panics (no unwrap, expect, panic!)
///   - Counters: saturating_add / saturating_sub
///   - Sequence numbers: wrapping_add
///   - No division without guarding divisor != 0
use crate::serial_println;
use crate::sync::Mutex;

// ============================================================================
// Constants
// ============================================================================

/// USB class code for Communications Device Class
pub const CDC_ACM_CLASS: u8 = 0x02;

/// Abstract Control Model subclass
pub const CDC_ACM_SUBCLASS: u8 = 0x02;

/// Maximum simultaneous virtual COM ports
pub const MAX_ACM_PORTS: usize = 8;

/// RX ring buffer size (must be a power of two for the modulo to be cheap)
pub const ACM_RX_BUF: usize = 2048;

/// TX ring buffer size
pub const ACM_TX_BUF: usize = 2048;

// ============================================================================
// Line encoding
// ============================================================================

/// CDC SET_LINE_CODING / GET_LINE_CODING payload.
#[derive(Copy, Clone)]
pub struct AcmLineEncoding {
    /// Baud rate (bits per second)
    pub baud: u32,
    /// Stop bits: 0 = 1 stop, 1 = 1.5 stop, 2 = 2 stop
    pub stop_bits: u8,
    /// Parity: 0 = None, 1 = Odd, 2 = Even
    pub parity: u8,
    /// Data bits: 5, 6, 7, 8, or 16
    pub data_bits: u8,
}

impl AcmLineEncoding {
    /// Standard 115200-8N1 default.
    pub const fn default() -> Self {
        AcmLineEncoding {
            baud: 115200,
            stop_bits: 0,
            parity: 0,
            data_bits: 8,
        }
    }
}

// ============================================================================
// Port record
// ============================================================================

/// Per-port state for a CDC-ACM virtual COM port.
///
/// All fields are plain data — no heap allocations.  The struct is `Copy` so
/// it can be placed in a `static Mutex<[AcmPort; N]>`.
#[derive(Copy, Clone)]
pub struct AcmPort {
    /// Port identifier
    pub port_id: u32,
    /// Serial line encoding (baud, stop bits, parity, data bits)
    pub line_encoding: AcmLineEncoding,
    /// RX ring buffer storage
    pub rx_buf: [u8; ACM_RX_BUF],
    /// RX write cursor (wrapping)
    pub rx_head: usize,
    /// RX read cursor (wrapping)
    pub rx_tail: usize,
    /// TX ring buffer storage
    pub tx_buf: [u8; ACM_TX_BUF],
    /// TX write cursor (wrapping)
    pub tx_head: usize,
    /// TX read cursor (wrapping)
    pub tx_tail: usize,
    /// Data Terminal Ready control line
    pub dtr: bool,
    /// Request To Send control line
    pub rts: bool,
    /// Port is open and operational
    pub active: bool,
}

impl AcmPort {
    /// Construct a zero-initialised, inactive port slot.
    pub const fn empty() -> Self {
        AcmPort {
            port_id: 0,
            line_encoding: AcmLineEncoding::default(),
            rx_buf: [0u8; ACM_RX_BUF],
            rx_head: 0,
            rx_tail: 0,
            tx_buf: [0u8; ACM_TX_BUF],
            tx_head: 0,
            tx_tail: 0,
            dtr: false,
            rts: false,
            active: false,
        }
    }
}

// AcmPort contains only plain integer types and byte arrays.
unsafe impl Send for AcmPort {}

// ============================================================================
// Global port table
// ============================================================================

/// Protected table of ACM port slots.
static ACM_PORTS: Mutex<[AcmPort; MAX_ACM_PORTS]> = Mutex::new([AcmPort::empty(); MAX_ACM_PORTS]);

// ============================================================================
// Ring buffer helpers
// ============================================================================

/// Returns `true` when the RX ring is full (cannot accept another byte).
///
/// Full condition: head is exactly one slot behind tail (mod BUF_SIZE).
#[inline]
fn rx_full(head: usize, tail: usize) -> bool {
    head.wrapping_sub(tail) >= ACM_RX_BUF
}

/// Returns `true` when the TX ring is full.
#[inline]
fn tx_full(head: usize, tail: usize) -> bool {
    head.wrapping_sub(tail) >= ACM_TX_BUF
}

/// Returns the number of bytes available for reading in the RX ring.
#[inline]
fn rx_available(head: usize, tail: usize) -> usize {
    head.wrapping_sub(tail) % (ACM_RX_BUF.wrapping_add(1).max(1))
}

/// Returns the number of bytes waiting for transmission in the TX ring.
#[inline]
fn tx_pending(head: usize, tail: usize) -> usize {
    head.wrapping_sub(tail) % (ACM_TX_BUF.wrapping_add(1).max(1))
}

// ============================================================================
// Public API
// ============================================================================

/// Open a port slot for the given `port_id`.
///
/// If the slot is already active this is a no-op and returns `true`.
/// Returns `false` if all slots are taken (port_id is assigned to whichever
/// free slot is found first; if port_id already exists, that slot is reused).
pub fn acm_open(port_id: u32) -> bool {
    let mut ports = ACM_PORTS.lock();

    // Check if port_id is already open
    for port in ports.iter() {
        if port.active && port.port_id == port_id {
            return true;
        }
    }

    // Find a free slot
    let mut free_idx: Option<usize> = None;
    for (i, port) in ports.iter().enumerate() {
        if !port.active {
            free_idx = Some(i);
            break;
        }
    }

    match free_idx {
        Some(idx) => {
            let port = &mut ports[idx];
            *port = AcmPort::empty();
            port.port_id = port_id;
            port.active = true;
            serial_println!("[usb_cdc_acm] port {} opened", port_id);
            true
        }
        None => {
            serial_println!("[usb_cdc_acm] acm_open: no free slots for port {}", port_id);
            false
        }
    }
}

/// Close a port, releasing its slot back to the pool.
pub fn acm_close(port_id: u32) {
    let mut ports = ACM_PORTS.lock();
    for port in ports.iter_mut() {
        if port.active && port.port_id == port_id {
            *port = AcmPort::empty();
            serial_println!("[usb_cdc_acm] port {} closed", port_id);
            return;
        }
    }
    serial_println!("[usb_cdc_acm] acm_close: port {} not found", port_id);
}

/// Apply a new line encoding to the port.
///
/// Returns `false` if the port is not found.
pub fn acm_set_line_encoding(port_id: u32, enc: AcmLineEncoding) -> bool {
    let mut ports = ACM_PORTS.lock();
    for port in ports.iter_mut() {
        if port.active && port.port_id == port_id {
            port.line_encoding = enc;
            return true;
        }
    }
    false
}

/// Retrieve the current line encoding of the port.
///
/// Returns `None` if the port is not found.
pub fn acm_get_line_encoding(port_id: u32) -> Option<AcmLineEncoding> {
    let ports = ACM_PORTS.lock();
    for port in ports.iter() {
        if port.active && port.port_id == port_id {
            return Some(port.line_encoding);
        }
    }
    None
}

/// Enqueue `data` bytes into the TX ring buffer.
///
/// Stops as soon as the ring is full.  Returns the number of bytes actually
/// queued (may be less than `data.len()` if the buffer fills up).
pub fn acm_write(port_id: u32, data: &[u8]) -> usize {
    let mut ports = ACM_PORTS.lock();
    for port in ports.iter_mut() {
        if port.active && port.port_id == port_id {
            let mut queued = 0usize;
            for &byte in data.iter() {
                if tx_full(port.tx_head, port.tx_tail) {
                    break;
                }
                port.tx_buf[port.tx_head % ACM_TX_BUF] = byte;
                port.tx_head = port.tx_head.wrapping_add(1);
                queued = queued.saturating_add(1);
            }
            return queued;
        }
    }
    0
}

/// Dequeue bytes from the RX ring buffer into `out`.
///
/// Returns the number of bytes copied.
pub fn acm_read(port_id: u32, out: &mut [u8]) -> usize {
    let mut ports = ACM_PORTS.lock();
    for port in ports.iter_mut() {
        if port.active && port.port_id == port_id {
            let mut read = 0usize;
            while read < out.len() {
                if port.rx_head == port.rx_tail {
                    // Ring is empty
                    break;
                }
                out[read] = port.rx_buf[port.rx_tail % ACM_RX_BUF];
                port.rx_tail = port.rx_tail.wrapping_add(1);
                read = read.saturating_add(1);
            }
            return read;
        }
    }
    0
}

/// Test helper: inject bytes directly into the RX ring as if they arrived
/// from the USB host.
///
/// Bytes that do not fit (ring full) are silently discarded.
pub fn acm_inject_rx(port_id: u32, data: &[u8]) {
    let mut ports = ACM_PORTS.lock();
    for port in ports.iter_mut() {
        if port.active && port.port_id == port_id {
            for &byte in data.iter() {
                if rx_full(port.rx_head, port.rx_tail) {
                    break;
                }
                port.rx_buf[port.rx_head % ACM_RX_BUF] = byte;
                port.rx_head = port.rx_head.wrapping_add(1);
            }
            return;
        }
    }
    serial_println!("[usb_cdc_acm] inject_rx: port {} not found", port_id);
}

/// Drain the TX ring and return the number of bytes flushed.
///
/// In a real driver this would submit the bytes to the USB bulk-IN endpoint.
/// Here it simply advances the TX tail (discarding the data) so that the
/// buffer is freed.
pub fn acm_tx_flush(port_id: u32) -> usize {
    let mut ports = ACM_PORTS.lock();
    for port in ports.iter_mut() {
        if port.active && port.port_id == port_id {
            let count = tx_pending(port.tx_head, port.tx_tail);
            // Advance tail all the way to head, discarding buffered bytes
            port.tx_tail = port.tx_head;
            return count;
        }
    }
    0
}

/// Set DTR and RTS control lines for the port.
pub fn acm_set_control(port_id: u32, dtr: bool, rts: bool) {
    let mut ports = ACM_PORTS.lock();
    for port in ports.iter_mut() {
        if port.active && port.port_id == port_id {
            port.dtr = dtr;
            port.rts = rts;
            return;
        }
    }
    serial_println!("[usb_cdc_acm] set_control: port {} not found", port_id);
}

// ============================================================================
// Module init
// ============================================================================

/// Initialise the CDC-ACM driver.
///
/// Opens port 0 as the default debug serial port.
pub fn init() {
    let _ = acm_open(0);
    serial_println!("[usb_cdc_acm] CDC-ACM serial driver initialized");
    super::register("usb-cdc-acm", super::DeviceType::Serial);
}

// ============================================================================
// Internal: ring-buffer availability helpers (used by tests / diagnostics)
// ============================================================================

/// Return the number of bytes available in the RX ring of `port_id`.
///
/// Returns 0 if the port is not found.
pub fn acm_rx_available(port_id: u32) -> usize {
    let ports = ACM_PORTS.lock();
    for port in ports.iter() {
        if port.active && port.port_id == port_id {
            return rx_available(port.rx_head, port.rx_tail);
        }
    }
    0
}

/// Return the number of bytes queued in the TX ring of `port_id`.
///
/// Returns 0 if the port is not found.
pub fn acm_tx_queued(port_id: u32) -> usize {
    let ports = ACM_PORTS.lock();
    for port in ports.iter() {
        if port.active && port.port_id == port_id {
            return tx_pending(port.tx_head, port.tx_tail);
        }
    }
    0
}
