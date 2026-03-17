use crate::serial_println;
/// Kernel function tracing infrastructure (ftrace-inspired) — Genesis AIOS.
///
/// Provides a fixed-size ring buffer of trace events, a per-function registry
/// with hit counters, and helpers for recording function entry/return, scheduler
/// events, syscall entries, and IRQ events.
///
/// ## Design constraints (bare-metal #![no_std])
/// - NO heap: no Vec / Box / String / alloc::* — all storage is fixed-size
///   static arrays.
/// - NO floats: no `as f64` / `as f32` anywhere.
/// - NO panics: no unwrap() / expect() / panic!() — early returns on error.
/// - All counters use saturating_add / saturating_sub.
/// - All sequence numbers use wrapping_add.
/// - Structs stored in static Mutex must be Copy + have `const fn empty()`.
use crate::sync::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Number of entries in the ring buffer. Must be a power of two.
pub const FTRACE_RING_SIZE: usize = 1024;

/// Maximum number of individually-registered traced functions.
pub const FTRACE_MAX_FUNCS: usize = 256;

/// Event type: function entry.
pub const FTRACE_EVENT_FUNC: u8 = 0;
/// Event type: function return.
pub const FTRACE_EVENT_RETURN: u8 = 1;
/// Event type: scheduler switch.
pub const FTRACE_EVENT_SCHED: u8 = 2;
/// Event type: IRQ entry/exit.
pub const FTRACE_EVENT_IRQ: u8 = 3;
/// Event type: syscall entry.
pub const FTRACE_EVENT_SYSCALL: u8 = 4;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single entry in the ftrace ring buffer.
#[derive(Copy, Clone)]
pub struct FtraceEntry {
    /// Timestamp in nanoseconds (TSC-derived).
    pub timestamp_ns: u64,
    /// CPU id (0-based).
    pub cpu: u8,
    /// Process id of the running task.
    pub pid: u32,
    /// One of the FTRACE_EVENT_* constants.
    pub event_type: u8,
    /// Address of the traced function (or syscall number for syscall events).
    pub func_addr: u64,
    /// Return address / caller address.
    pub caller_addr: u64,
    /// Event-specific extra field (see individual record functions).
    pub extra: u64,
}

impl FtraceEntry {
    /// Return an all-zero entry suitable for static initialisation.
    pub const fn empty() -> Self {
        FtraceEntry {
            timestamp_ns: 0,
            cpu: 0,
            pid: 0,
            event_type: 0,
            func_addr: 0,
            caller_addr: 0,
            extra: 0,
        }
    }
}

/// Per-function registration record.
#[derive(Copy, Clone)]
pub struct FtraceFunc {
    /// Kernel virtual address of the function.
    pub addr: u64,
    /// Human-readable name (UTF-8, zero-padded).
    pub name: [u8; 32],
    /// Number of valid bytes in `name`.
    pub name_len: u8,
    /// Number of times this function has been entered.
    pub hit_count: u64,
    /// Whether per-function tracing is enabled for this entry.
    pub enabled: bool,
}

