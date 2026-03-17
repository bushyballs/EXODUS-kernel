/// VirtIO Serial Console Driver — no-heap, static-buffer implementation
///
/// VirtIO console (PCI vendor 0x1AF4, device 0x1003) presents one or more
/// serial port-like "ports" to the guest via virtqueues.  Each port has an
/// independent RX ring buffer (host → guest) and TX ring buffer
/// (guest → host).
///
/// Ring buffer semantics (shared by both RX and TX):
///   head = write pointer, tail = read pointer
///   write: buf[head % BUF_SIZE] = byte;  head = head.wrapping_add(1)
///   read:  byte = buf[tail % BUF_SIZE];  tail = tail.wrapping_add(1)
///   empty: head == tail
///   full:  head.wrapping_sub(tail) >= BUF_SIZE
///
/// Public API:
///   vtcon_open(port_id)           -> bool    allocate and open a port slot
///   vtcon_close(port_id)                     release port slot
///   vtcon_write(port_id, data)    -> usize   enqueue bytes in TX ring
///   vtcon_read(port_id, out)      -> usize   dequeue bytes from RX ring
///   vtcon_inject_rx(port_id, data)           test helper: push into RX ring
///   vtcon_tx_flush(port_id)       -> usize   drain TX ring (simulate send)
///   init()                                   called by drivers::init()
///
/// SAFETY RULES:
///   - No as f32 / as f64
///   - saturating_add/saturating_sub for counters
///   - wrapping_add/wrapping_sub for ring head/tail indices
///   - read_volatile/write_volatile for MMIO/shared-ring accesses
///   - No panic — use serial_println! + return false/0 on fatal errors
///   - No Vec, Box, String, alloc::* — fixed-size static arrays only
use crate::serial_println;
use crate::sync::Mutex;

// ============================================================================
// PCI IDs
// ============================================================================

pub const VIRTIO_CONSOLE_VENDOR: u16 = 0x1AF4;
pub const VIRTIO_CONSOLE_DEV_ID: u16 = 0x1003;

// ============================================================================
// Buffer / port constants
// ============================================================================

pub const VTCON_RX_BUF: usize = 4096;
pub const VTCON_TX_BUF: usize = 4096;
pub const MAX_VTCON_PORTS: usize = 4;

// ============================================================================
// Per-port state
// ============================================================================

/// State for a single VirtIO console port.
///
/// `head` is the write pointer; `tail` is the read pointer.  Both are
/// maintained as `usize` wrapping counters — the actual buffer index is
/// `ptr % BUF_SIZE`.  This avoids any branch for wrap-around.
///
/// `active` — allocated slot in VTCON_PORTS
/// `open`   — port has been opened by the caller (vtcon_open called)
#[derive(Copy, Clone)]
pub struct VtconPort {
    /// Logical port identifier assigned by the caller
    pub port_id: u32,
    /// Receive buffer (host → guest)
    pub rx_buf: [u8; VTCON_RX_BUF],
    /// RX write pointer (host writes here)
    pub rx_head: usize,
    /// RX read pointer (guest reads from here)
    pub rx_tail: usize,
    /// Transmit buffer (guest → host)
    pub tx_buf: [u8; VTCON_TX_BUF],
    /// TX write pointer (guest writes here)
    pub tx_head: usize,
    /// TX read pointer (simulated host drains from here)
    pub tx_tail: usize,
    /// Port has been opened (vtcon_open was called)
    pub open: bool,
    /// Slot is in use
    pub active: bool,
    /// Lifetime bytes received (saturating counter)
    pub rx_total: u64,
    /// Lifetime bytes transmitted (saturating counter)
    pub tx_total: u64,
}

impl VtconPort {
    pub const fn empty() -> Self {
        VtconPort {
            port_id: 0,
            rx_buf: [0u8; VTCON_RX_BUF],
            rx_head: 0,
            rx_tail: 0,
            tx_buf: [0u8; VTCON_TX_BUF],
            tx_head: 0,
            tx_tail: 0,
            open: false,
            active: false,
            rx_total: 0,
            tx_total: 0,
        }
    }
}

// ============================================================================
// Ring buffer helpers (inline, operate on raw fields)
// ============================================================================

/// Number of bytes available to read from an RX ring.
#[inline]
fn rx_available(port: &VtconPort) -> usize {
    port.rx_head.wrapping_sub(port.rx_tail)
}

/// Number of free bytes in the RX ring (space for writes from host).
#[inline]
fn rx_free(port: &VtconPort) -> usize {
    VTCON_RX_BUF.saturating_sub(rx_available(port))
}

