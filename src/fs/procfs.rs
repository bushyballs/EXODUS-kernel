/// procfs — /proc virtual filesystem for Genesis
///
/// Exposes kernel and process information as files.
/// Read-only synthetic files generated on-the-fly from kernel state.
///
/// Key files:
///   /proc/version        — Kernel version string
///   /proc/uptime         — System uptime in seconds
///   /proc/loadavg        — Load average (stub)
///   /proc/meminfo        — Memory statistics
///   /proc/cpuinfo        — CPU information
///   /proc/stat           — CPU and process statistics
///   /proc/interrupts     — IRQ table
///   /proc/ioports        — I/O port ranges (stub)
///   /proc/iomem          — Memory map (stub)
///   /proc/net/dev        — Network interface statistics
///   /proc/net/route      — Routing table (stub)
///   /proc/net/if_inet6   — IPv6 interfaces (stub)
///   /proc/sys/kernel/hostname   — System hostname (r/w)
///   /proc/sys/kernel/ostype     — OS type
///   /proc/sys/kernel/osrelease  — OS release string
///   /proc/sys/vm/overcommit_memory — VM overcommit policy (r/w)
///   /proc/self/status    — Current process status (stub)
///   /proc/self/maps      — Current process memory maps (stub)
///   /proc/[pid]/status   — Per-process status
///   /proc/mounts         — Mounted filesystems
///   /proc/buddyinfo      — Buddy allocator info
///   /proc/slabinfo       — Slab allocator info
///   /proc/vmallocinfo    — vmalloc info
///   /proc/filesystems    — Registered filesystem types
///   /proc/kmsg           — Kernel log ring buffer
///
/// No-heap interface: procfs_read / procfs_write / procfs_readdir / procfs_is_path
/// work entirely with fixed-size stack buffers and static arrays.
///
/// Heap-based interface (read / list_dir / pid_status / pid_maps / pid_cmdline /
/// kmsg / kmsg_read) is preserved for callers in vfs.rs that already use alloc.
///
/// Inspired by: Linux procfs (fs/proc/). All code is original.
use crate::sync::Mutex;

// ─── Heap-based API (kept for VFS compatibility) ──────────────────────────────

use alloc::format;
use alloc::string::String;

// ─── Byte-level helpers ───────────────────────────────────────────────────────

/// Write the decimal ASCII representation of `val` into `buf[..32]`.
/// Returns the number of bytes written (1 for zero).
/// No heap, no format!, no float casts.
fn u64_to_ascii(val: u64, buf: &mut [u8; 32]) -> usize {
    if val == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 32];
    let mut pos = 32usize;
    let mut v = val;
    while v > 0 {
        pos = pos.saturating_sub(1);
        tmp[pos] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    let len = 32 - pos;
    buf[..len].copy_from_slice(&tmp[pos..32]);
    len
}

/// Append a static string literal to `buf` starting at `*pos`.
/// Returns the new position. Silently truncates if `buf` is full.
#[inline]
fn append_str(buf: &mut [u8; 4096], pos: &mut usize, s: &[u8]) {
    for &b in s {
        if *pos < 4096 {
            buf[*pos] = b;
            *pos = pos.saturating_add(1);
        } else {
            break;
        }
    }
}

/// Append the decimal representation of `val` to `buf` at `*pos`.
#[inline]
fn append_u64(buf: &mut [u8; 4096], pos: &mut usize, val: u64) {
    let mut tmp = [0u8; 32];
    let len = u64_to_ascii(val, &mut tmp);
    append_str(buf, pos, &tmp[..len]);
}

/// Append a newline.
#[inline]
fn append_nl(buf: &mut [u8; 4096], pos: &mut usize) {
    if *pos < 4096 {
        buf[*pos] = b'\n';
        *pos = pos.saturating_add(1);
    }
}

/// Compare a path byte slice against a static string literal.
#[inline]
fn path_eq(path: &[u8], s: &[u8]) -> bool {
    path == s
}

/// Return true if `path` starts with `prefix`.
#[inline]
fn path_starts_with(path: &[u8], prefix: &[u8]) -> bool {
    path.len() >= prefix.len() && &path[..prefix.len()] == prefix
}

/// Extract a numeric PID from a byte path of the form b"/proc/<digits>[/...]".
/// Returns `None` if the segment is not purely decimal or is out of range.
fn extract_pid_from_path(path: &[u8]) -> Option<u32> {
    // Skip "/proc/"
    if path.len() < 7 {
        return None;
    }
    let after_proc = &path[6..]; // skip b"/proc/"
                                 // Find end of digit sequence (slash or end-of-string)
    let end = after_proc
        .iter()
        .position(|&b| b == b'/')
        .unwrap_or(after_proc.len());
    let digit_bytes = &after_proc[..end];
    if digit_bytes.is_empty() || digit_bytes.len() > 10 {
        return None;
    }
    let mut val: u32 = 0;
    for &b in digit_bytes {
        if b < b'0' || b > b'9' {
            return None;
        }
        // saturating: if overflow just return None
        let digit = (b - b'0') as u32;
        val = match val.checked_mul(10).and_then(|v| v.checked_add(digit)) {
            Some(v) => v,
            None => return None,
        };
    }
    Some(val)
}

/// Parse a single decimal digit byte as a u8.  Returns None on non-digit.
fn parse_digit_byte(b: u8) -> Option<u8> {
    if b >= b'0' && b <= b'9' {
        Some(b - b'0')
    } else {
        None
    }
}

// ─── ProcEntry registry (no-heap static table) ───────────────────────────────