impl FtraceFunc {
    /// Return an all-zero entry suitable for static initialisation.
    pub const fn empty() -> Self {
        FtraceFunc {
            addr: 0,
            name: [0u8; 32],
            name_len: 0,
            hit_count: 0,
            enabled: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Static state
// ---------------------------------------------------------------------------

/// Ring buffer of trace entries.
static FTRACE_RING: Mutex<[FtraceEntry; FTRACE_RING_SIZE]> =
    Mutex::new([FtraceEntry::empty(); FTRACE_RING_SIZE]);

/// Write pointer into FTRACE_RING (wrapping, masked to FTRACE_RING_SIZE − 1).
static FTRACE_HEAD: AtomicU32 = AtomicU32::new(0);

/// Read pointer into FTRACE_RING.
static FTRACE_TAIL: AtomicU32 = AtomicU32::new(0);

/// Global tracing enable flag.
pub static FTRACE_ENABLED: AtomicBool = AtomicBool::new(false);

/// Registered function table.
static FTRACE_FUNCS: Mutex<[FtraceFunc; FTRACE_MAX_FUNCS]> =
    Mutex::new([FtraceFunc::empty(); FTRACE_MAX_FUNCS]);

/// Number of entries dropped because the ring was full.
static FTRACE_DROP_COUNT: AtomicU64 = AtomicU64::new(0);

/// Total entries written (including later-dropped ones, so we know the demand).
/// Incremented before the ring-full check so it accurately reflects demand.
static FTRACE_TOTAL_WRITTEN: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Integer-to-ASCII helpers (no format!, no heap)
// ---------------------------------------------------------------------------

/// Write the decimal representation of `n` into `buf` starting at `offset`.
/// Returns the new offset (i.e. offset + number of digits written).
/// Writes at least one digit ("0" for n==0).
#[inline]
fn write_u64_decimal(buf: &mut [u8; 1024], mut offset: usize, mut n: u64) -> usize {
    if offset >= buf.len() {
        return offset;
    }
    if n == 0 {
        buf[offset] = b'0';
        return offset.saturating_add(1);
    }
    // Write digits in reverse into a temp buffer, then copy forward.
    let mut tmp = [0u8; 20];
    let mut len = 0usize;
    while n > 0 && len < 20 {
        tmp[len] = b'0' + (n % 10) as u8;
        n /= 10;
        len = len.saturating_add(1);
    }
    // Reverse the temp buffer.
    let mut i = 0usize;
    let mut j = len.saturating_sub(1);
    while i < j {
        tmp.swap(i, j);
        i = i.saturating_add(1);
        j = j.saturating_sub(1);
    }
    // Copy to output.
    for k in 0..len {
        if offset < buf.len() {
            buf[offset] = tmp[k];
            offset = offset.saturating_add(1);
        }
    }
    offset
}

/// Write a string literal (as a byte slice) into `buf` starting at `offset`.
/// Returns the new offset.
#[inline]
fn write_str(buf: &mut [u8; 1024], mut offset: usize, s: &[u8]) -> usize {
    for &b in s {
        if offset >= buf.len() {
            break;
        }
        buf[offset] = b;
        offset = offset.saturating_add(1);
    }
    offset
}

// ---------------------------------------------------------------------------
// Global enable / disable
// ---------------------------------------------------------------------------

/// Enable global function tracing.
pub fn ftrace_enable() {
    FTRACE_ENABLED.store(true, Ordering::Release);
}

/// Disable global function tracing.
pub fn ftrace_disable() {
    FTRACE_ENABLED.store(false, Ordering::Release);
}

// ---------------------------------------------------------------------------
// Function registration
// ---------------------------------------------------------------------------

/// Register a function address and name for per-function tracking.
///
/// Returns `true` if registered successfully, `false` if the table is full or
/// the address is already registered.
pub fn ftrace_register_func(addr: u64, name: &[u8]) -> bool {
    if addr == 0 {
        return false;
    }
    let mut funcs = FTRACE_FUNCS.lock();
    // Check for duplicate.
    for i in 0..FTRACE_MAX_FUNCS {
        if funcs[i].addr == addr {
            return false;
        }
    }
    // Find a free slot.
    for i in 0..FTRACE_MAX_FUNCS {
        if funcs[i].addr == 0 {
            funcs[i].addr = addr;
            funcs[i].hit_count = 0;
            funcs[i].enabled = true;
            // Copy name bytes (up to 32 characters).
            let copy_len = if name.len() < 32 { name.len() } else { 32 };
            funcs[i].name_len = copy_len as u8;
            for j in 0..copy_len {
                funcs[i].name[j] = name[j];
            }
            // Zero the rest.
            for j in copy_len..32 {
                funcs[i].name[j] = 0;
            }
            return true;
        }
    }
    false
}

/// Enable per-function tracing for a previously registered address.
pub fn ftrace_enable_func(addr: u64) -> bool {
    let mut funcs = FTRACE_FUNCS.lock();
    for i in 0..FTRACE_MAX_FUNCS {
        if funcs[i].addr == addr {
            funcs[i].enabled = true;
            return true;
        }
    }
    false
}

/// Disable per-function tracing for a previously registered address.
pub fn ftrace_disable_func(addr: u64) -> bool {
    let mut funcs = FTRACE_FUNCS.lock();
    for i in 0..FTRACE_MAX_FUNCS {
        if funcs[i].addr == addr {
            funcs[i].enabled = false;
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Core ring-buffer record
// ---------------------------------------------------------------------------

/// Write one entry into the ring buffer.
///
/// The ring is considered full when `(head + 1) % RING_SIZE == tail`.
/// On overflow the entry is dropped and `FTRACE_DROP_COUNT` is incremented.
/// `head` advances with `wrapping_add`, masked to `FTRACE_RING_SIZE − 1`.
pub fn ftrace_record(entry: FtraceEntry) {
    FTRACE_TOTAL_WRITTEN.fetch_add(1, Ordering::Relaxed);

    let head = FTRACE_HEAD.load(Ordering::Relaxed);
    let tail = FTRACE_TAIL.load(Ordering::Acquire);

    let next_head = head.wrapping_add(1) & (FTRACE_RING_SIZE as u32).wrapping_sub(1);
    if next_head == tail {
        // Ring full — drop this entry.
        FTRACE_DROP_COUNT.fetch_add(1, Ordering::Relaxed);
        return;
    }

    {
        let mut ring = FTRACE_RING.lock();
        ring[head as usize] = entry;
    }

    FTRACE_HEAD.store(next_head, Ordering::Release);
}

// ---------------------------------------------------------------------------
// TSC timestamp
// ---------------------------------------------------------------------------

/// Read the 64-bit TSC and return it as a nanosecond-domain timestamp.
///
/// For a real kernel the TSC would be converted via the calibrated frequency;
/// here we return raw TSC ticks because we have no float arithmetic and the
/// exact units do not matter for relative ordering.
#[inline]
fn timestamp_ns() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags)
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

// ---------------------------------------------------------------------------
// High-level record helpers
// ---------------------------------------------------------------------------

/// Record a function entry event and increment the per-function hit counter.
pub fn ftrace_function_entry(func_addr: u64, caller_addr: u64, pid: u32) {
    if !FTRACE_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    // Increment hit_count for the matching function entry.
    {
        let mut funcs = FTRACE_FUNCS.lock();
        for i in 0..FTRACE_MAX_FUNCS {
            if funcs[i].addr == func_addr && funcs[i].enabled {
                funcs[i].hit_count = funcs[i].hit_count.saturating_add(1);
                break;
            }
        }
    }
    let entry = FtraceEntry {
        timestamp_ns: timestamp_ns(),
        cpu: 0,
        pid,
        event_type: FTRACE_EVENT_FUNC,
        func_addr,
        caller_addr,
        extra: 0,
    };
    ftrace_record(entry);
}

/// Record a function return event.
pub fn ftrace_function_return(func_addr: u64, caller_addr: u64, pid: u32) {
    if !FTRACE_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let entry = FtraceEntry {
        timestamp_ns: timestamp_ns(),
        cpu: 0,
        pid,
        event_type: FTRACE_EVENT_RETURN,
        func_addr,
        caller_addr,
        extra: 0,
    };
    ftrace_record(entry);
}

/// Record a scheduler switch event.
///
/// `extra` encodes both PIDs: `(prev_pid as u64) << 32 | next_pid as u64`.
pub fn ftrace_sched_event(prev_pid: u32, next_pid: u32) {
    if !FTRACE_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let extra = ((prev_pid as u64) << 32) | (next_pid as u64);
    let entry = FtraceEntry {
        timestamp_ns: timestamp_ns(),
        cpu: 0,
        pid: prev_pid,
        event_type: FTRACE_EVENT_SCHED,
        func_addr: 0,
        caller_addr: 0,
        extra,
    };
    ftrace_record(entry);
}

/// Record a syscall entry event.
///
/// `func_addr` carries the syscall number; `extra` is 0.
pub fn ftrace_syscall_entry(syscall_nr: u64, pid: u32) {
    if !FTRACE_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let entry = FtraceEntry {
        timestamp_ns: timestamp_ns(),
        cpu: 0,
        pid,
        event_type: FTRACE_EVENT_SYSCALL,
        func_addr: syscall_nr,
        caller_addr: 0,
        extra: 0,
    };
    ftrace_record(entry);
}

/// Record an IRQ entry or exit event.
///
/// `extra` is 1 if entering the IRQ handler, 0 if leaving.
pub fn ftrace_irq_event(irq_num: u32, entering: bool) {
    if !FTRACE_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let entry = FtraceEntry {
        timestamp_ns: timestamp_ns(),
        cpu: 0,
        pid: 0,
        event_type: FTRACE_EVENT_IRQ,
        func_addr: irq_num as u64,
        caller_addr: 0,
        extra: entering as u64,
    };
    ftrace_record(entry);
}

// ---------------------------------------------------------------------------
// Ring-buffer drain
// ---------------------------------------------------------------------------

/// Drain up to 64 entries from the ring buffer into `out`.
///
/// Advances `FTRACE_TAIL` with wrapping arithmetic.
/// Returns the number of entries actually copied.
pub fn ftrace_read(out: &mut [FtraceEntry; 64]) -> u32 {
    let mut count: u32 = 0;
    let ring = FTRACE_RING.lock();
    loop {
        if count >= 64 {
            break;
        }
        let tail = FTRACE_TAIL.load(Ordering::Acquire);
        let head = FTRACE_HEAD.load(Ordering::Acquire);
        if tail == head {
            // Ring empty.
            break;
        }
        out[count as usize] = ring[tail as usize];
        let next_tail = tail.wrapping_add(1) & (FTRACE_RING_SIZE as u32).wrapping_sub(1);
        FTRACE_TAIL.store(next_tail, Ordering::Release);
        count = count.saturating_add(1);
    }
    count
}

// ---------------------------------------------------------------------------
// Maintenance
// ---------------------------------------------------------------------------

/// Reset the ring buffer (head = tail = 0) and clear all function hit counts.
pub fn ftrace_clear() {
    FTRACE_HEAD.store(0, Ordering::Release);
    FTRACE_TAIL.store(0, Ordering::Release);
    FTRACE_TOTAL_WRITTEN.store(0, Ordering::Relaxed);
    FTRACE_DROP_COUNT.store(0, Ordering::Relaxed);
    let mut funcs = FTRACE_FUNCS.lock();
    for i in 0..FTRACE_MAX_FUNCS {
        funcs[i].hit_count = 0;
    }
}

/// Return aggregate statistics: `(total_entries_written, drop_count, func_hit_total)`.
pub fn ftrace_get_stats() -> (u64, u64, u64) {
    let total = FTRACE_TOTAL_WRITTEN.load(Ordering::Relaxed);
    let drops = FTRACE_DROP_COUNT.load(Ordering::Relaxed);
    let funcs = FTRACE_FUNCS.lock();
    let mut hit_total: u64 = 0;
    for i in 0..FTRACE_MAX_FUNCS {
        hit_total = hit_total.saturating_add(funcs[i].hit_count);
    }
    (total, drops, hit_total)
}

/// Write human-readable stats into `buf` using only integer-to-ASCII helpers.
/// Returns the number of bytes written.
pub fn ftrace_format_stats(buf: &mut [u8; 1024]) -> usize {
    let (total, drops, hit_total) = ftrace_get_stats();

    let head = FTRACE_HEAD.load(Ordering::Relaxed);
    let tail = FTRACE_TAIL.load(Ordering::Relaxed);
    // Ring occupancy: entries currently buffered.
    let occupancy = head.wrapping_sub(tail) & (FTRACE_RING_SIZE as u32).wrapping_sub(1);

    let mut off = 0usize;
    off = write_str(buf, off, b"[ftrace] stats\n");
    off = write_str(buf, off, b"  total_written : ");
    off = write_u64_decimal(buf, off, total);
    off = write_str(buf, off, b"\n");
    off = write_str(buf, off, b"  dropped       : ");
    off = write_u64_decimal(buf, off, drops);
    off = write_str(buf, off, b"\n");
    off = write_str(buf, off, b"  func_hits     : ");
    off = write_u64_decimal(buf, off, hit_total);
    off = write_str(buf, off, b"\n");
    off = write_str(buf, off, b"  ring_occupancy: ");
    off = write_u64_decimal(buf, off, occupancy as u64);
    off = write_str(buf, off, b" / ");
    off = write_u64_decimal(buf, off, FTRACE_RING_SIZE as u64);
    off = write_str(buf, off, b"\n");
    off = write_str(buf, off, b"  enabled       : ");
    if FTRACE_ENABLED.load(Ordering::Relaxed) {
        off = write_str(buf, off, b"yes\n");
    } else {
        off = write_str(buf, off, b"no\n");
    }

    // Per-function table.
    let funcs = FTRACE_FUNCS.lock();
    off = write_str(buf, off, b"  functions:\n");
    for i in 0..FTRACE_MAX_FUNCS {
        if funcs[i].addr == 0 {
            continue;
        }
        if off.saturating_add(80) >= 1024 {
            break;
        }
        off = write_str(buf, off, b"    ");
        let nlen = funcs[i].name_len as usize;
        for j in 0..nlen {
            if off < 1024 {
                buf[off] = funcs[i].name[j];
                off = off.saturating_add(1);
            }
        }
        off = write_str(buf, off, b" hits=");
        off = write_u64_decimal(buf, off, funcs[i].hit_count);
        off = write_str(buf, off, b"\n");
    }

    off
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialise the ftrace subsystem and pre-register standard kernel functions.
pub fn init() {
    ftrace_register_func(0xFFFF_0001_0000_0001, b"sys_read");
    ftrace_register_func(0xFFFF_0001_0000_0002, b"sys_write");
    ftrace_register_func(0xFFFF_0001_0000_0003, b"schedule");
    ftrace_register_func(0xFFFF_0001_0000_0004, b"handle_page_fault");
    serial_println!("[ftrace] tracing infrastructure initialized");
}
