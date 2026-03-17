use crate::sync::Mutex;
/// 8250/16550 UART serial driver for Genesis — no-heap, fixed-size static arrays
///
/// Supports COM1-COM4 (0x3F8, 0x2F8, 0x3E8, 0x2E8).
/// Each port is probed via the scratch register, then configured for
/// 8N1 framing, FIFO enabled, and IRQ-driven receive.
///
/// All critical rules strictly observed:
///   - No heap: no Vec, Box, String, alloc::*
///   - No panics: no unwrap(), expect(), panic!()
///   - No float casts: no as f64, as f32
///   - Saturating arithmetic for counters
///   - Wrapping arithmetic for sequence numbers
///   - Structs in static Mutex are Copy with const fn empty()
///   - No division without divisor != 0 guard
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Port bases
// ---------------------------------------------------------------------------

pub const UART_COM1: u16 = 0x3F8;
pub const UART_COM2: u16 = 0x2F8;
pub const UART_COM3: u16 = 0x3E8;
pub const UART_COM4: u16 = 0x2E8;
pub const MAX_UART_PORTS: usize = 4;

// ---------------------------------------------------------------------------
// 8250 register offsets from base I/O port
// ---------------------------------------------------------------------------

pub const UART_RBR: u16 = 0; // Receive Buffer Register  (DLAB=0, read)
pub const UART_THR: u16 = 0; // Transmit Holding Register (DLAB=0, write)
pub const UART_DLL: u16 = 0; // Divisor Latch Low         (DLAB=1)
pub const UART_DLH: u16 = 1; // Divisor Latch High        (DLAB=1)
pub const UART_IER: u16 = 1; // Interrupt Enable Register (DLAB=0)
pub const UART_IIR: u16 = 2; // Interrupt Identification Register (read)
pub const UART_FCR: u16 = 2; // FIFO Control Register             (write)
pub const UART_LCR: u16 = 3; // Line Control Register
pub const UART_MCR: u16 = 4; // Modem Control Register
pub const UART_LSR: u16 = 5; // Line Status Register
pub const UART_MSR: u16 = 6; // Modem Status Register
const UART_SCR: u16 = 7; // Scratch Register (used for probe)

// ---------------------------------------------------------------------------
// LSR bits
// ---------------------------------------------------------------------------

pub const UART_LSR_DR: u8 = 0x01; // Data Ready
pub const UART_LSR_THRE: u8 = 0x20; // TX Holding Register Empty

// ---------------------------------------------------------------------------
// LCR bits / values
// ---------------------------------------------------------------------------

/// 8 data bits, no parity, 1 stop bit (8N1)
const UART_LCR_8N1: u8 = 0x03;
/// Divisor Latch Access Bit
const UART_LCR_DLAB: u8 = 0x80;

// ---------------------------------------------------------------------------
// FCR / MCR / IER values
// ---------------------------------------------------------------------------

/// Enable FIFO, clear both FIFOs, set 14-byte trigger level
const UART_FCR_ENABLE: u8 = 0xC7;
/// Enable Received-Data-Available interrupt only
const UART_IER_RDA: u8 = 0x01;
/// Enable loopback mode for self-test (bits 4-6 are MCR OUT1/OUT2/LOOP)
const UART_MCR_LOOPBACK: u8 = 0x1E;
/// Normal operating mode (OUT1, OUT2, RTS, DTR asserted)
const UART_MCR_NORMAL: u8 = 0x0F;

// ---------------------------------------------------------------------------
// Baud rate divisors  (base clock = 1 843 200 Hz  →  115 200 bps at div=1)
// ---------------------------------------------------------------------------

pub const UART_BAUD_115200: u16 = 1;
pub const UART_BAUD_57600: u16 = 2;
pub const UART_BAUD_38400: u16 = 3;
pub const UART_BAUD_9600: u16 = 12;

// ---------------------------------------------------------------------------
// RX ring buffer size
// ---------------------------------------------------------------------------

pub const UART_RX_BUF: usize = 1024;

// ---------------------------------------------------------------------------
// Per-port state
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct Uart8250 {
    pub base_port: u16,
    pub baud_divisor: u16,
    pub rx_buf: [u8; UART_RX_BUF],
    pub rx_head: usize, // write position (producer)
    pub rx_tail: usize, // read  position (consumer)
    pub tx_count: u64,
    pub rx_count: u64,
    pub present: bool,
    pub active: bool,
}

