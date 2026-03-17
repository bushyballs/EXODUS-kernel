/// /proc filesystem compatibility layer
///
/// Part of the AIOS compatibility layer.
///
/// Provides a Linux-compatible /proc pseudo-filesystem that generates
/// process and system information on the fly. Programs that read
/// /proc/self/status, /proc/self/maps, /proc/cpuinfo, etc. get
/// compatible output.
///
/// Design:
///   - ProcEntry generators are registered by path pattern.
///   - Per-PID entries use a factory function that receives the PID.
///   - System-wide entries (/proc/cpuinfo, /proc/meminfo, /proc/uptime)
///     use static generators.
///   - The compat layer intercepts VFS reads to /proc paths and routes
///     them through the appropriate generator.
///   - Global Mutex<Option<Inner>> singleton.
///
/// Inspired by: Linux procfs (fs/proc). All code is original.

use alloc::string::String;
use alloc::vec::Vec;
use crate::sync::Mutex;
use crate::serial_println;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Type of proc entry.
#[derive(Clone, Copy, PartialEq)]
pub enum ProcEntryType {
    /// System-wide (e.g. /proc/cpuinfo)
    System,
    /// Per-process (e.g. /proc/<pid>/status)
    PerProcess,
    /// Per-process per-task (e.g. /proc/<pid>/task/<tid>/stat)
    PerTask,
}

/// Generator for system-wide entries.
type SystemGenerator = fn() -> Vec<u8>;

/// Generator for per-process entries. Receives the PID.
type ProcessGenerator = fn(u32) -> Vec<u8>;

/// A registered /proc entry.
enum ProcEntry {
    System {
        path: String,
        generator: SystemGenerator,
    },
    PerProcess {
        /// Suffix after /proc/<pid>/ (e.g. "status", "maps")
        suffix: String,
        generator: ProcessGenerator,
    },
}

/// Inner state.
struct Inner {
    entries: Vec<ProcEntry>,
}

// ---------------------------------------------------------------------------
// Default generators
// ---------------------------------------------------------------------------

fn gen_cpuinfo() -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"processor\t: 0\n");
    buf.extend_from_slice(b"vendor_id\t: GenesisAIOS\n");
    buf.extend_from_slice(b"cpu family\t: 6\n");
    buf.extend_from_slice(b"model name\t: AIOS Virtual CPU\n");
    buf.extend_from_slice(b"stepping\t: 0\n");
    buf.extend_from_slice(b"cpu MHz\t\t: 1000.000\n");
    buf.extend_from_slice(b"cache size\t: 256 KB\n");
    buf.extend_from_slice(b"bogomips\t: 2000.00\n");
    buf.extend_from_slice(b"flags\t\t: fpu vme de pse tsc\n");
    buf
}

fn gen_meminfo() -> Vec<u8> {
    let mut buf = Vec::new();
    // In a real implementation, pull actual values from the memory manager.
    buf.extend_from_slice(b"MemTotal:       131072 kB\n");
    buf.extend_from_slice(b"MemFree:         65536 kB\n");
    buf.extend_from_slice(b"MemAvailable:    98304 kB\n");
    buf.extend_from_slice(b"Buffers:          4096 kB\n");
    buf.extend_from_slice(b"Cached:          16384 kB\n");
    buf.extend_from_slice(b"SwapTotal:           0 kB\n");
    buf.extend_from_slice(b"SwapFree:            0 kB\n");
    buf
}

fn gen_uptime() -> Vec<u8> {
    let mut buf = Vec::new();
    // Seconds since boot + idle time
    buf.extend_from_slice(b"0.00 0.00\n");
    buf
}

fn gen_version() -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"Genesis AIOS version 0.1.0 (genesis@build) (rustc 1.80) #1 SMP\n");
    buf
}

fn gen_loadavg() -> Vec<u8> {
    Vec::from(&b"0.00 0.00 0.00 1/1 1\n"[..])
}

fn gen_stat() -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"cpu  0 0 0 0 0 0 0 0 0 0\n");
    buf.extend_from_slice(b"cpu0 0 0 0 0 0 0 0 0 0 0\n");
    buf.extend_from_slice(b"intr 0\n");
    buf.extend_from_slice(b"ctxt 0\n");
    buf.extend_from_slice(b"btime 0\n");
    buf.extend_from_slice(b"processes 1\n");
    buf.extend_from_slice(b"procs_running 1\n");
    buf.extend_from_slice(b"procs_blocked 0\n");
    buf
}

fn gen_filesystems() -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"\thoagsfs\n");
    buf.extend_from_slice(b"\tfat32\n");
    buf.extend_from_slice(b"\text2\n");
    buf.extend_from_slice(b"\ttmpfs\n");
    buf.extend_from_slice(b"nodev\tdevfs\n");
    buf.extend_from_slice(b"nodev\tprocfs\n");
    buf.extend_from_slice(b"nodev\tsysfs\n");
    buf
}

fn gen_mounts() -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"rootfs / hoagsfs rw 0 0\n");
    buf.extend_from_slice(b"devfs /dev devfs rw 0 0\n");
    buf.extend_from_slice(b"proc /proc procfs rw 0 0\n");
    buf.extend_from_slice(b"sysfs /sys sysfs rw 0 0\n");
    buf.extend_from_slice(b"tmpfs /tmp tmpfs rw 0 0\n");
    buf
}

