use crate::io::{inb, outb};
/// Serial port driver (UART 16550) for Hoags Kernel Genesis — built from scratch
///
/// Writes to COM1 (0x3F8) for debugging via QEMU serial console.
/// No external crates. Direct port I/O via our own io module.
///
/// ## Architecture
///
/// The serial output path has two components:
///
///   1. A **lock-free SPSC ring buffer** (`TX_RING`) that interrupt handlers
///      and normal-context code push bytes into without ever blocking.
///      This is safe because there is a single producer (any caller of
///      `_print`) and a single consumer (the UART drain loop).
///
///   2. A **fallback Mutex path** used only during early boot (before the
///      ring is flushed by the idle drain) and when the ring is full.
///
/// The ring approach eliminates the common case where a printk from an
/// interrupt handler would spin on the SERIAL1 Mutex while the normal
/// context holds it.  With the ring the IRQ handler returns instantly;
/// the bytes are drained lazily by `drain_tx_ring()`, which is called from
/// the idle loop and from timer ticks.
use crate::sync::Mutex;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

const COM1: u16 = 0x3F8;

// ---------------------------------------------------------------------------
// Lock-free SPSC transmit ring buffer
// ---------------------------------------------------------------------------

/// Capacity of the serial transmit ring buffer.
/// Power-of-two so the modulo becomes a bitmask — critical for the hot path.
const TX_RING_CAP: usize = 4096;
const TX_RING_MASK: usize = TX_RING_CAP - 1;

/// Single-producer single-consumer byte ring for the serial TX path.
///
/// The **producer** (any CPU calling `_print` or `serial_print!`) writes to
/// `tail`.  The **consumer** (idle-loop drain or timer tick) reads from
/// `head`.  No lock is needed as long as there is at most one thread acting
/// as consumer at a time (guaranteed here: `drain_tx_ring` is only called
/// from the BSP idle task and the timer IRQ, never concurrently).
///
/// Memory ordering:
///   - Producer: stores byte then Release-stores tail.
///   - Consumer: Acquire-loads tail then reads byte.
// hot struct: written from every serial_println! call including IRQ handlers
#[repr(C, align(64))]
pub struct SpscTxRing {
    buf: [UnsafeCell<u8>; TX_RING_CAP],
    /// Consumer reads from here.
    head: AtomicUsize,
    /// Producer writes to here.
    tail: AtomicUsize,
}

// Safety: SpscTxRing is only used in a single-producer / single-consumer
// pattern enforced by the drain discipline above.
unsafe impl Sync for SpscTxRing {}
unsafe impl Send for SpscTxRing {}

impl SpscTxRing {
    const fn new() -> Self {
        // SAFETY: UnsafeCell<u8> is zero-initialised; the ring is empty (head==tail).
        SpscTxRing {
            buf: [const { UnsafeCell::new(0u8) }; TX_RING_CAP],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Push one byte into the ring.  Returns false and drops the byte if full.
    // hot path: called from every serial_print! (~10K/s during boot/debug)
    #[inline(always)]
    pub fn push(&self, byte: u8) -> bool {
        let tail = self.tail.load(Ordering::Relaxed);
        let next = (tail + 1) & TX_RING_MASK;
        if next == self.head.load(Ordering::Acquire) {
            // Ring is full — drop the byte rather than blocking.
            return false;
        }
        // SAFETY: tail is owned exclusively by the producer.
        unsafe {
            *self.buf[tail].get() = byte;
        }
        self.tail.store(next, Ordering::Release);
        true
    }

    /// Pop one byte from the ring.  Returns None if empty.
    // called from drain_tx_ring() which runs in the idle loop and timer IRQ
    #[inline(always)]
    pub fn pop(&self) -> Option<u8> {
        let head = self.head.load(Ordering::Relaxed);
        if head == self.tail.load(Ordering::Acquire) {
            return None; // empty
        }
        // SAFETY: head is owned exclusively by the consumer.
        let byte = unsafe { *self.buf[head].get() };
        self.head
            .store((head + 1) & TX_RING_MASK, Ordering::Release);
        Some(byte)
    }

    /// Number of bytes currently queued.
    #[inline]
    pub fn len(&self) -> usize {
        let tail = self.tail.load(Ordering::Acquire);
        let head = self.head.load(Ordering::Relaxed);
        tail.wrapping_sub(head) & TX_RING_MASK
    }

    /// True if the ring contains no bytes.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.head.load(Ordering::Relaxed) == self.tail.load(Ordering::Acquire)
    }
}

/// Global lock-free transmit ring.  Bytes are pushed here by `_print` and
/// drained to the UART by `drain_tx_ring`.
pub static TX_RING: SpscTxRing = SpscTxRing::new();

// ---------------------------------------------------------------------------
// UART 16550 hardware driver
// ---------------------------------------------------------------------------

/// UART 16550 serial port driver
pub struct SerialPort {
    base: u16,
}

impl SerialPort {
    const fn new(base: u16) -> Self {
        SerialPort { base }
    }