impl Uart8250 {
    pub const fn empty() -> Self {
        Uart8250 {
            base_port: 0,
            baud_divisor: UART_BAUD_115200,
            rx_buf: [0u8; UART_RX_BUF],
            rx_head: 0,
            rx_tail: 0,
            tx_count: 0,
            rx_count: 0,
            present: false,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static UART_PORTS: Mutex<[Uart8250; MAX_UART_PORTS]> = Mutex::new([
    Uart8250::empty(),
    Uart8250::empty(),
    Uart8250::empty(),
    Uart8250::empty(),
]);

// ---------------------------------------------------------------------------
// Low-level I/O helpers
// ---------------------------------------------------------------------------

#[inline(always)]
fn uart_outb(port: u16, offset: u16, val: u8) {
    crate::io::outb(port + offset, val);
}

#[inline(always)]
fn uart_inb(port: u16, offset: u16) -> u8 {
    crate::io::inb(port + offset)
}

// ---------------------------------------------------------------------------
// Probe
// ---------------------------------------------------------------------------

/// Probe for a UART at `base` using the scratch register (offset 7).
/// Writes 0xAB and reads back; if it matches the chip is present.
pub fn uart_probe(base: u16) -> bool {
    uart_outb(base, UART_SCR, 0xAB);
    uart_inb(base, UART_SCR) == 0xAB
}

// ---------------------------------------------------------------------------
// Initialise one port
// ---------------------------------------------------------------------------

/// Configure the UART at `base` for 8N1, the given baud divisor,
/// FIFO enabled, RDA interrupt enabled, and pass a loopback self-test.
///
/// Returns `true` if the loopback test passes (hardware is functional).
pub fn uart_init_port(base: u16, baud_divisor: u16) -> bool {
    // 1. Disable all interrupts first
    uart_outb(base, UART_IER, 0x00);

    // 2. Set DLAB=1 to access divisor latches
    uart_outb(base, UART_LCR, UART_LCR_DLAB);

    // 3. Write baud rate divisor (low byte, high byte)
    uart_outb(base, UART_DLL, (baud_divisor & 0xFF) as u8);
    uart_outb(base, UART_DLH, (baud_divisor >> 8) as u8);

    // 4. Clear DLAB, set 8N1 framing
    uart_outb(base, UART_LCR, UART_LCR_8N1);

    // 5. Enable and reset FIFOs (14-byte trigger level)
    uart_outb(base, UART_FCR, UART_FCR_ENABLE);

    // 6. Enable loopback mode for self-test
    uart_outb(base, UART_MCR, UART_MCR_LOOPBACK);

    // 7. Send a test byte and read it back through the loopback path
    uart_outb(base, UART_THR, 0xAE);
    // Spin-wait for DR bit (up to 10 000 iterations)
    let mut ok = false;
    for _ in 0..10_000u32 {
        if uart_inb(base, UART_LSR) & UART_LSR_DR != 0 {
            ok = true;
            break;
        }
        core::hint::spin_loop();
    }
    if !ok {
        // Loopback test timed out — leave port inactive
        return false;
    }
    let echo = uart_inb(base, UART_RBR);
    if echo != 0xAE {
        return false;
    }

    // 8. Switch to normal operating mode and enable RDA interrupt
    uart_outb(base, UART_MCR, UART_MCR_NORMAL);
    uart_outb(base, UART_IER, UART_IER_RDA);

    true
}

// ---------------------------------------------------------------------------
// TX
// ---------------------------------------------------------------------------

/// Write a single byte to the UART at `port_idx`.
/// Spin-waits for the TX holding register to become empty.
/// Returns `false` if the index is out of range or the port is not active.
pub fn uart_write_byte(port_idx: usize, b: u8) -> bool {
    if port_idx >= MAX_UART_PORTS {
        return false;
    }
    let mut ports = UART_PORTS.lock();
    if !ports[port_idx].active {
        return false;
    }
    let base = ports[port_idx].base_port;
    // Unlock while spin-waiting to avoid holding the lock for long.
    // We must re-acquire after the wait; drop + re-lock is the safe pattern.
    drop(ports);

    // Spin-wait for THRE outside the lock
    for _ in 0..100_000u32 {
        if crate::io::inb(base + UART_LSR) & UART_LSR_THRE != 0 {
            break;
        }
        core::hint::spin_loop();
    }

    // Write byte and update counter
    crate::io::outb(base + UART_THR, b);
    let mut ports = UART_PORTS.lock();
    ports[port_idx].tx_count = ports[port_idx].tx_count.saturating_add(1);
    true
}

/// Write a slice of bytes to the UART at `port_idx`.
/// Returns the number of bytes successfully written.
pub fn uart_write(port_idx: usize, data: &[u8]) -> usize {
    let mut written = 0usize;
    for &b in data {
        if uart_write_byte(port_idx, b) {
            written = written.saturating_add(1);
        } else {
            break;
        }
    }
    written
}

// ---------------------------------------------------------------------------
// RX
// ---------------------------------------------------------------------------

/// Check if the UART has a received byte available; if so, enqueue it into
/// the RX ring buffer and return `Some(byte)`.  Returns `None` if no data
/// is available or the port is inactive.
pub fn uart_read_byte(port_idx: usize) -> Option<u8> {
    if port_idx >= MAX_UART_PORTS {
        return None;
    }
    let mut ports = UART_PORTS.lock();
    if !ports[port_idx].active {
        return None;
    }
    let base = ports[port_idx].base_port;
    if crate::io::inb(base + UART_LSR) & UART_LSR_DR == 0 {
        return None;
    }
    let byte = crate::io::inb(base + UART_RBR);

    // Enqueue into the ring buffer
    let head = ports[port_idx].rx_head;
    let next_head = (head.wrapping_add(1)) % UART_RX_BUF;
    if next_head != ports[port_idx].rx_tail {
        // Buffer not full — store byte
        ports[port_idx].rx_buf[head] = byte;
        ports[port_idx].rx_head = next_head;
        ports[port_idx].rx_count = ports[port_idx].rx_count.saturating_add(1);
    }
    // Return the byte regardless of whether we could buffer it
    Some(byte)
}

/// Drain all immediately available RX bytes from the hardware FIFO into
/// the internal ring buffer for `port_idx`.
pub fn uart_poll_rx(port_idx: usize) {
    if port_idx >= MAX_UART_PORTS {
        return;
    }
    // Keep reading while DR bit is set
    loop {
        let mut ports = UART_PORTS.lock();
        if !ports[port_idx].active {
            return;
        }
        let base = ports[port_idx].base_port;
        if crate::io::inb(base + UART_LSR) & UART_LSR_DR == 0 {
            return;
        }
        let byte = crate::io::inb(base + UART_RBR);
        let head = ports[port_idx].rx_head;
        let next_head = (head.wrapping_add(1)) % UART_RX_BUF;
        if next_head != ports[port_idx].rx_tail {
            ports[port_idx].rx_buf[head] = byte;
            ports[port_idx].rx_head = next_head;
            ports[port_idx].rx_count = ports[port_idx].rx_count.saturating_add(1);
        }
        // Drop lock before next iteration to avoid long hold
        drop(ports);
    }
}

/// Pop one byte from the internal RX ring buffer for `port_idx`.
/// Returns `None` if the ring buffer is empty or the port index is invalid.
pub fn uart_rx_dequeue(port_idx: usize) -> Option<u8> {
    if port_idx >= MAX_UART_PORTS {
        return None;
    }
    let mut ports = UART_PORTS.lock();
    let tail = ports[port_idx].rx_tail;
    let head = ports[port_idx].rx_head;
    if tail == head {
        // Empty
        return None;
    }
    let byte = ports[port_idx].rx_buf[tail];
    ports[port_idx].rx_tail = (tail.wrapping_add(1)) % UART_RX_BUF;
    Some(byte)
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialise all four COM ports.  Each port is probed; if present it is
/// configured for 115 200 baud 8N1 with FIFO and RDA interrupts enabled.
pub fn init() {
    const BASES: [u16; MAX_UART_PORTS] = [UART_COM1, UART_COM2, UART_COM3, UART_COM4];
    let mut found: u32 = 0;

    for (idx, &base) in BASES.iter().enumerate() {
        if !uart_probe(base) {
            continue;
        }
        let ok = uart_init_port(base, UART_BAUD_115200);
        if ok {
            let mut ports = UART_PORTS.lock();
            ports[idx].base_port = base;
            ports[idx].baud_divisor = UART_BAUD_115200;
            ports[idx].present = true;
            ports[idx].active = true;
            found = found.saturating_add(1);
        }
    }

    serial_println!(
        "[uart8250] serial driver initialized, {} ports found",
        found
    );
}