fn gen_pid_status(pid: u32) -> Vec<u8> {
    let mut buf = Vec::new();
    // Format a minimal /proc/<pid>/status
    buf.extend_from_slice(b"Name:\tprocess\n");
    let pid_str = format_u32(pid);
    buf.extend_from_slice(b"Pid:\t");
    buf.extend_from_slice(pid_str.as_bytes());
    buf.extend_from_slice(b"\n");
    buf.extend_from_slice(b"State:\tR (running)\n");
    buf.extend_from_slice(b"Uid:\t0\t0\t0\t0\n");
    buf.extend_from_slice(b"Gid:\t0\t0\t0\t0\n");
    buf.extend_from_slice(b"VmSize:\t    4096 kB\n");
    buf.extend_from_slice(b"VmRSS:\t    1024 kB\n");
    buf.extend_from_slice(b"Threads:\t1\n");
    buf
}

fn gen_pid_maps(pid: u32) -> Vec<u8> {
    let _ = pid;
    let mut buf = Vec::new();
    buf.extend_from_slice(b"00400000-00401000 r-xp 00000000 00:00 0  [text]\n");
    buf.extend_from_slice(b"00600000-00601000 rw-p 00000000 00:00 0  [data]\n");
    buf.extend_from_slice(b"7fff0000-80000000 rw-p 00000000 00:00 0  [stack]\n");
    buf
}

fn gen_pid_cmdline(pid: u32) -> Vec<u8> {
    let _ = pid;
    Vec::from(&b"/bin/init\0"[..])
}

fn gen_pid_stat(pid: u32) -> Vec<u8> {
    let pid_str = format_u32(pid);
    let mut buf = Vec::new();
    buf.extend_from_slice(pid_str.as_bytes());
    buf.extend_from_slice(b" (process) R 0 0 0 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 0 4096 256 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n");
    buf
}

/// Simple u32 to decimal string (no_std).
fn format_u32(mut n: u32) -> String {
    if n == 0 {
        return String::from("0");
    }
    let mut digits = Vec::new();
    while n > 0 {
        digits.push(b'0' + (n % 10) as u8);
        n /= 10;
    }
    digits.reverse();
    String::from(core::str::from_utf8(&digits).unwrap_or("0"))
}

// ---------------------------------------------------------------------------
// Inner implementation
// ---------------------------------------------------------------------------

impl Inner {
    fn new() -> Self {
        Inner {
            entries: Vec::new(),
        }
    }

    fn register_system(&mut self, path: &str, gen: SystemGenerator) {
        self.entries.push(ProcEntry::System {
            path: String::from(path),
            generator: gen,
        });
    }

    fn register_per_process(&mut self, suffix: &str, gen: ProcessGenerator) {
        self.entries.push(ProcEntry::PerProcess {
            suffix: String::from(suffix),
            generator: gen,
        });
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        // Check system entries first
        for entry in self.entries.iter() {
            if let ProcEntry::System {
                path: ep,
                generator,
            } = entry
            {
                if ep == path {
                    return Some(generator());
                }
            }
        }

        // Try per-process: /proc/<pid>/<suffix>
        // Also handle /proc/self/<suffix>
        let trimmed = path.strip_prefix("/proc/")?;
        let (pid_str, suffix) = trimmed.split_once('/')?;

        let pid: u32 = if pid_str == "self" {
            1 // Current process PID placeholder
        } else {
            // Parse pid manually (no_std)
            let mut pid = 0u32;
            for b in pid_str.as_bytes() {
                if *b < b'0' || *b > b'9' {
                    return None;
                }
                pid = pid * 10 + (*b - b'0') as u32;
            }
            pid
        };

        for entry in self.entries.iter() {
            if let ProcEntry::PerProcess {
                suffix: es,
                generator,
            } = entry
            {
                if es == suffix {
                    return Some(generator(pid));
                }
            }
        }

        None
    }

    fn populate_defaults(&mut self) {
        // System-wide entries
        self.register_system("/proc/cpuinfo", gen_cpuinfo);
        self.register_system("/proc/meminfo", gen_meminfo);
        self.register_system("/proc/uptime", gen_uptime);
        self.register_system("/proc/version", gen_version);
        self.register_system("/proc/loadavg", gen_loadavg);
        self.register_system("/proc/stat", gen_stat);
        self.register_system("/proc/filesystems", gen_filesystems);
        self.register_system("/proc/mounts", gen_mounts);

        // Per-process entries
        self.register_per_process("status", gen_pid_status);
        self.register_per_process("maps", gen_pid_maps);
        self.register_per_process("cmdline", gen_pid_cmdline);
        self.register_per_process("stat", gen_pid_stat);
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static PROC_COMPAT: Mutex<Option<Inner>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Read a /proc entry by full path.
pub fn read(path: &str) -> Option<Vec<u8>> {
    let guard = PROC_COMPAT.lock();
    guard.as_ref().and_then(|inner| inner.read(path))
}

/// Register a custom system-wide /proc entry.
pub fn register_system(path: &str, gen: SystemGenerator) {
    let mut guard = PROC_COMPAT.lock();
    if let Some(inner) = guard.as_mut() {
        inner.register_system(path, gen);
    }
}

/// Register a custom per-process /proc entry.
pub fn register_per_process(suffix: &str, gen: ProcessGenerator) {
    let mut guard = PROC_COMPAT.lock();
    if let Some(inner) = guard.as_mut() {
        inner.register_per_process(suffix, gen);
    }
}

/// Initialize the /proc compatibility layer.
pub fn init() {
    let mut guard = PROC_COMPAT.lock();
    let mut inner = Inner::new();
    inner.populate_defaults();
    let count = inner.entries.len();
    *guard = Some(inner);
    serial_println!("    proc_compat: {} /proc entries registered", count);
}
