use crate::sync::Mutex;
/// Kernel print ring buffer, log levels, and early console output.
///
/// Part of the AIOS kernel.
///
/// ## Performance notes
///
/// The original implementation used `Vec::remove(0)` to drop the oldest
/// entry when the buffer was full.  `remove(0)` is O(n) because it shifts
/// every remaining element down by one slot.  At 512-entry capacity that is
/// up to 512 pointer-width memmoves on every overwrite — unacceptable on the
/// serial/printk hot path.
///
/// This version replaces the Vec with a **power-of-two ring** indexed by
/// `head` and `tail` counters.  Insertion and eviction are both O(1); the
/// modulo is a bitmask because the capacity is always a power of two.
///
/// ## Raw byte ring buffer (`KMSG_RING`)
///
/// In addition to the `PrintkBuffer` (which stores structured `LogEntry`
/// objects and requires heap allocation), a second **static byte ring**
/// (`KMSG_RING`) is maintained.  This is the backing store for the
/// `/proc/kmsg`-style `printk_read()` interface: every message appended via
/// `printk_append()` is also serialised as a human-readable line into the
/// byte ring so that a user-space reader can drain it without touching the
/// heap at all.
///
/// Capacity: 16 384 bytes (16 KiB).  Wrap-around is silent; the oldest bytes
/// are overwritten when the ring is full.
///
/// ## TSC timestamps
///
/// `current_ticks()` reads the hardware TSC with `rdtsc` in a `no_std`
/// context.  On boot the TSC starts at an arbitrary value; the caller should
/// treat it as a monotonically increasing counter rather than wall-clock time.
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// TSC helper
// ---------------------------------------------------------------------------

/// Read the hardware Time Stamp Counter.
///
/// Returns the raw 64-bit TSC value.  On most modern x86_64 CPUs this is a
/// monotonically increasing counter running at a constant rate independent of
/// CPU frequency scaling (invariant TSC, CPUID.80000007H:EDX[8]).
///
/// # Safety
/// `rdtsc` is a non-privileged instruction available at CPL 3, but we call it
/// here from kernel context.  There are no alignment or memory-safety concerns.
#[inline]
pub fn current_ticks() -> u64 {
    // SAFETY: rdtsc has no side-effects and is always available on x86_64.
    unsafe { core::arch::x86_64::_rdtsc() }
}

// ---------------------------------------------------------------------------
// Log severity levels
// ---------------------------------------------------------------------------

/// Log severity levels matching syslog conventions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Emergency = 0,
    Alert = 1,
    Critical = 2,
    Error = 3,
    Warning = 4,
    Notice = 5,
    Info = 6,
    Debug = 7,
}

impl LogLevel {
    /// Single-character prefix used when serialising to the byte ring.
    pub fn prefix(self) -> char {
        match self {
            LogLevel::Emergency => 'M',
            LogLevel::Alert => 'A',
            LogLevel::Critical => 'C',
            LogLevel::Error => 'E',
            LogLevel::Warning => 'W',
            LogLevel::Notice => 'N',
            LogLevel::Info => 'I',
            LogLevel::Debug => 'D',
        }
    }
}

// ---------------------------------------------------------------------------
// LogEntry
// ---------------------------------------------------------------------------

/// A single entry in the kernel log ring buffer.
pub struct LogEntry {
    pub level: LogLevel,
    pub message: String,
    /// Raw TSC value captured when the entry was written.
    pub timestamp_ticks: u64,
}

// ---------------------------------------------------------------------------
// PrintkBuffer — structured (heap) ring
// ---------------------------------------------------------------------------

/// Power-of-two ring buffer holding recent kernel log messages.
///
/// Entries are overwritten in FIFO order when the buffer is full (the oldest
/// entry is silently replaced by the newest).  All operations are O(1).
pub struct PrintkBuffer {
    /// The ring storage.  Allocated on first `new()`.
    entries: Vec<Option<LogEntry>>,
    /// Index of the oldest valid entry (read position).
    head: usize,
    /// Index where the next entry will be written (write position).
    tail: usize,
    /// Number of valid entries currently in the ring.
    count: usize,
    /// Ring capacity (always a power of two).
    capacity: usize,
    /// Bitmask == capacity - 1 for fast modulo.
    mask: usize,
}