/// Classification of a /proc entry.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ProcEntryType {
    File,
    Directory,
    Symlink,
}

/// A single registered /proc entry.
#[derive(Copy, Clone)]
pub struct ProcEntry {
    /// NUL-padded UTF-8 path (e.g. b"/proc/version\0...").
    pub path: [u8; 64],
    pub entry_type: ProcEntryType,
    pub active: bool,
}

impl ProcEntry {
    pub const fn empty() -> Self {
        ProcEntry {
            path: [0u8; 64],
            entry_type: ProcEntryType::File,
            active: false,
        }
    }
}

/// Registry of known /proc entries.  Populated during init().
static PROC_ENTRIES: Mutex<[ProcEntry; 128]> = Mutex::new([const { ProcEntry::empty() }; 128]);

/// Register a /proc entry in the static table.
fn register_entry(path: &[u8], entry_type: ProcEntryType) {
    let mut table = PROC_ENTRIES.lock();
    for slot in table.iter_mut() {
        if !slot.active {
            let len = path.len().min(63);
            slot.path[..len].copy_from_slice(&path[..len]);
            slot.path[len] = 0;
            slot.entry_type = entry_type;
            slot.active = true;
            return;
        }
    }
    // Table full — silently drop (no panic)
}

// ─── Overcommit setting (writable via /proc/sys/vm/overcommit_memory) ─────────

static OVERCOMMIT_MEMORY: Mutex<u8> = Mutex::new(0);

// ─── Hostname buffer (writable via /proc/sys/kernel/hostname) ─────────────────

static HOSTNAME: Mutex<[u8; 64]> = Mutex::new(*b"genesis\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0");
static HOSTNAME_LEN: Mutex<usize> = Mutex::new(7);

// ─── procfs_is_path ────────────────────────────────────────────────────────────

/// Returns true if `path` is within the /proc virtual filesystem.
pub fn procfs_is_path(path: &[u8]) -> bool {
    path_eq(path, b"/proc") || path_starts_with(path, b"/proc/")
}

// ─── procfs_read ──────────────────────────────────────────────────────────────

