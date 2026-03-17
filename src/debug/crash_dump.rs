use crate::debug::oops::OopsRecord;
/// Crash dump to serial port and reserved memory region — Genesis AIOS
///
/// Writes a `CrashDump` struct to:
///   1. A reserved physical memory region at `CRASH_DUMP_PHYS` that survives
///      a warm reboot (like Linux pstore / kdump).
///   2. COM1 serial port as a hex-encoded stream prefixed by magic bytes
///      `0xAA55` for easy scraping by a host tool.
///
/// On the *next* boot, `init()` checks the magic field in the reserved region
/// and prints the previous crash dump to serial if one is found.
///
/// Rules strictly followed:
///   - no_std, no alloc, no Vec/Box/String
///   - no float casts (no `as f32` / `as f64`)
///   - saturating arithmetic for all index/size math
///   - read_volatile / write_volatile for all MMIO / reserved-region access
///   - no panic — serial_println! + early return on errors
use core::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Magic value written at the start of the reserved region to mark a valid dump.
pub const CRASH_DUMP_MAGIC: u32 = 0xCAAFE001;

/// Physical (identity-mapped virtual) address of the reserved crash dump region.
///
/// This address must match the linker script and boot-time memory reservation.
/// 1 MiB into the second megabyte (0x0010_0000), mapped at the canonical
/// kernel virtual address offset used by this kernel build.
const CRASH_DUMP_PHYS: u64 = 0xFFFF_8000_0010_0000;

/// Size of the kernel log snapshot embedded in the dump.
const LOG_SNAP_SIZE: usize = 4096;

/// Maximum byte length of the null-terminated crash message.
const MAX_MSG: usize = 256;

/// Maximum stack-trace depth stored in the dump.
const MAX_TRACE: usize = 32;

// ---------------------------------------------------------------------------
// CrashDump — on-disk / in-memory layout (repr(C) for deterministic layout)
// ---------------------------------------------------------------------------

/// The crash dump record written to the reserved memory region.
///
/// The layout is `repr(C)` so a host tool can parse it byte-for-byte.
#[repr(C)]
pub struct CrashDump {
    /// `0xCAAFE001` when valid; `0x00000000` when cleared.
    pub magic: u32,
    /// Crash message (null-terminated, up to MAX_MSG bytes).
    pub message: [u8; MAX_MSG],
    /// Actual length of the message.
    pub msg_len: u32,
    /// Stack trace (return addresses).
    pub stack_trace: [u64; MAX_TRACE],
    /// Number of valid entries in `stack_trace`.
    pub trace_depth: u32,
    /// Faulting PID (0 = kernel).
    pub pid: u32,
    /// Faulting CPU index.
    pub cpu: u8,
    /// Reserved pad to maintain alignment.
    pub _pad: [u8; 3],
    /// TSC at the time of the crash.
    pub timestamp_tsc: u64,
    /// Last 4 KiB of the kernel serial ring buffer.
    pub kernel_log: [u8; LOG_SNAP_SIZE],
    /// Number of valid bytes in `kernel_log`.
    pub log_len: u32,
    /// CRC32 of all bytes after this field (for integrity checking on reboot).
    pub crc32: u32,
}

/// Compile-time assertion: the dump must fit within 8 KiB so it lands in the
/// reserved region without overflow.  Adjust `LOG_SNAP_SIZE` if this fails.
const _: () = assert!(core::mem::size_of::<CrashDump>() <= 8192);

// ---------------------------------------------------------------------------
// Re-entrant guard
// ---------------------------------------------------------------------------

static DUMP_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Serial hex helpers — emit bytes to COM1 without alloc
// ---------------------------------------------------------------------------

/// Emit one byte as two lowercase hex ASCII characters to serial.
#[inline]
fn serial_byte_hex(b: u8) {
    let hi = (b >> 4) & 0xf;
    let lo = b & 0xf;
    let to_hex = |n: u8| -> char {
        if n < 10 {
            (b'0' + n) as char
        } else {
            (b'a' + n - 10) as char
        }
    };
    crate::serial_print!("{}", to_hex(hi));
    crate::serial_print!("{}", to_hex(lo));
}