/// Number of bytes available to read from a TX ring (i.e., waiting to be sent).
#[inline]
fn tx_pending(port: &VtconPort) -> usize {
    port.tx_head.wrapping_sub(port.tx_tail)
}

/// Number of free bytes in the TX ring (space for writes from guest).
#[inline]
fn tx_free(port: &VtconPort) -> usize {
    VTCON_TX_BUF.saturating_sub(tx_pending(port))
}

// ============================================================================
// Global port table
// ============================================================================

static VTCON_PORTS: Mutex<[VtconPort; MAX_VTCON_PORTS]> =
    Mutex::new([VtconPort::empty(); MAX_VTCON_PORTS]);

// ============================================================================
// PCI device presence flag
// ============================================================================

use core::sync::atomic::{AtomicBool, Ordering};

static VTCON_PRESENT: AtomicBool = AtomicBool::new(false);

// ============================================================================
// Public: open / close
// ============================================================================

/// Open a console port with the given `port_id`.
///
/// Finds a free slot in `VTCON_PORTS`, marks it active+open, and assigns
/// `port_id`.  Returns `true` on success, `false` if all slots are taken or
/// the port_id is already open.
pub fn vtcon_open(port_id: u32) -> bool {
    let mut ports = VTCON_PORTS.lock();

    // Reject if port_id is already open
    for slot in ports.iter() {
        if slot.active && slot.open && slot.port_id == port_id {
            serial_println!("[virtio_console] port {} already open", port_id);
            return false;
        }
    }

    // Find a free slot
    for slot in ports.iter_mut() {
        if !slot.active {
            // Reset the slot before handing it out
            *slot = VtconPort::empty();
            slot.port_id = port_id;
            slot.active = true;
            slot.open = true;
            return true;
        }
    }

    serial_println!("[virtio_console] no free port slots for port {}", port_id);
    false
}

/// Close a port and release its slot.
///
/// Does nothing if the port is not found.
pub fn vtcon_close(port_id: u32) {
    let mut ports = VTCON_PORTS.lock();
    for slot in ports.iter_mut() {
        if slot.active && slot.port_id == port_id {
            *slot = VtconPort::empty(); // clears active + open flags
            return;
        }
    }
}

// ============================================================================
// Public: write to TX ring
// ============================================================================

/// Write `data` bytes into the TX ring buffer of `port_id`.
///
/// Bytes that would overflow the ring (when `tx_free == 0`) are silently
/// dropped — this matches the semantics of a non-blocking serial port.
///
/// Returns the number of bytes actually enqueued.
pub fn vtcon_write(port_id: u32, data: &[u8]) -> usize {
    if data.is_empty() {
        return 0;
    }

    let mut ports = VTCON_PORTS.lock();
    for slot in ports.iter_mut() {
        if slot.active && slot.open && slot.port_id == port_id {
            let mut written = 0usize;
            for &byte in data.iter() {
                if tx_free(slot) == 0 {
                    break; // TX buffer full — drop remainder
                }
                let idx = slot.tx_head % VTCON_TX_BUF;
                // Bounds guard (VTCON_TX_BUF is a power of two and idx < VTCON_TX_BUF)
                if let Some(cell) = slot.tx_buf.get_mut(idx) {
                    *cell = byte;
                }
                slot.tx_head = slot.tx_head.wrapping_add(1);
                written = written.saturating_add(1);
            }
            slot.tx_total = slot.tx_total.saturating_add(written as u64);
            return written;
        }
    }
    0 // port not found or not open
}

// ============================================================================
// Public: read from RX ring
// ============================================================================

/// Drain up to `out.len()` bytes from the RX ring of `port_id` into `out`.
///
/// Returns the number of bytes actually read (0 if the ring is empty or the
/// port does not exist).
pub fn vtcon_read(port_id: u32, out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }

    let mut ports = VTCON_PORTS.lock();
    for slot in ports.iter_mut() {
        if slot.active && slot.open && slot.port_id == port_id {
            let mut read = 0usize;
            while read < out.len() {
                if rx_available(slot) == 0 {
                    break; // RX ring empty
                }
                let idx = slot.rx_tail % VTCON_RX_BUF;
                let byte = slot.rx_buf.get(idx).copied().unwrap_or(0);
                if let Some(dst) = out.get_mut(read) {
                    *dst = byte;
                }
                slot.rx_tail = slot.rx_tail.wrapping_add(1);
                read = read.saturating_add(1);
            }
            return read;
        }
    }
    0
}