impl PrintkBuffer {
    /// Create a new ring buffer.
    ///
    /// `capacity` is rounded up to the next power of two (minimum 16).
    pub fn new(capacity: usize) -> Self {
        // Round up to next power of two, minimum 16.
        let cap = {
            let mut c = if capacity < 16 { 16 } else { capacity };
            c = c.saturating_sub(1);
            c |= c >> 1;
            c |= c >> 2;
            c |= c >> 4;
            c |= c >> 8;
            c |= c >> 16;
            c |= c >> 32;
            c.saturating_add(1)
        };
        let mut entries = Vec::with_capacity(cap);
        for _ in 0..cap {
            entries.push(None);
        }
        PrintkBuffer {
            entries,
            head: 0,
            tail: 0,
            count: 0,
            capacity: cap,
            mask: cap - 1,
        }
    }

    /// Append a log entry to the ring buffer.
    ///
    /// If the buffer is full the **oldest** entry is silently overwritten.
    /// This is O(1) — no shifting, no allocation beyond the `String` in
    /// the log entry itself.
    ///
    /// The `timestamp_ticks` field is populated via `current_ticks()` (TSC).
    // hot path: called from every kernel log statement (~1K/s during normal operation)
    #[inline]
    pub fn write(&mut self, level: LogLevel, msg: &str) {
        if self.capacity == 0 {
            return;
        }

        let entry = LogEntry {
            level,
            message: String::from(msg),
            timestamp_ticks: current_ticks(),
        };

        // Write at tail.
        self.entries[self.tail & self.mask] = Some(entry);
        self.tail = (self.tail + 1) & self.mask;

        if self.count < self.capacity {
            // Ring not yet full — just advance count.
            self.count += 1;
        } else {
            // Ring full — advance head to discard the oldest entry.
            self.head = (self.head + 1) & self.mask;
        }
    }

    /// Iterate over all valid entries from oldest to newest.
    pub fn iter(&self) -> impl Iterator<Item = &LogEntry> {
        let head = self.head;
        let count = self.count;
        let mask = self.mask;
        (0..count).filter_map(move |i| {
            let idx = (head + i) & mask;
            self.entries[idx].as_ref()
        })
    }

    /// Number of entries currently in the buffer.
    pub fn len(&self) -> usize {
        self.count
    }

    /// True if the buffer contains no entries.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

// ---------------------------------------------------------------------------
// KMSG_RING — static byte ring (16 KiB) for /proc/kmsg-style drain
// ---------------------------------------------------------------------------

/// Size of the raw byte ring.  Must be a power of two.
const KMSG_RING_SIZE: usize = 16384; // 16 KiB
/// Bitmask for wrap-around indexing.
const KMSG_RING_MASK: usize = KMSG_RING_SIZE - 1;

/// Raw-byte circular buffer state.
struct KmsgRing {
    /// Backing storage.  Zeroed at init.
    buf: [u8; KMSG_RING_SIZE],
    /// Write cursor (absolute, never reset — use `& KMSG_RING_MASK`).
    head: usize,
    /// Read cursor for `printk_read()` (absolute).
    tail: usize,
}

impl KmsgRing {
    const fn new() -> Self {
        KmsgRing {
            buf: [0u8; KMSG_RING_SIZE],
            head: 0,
            tail: 0,
        }
    }

    /// Number of unread bytes available.
    #[inline]
    fn available(&self) -> usize {
        self.head.wrapping_sub(self.tail)
    }

    /// Append a single byte, advancing `head`.
    ///
    /// If the ring is full (all 16 KiB occupied) the oldest byte is
    /// implicitly overwritten and `tail` is advanced to keep the window
    /// consistent.
    #[inline]
    fn push_byte(&mut self, b: u8) {
        self.buf[self.head & KMSG_RING_MASK] = b;
        self.head = self.head.wrapping_add(1);
        // If the ring is now overfull, evict the oldest byte.
        if self.available() > KMSG_RING_SIZE {
            self.tail = self.tail.wrapping_add(1);
        }
    }