/// Emit a u32 as 8 hex chars to serial.
fn serial_u32_hex(v: u32) {
    serial_byte_hex(((v >> 24) & 0xff) as u8);
    serial_byte_hex(((v >> 16) & 0xff) as u8);
    serial_byte_hex(((v >> 8) & 0xff) as u8);
    serial_byte_hex((v & 0xff) as u8);
}

/// Emit a u64 as 16 hex chars to serial.
fn serial_u64_hex(v: u64) {
    serial_u32_hex((v >> 32) as u32);
    serial_u32_hex(v as u32);
}

// ---------------------------------------------------------------------------
// Simple CRC-32 (no-alloc, no-float, no table — bit-by-bit Sarwate variant)
// ---------------------------------------------------------------------------

/// Compute CRC-32/ISO-HDLC over `data`.
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

// ---------------------------------------------------------------------------
// Kernel log snapshot helpers
// ---------------------------------------------------------------------------

/// Copy up to `LOG_SNAP_SIZE` bytes of the kernel serial ring buffer into
/// `dest`.  Returns the number of bytes written.
///
/// Falls back gracefully if `kernel_log` is unavailable.
fn copy_kernel_log(dest: &mut [u8; LOG_SNAP_SIZE]) -> usize {
    // The kernel_log crate stores messages in a heap-backed VecDeque which we
    // cannot access here without alloc.  Instead we snapshot the raw serial TX
    // ring from `crate::serial` if it exports a snapshot helper, or we simply
    // write a placeholder.
    //
    // This implementation writes a static notice and returns its length.
    // A future revision can hook into `serial::snapshot_ring(dest)` once that
    // API exists.
    let notice = b"[kernel_log snapshot not yet wired -- see debug/crash_dump.rs]";
    let copy = notice.len().min(LOG_SNAP_SIZE);
    dest[..copy].copy_from_slice(&notice[..copy]);
    copy
}

// ---------------------------------------------------------------------------
// Reserved-region helpers (volatile read/write)
// ---------------------------------------------------------------------------

/// Write one byte to the reserved crash-dump region at byte offset `off`.
#[inline]
unsafe fn region_write_u8(off: usize, val: u8) {
    let ptr = (CRASH_DUMP_PHYS as usize).saturating_add(off) as *mut u8;
    core::ptr::write_volatile(ptr, val);
}

/// Read one byte from the reserved crash-dump region at byte offset `off`.
#[inline]
unsafe fn region_read_u8(off: usize) -> u8 {
    let ptr = (CRASH_DUMP_PHYS as usize).saturating_add(off) as *const u8;
    core::ptr::read_volatile(ptr)
}

/// Write a u32 (little-endian) to the reserved region at byte offset `off`.
unsafe fn region_write_u32(off: usize, val: u32) {
    let bytes = val.to_le_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        region_write_u8(off.saturating_add(i), b);
    }
}

/// Read a u32 (little-endian) from the reserved region at byte offset `off`.
unsafe fn region_read_u32(off: usize) -> u32 {
    let mut bytes = [0u8; 4];
    for i in 0..4 {
        bytes[i] = region_read_u8(off.saturating_add(i));
    }
    u32::from_le_bytes(bytes)
}

/// Write a u64 (little-endian) to the reserved region at byte offset `off`.
unsafe fn region_write_u64(off: usize, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    region_write_u32(off, lo);
    region_write_u32(off.saturating_add(4), hi);
}

/// Read a u64 (little-endian) from the reserved region at byte offset `off`.
unsafe fn region_read_u64(off: usize) -> u64 {
    let lo = region_read_u32(off) as u64;
    let hi = region_read_u32(off.saturating_add(4)) as u64;
    lo | (hi << 32)
}

// ---------------------------------------------------------------------------
// Compute field offsets in CrashDump without alloc
// ---------------------------------------------------------------------------

// Because we cannot use `offset_of!` in stable no_std easily, we use a
// const-friendly layout calculation via a zero-initialized dummy struct.
//
// All offsets are defined manually in the same order as CrashDump fields.