/// Read a /proc file into `buf`.
///
/// Returns the number of bytes written on success (>= 0), or:
///   -1  (EROFS) — path is read-only and a write was attempted elsewhere
///   -2  (ENOENT) — path not recognized
pub fn procfs_read(path: &[u8], buf: &mut [u8; 4096]) -> isize {
    let mut pos = 0usize;

    // ── /proc/version ──────────────────────────────────────────────────────────
    if path_eq(path, b"/proc/version") {
        append_str(
            buf,
            &mut pos,
            b"Linux version 6.1.0-genesis (genesis@kernel) (rustc)\n",
        );
        return pos as isize;
    }

    // ── /proc/uptime ───────────────────────────────────────────────────────────
    if path_eq(path, b"/proc/uptime") {
        let ms = crate::time::clock::uptime_ms();
        let secs = ms / 1000;
        let cs = (ms % 1000) / 10; // centiseconds (no float)
        append_u64(buf, &mut pos, secs);
        buf[pos] = b'.';
        pos = pos.saturating_add(1);
        // zero-pad centiseconds to 2 digits
        if cs < 10 {
            buf[pos] = b'0';
            pos = pos.saturating_add(1);
        }
        append_u64(buf, &mut pos, cs);
        append_str(buf, &mut pos, b" ");
        // idle_secs = uptime (single CPU, idle == uptime for stub)
        append_u64(buf, &mut pos, secs);
        buf[pos] = b'.';
        pos = pos.saturating_add(1);
        if cs < 10 {
            buf[pos] = b'0';
            pos = pos.saturating_add(1);
        }
        append_u64(buf, &mut pos, cs);
        append_nl(buf, &mut pos);
        return pos as isize;
    }

    // ── /proc/loadavg ──────────────────────────────────────────────────────────
    if path_eq(path, b"/proc/loadavg") {
        append_str(buf, &mut pos, b"0.00 0.00 0.00 1/1 1\n");
        return pos as isize;
    }

    // ── /proc/meminfo ──────────────────────────────────────────────────────────
    if path_eq(path, b"/proc/meminfo") {
        // Try to get real stats from the frame allocator
        let (total_kb, free_kb, used_kb) = {
            let fa = crate::memory::frame_allocator::FRAME_ALLOCATOR.lock();
            let total = crate::memory::frame_allocator::MAX_MEMORY;
            let free = fa
                .free_count()
                .saturating_mul(crate::memory::frame_allocator::FRAME_SIZE);
            let used = fa
                .used_count()
                .saturating_mul(crate::memory::frame_allocator::FRAME_SIZE);
            drop(fa);
            (total / 1024, free / 1024, used / 1024)
        };
        let avail_kb = free_kb;

        macro_rules! memline {
            ($label:expr, $val:expr, $p:expr) => {{
                append_str(buf, $p, $label);
                append_u64(buf, $p, $val as u64);
                append_str(buf, $p, b" kB\n");
            }};
        }
        memline!(b"MemTotal:     ", total_kb, &mut pos);
        memline!(b"MemFree:      ", free_kb, &mut pos);
        memline!(b"MemAvailable: ", avail_kb, &mut pos);
        memline!(b"Buffers:      ", 0, &mut pos);
        memline!(b"Cached:       ", 0, &mut pos);
        memline!(b"SwapCached:   ", 0, &mut pos);
        memline!(b"Active:       ", used_kb, &mut pos);
        memline!(b"Inactive:     ", 0, &mut pos);
        memline!(b"SwapTotal:    ", 0, &mut pos);
        memline!(b"SwapFree:     ", 0, &mut pos);
        memline!(b"Dirty:        ", 0, &mut pos);
        memline!(b"Writeback:    ", 0, &mut pos);
        memline!(b"Slab:         ", 0, &mut pos);
        return pos as isize;
    }

    // ── /proc/cpuinfo ──────────────────────────────────────────────────────────
    if path_eq(path, b"/proc/cpuinfo") {
        append_str(
            buf,
            &mut pos,
            b"processor\t: 0\n\
              vendor_id\t: GenuineIntel\n\
              cpu family\t: 6\n\
              model\t: 85\n\
              model name\t: Genesis AI OS Processor\n\
              cpu MHz\t: 3000\n\
              cache size\t: 8192 KB\n\
              bogomips\t: 6000.00\n\n",
        );
        return pos as isize;
    }

    // ── /proc/interrupts ───────────────────────────────────────────────────────
    if path_eq(path, b"/proc/interrupts") {
        append_str(
            buf,
            &mut pos,
            b"           CPU0\n\
               0:          1   XT-PIC  timer\n\
               1:          0   XT-PIC  keyboard\n\
               8:          0   XT-PIC  rtc\n\
              14:          0   XT-PIC  primary_ide\n\
              15:          0   XT-PIC  secondary_ide\n",
        );
        return pos as isize;
    }

    // ── /proc/ioports ──────────────────────────────────────────────────────────
    if path_eq(path, b"/proc/ioports") {
        append_str(
            buf,
            &mut pos,
            b"0000-001f : dma1\n\
              0020-003f : pic1\n\
              0040-005f : timer\n\
              0060-006f : keyboard\n\
              0070-007f : rtc\n\
              00a0-00bf : pic2\n\
              00c0-00df : dma2\n\
              00f0-00ff : fpu\n\
              0170-0177 : ide1\n\
              01f0-01f7 : ide0\n\
              03f8-03ff : serial\n",
        );
        return pos as isize;
    }

    // ── /proc/iomem ────────────────────────────────────────────────────────────
    if path_eq(path, b"/proc/iomem") {
        append_str(
            buf,
            &mut pos,
            b"00000000-0009ffff : System RAM\n\
              00100000-1fffffff : System RAM\n",
        );
        return pos as isize;
    }

    // ── /proc/stat ─────────────────────────────────────────────────────────────
    if path_eq(path, b"/proc/stat") {
        let uptime = crate::time::clock::uptime_secs();
        // idle jiffies = uptime * 100 (centiseconds / jiffies)
        let idle = uptime.saturating_mul(100);
        append_str(buf, &mut pos, b"cpu 0 0 0 ");
        append_u64(buf, &mut pos, idle);
        append_str(buf, &mut pos, b" 0 0 0 0 0 0\n");
        append_str(buf, &mut pos, b"cpu0 0 0 0 ");
        append_u64(buf, &mut pos, idle);
        append_str(buf, &mut pos, b" 0 0 0 0 0 0\n");
        append_str(buf, &mut pos, b"processes 1\n");
        append_str(buf, &mut pos, b"procs_running 1\n");
        append_str(buf, &mut pos, b"procs_blocked 0\n");
        return pos as isize;
    }

    // ── /proc/net/dev ──────────────────────────────────────────────────────────
    if path_eq(path, b"/proc/net/dev") {
        append_str(buf, &mut pos,
            b"Inter-|   Receive                                                |  Transmit\n\
               face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n\
                 lo:       0       0    0    0    0     0          0         0        0       0    0    0    0     0       0          0\n\
               eth0:       0       0    0    0    0     0          0         0        0       0    0    0    0     0       0          0\n");
        return pos as isize;
    }

    // ── /proc/net/route ────────────────────────────────────────────────────────
    if path_eq(path, b"/proc/net/route") {
        append_str(
            buf,
            &mut pos,
            b"Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n",
        );
        return pos as isize;
    }

    // ── /proc/net/if_inet6 ─────────────────────────────────────────────────────
    if path_eq(path, b"/proc/net/if_inet6") {
        // Empty — no IPv6 addresses configured
        return 0;
    }

    // ── /proc/sys/kernel/hostname ──────────────────────────────────────────────
    if path_eq(path, b"/proc/sys/kernel/hostname") {
        let len = *HOSTNAME_LEN.lock();
        let hn = *HOSTNAME.lock();
        let copy = len.min(4095);
        buf[..copy].copy_from_slice(&hn[..copy]);
        pos = copy;
        append_nl(buf, &mut pos);
        return pos as isize;
    }

    // ── /proc/sys/kernel/ostype ────────────────────────────────────────────────
    if path_eq(path, b"/proc/sys/kernel/ostype") {
        append_str(buf, &mut pos, b"Linux\n");
        return pos as isize;
    }

    // ── /proc/sys/kernel/osrelease ────────────────────────────────────────────
    if path_eq(path, b"/proc/sys/kernel/osrelease") {
        append_str(buf, &mut pos, b"6.1.0-genesis\n");
        return pos as isize;
    }

    // ── /proc/sys/vm/overcommit_memory ────────────────────────────────────────
    if path_eq(path, b"/proc/sys/vm/overcommit_memory") {
        let v = *OVERCOMMIT_MEMORY.lock();
        buf[0] = b'0' + v;
        pos = 1;
        append_nl(buf, &mut pos);
        return pos as isize;
    }

    // ── /proc/self/status ──────────────────────────────────────────────────────
    if path_eq(path, b"/proc/self/status") {
        append_str(
            buf,
            &mut pos,
            b"Name:\tself\nState:\tR (running)\nPid:\t0\nPPid:\t0\nThreads:\t1\n",
        );
        return pos as isize;
    }

    // ── /proc/self/maps ────────────────────────────────────────────────────────
    if path_eq(path, b"/proc/self/maps") {
        append_str(buf, &mut pos, b"00000000-ffffffff r-xp 00000000 00:00 0\n");
        return pos as isize;
    }

    // ── /proc/mounts ───────────────────────────────────────────────────────────
    if path_eq(path, b"/proc/mounts") {
        append_str(
            buf,
            &mut pos,
            b"rootfs / rootfs rw 0 0\n\
              devfs /dev devfs rw 0 0\n\
              proc /proc proc rw 0 0\n\
              sys /sys sysfs rw 0 0\n\
              tmpfs /tmp tmpfs rw 0 0\n\
              tmpfs /run tmpfs rw 0 0\n",
        );
        return pos as isize;
    }

    // ── /proc/filesystems ──────────────────────────────────────────────────────
    if path_eq(path, b"/proc/filesystems") {
        append_str(
            buf,
            &mut pos,
            b"nodev\tproc\nnodev\tsysfs\nnodev\tdevfs\nnodev\ttmpfs\n\text2\n\tfat32\n\thoagsfs\n",
        );
        return pos as isize;
    }

    // ── /proc/kmsg ─────────────────────────────────────────────────────────────
    if path_eq(path, b"/proc/kmsg") {
        let n = crate::kernel::printk::printk_read(buf);
        return n as isize;
    }

    // ── /proc/{PID}/status and /proc/{PID}/maps ────────────────────────────────
    if path_starts_with(path, b"/proc/") {
        // Check for /proc/<digits>/status or /proc/<digits>/maps
        if let Some(pid) = extract_pid_from_path(path) {
            // Find the sub-path suffix after "/proc/<digits>"
            let after_proc = &path[6..];
            let slash_pos = after_proc.iter().position(|&b| b == b'/');
            if let Some(sp) = slash_pos {
                let sub = &after_proc[sp.saturating_add(1)..];
                if sub == b"status" {
                    // Generate basic status for the PID
                    append_str(
                        buf,
                        &mut pos,
                        b"Name:\tprocess\nState:\tR (running)\nPid:\t",
                    );
                    append_u64(buf, &mut pos, pid as u64);
                    append_str(
                        buf,
                        &mut pos,
                        b"\nPPid:\t0\nUid:\t0\nGid:\t0\nThreads:\t1\n",
                    );
                    return pos as isize;
                }
                if sub == b"maps" {
                    append_str(buf, &mut pos, b"00000000-ffffffff r-xp 00000000 00:00 0\n");
                    return pos as isize;
                }
                if sub == b"cmdline" {
                    append_str(buf, &mut pos, b"genesis\0");
                    return pos as isize;
                }
            }
        }
    }

    // Unrecognized /proc path
    -2 // ENOENT
}