    /// Append a UTF-8 string slice byte-by-byte.
    fn push_str(&mut self, s: &str) {
        for b in s.bytes() {
            self.push_byte(b);
        }
    }

    /// Drain up to `out.len()` bytes into `out`, advancing the read cursor.
    ///
    /// Returns the number of bytes actually written.
    fn drain(&mut self, out: &mut [u8]) -> usize {
        let avail = self.available().min(out.len());
        for i in 0..avail {
            out[i] = self.buf[(self.tail.wrapping_add(i)) & KMSG_RING_MASK];
        }
        self.tail = self.tail.wrapping_add(avail);
        avail
    }
}

/// Kernel-global byte ring buffer, protected by a spinlock.
static KMSG_RING: Mutex<KmsgRing> = Mutex::new(KmsgRing::new());

/// Kernel-global structured log ring, protected by a spinlock.
///
/// Initialised by `init()` as a 512-entry `PrintkBuffer`.
static GLOBAL_PRINTK_BUF: Mutex<Option<PrintkBuffer>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Append a message to both the structured ring and the raw byte ring.
///
/// The raw ring entry has the format:
///   `<L tsc> message\n`
/// where `L` is the single-character level prefix and `tsc` is the raw TSC
/// value in decimal.
///
/// This function is the canonical write path; `init()` and `serial_println!`
/// call through here.
pub fn printk_append(level: LogLevel, msg: &str) {
    let tsc = current_ticks();

    // 1. Structured ring (heap-backed LogEntry).
    {
        let mut guard = GLOBAL_PRINTK_BUF.lock();
        if let Some(ref mut buf) = *guard {
            buf.write(level, msg);
        }
    }

    // 2. Raw byte ring: "<L tsc> msg\n"
    {
        let mut ring = KMSG_RING.lock();
        ring.push_byte(b'<');
        ring.push_byte(level.prefix() as u8);
        ring.push_byte(b' ');

        // Write TSC as decimal digits (no alloc required).
        let mut tmp = [0u8; 20]; // max 20 decimal digits for u64
        let mut n = tsc;
        let mut len = 0usize;
        if n == 0 {
            tmp[0] = b'0';
            len = 1;
        } else {
            while n > 0 {
                tmp[len] = b'0' + (n % 10) as u8;
                n /= 10;
                len += 1;
            }
            tmp[..len].reverse();
        }
        for i in 0..len {
            ring.push_byte(tmp[i]);
        }

        ring.push_str("> ");
        ring.push_str(msg);
        ring.push_byte(b'\n');
    }
}

/// Drain unread bytes from the raw byte ring into `buf`.
///
/// This is the kernel-side implementation of a `/proc/kmsg` read: user-space
/// (or a kernel debugger) calls this to retrieve text that has been written
/// via `printk_append()` since the last `printk_read()` call.
///
/// Returns the number of bytes written into `buf`.  If `buf` is smaller than
/// the available data the remainder stays in the ring and can be read on the
/// next call.
pub fn printk_read(buf: &mut [u8]) -> usize {
    KMSG_RING.lock().drain(buf)
}

/// Returns a reference to the global structured log buffer.
///
/// Callers that need to iterate over `LogEntry` items (e.g. the in-kernel
/// `dmesg` equivalent) should lock this and call `.iter()` on the inner
/// `PrintkBuffer`.
pub fn with_printk_buf<F, R>(f: F) -> R
where
    F: FnOnce(Option<&PrintkBuffer>) -> R,
{
    let guard = GLOBAL_PRINTK_BUF.lock();
    f(guard.as_ref())
}

// ---------------------------------------------------------------------------
// Initialiser
// ---------------------------------------------------------------------------

/// Initialise the printk subsystem.
///
/// Creates the global 512-entry structured ring, zeroes the byte ring, and
/// writes an "online" banner so the boot log confirms the path is live.
pub fn init() {
    *GLOBAL_PRINTK_BUF.lock() = Some(PrintkBuffer::new(512));

    printk_append(
        LogLevel::Info,
        "printk: subsystem online (TSC timestamps, 16 KiB byte ring)",
    );
    crate::serial_println!("  printk: initialized (512-entry structured ring + 16 KiB byte ring)");
}