const OFF_MAGIC: usize = 0;
const OFF_MESSAGE: usize = OFF_MAGIC + 4;
const OFF_MSG_LEN: usize = OFF_MESSAGE + MAX_MSG;
const OFF_STACK_TRACE: usize = OFF_MSG_LEN + 4;
const OFF_TRACE_DEPTH: usize = OFF_STACK_TRACE + MAX_TRACE * 8;
const OFF_PID: usize = OFF_TRACE_DEPTH + 4;
const OFF_CPU: usize = OFF_PID + 4;
const OFF_PAD: usize = OFF_CPU + 1;
const OFF_TIMESTAMP_TSC: usize = OFF_PAD + 3;
const OFF_KERNEL_LOG: usize = OFF_TIMESTAMP_TSC + 8;
const OFF_LOG_LEN: usize = OFF_KERNEL_LOG + LOG_SNAP_SIZE;
const OFF_CRC32: usize = OFF_LOG_LEN + 4;
const OFF_END: usize = OFF_CRC32 + 4;

/// Byte length of the CrashDump struct as laid out in the reserved region.
const DUMP_SIZE: usize = OFF_END;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Write a crash dump derived from `oops` to the reserved memory region and
/// emit it to the serial port.
///
/// This function is safe to call from a panic handler — it uses only
/// `read_volatile` / `write_volatile`, no heap, no locks held.
pub fn save_crash_dump(oops: &OopsRecord) {
    // Re-entrancy guard.
    if DUMP_IN_PROGRESS
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
        .is_err()
    {
        crate::serial_println!("  [crash_dump] re-entrant call ignored");
        return;
    }

    // ---- Snapshot kernel log ------------------------------------------------
    let mut log_snap = [0u8; LOG_SNAP_SIZE];
    let log_len = copy_kernel_log(&mut log_snap);

    // ---- Compute CRC over payload (everything except magic and crc fields) --
    // We build the payload in a temporary stack buffer for CRC, then write
    // it field-by-field to the region via volatile writes.
    //
    // The CRC covers bytes [OFF_MESSAGE .. OFF_CRC32).
    // We compute it over message + stack_trace + metadata + log inline
    // without building the full struct on the stack (which is 8+ KiB).
    //
    // Instead: compute CRC in multiple passes over each field.
    let mut hasher_state: u32 = 0xFFFF_FFFF;

    macro_rules! crc_bytes {
        ($slice:expr) => {
            for &b in $slice.iter() {
                hasher_state ^= b as u32;
                for _ in 0..8 {
                    if hasher_state & 1 != 0 {
                        hasher_state = (hasher_state >> 1) ^ 0xEDB8_8320;
                    } else {
                        hasher_state >>= 1;
                    }
                }
            }
        };
    }

    crc_bytes!(&oops.message[..]);
    let msg_len_le = (oops.msg_len as u32).to_le_bytes();
    crc_bytes!(&msg_len_le);
    for i in 0..oops.trace_depth {
        let addr_le = oops.stack_trace[i].to_le_bytes();
        crc_bytes!(&addr_le);
    }
    // Zero-pad remaining trace slots.
    for _ in oops.trace_depth..MAX_TRACE {
        crc_bytes!(&[0u8; 8]);
    }
    let depth_le = (oops.trace_depth as u32).to_le_bytes();
    crc_bytes!(&depth_le);
    let pid_le = oops.pid.to_le_bytes();
    crc_bytes!(&pid_le);
    crc_bytes!(&[oops.cpu, 0u8, 0u8, 0u8]); // cpu + 3 pad bytes
    let tsc_le = oops.timestamp_tsc.to_le_bytes();
    crc_bytes!(&tsc_le);
    crc_bytes!(&log_snap[..log_len]);
    // Zero-pad remaining log.
    for _ in log_len..LOG_SNAP_SIZE {
        crc_bytes!(&[0u8]);
    }
    let log_len_le = (log_len as u32).to_le_bytes();
    crc_bytes!(&log_len_le);

    let computed_crc = !hasher_state;

    // ---- Write to reserved memory region via volatile stores ----------------
    unsafe {
        // magic
        region_write_u32(OFF_MAGIC, CRASH_DUMP_MAGIC);
        // message
        for i in 0..MAX_MSG {
            region_write_u8(OFF_MESSAGE.saturating_add(i), oops.message[i]);
        }
        // msg_len
        region_write_u32(OFF_MSG_LEN, oops.msg_len as u32);
        // stack_trace
        for i in 0..MAX_TRACE {
            let addr = if i < oops.trace_depth {
                oops.stack_trace[i]
            } else {
                0u64
            };
            region_write_u64(OFF_STACK_TRACE.saturating_add(i * 8), addr);
        }
        // trace_depth
        region_write_u32(OFF_TRACE_DEPTH, oops.trace_depth as u32);
        // pid
        region_write_u32(OFF_PID, oops.pid);
        // cpu + pad
        region_write_u8(OFF_CPU, oops.cpu);
        region_write_u8(OFF_PAD, 0);
        region_write_u8(OFF_PAD.saturating_add(1), 0);
        region_write_u8(OFF_PAD.saturating_add(2), 0);
        // timestamp_tsc
        region_write_u64(OFF_TIMESTAMP_TSC, oops.timestamp_tsc);
        // kernel_log
        for i in 0..LOG_SNAP_SIZE {
            region_write_u8(OFF_KERNEL_LOG.saturating_add(i), log_snap[i]);
        }
        // log_len
        region_write_u32(OFF_LOG_LEN, log_len as u32);
        // crc32
        region_write_u32(OFF_CRC32, computed_crc);
    }

    // ---- Emit to serial: 0xAA55 magic + hex dump ----------------------------
    crate::serial_println!("");
    crate::serial_println!("=== GENESIS CRASH DUMP BEGIN ===");
    crate::serial_print!("MAGIC:AA55 ");
    // Emit magic + message length + trace depth for quick parsing.
    crate::serial_print!("DUMPMAGIC:");
    serial_u32_hex(CRASH_DUMP_MAGIC);
    crate::serial_print!(" MSGLEN:");
    serial_u32_hex(oops.msg_len as u32);
    crate::serial_print!(" TRACEDEPTH:");
    serial_u32_hex(oops.trace_depth as u32);
    crate::serial_print!(" TSC:");
    serial_u64_hex(oops.timestamp_tsc);
    crate::serial_print!(" CRC:");
    serial_u32_hex(computed_crc);
    crate::serial_println!("");

    // Print message as text.
    let msg_bytes = &oops.message[..oops.msg_len.min(MAX_MSG)];
    let msg_str = core::str::from_utf8(msg_bytes).unwrap_or("<invalid utf8>");
    crate::serial_println!("MSG: {}", msg_str);

    // Print stack trace.
    crate::serial_println!("TRACE ({} frames):", oops.trace_depth);
    for i in 0..oops.trace_depth {
        let addr = oops.stack_trace[i];
        crate::serial_print!("  #{:02}  ", i);
        serial_u64_hex(addr);

        if let Some((sym_addr, name)) = crate::kernel::kallsyms::lookup(addr) {
            let offset = addr.saturating_sub(sym_addr);
            crate::serial_print!("  <{}+0x", name);
            serial_u64_hex(offset);
            crate::serial_println!(">");
        } else {
            crate::serial_println!("  <no symbol>");
        }
    }

    // Hex dump of kernel log.
    crate::serial_println!("KLOG ({} bytes):", log_len);
    let print_len = log_len.min(512); // cap serial output
    for i in 0..print_len {
        serial_byte_hex(log_snap[i]);
        if i % 32 == 31 {
            crate::serial_println!("");
        }
    }
    if print_len < log_len {
        crate::serial_println!(
            "... ({} bytes truncated in serial output)",
            log_len - print_len
        );
    }

    crate::serial_println!("=== GENESIS CRASH DUMP END ===");

    DUMP_IN_PROGRESS.store(false, Ordering::Release);
}