// ─── procfs_write ─────────────────────────────────────────────────────────────

/// Write to a writable /proc file.
///
/// Returns:
///   >= 0  bytes consumed on success
///   -1    EROFS — path is read-only
///   -2    ENOENT — path not found
pub fn procfs_write(path: &[u8], data: &[u8]) -> isize {
    // ── /proc/sys/kernel/hostname ─────────────────────────────────────────────
    if path_eq(path, b"/proc/sys/kernel/hostname") {
        // Accept up to 63 bytes; strip trailing newline
        let len = data.len().min(63);
        let trimmed = if len > 0 && data[len.saturating_sub(1)] == b'\n' {
            &data[..len.saturating_sub(1)]
        } else {
            &data[..len]
        };
        let write_len = trimmed.len().min(63);
        let mut hn = HOSTNAME.lock();
        // Zero out the buffer first
        for b in hn.iter_mut() {
            *b = 0;
        }
        hn[..write_len].copy_from_slice(&trimmed[..write_len]);
        drop(hn);
        *HOSTNAME_LEN.lock() = write_len;
        crate::serial_println!("  [procfs] hostname updated ({} bytes)", write_len);
        return data.len() as isize;
    }

    // ── /proc/sys/vm/overcommit_memory ────────────────────────────────────────
    if path_eq(path, b"/proc/sys/vm/overcommit_memory") {
        // Expect a single digit byte '0', '1', or '2'
        if let Some(&first) = data.first() {
            if let Some(digit) = parse_digit_byte(first) {
                if digit <= 2 {
                    *OVERCOMMIT_MEMORY.lock() = digit;
                    crate::serial_println!("  [procfs] overcommit_memory set to {}", digit);
                    return data.len() as isize;
                }
            }
        }
        return -1; // EROFS / invalid value
    }

    // All other /proc paths are read-only
    if procfs_is_path(path) {
        return -1; // EROFS
    }

    -2 // ENOENT
}

// ─── procfs_readdir ───────────────────────────────────────────────────────────