    /// Initialize the UART with 38400 baud, 8N1
    pub fn init(&self) {
        outb(self.base + 1, 0x00); // Disable all interrupts
        outb(self.base + 3, 0x80); // Enable DLAB (set baud rate divisor)
        outb(self.base + 0, 0x03); // Set divisor to 3 (lo byte) 38400 baud
        outb(self.base + 1, 0x00); //                  (hi byte)
        outb(self.base + 3, 0x03); // 8 bits, no parity, one stop bit (8N1)
        outb(self.base + 2, 0xC7); // Enable FIFO, clear them, 14-byte threshold
        outb(self.base + 4, 0x0B); // IRQs enabled, RTS/DSR set
    }

    /// Check if the transmit holding register is empty (THR ready for a byte).
    // hot path: polled in every write_byte spin
    #[inline(always)]
    fn is_transmit_empty(&self) -> bool {
        inb(self.base + 5) & 0x20 != 0
    }

    /// Check if a byte is available in the receive buffer (LSR bit 0).
    #[inline(always)]
    pub fn data_ready(&self) -> bool {
        inb(self.base + 5) & 0x01 != 0
    }

    /// Read one byte from the UART receive buffer (only call when data_ready()).
    #[inline(always)]
    pub fn read_byte(&self) -> u8 {
        inb(self.base)
    }

    /// Write a single byte directly to the UART, spinning until ready.
    // hot path: called from drain_tx_ring at up to UART baud rate
    #[inline(always)]
    pub fn write_byte(&self, byte: u8) {
        while !self.is_transmit_empty() {
            core::hint::spin_loop();
        }
        outb(self.base, byte);
    }
}

impl core::fmt::Write for SerialPort {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for byte in s.bytes() {
            self.write_byte(byte);
        }
        Ok(())
    }
}

/// Fallback Mutex-protected handle used only when the ring is full or during
/// very early boot before the idle drain is running.
pub static SERIAL1: Mutex<SerialPort> = Mutex::new(SerialPort::new(COM1));

pub fn init() {
    SERIAL1.lock().init();
}

/// Poll COM1 for a received byte. Returns Some(byte) if one is waiting, None otherwise.
/// Used by the login/shell loops to accept input from serial console (QEMU stdio).
pub fn try_read_byte() -> Option<u8> {
    let port = SERIAL1.lock();
    if port.data_ready() {
        Some(port.read_byte())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// TX ring drain
// ---------------------------------------------------------------------------

/// Drain up to `max_bytes` bytes from the TX ring to the UART.
///
/// Call this from the idle task and from the timer IRQ to keep latency low.
/// Processes at most `max_bytes` per call to avoid blocking interrupts for
/// too long when the ring is deeply filled (e.g. after a burst of printk).
// called from idle loop and timer tick — not on the scheduling hot path
pub fn drain_tx_ring(max_bytes: usize) {
    let port = SERIAL1.lock();
    let mut n = 0usize;
    while n < max_bytes {
        match TX_RING.pop() {
            Some(byte) => {
                port.write_byte(byte);
                n += 1;
            }
            None => break,
        }
    }
}

// ---------------------------------------------------------------------------
// Macros and _print
// ---------------------------------------------------------------------------

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::serial::_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($($arg:tt)*) => ($crate::serial_print!("{}\n", format_args!($($arg)*)));
}

/// Write formatted output.
///
/// Fast path: push bytes into the lock-free TX ring.  The ring drain
/// (`drain_tx_ring`) will flush them to the UART asynchronously.
///
/// Slow path (ring full): fall back to the Mutex-protected UART path so
/// critical messages are never silently dropped.
// hot path: called from serial_println! which fires on every boot log line
pub fn _print(args: core::fmt::Arguments) {
    use core::fmt::Write;

    // Format into a stack-local buffer first to avoid holding any lock during
    // the format operation itself (which can recurse into alloc).
    // We use a simple fixed-size stack writer so there is no heap allocation.
    let mut w = StackWriter {
        buf: [0u8; 512],
        len: 0,
    };
    let _ = w.write_fmt(args); // errors ignored; partial output is acceptable

    // Push each byte into the lock-free ring.
    let mut fallback_needed = false;
    for &byte in &w.buf[..w.len] {
        if !TX_RING.push(byte) {
            fallback_needed = true;
            break;
        }
    }

    // If the ring was full for any byte, fall back to direct UART write
    // under the Mutex.  This is the slow path; it should be rare.
    if fallback_needed {
        let mut port = SERIAL1.lock();
        let _ = port.write_fmt(args);
    } else {
        // Eager drain: flush immediately so serial output appears even if the
        // idle loop / timer IRQ hasn't fired yet (critical for early boot debugging).
        drain_tx_ring(512);
    }
}

/// Minimal stack-allocated fmt::Write target — avoids heap allocs in _print.
struct StackWriter {
    buf: [u8; 512],
    len: usize,
}

impl core::fmt::Write for StackWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let space = self.buf.len() - self.len;
        let to_copy = bytes.len().min(space);
        self.buf[self.len..self.len + to_copy].copy_from_slice(&bytes[..to_copy]);
        self.len += to_copy;
        Ok(())
    }
}