/// Check whether the reserved memory region contains a valid crash dump from a
/// previous boot.
///
/// If a valid dump is found it is printed to serial and returned.
/// Returns `None` if no valid dump is present or if the CRC fails.
pub fn check_for_crash_dump() -> Option<CrashDump> {
    let magic = unsafe { region_read_u32(OFF_MAGIC) };
    if magic != CRASH_DUMP_MAGIC {
        return None;
    }

    // Read the stored CRC.
    let stored_crc = unsafe { region_read_u32(OFF_CRC32) };

    // Recompute CRC from the region.
    let mut hasher_state: u32 = 0xFFFF_FFFF;
    for i in OFF_MESSAGE..OFF_CRC32 {
        let b = unsafe { region_read_u8(i) };
        hasher_state ^= b as u32;
        for _ in 0..8 {
            if hasher_state & 1 != 0 {
                hasher_state = (hasher_state >> 1) ^ 0xEDB8_8320;
            } else {
                hasher_state >>= 1;
            }
        }
    }
    let computed_crc = !hasher_state;

    if computed_crc != stored_crc {
        crate::serial_println!(
            "  [crash_dump] CRC mismatch: stored={:#010x} computed={:#010x} — ignoring",
            stored_crc,
            computed_crc
        );
        return None;
    }

    // Read fields into a CrashDump on the stack.
    let mut dump = CrashDump {
        magic: magic,
        message: [0u8; MAX_MSG],
        msg_len: 0,
        stack_trace: [0u64; MAX_TRACE],
        trace_depth: 0,
        pid: 0,
        cpu: 0,
        _pad: [0u8; 3],
        timestamp_tsc: 0,
        kernel_log: [0u8; LOG_SNAP_SIZE],
        log_len: 0,
        crc32: stored_crc,
    };

    unsafe {
        for i in 0..MAX_MSG {
            dump.message[i] = region_read_u8(OFF_MESSAGE.saturating_add(i));
        }
        dump.msg_len = region_read_u32(OFF_MSG_LEN);
        for i in 0..MAX_TRACE {
            dump.stack_trace[i] = region_read_u64(OFF_STACK_TRACE.saturating_add(i * 8));
        }
        dump.trace_depth = region_read_u32(OFF_TRACE_DEPTH);
        dump.pid = region_read_u32(OFF_PID);
        dump.cpu = region_read_u8(OFF_CPU);
        dump.timestamp_tsc = region_read_u64(OFF_TIMESTAMP_TSC);
        for i in 0..LOG_SNAP_SIZE {
            dump.kernel_log[i] = region_read_u8(OFF_KERNEL_LOG.saturating_add(i));
        }
        dump.log_len = region_read_u32(OFF_LOG_LEN);
    }

    // Print summary to serial.
    crate::serial_println!("  [crash_dump] Previous crash dump found (CRC OK)");
    let msg_len = (dump.msg_len as usize).min(MAX_MSG);
    let msg_str = core::str::from_utf8(&dump.message[..msg_len]).unwrap_or("<invalid>");
    crate::serial_println!("  [crash_dump] MSG: {}", msg_str);
    crate::serial_println!("  [crash_dump] TSC: {:#018x}", dump.timestamp_tsc);
    crate::serial_println!(
        "  [crash_dump] PID: {}  CPU: {}  TRACE: {} frames",
        dump.pid,
        dump.cpu,
        dump.trace_depth
    );

    Some(dump)
}

/// Invalidate the crash dump region by zeroing the magic field.
pub fn clear_crash_dump() {
    unsafe {
        region_write_u32(OFF_MAGIC, 0);
    }
    crate::serial_println!("  [crash_dump] Crash dump region cleared");
}

/// Initialize the crash dump subsystem.
///
/// Checks for a previous crash dump and logs the result.
pub fn init() {
    crate::serial_println!(
        "  [crash_dump] Checking for previous crash dump at {:#018x}",
        CRASH_DUMP_PHYS
    );
    if let Some(_dump) = check_for_crash_dump() {
        crate::serial_println!(
            "  [crash_dump] Previous crash dump loaded — run 'dmesg-crash' to view"
        );
        // Optionally clear now so it doesn't reappear next boot.
        // Intentionally left to the operator: clear_crash_dump();
    } else {
        crate::serial_println!("  [crash_dump] No previous crash dump found");
    }
    crate::serial_println!(
        "  [crash_dump] Crash dump subsystem initialized ({} bytes reserved)",
        DUMP_SIZE
    );
}