/// List entries in a /proc directory.
///
/// Writes null-terminated entry names (not full paths) into `out`.
/// Returns the count of entries filled in (0..=32).
pub fn procfs_readdir(path: &[u8], out: &mut [[u8; 64]; 32]) -> u32 {
    let mut count = 0u32;

    // Helper: add a name to the output array
    macro_rules! add_entry {
        ($name:expr) => {{
            if (count as usize) < 32 {
                let idx = count as usize;
                let name: &[u8] = $name;
                let len = name.len().min(63);
                out[idx][..len].copy_from_slice(&name[..len]);
                out[idx][len] = 0;
                count = count.saturating_add(1);
            }
        }};
    }

    // ── /proc/ root ───────────────────────────────────────────────────────────
    if path_eq(path, b"/proc") || path_eq(path, b"/proc/") {
        add_entry!(b"version");
        add_entry!(b"uptime");
        add_entry!(b"loadavg");
        add_entry!(b"meminfo");
        add_entry!(b"cpuinfo");
        add_entry!(b"stat");
        add_entry!(b"interrupts");
        add_entry!(b"ioports");
        add_entry!(b"iomem");
        add_entry!(b"mounts");
        add_entry!(b"filesystems");
        add_entry!(b"kmsg");
        add_entry!(b"net");
        add_entry!(b"sys");
        add_entry!(b"self");
        add_entry!(b"1");
        add_entry!(b"2");
        return count;
    }

    // ── /proc/net/ ────────────────────────────────────────────────────────────
    if path_eq(path, b"/proc/net") || path_eq(path, b"/proc/net/") {
        add_entry!(b"dev");
        add_entry!(b"route");
        add_entry!(b"if_inet6");
        return count;
    }

    // ── /proc/sys/ ────────────────────────────────────────────────────────────
    if path_eq(path, b"/proc/sys") || path_eq(path, b"/proc/sys/") {
        add_entry!(b"kernel");
        add_entry!(b"vm");
        return count;
    }

    // ── /proc/sys/kernel/ ────────────────────────────────────────────────────
    if path_eq(path, b"/proc/sys/kernel") || path_eq(path, b"/proc/sys/kernel/") {
        add_entry!(b"hostname");
        add_entry!(b"ostype");
        add_entry!(b"osrelease");
        return count;
    }

    // ── /proc/sys/vm/ ────────────────────────────────────────────────────────
    if path_eq(path, b"/proc/sys/vm") || path_eq(path, b"/proc/sys/vm/") {
        add_entry!(b"overcommit_memory");
        return count;
    }

    // ── /proc/self/ ──────────────────────────────────────────────────────────
    if path_eq(path, b"/proc/self") || path_eq(path, b"/proc/self/") {
        add_entry!(b"status");
        add_entry!(b"maps");
        add_entry!(b"cmdline");
        return count;
    }

    // ── /proc/<PID>/ ─────────────────────────────────────────────────────────
    if path_starts_with(path, b"/proc/") {
        if let Some(_pid) = extract_pid_from_path(path) {
            add_entry!(b"status");
            add_entry!(b"maps");
            add_entry!(b"cmdline");
            return count;
        }
    }

    count
}

// ─── Heap-based functions (kept for vfs.rs compatibility) ─────────────────────

/// Generate /proc/cpuinfo content (heap-based, for vfs.rs)
pub fn cpuinfo() -> String {
    let mut s = String::new();

    // Read CPUID
    let (vendor, family, model, stepping) = unsafe {
        let ebx: u32;
        let ecx: u32;
        let edx: u32;

        // Vendor string — save/restore rbx since LLVM reserves it
        core::arch::asm!(
            "push rbx",
            "mov eax, 0",
            "cpuid",
            "mov {0:e}, ebx",
            "mov {1:e}, ecx",
            "mov {2:e}, edx",
            "pop rbx",
            out(reg) ebx,
            out(reg) ecx,
            out(reg) edx,
            out("eax") _,
        );
        let vendor_bytes: [u8; 12] = [
            ebx as u8,
            (ebx >> 8) as u8,
            (ebx >> 16) as u8,
            (ebx >> 24) as u8,
            edx as u8,
            (edx >> 8) as u8,
            (edx >> 16) as u8,
            (edx >> 24) as u8,
            ecx as u8,
            (ecx >> 8) as u8,
            (ecx >> 16) as u8,
            (ecx >> 24) as u8,
        ];
        let vendor = core::str::from_utf8(&vendor_bytes).unwrap_or("Unknown");

        // Family/model/stepping
        let eax2: u32;
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "mov {0:e}, eax",
            "pop rbx",
            out(reg) eax2,
            out("ecx") _,
            out("edx") _,
        );
        let stepping_val = eax2 & 0xF;
        let model_val = ((eax2 >> 4) & 0xF) | (((eax2 >> 16) & 0xF) << 4);
        let family_val = ((eax2 >> 8) & 0xF) + ((eax2 >> 20) & 0xFF);

        (String::from(vendor), family_val, model_val, stepping_val)
    };

    let num_cpus = crate::smp::num_cpus();
    for i in 0..num_cpus {
        s.push_str(&format!("processor\t: {}\n", i));
        s.push_str(&format!("vendor_id\t: {}\n", vendor));
        s.push_str(&format!("cpu family\t: {}\n", family));
        s.push_str(&format!("model\t\t: {}\n", model));
        s.push_str(&format!("stepping\t: {}\n", stepping));
        s.push_str("model name\t: Hoags Genesis CPU\n");
        s.push_str(&format!(
            "bogomips\t: {}\n",
            crate::time::clock::tsc_freq_mhz() * 2
        ));
        s.push_str("flags\t\t: fpu sse sse2 nx rdrand tsc\n");
        s.push('\n');
    }
    s
}