// ============================================================================
// Public: inject RX data (test helper)
// ============================================================================

/// Inject `data` bytes directly into the RX ring of `port_id`.
///
/// This simulates the host pushing data to the guest over the virtqueue.
/// Bytes that would overflow the RX ring are silently dropped.
///
/// Intended for unit testing and simulation; not called in the normal
/// hardware I/O path.
pub fn vtcon_inject_rx(port_id: u32, data: &[u8]) {
    if data.is_empty() {
        return;
    }

    let mut ports = VTCON_PORTS.lock();
    for slot in ports.iter_mut() {
        if slot.active && slot.port_id == port_id {
            for &byte in data.iter() {
                if rx_free(slot) == 0 {
                    break; // RX buffer full — drop remainder
                }
                let idx = slot.rx_head % VTCON_RX_BUF;
                if let Some(cell) = slot.rx_buf.get_mut(idx) {
                    *cell = byte;
                }
                slot.rx_head = slot.rx_head.wrapping_add(1);
                slot.rx_total = slot.rx_total.saturating_add(1);
            }
            return;
        }
    }
}

// ============================================================================
// Public: flush TX ring (simulate host drain)
// ============================================================================

/// Drain all pending bytes from the TX ring of `port_id`.
///
/// In a real driver these bytes would be DMA'd to the host via the TX
/// virtqueue.  Here we simply advance `tx_tail` to `tx_head`, discarding
/// the bytes (simulating a successful send).
///
/// Returns the number of bytes drained.
pub fn vtcon_tx_flush(port_id: u32) -> usize {
    let mut ports = VTCON_PORTS.lock();
    for slot in ports.iter_mut() {
        if slot.active && slot.port_id == port_id {
            let pending = tx_pending(slot);
            slot.tx_tail = slot.tx_head; // advance tail to head — ring is now empty
            return pending;
        }
    }
    0
}

// ============================================================================
// Public: query helpers
// ============================================================================

/// Returns `true` if the PCI VirtIO console device was found at init time.
#[inline]
pub fn vtcon_is_present() -> bool {
    VTCON_PRESENT.load(Ordering::Acquire)
}

/// Returns the number of bytes waiting in the TX ring for `port_id`.
pub fn vtcon_tx_pending(port_id: u32) -> usize {
    let ports = VTCON_PORTS.lock();
    for slot in ports.iter() {
        if slot.active && slot.port_id == port_id {
            return tx_pending(slot);
        }
    }
    0
}

/// Returns the number of bytes available to read in the RX ring for `port_id`.
pub fn vtcon_rx_available(port_id: u32) -> usize {
    let ports = VTCON_PORTS.lock();
    for slot in ports.iter() {
        if slot.active && slot.port_id == port_id {
            return rx_available(slot);
        }
    }
    0
}

// ============================================================================
// Internal: PCI probe
// ============================================================================

/// Probe the PCI bus for a VirtIO console device.
///
/// Performs the minimal VirtIO legacy handshake and registers the driver.
/// Returns `true` if the device was found.
fn vtcon_probe() -> bool {
    match super::virtio::pci_find_virtio(VIRTIO_CONSOLE_VENDOR, VIRTIO_CONSOLE_DEV_ID) {
        Some((io_base, _bus, _dev, _func)) => {
            // Minimal VirtIO legacy handshake: RESET -> ACK -> DRIVER -> DRIVER_OK
            let _dev_features = super::virtio::device_begin_init(io_base);
            // Negotiate zero features (basic console needs no optional feature bits)
            let _ = super::virtio::device_set_features(io_base, 0);
            super::virtio::device_driver_ok(io_base);
            VTCON_PRESENT.store(true, Ordering::Release);
            super::register("virtio-console", super::DeviceType::Serial);
            true
        }
        None => false,
    }
}

// ============================================================================
// Module entry point — called by drivers::init()
// ============================================================================

/// Initialise the VirtIO console driver.
///
/// 1. Probes PCI for the device (optional — falls back gracefully).
/// 2. Opens port 0 as the primary console port.
/// 3. Logs the result to the serial port.
///
/// Called once during kernel boot by `drivers::init()`.
pub fn init() {
    // Probe PCI (result is informational; we proceed either way)
    let _hw_present = vtcon_probe();

    // Open port 0 as the primary console port
    if !vtcon_open(0) {
        serial_println!("[virtio_console] WARNING: could not open primary port 0");
        return;
    }

    serial_println!("[virtio_console] virtual console initialized");
}