/// Generate /proc/meminfo content (heap-based, for vfs.rs)
pub fn meminfo() -> String {
    let buddy = crate::memory::buddy::BUDDY.lock();
    let total_kb = (buddy.total_count() * 4096) / 1024;
    let free_kb = (buddy.free_count() * 4096) / 1024;
    let used_kb = total_kb - free_kb;
    let cached_pages = crate::memory::page_cache::PAGE_CACHE.lock().cached_count();
    let cached_kb = (cached_pages * 4096) / 1024;
    drop(buddy);

    format!(
        "MemTotal:       {:>8} kB\n\
         MemFree:        {:>8} kB\n\
         MemAvailable:   {:>8} kB\n\
         Buffers:        {:>8} kB\n\
         Cached:         {:>8} kB\n\
         SwapTotal:      {:>8} kB\n\
         SwapFree:       {:>8} kB\n\
         Active:         {:>8} kB\n\
         Inactive:       {:>8} kB\n\
         Dirty:          {:>8} kB\n\
         Slab:           {:>8} kB\n\
         VmallocTotal:   {:>8} kB\n\
         VmallocUsed:    {:>8} kB\n",
        total_kb,
        free_kb,
        free_kb + cached_kb,
        0,
        cached_kb,
        0,
        0,
        used_kb,
        cached_kb,
        (crate::memory::page_cache::PAGE_CACHE.lock().dirty_count() as usize * 4096) / 1024,
        0,
        crate::memory::vmalloc::VMALLOC_SIZE / 1024,
        0,
    )
}

/// Generate /proc/uptime content (heap-based, for vfs.rs)
pub fn uptime() -> String {
    let ms = crate::time::clock::uptime_ms();
    let secs = ms / 1000;
    let frac = (ms % 1000) / 10;
    format!("{}.{:02} 0.00\n", secs, frac)
}

/// Generate /proc/version content (heap-based, for vfs.rs)
pub fn version() -> String {
    format!(
        "Genesis version 1.0.0 (hoags@genesis) (rustc 1.82) #1 SMP {}\n",
        "2026-02-14"
    )
}

/// Generate /proc/loadavg content (heap-based, for vfs.rs)
pub fn loadavg() -> String {
    let nr_running = crate::process::scheduler::SCHEDULER.lock().queue_length();
    let total = {
        let table = crate::process::pcb::PROCESS_TABLE.lock();
        table.iter().filter(|p| p.is_some()).count()
    };
    format!("0.00 0.00 0.00 {}/{} 1\n", nr_running, total)
}

/// Generate /proc/stat content (heap-based, for vfs.rs)
pub fn stat() -> String {
    let ticks = crate::time::clock::uptime_ms() / 10; // approximate jiffies
    let num_cpus = crate::smp::num_cpus();
    let mut s = format!("cpu  {} 0 {} 0 0 0 0 0 0 0\n", ticks / 2, ticks / 2);
    for i in 0..num_cpus {
        s.push_str(&format!(
            "cpu{} {} 0 {} 0 0 0 0 0 0 0\n",
            i,
            ticks / (2 * num_cpus as u64),
            ticks / (2 * num_cpus as u64)
        ));
    }
    s.push_str(&format!("processes {}\n", 0));
    s.push_str(&format!(
        "procs_running {}\n",
        crate::process::scheduler::SCHEDULER.lock().queue_length()
    ));
    s.push_str("procs_blocked 0\n");
    s.push_str(&format!("btime {}\n", 0));
    s
}

/// Generate /proc/mounts content (heap-based, for vfs.rs)
pub fn mounts() -> String {
    let mut s = String::new();
    s.push_str("rootfs / rootfs rw 0 0\n");
    s.push_str("devfs /dev devfs rw 0 0\n");
    s.push_str("proc /proc proc rw 0 0\n");
    s.push_str("sys /sys sysfs rw 0 0\n");
    s.push_str("tmpfs /tmp tmpfs rw 0 0\n");
    s.push_str("tmpfs /run tmpfs rw 0 0\n");
    s
}

/// Generate /proc/buddyinfo content (heap-based, for vfs.rs)
pub fn buddyinfo() -> String {
    crate::memory::buddy::BUDDY.lock().buddyinfo()
}

/// Generate /proc/slabinfo content (heap-based, for vfs.rs)
pub fn slabinfo() -> String {
    crate::memory::slab::SLAB.lock().slabinfo()
}

/// Generate /proc/interrupts content (heap-based, for vfs.rs)
///
/// Format mirrors Linux /proc/interrupts:
///   <irq_nr>:  <count_cpu0>  [<count_cpuN> ...]   XT-PIC  <name>
///
/// Uses `crate::interrupts::irq_count(irq)` to read per-IRQ counters.
/// No float arithmetic — all values are integer u64.
pub fn interrupts() -> String {
    // Standard ISA IRQ names (IRQs 0-15).
    const IRQ_NAMES: [&str; 16] = [
        "timer",         // 0
        "keyboard",      // 1
        "cascade",       // 2
        "com2",          // 3
        "com1",          // 4
        "lpt2",          // 5
        "floppy",        // 6
        "lpt1",          // 7
        "rtc",           // 8
        "acpi",          // 9
        "reserved",      // 10
        "reserved",      // 11
        "mouse",         // 12
        "fpu",           // 13
        "primary_ide",   // 14
        "secondary_ide", // 15
    ];

    let num_cpus = crate::smp::num_cpus();
    let mut s = String::new();

    // Header line: "           CPU0  CPU1  ..."
    s.push_str("           ");
    for c in 0..num_cpus {
        s.push_str(&format!("CPU{}  ", c));
    }
    s.push('\n');

    for irq in 0u8..16u8 {
        let count = crate::interrupts::irq_count(irq);
        // Skip IRQs that have never fired to keep output compact.
        if count == 0 {
            continue;
        }
        // IRQ column (right-aligned in 3 chars)
        s.push_str(&format!("{:>3}:", irq));
        // Per-CPU counts — only CPU 0 has real data; others are zero.
        for c in 0..num_cpus {
            let cpu_count: u64 = if c == 0 { count } else { 0 };
            s.push_str(&format!(" {:>10}", cpu_count));
        }
        // Controller + name
        let name = IRQ_NAMES.get(irq as usize).copied().unwrap_or("unknown");
        s.push_str(&format!("   XT-PIC  {}\n", name));
    }

    // Spurious interrupt counter
    let spurious = crate::interrupts::spurious_irq_count();
    if spurious > 0 {
        s.push_str(&format!("SPU: {:>10}   spurious\n", spurious));
    }

    s
}

/// Generate /proc/{pid}/status content (heap-based, for vfs.rs)
pub fn pid_status(pid: u32) -> Option<String> {
    let table = crate::process::pcb::PROCESS_TABLE.lock();
    let proc = table[pid as usize].as_ref()?;
    Some(format!(
        "Name:\t{}\n\
         State:\t{:?}\n\
         Pid:\t{}\n\
         PPid:\t{}\n\
         Uid:\t{}\n\
         Gid:\t{}\n\
         VmSize:\t{} kB\n\
         Threads:\t1\n\
         SigPnd:\t{:#010x}\n",
        proc.name,
        proc.state,
        proc.pid,
        proc.parent_pid,
        proc.uid,
        proc.gid,
        proc.mmaps.len() * 4,
        proc.pending_signals,
    ))
}

/// Generate /proc/{pid}/maps content (heap-based, for vfs.rs)
pub fn pid_maps(pid: u32) -> Option<String> {
    let table = crate::process::pcb::PROCESS_TABLE.lock();
    let proc = table[pid as usize].as_ref()?;
    let mut s = String::new();
    for (virt, pages, flags) in &proc.mmaps {
        let end = virt + pages * 4096;
        let perms = format!(
            "{}{}{}p",
            if flags & 0x1 != 0 { "r" } else { "-" },
            if flags & 0x2 != 0 { "w" } else { "-" },
            if flags & 0x4 != 0 { "x" } else { "-" },
        );
        s.push_str(&format!(
            "{:08x}-{:08x} {} 00000000 00:00 0\n",
            virt, end, perms
        ));
    }
    Some(s)
}

/// Generate /proc/{pid}/cmdline content (heap-based, for vfs.rs)
pub fn pid_cmdline(pid: u32) -> Option<String> {
    let table = crate::process::pcb::PROCESS_TABLE.lock();
    let proc = table[pid as usize].as_ref()?;
    Some(proc.name.clone())
}

/// Generate /proc/kmsg content by draining the printk byte ring (heap-based).
pub fn kmsg() -> String {
    let mut buf = [0u8; 4096];
    let n = crate::kernel::printk::printk_read(&mut buf);
    let text = core::str::from_utf8(&buf[..n]).unwrap_or("");
    String::from(text)
}

/// Read raw bytes from /proc/kmsg into a caller-supplied buffer.
///
/// Used by the VFS layer for low-level `read(2)` handling.
/// Returns the number of bytes written into `buf`.
pub fn kmsg_read(buf: &mut [u8]) -> usize {
    crate::kernel::printk::printk_read(buf)
}

// ─── read() — heap dispatch used by vfs.rs ────────────────────────────────────

/// Read any procfs path, returning heap-allocated String content.
/// Called by `fs/vfs.rs` via `super::procfs::read(path)`.
pub fn read(path: &str) -> Option<String> {
    let path = path
        .trim_start_matches("/proc/")
        .trim_start_matches("/proc");

    match path {
        "cpuinfo"    => Some(cpuinfo()),
        "meminfo"    => Some(meminfo()),
        "uptime"     => Some(uptime()),
        "version"    => Some(version()),
        "loadavg"    => Some(loadavg()),
        "stat"       => Some(stat()),
        "mounts"     => Some(mounts()),
        "buddyinfo"  => Some(buddyinfo()),
        "slabinfo"   => Some(slabinfo()),
        "vmallocinfo" => Some(crate::memory::vmalloc::vmallocinfo()),
        "filesystems" => Some(String::from(
            "nodev\tproc\nnodev\tsysfs\nnodev\tdevfs\nnodev\ttmpfs\n\text2\n\tfat32\n\thoagsfs\n")),
        "interrupts" => Some(interrupts()),
        "kmsg"       => Some(kmsg()),
        "net/dev"    => Some(String::from(
            "Inter-|   Receive                                                |  Transmit\n \
             face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n \
               lo:       0       0    0    0    0     0          0         0        0       0    0    0    0     0       0          0\n \
             eth0:       0       0    0    0    0     0          0         0        0       0    0    0    0     0       0          0\n")),
        "net/route"  => Some(String::from(
            "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n")),
        "net/if_inet6" => Some(String::new()),
        "sys/kernel/hostname" => {
            let len = *HOSTNAME_LEN.lock();
            let hn  = *HOSTNAME.lock();
            let s = core::str::from_utf8(&hn[..len]).unwrap_or("genesis");
            Some(format!("{}\n", s))
        }
        "sys/kernel/ostype"    => Some(String::from("Linux\n")),
        "sys/kernel/osrelease" => Some(String::from("6.1.0-genesis\n")),
        "sys/vm/overcommit_memory" => {
            let v = *OVERCOMMIT_MEMORY.lock();
            Some(format!("{}\n", v))
        }
        "self/status" => Some(String::from(
            "Name:\tself\nState:\tR (running)\nPid:\t0\nPPid:\t0\nThreads:\t1\n")),
        "self/maps" => Some(String::from(
            "00000000-ffffffff r-xp 00000000 00:00 0\n")),
        _ => {
            // Check for /proc/[pid]/... paths
            let parts: alloc::vec::Vec<&str> = path.split('/').collect();
            if !parts.is_empty() {
                if let Ok(pid) = parts[0].parse::<u32>() {
                    if parts.len() == 1 {
                        return Some(String::from("status\nmaps\ncmdline\n"));
                    }
                    match parts.get(1).copied() {
                        Some("status")  => return pid_status(pid),
                        Some("maps")    => return pid_maps(pid),
                        Some("cmdline") => return pid_cmdline(pid),
                        _ => {}
                    }
                }
            }
            None
        }
    }
}

// ─── procfs_read (str variant) — zero-copy, used by vfs.rs ────────────────────

/// Read a procfs path directly into a caller-supplied byte buffer.
///
/// Fast path for /proc/kmsg drains the ring directly.
/// For all other paths, generates String content and copies it in.
/// Returns the number of bytes written.
pub fn procfs_read_str(path: &str, buf: &mut [u8]) -> usize {
    let stripped = path
        .trim_start_matches("/proc/")
        .trim_start_matches("/proc");
    if stripped == "kmsg" {
        return kmsg_read(buf);
    }
    match read(path) {
        Some(s) => {
            let bytes = s.as_bytes();
            let n = bytes.len().min(buf.len());
            buf[..n].copy_from_slice(&bytes[..n]);
            n
        }
        None => 0,
    }
}

// ─── list_dir (heap-based, for vfs.rs) ────────────────────────────────────────

/// List /proc directory entries.  Called by `fs/vfs.rs`.
pub fn list_dir(path: &str) -> alloc::vec::Vec<String> {
    let mut entries = alloc::vec::Vec::new();
    let path = path.trim_end_matches('/');

    if path == "/proc" || path.is_empty() {
        entries.push(String::from("cpuinfo"));
        entries.push(String::from("meminfo"));
        entries.push(String::from("uptime"));
        entries.push(String::from("version"));
        entries.push(String::from("loadavg"));
        entries.push(String::from("stat"));
        entries.push(String::from("mounts"));
        entries.push(String::from("buddyinfo"));
        entries.push(String::from("slabinfo"));
        entries.push(String::from("vmallocinfo"));
        entries.push(String::from("filesystems"));
        entries.push(String::from("interrupts"));
        entries.push(String::from("kmsg"));
        entries.push(String::from("net"));
        entries.push(String::from("sys"));
        entries.push(String::from("self"));

        // Add PID directories
        let table = crate::process::pcb::PROCESS_TABLE.lock();
        for i in 0..crate::process::MAX_PROCESSES {
            if table[i].is_some() {
                entries.push(format!("{}", i));
            }
        }
    }
    entries
}

// ─── init ─────────────────────────────────────────────────────────────────────

/// Initialize the procfs module: register all known /proc entries.
pub fn init() {
    // Directories
    register_entry(b"/proc", ProcEntryType::Directory);
    register_entry(b"/proc/net", ProcEntryType::Directory);
    register_entry(b"/proc/sys", ProcEntryType::Directory);
    register_entry(b"/proc/sys/kernel", ProcEntryType::Directory);
    register_entry(b"/proc/sys/vm", ProcEntryType::Directory);
    register_entry(b"/proc/self", ProcEntryType::Directory);

    // Root-level files
    register_entry(b"/proc/version", ProcEntryType::File);
    register_entry(b"/proc/uptime", ProcEntryType::File);
    register_entry(b"/proc/loadavg", ProcEntryType::File);
    register_entry(b"/proc/meminfo", ProcEntryType::File);
    register_entry(b"/proc/cpuinfo", ProcEntryType::File);
    register_entry(b"/proc/stat", ProcEntryType::File);
    register_entry(b"/proc/interrupts", ProcEntryType::File);
    register_entry(b"/proc/ioports", ProcEntryType::File);
    register_entry(b"/proc/iomem", ProcEntryType::File);
    register_entry(b"/proc/mounts", ProcEntryType::File);
    register_entry(b"/proc/filesystems", ProcEntryType::File);
    register_entry(b"/proc/kmsg", ProcEntryType::File);
    register_entry(b"/proc/buddyinfo", ProcEntryType::File);
    register_entry(b"/proc/slabinfo", ProcEntryType::File);
    register_entry(b"/proc/vmallocinfo", ProcEntryType::File);

    // /proc/net files
    register_entry(b"/proc/net/dev", ProcEntryType::File);
    register_entry(b"/proc/net/route", ProcEntryType::File);
    register_entry(b"/proc/net/if_inet6", ProcEntryType::File);

    // /proc/sys/kernel files
    register_entry(b"/proc/sys/kernel/hostname", ProcEntryType::File);
    register_entry(b"/proc/sys/kernel/ostype", ProcEntryType::File);
    register_entry(b"/proc/sys/kernel/osrelease", ProcEntryType::File);

    // /proc/sys/vm files
    register_entry(b"/proc/sys/vm/overcommit_memory", ProcEntryType::File);

    // /proc/self files
    register_entry(b"/proc/self/status", ProcEntryType::File);
    register_entry(b"/proc/self/maps", ProcEntryType::File);

    crate::serial_println!("  [procfs] /proc filesystem initialized");
}
