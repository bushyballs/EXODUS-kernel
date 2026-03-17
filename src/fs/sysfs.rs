/// sysfs — /sys virtual filesystem for Genesis
///
/// Exposes kernel objects (devices, drivers, buses, classes) as a hierarchy.
/// Each kobject becomes a directory, each attribute becomes a readable/writable file.
///
/// /sys/devices/     — device tree
/// /sys/bus/         — bus types (pci, usb, i2c)
/// /sys/class/       — device classes (net, block, input, tty)
/// /sys/kernel/      — kernel parameters
/// /sys/firmware/    — firmware info (ACPI, DMI)
/// /sys/power/       — power management state
/// /sys/block/       — block devices
/// /sys/module/      — kernel modules
/// /sys/fs/          — filesystem info
///
/// No-heap: all storage is fixed-size static arrays.
/// No float casts, no panics, no Vec/Box/String.
///
/// Inspired by: Linux sysfs (fs/sysfs/). All code is original.
use crate::sync::Mutex;

// ─── Byte-level helpers ───────────────────────────────────────────────────────

/// Write the decimal ASCII representation of `val` into `buf`.
/// Returns the number of bytes written (1 for zero).
/// No heap, no format!, no float casts.
fn u64_to_dec(val: u64, buf: &mut [u8; 32]) -> usize {
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

/// Append raw bytes to a 4096-byte output buffer at `*pos`.
/// Silently truncates if the buffer is full.
#[inline]
fn append_bytes(out: &mut [u8; 4096], pos: &mut usize, data: &[u8]) {
    for &b in data {
        if *pos < 4096 {
            out[*pos] = b;
            *pos = pos.saturating_add(1);
        } else {
            break;
        }
    }
}

/// Append a string literal to the output buffer.
#[inline]
fn append_str(out: &mut [u8; 4096], pos: &mut usize, s: &str) {
    append_bytes(out, pos, s.as_bytes());
}

/// Append the decimal representation of `val` to the output buffer.
#[inline]
fn append_u64(out: &mut [u8; 4096], pos: &mut usize, val: u64) {
    let mut tmp = [0u8; 32];
    let len = u64_to_dec(val, &mut tmp);
    append_bytes(out, pos, &tmp[..len]);
}

// ─── Path helpers ─────────────────────────────────────────────────────────────

#[inline]
fn path_eq(a: &[u8], b: &[u8]) -> bool {
    a == b
}

#[inline]
fn path_starts_with(path: &[u8], prefix: &[u8]) -> bool {
    path.len() >= prefix.len() && &path[..prefix.len()] == prefix
}

// ─── SysfsAttr ────────────────────────────────────────────────────────────────

/// Access mode for a sysfs attribute.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum SysfsAttrType {
    /// Read-only attribute.
    Ro,
    /// Read-write attribute.
    Rw,
    /// Write-only attribute.
    Wo,
}

/// A single sysfs attribute (virtual file).
///
/// `path` stores the full path like "/sys/class/net/eth0/mtu".
/// `value` stores the current ASCII text value (e.g., "1500\n").
#[derive(Copy, Clone)]
pub struct SysfsAttr {
    /// Full path, NUL-padded.  path_len bytes are valid.
    pub path: [u8; 128],
    /// Number of valid bytes in `path`.
    pub path_len: u8,
    /// Access mode.
    pub attr_type: SysfsAttrType,
    /// Current ASCII value, NUL-padded.  value_len bytes are valid.
    pub value: [u8; 256],
    /// Number of valid bytes in `value`.
    pub value_len: u8,
    /// Whether this slot is occupied.
    pub active: bool,
}

impl SysfsAttr {
    pub const fn empty() -> Self {
        SysfsAttr {
            path: [0u8; 128],
            path_len: 0,
            attr_type: SysfsAttrType::Ro,
            value: [0u8; 256],
            value_len: 0,
            active: false,
        }
    }
}

/// Static registry of all sysfs attributes.
/// 512 slots covers the full default hierarchy with room for dynamic entries.
static SYSFS_ATTRS: Mutex<[SysfsAttr; 512]> = Mutex::new([const { SysfsAttr::empty() }; 512]);

// ─── Core sysfs API ──────────────────────────────────────────────────────────

/// Returns true if `path` begins with "/sys/".
pub fn sysfs_is_path(path: &[u8]) -> bool {
    path_eq(path, b"/sys") || path_starts_with(path, b"/sys/")
}

/// Register a sysfs attribute.
///
/// `path` must start with "/sys/".
/// `default_value` is the initial ASCII text (may include a trailing newline).
/// Returns true on success, false if the table is full or arguments are too long.
pub fn sysfs_register(path: &[u8], attr_type: SysfsAttrType, default_value: &[u8]) -> bool {
    if path.len() > 127 || default_value.len() > 255 {
        return false;
    }
    let mut table = SYSFS_ATTRS.lock();
    // Reject duplicates — update instead.
    for slot in table.iter_mut() {
        if slot.active && &slot.path[..slot.path_len as usize] == path {
            // Update value and type in place.
            let vlen = default_value.len().min(255);
            slot.value[..vlen].copy_from_slice(&default_value[..vlen]);
            // Zero remaining bytes.
            for b in slot.value[vlen..].iter_mut() {
                *b = 0;
            }
            slot.value_len = vlen as u8;
            slot.attr_type = attr_type;
            return true;
        }
    }
    // Find a free slot.
    for slot in table.iter_mut() {
        if !slot.active {
            let plen = path.len().min(127);
            slot.path[..plen].copy_from_slice(&path[..plen]);
            for b in slot.path[plen..].iter_mut() {
                *b = 0;
            }
            slot.path_len = plen as u8;

            let vlen = default_value.len().min(255);
            slot.value[..vlen].copy_from_slice(&default_value[..vlen]);
            for b in slot.value[vlen..].iter_mut() {
                *b = 0;
            }
            slot.value_len = vlen as u8;

            slot.attr_type = attr_type;
            slot.active = true;
            return true;
        }
    }
    false // table full
}

/// Update the value of a registered sysfs attribute.
///
/// Returns true if the attribute was found and updated.
pub fn sysfs_update(path: &[u8], value: &[u8]) -> bool {
    if value.len() > 255 {
        return false;
    }
    let mut table = SYSFS_ATTRS.lock();
    for slot in table.iter_mut() {
        if slot.active && &slot.path[..slot.path_len as usize] == path {
            let vlen = value.len().min(255);
            slot.value[..vlen].copy_from_slice(&value[..vlen]);
            for b in slot.value[vlen..].iter_mut() {
                *b = 0;
            }
            slot.value_len = vlen as u8;
            return true;
        }
    }
    false
}

/// Read a sysfs attribute into `buf`.
///
/// Returns the number of bytes written (>= 0) or -2 (ENOENT).
pub fn sysfs_read(path: &[u8], buf: &mut [u8; 4096]) -> isize {
    // Check the static attribute table first.
    let table = SYSFS_ATTRS.lock();
    for slot in table.iter() {
        if !slot.active {
            continue;
        }
        let slen = slot.path_len as usize;
        if &slot.path[..slen] == path {
            // Wo attrs are not readable.
            if slot.attr_type == SysfsAttrType::Wo {
                return -1; // EPERM
            }
            let vlen = slot.value_len as usize;
            let copy = vlen.min(4095);
            buf[..copy].copy_from_slice(&slot.value[..copy]);
            // Ensure trailing newline.
            if copy > 0 && buf[copy.saturating_sub(1)] != b'\n' {
                if copy < 4095 {
                    buf[copy] = b'\n';
                    return (copy + 1) as isize;
                }
            }
            return copy as isize;
        }
    }
    drop(table);

    // -2 = ENOENT
    -2
}

/// Write to a sysfs attribute.
///
/// Returns the number of bytes consumed (>= 0) on success, or -errno:
///   -1  EROFS  — attribute is read-only
///   -2  ENOENT — attribute not found
///   -22 EINVAL — invalid value
pub fn sysfs_write(path: &[u8], data: &[u8]) -> isize {
    // Handle special writable attributes that trigger side effects.

    // /sys/power/state  — writing "mem" triggers suspend
    if path_eq(path, b"/sys/power/state") {
        // Accept "mem", "freeze", "disk".
        let trimmed = trim_newline(data);
        if trimmed == b"mem" {
            crate::power_mgmt::suspend::enter_suspend(crate::power_mgmt::suspend::SleepState::S3);
            sysfs_update(b"/sys/power/state", b"freeze mem disk\n");
            return data.len() as isize;
        }
        if trimmed == b"freeze" {
            crate::power_mgmt::suspend::enter_suspend(crate::power_mgmt::suspend::SleepState::S1);
            return data.len() as isize;
        }
        if trimmed == b"disk" {
            crate::power_mgmt::suspend::enter_suspend(crate::power_mgmt::suspend::SleepState::S4);
            return data.len() as isize;
        }
        return -22; // EINVAL
    }

    // /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor — set cpufreq governor
    if path_eq(
        path,
        b"/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor",
    ) {
        let trimmed = trim_newline(data);
        let gov = match trimmed {
            b"performance" => Some(crate::power_mgmt::cpufreq::Governor::Performance),
            b"powersave" => Some(crate::power_mgmt::cpufreq::Governor::Powersave),
            b"ondemand" => Some(crate::power_mgmt::cpufreq::Governor::Ondemand),
            b"conservative" => Some(crate::power_mgmt::cpufreq::Governor::Conservative),
            b"schedutil" => Some(crate::power_mgmt::cpufreq::Governor::Schedutil),
            _ => None,
        };
        if let Some(g) = gov {
            crate::power_mgmt::cpufreq::set_governor(g);
            sysfs_update(
                b"/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor",
                data,
            );
            return data.len() as isize;
        }
        return -22; // EINVAL
    }

    // /sys/class/net/eth0/mtu — validate and update
    if path_eq(path, b"/sys/class/net/eth0/mtu") {
        let trimmed = trim_newline(data);
        // Parse numeric value; accept 68..65535
        if let Some(mtu) = parse_u32(trimmed) {
            if mtu >= 68 && mtu <= 65535 {
                sysfs_update(b"/sys/class/net/eth0/mtu", data);
                // Notify net stack (best-effort, ignore error)
                let _ = mtu;
                crate::serial_println!("  [sysfs] eth0 MTU updated to {}", mtu);
                return data.len() as isize;
            }
        }
        return -22; // EINVAL
    }

    // Generic writable attribute — check table and update.
    {
        let table = SYSFS_ATTRS.lock();
        for slot in table.iter() {
            if !slot.active {
                continue;
            }
            if &slot.path[..slot.path_len as usize] == path {
                if slot.attr_type == SysfsAttrType::Ro {
                    return -1; // EROFS
                }
                drop(table);
                sysfs_update(path, data);
                return data.len() as isize;
            }
        }
    }

    -2 // ENOENT
}

/// List entries in a sysfs directory.
///
/// Writes null-terminated entry names into `out` (up to 64 entries of 64 bytes each).
/// Returns the count of entries filled in.
pub fn sysfs_readdir(path: &[u8], out: &mut [[u8; 64]; 64]) -> u32 {
    let mut count = 0u32;

    macro_rules! add_entry {
        ($name:expr) => {{
            if (count as usize) < 64 {
                let idx = count as usize;
                let name: &[u8] = $name;
                let len = name.len().min(63);
                out[idx][..len].copy_from_slice(&name[..len]);
                out[idx][len] = 0;
                count = count.saturating_add(1);
            }
        }};
    }

    // ── /sys root ─────────────────────────────────────────────────────────────
    if path_eq(path, b"/sys") || path_eq(path, b"/sys/") {
        add_entry!(b"bus");
        add_entry!(b"class");
        add_entry!(b"devices");
        add_entry!(b"firmware");
        add_entry!(b"fs");
        add_entry!(b"kernel");
        add_entry!(b"module");
        add_entry!(b"power");
        add_entry!(b"block");
        return count;
    }

    // ── /sys/class ────────────────────────────────────────────────────────────
    if path_eq(path, b"/sys/class") || path_eq(path, b"/sys/class/") {
        add_entry!(b"block");
        add_entry!(b"net");
        add_entry!(b"input");
        add_entry!(b"tty");
        add_entry!(b"rtc");
        add_entry!(b"leds");
        add_entry!(b"thermal");
        add_entry!(b"hwmon");
        add_entry!(b"sound");
        add_entry!(b"graphics");
        return count;
    }

    // ── /sys/class/net ────────────────────────────────────────────────────────
    if path_eq(path, b"/sys/class/net") || path_eq(path, b"/sys/class/net/") {
        add_entry!(b"eth0");
        add_entry!(b"lo");
        return count;
    }

    // ── /sys/class/net/eth0 ───────────────────────────────────────────────────
    if path_eq(path, b"/sys/class/net/eth0") || path_eq(path, b"/sys/class/net/eth0/") {
        add_entry!(b"address");
        add_entry!(b"mtu");
        add_entry!(b"operstate");
        add_entry!(b"carrier");
        add_entry!(b"speed");
        add_entry!(b"duplex");
        add_entry!(b"tx_queue_len");
        add_entry!(b"type");
        return count;
    }

    // ── /sys/class/net/lo ─────────────────────────────────────────────────────
    if path_eq(path, b"/sys/class/net/lo") || path_eq(path, b"/sys/class/net/lo/") {
        add_entry!(b"address");
        add_entry!(b"mtu");
        add_entry!(b"operstate");
        return count;
    }

    // ── /sys/class/block ──────────────────────────────────────────────────────
    if path_eq(path, b"/sys/class/block") || path_eq(path, b"/sys/class/block/") {
        add_entry!(b"sda");
        return count;
    }

    // ── /sys/bus ──────────────────────────────────────────────────────────────
    if path_eq(path, b"/sys/bus") || path_eq(path, b"/sys/bus/") {
        add_entry!(b"pci");
        add_entry!(b"usb");
        add_entry!(b"i2c");
        add_entry!(b"platform");
        return count;
    }

    // ── /sys/bus/pci ──────────────────────────────────────────────────────────
    if path_eq(path, b"/sys/bus/pci") || path_eq(path, b"/sys/bus/pci/") {
        add_entry!(b"devices");
        add_entry!(b"drivers");
        return count;
    }

    // ── /sys/devices ──────────────────────────────────────────────────────────
    if path_eq(path, b"/sys/devices") || path_eq(path, b"/sys/devices/") {
        add_entry!(b"system");
        add_entry!(b"pci0000:00");
        return count;
    }

    // ── /sys/devices/system ───────────────────────────────────────────────────
    if path_eq(path, b"/sys/devices/system") || path_eq(path, b"/sys/devices/system/") {
        add_entry!(b"cpu");
        add_entry!(b"memory");
        return count;
    }

    // ── /sys/devices/system/cpu ───────────────────────────────────────────────
    if path_eq(path, b"/sys/devices/system/cpu") || path_eq(path, b"/sys/devices/system/cpu/") {
        add_entry!(b"cpu0");
        add_entry!(b"possible");
        add_entry!(b"present");
        add_entry!(b"online");
        return count;
    }

    // ── /sys/devices/system/cpu/cpu0 ─────────────────────────────────────────
    if path_eq(path, b"/sys/devices/system/cpu/cpu0")
        || path_eq(path, b"/sys/devices/system/cpu/cpu0/")
    {
        add_entry!(b"cpufreq");
        add_entry!(b"topology");
        add_entry!(b"online");
        return count;
    }

    // ── /sys/devices/system/cpu/cpu0/cpufreq ─────────────────────────────────
    if path_eq(path, b"/sys/devices/system/cpu/cpu0/cpufreq")
        || path_eq(path, b"/sys/devices/system/cpu/cpu0/cpufreq/")
    {
        add_entry!(b"scaling_governor");
        add_entry!(b"scaling_cur_freq");
        add_entry!(b"scaling_min_freq");
        add_entry!(b"scaling_max_freq");
        return count;
    }

    // ── /sys/devices/system/cpu/cpu0/topology ────────────────────────────────
    if path_eq(path, b"/sys/devices/system/cpu/cpu0/topology")
        || path_eq(path, b"/sys/devices/system/cpu/cpu0/topology/")
    {
        add_entry!(b"core_id");
        add_entry!(b"physical_package_id");
        return count;
    }

    // ── /sys/devices/system/memory ───────────────────────────────────────────
    if path_eq(path, b"/sys/devices/system/memory") || path_eq(path, b"/sys/devices/system/memory/")
    {
        add_entry!(b"block_size_bytes");
        return count;
    }

    // ── /sys/kernel ───────────────────────────────────────────────────────────
    if path_eq(path, b"/sys/kernel") || path_eq(path, b"/sys/kernel/") {
        add_entry!(b"hostname");
        add_entry!(b"ostype");
        add_entry!(b"osrelease");
        add_entry!(b"version");
        add_entry!(b"tainted");
        add_entry!(b"printk");
        add_entry!(b"panic");
        add_entry!(b"ngroups_max");
        add_entry!(b"pid_max");
        add_entry!(b"threads-max");
        return count;
    }

    // ── /sys/power ────────────────────────────────────────────────────────────
    if path_eq(path, b"/sys/power") || path_eq(path, b"/sys/power/") {
        add_entry!(b"state");
        add_entry!(b"pm_async");
        add_entry!(b"pm_wakeup_irq");
        add_entry!(b"wakeup_count");
        return count;
    }

    // ── /sys/block ────────────────────────────────────────────────────────────
    if path_eq(path, b"/sys/block") || path_eq(path, b"/sys/block/") {
        add_entry!(b"sda");
        return count;
    }

    // ── /sys/block/sda ────────────────────────────────────────────────────────
    if path_eq(path, b"/sys/block/sda") || path_eq(path, b"/sys/block/sda/") {
        add_entry!(b"size");
        add_entry!(b"removable");
        add_entry!(b"ro");
        add_entry!(b"queue");
        return count;
    }

    // ── /sys/block/sda/queue ─────────────────────────────────────────────────
    if path_eq(path, b"/sys/block/sda/queue") || path_eq(path, b"/sys/block/sda/queue/") {
        add_entry!(b"scheduler");
        add_entry!(b"hw_sector_size");
        add_entry!(b"logical_block_size");
        return count;
    }

    // ── /sys/firmware ─────────────────────────────────────────────────────────
    if path_eq(path, b"/sys/firmware") || path_eq(path, b"/sys/firmware/") {
        add_entry!(b"acpi");
        return count;
    }

    // ── /sys/module ───────────────────────────────────────────────────────────
    if path_eq(path, b"/sys/module") || path_eq(path, b"/sys/module/") {
        // No modules registered by default.
        return 0;
    }

    // ── /sys/fs ───────────────────────────────────────────────────────────────
    if path_eq(path, b"/sys/fs") || path_eq(path, b"/sys/fs/") {
        add_entry!(b"ext2");
        add_entry!(b"fat");
        return count;
    }

    // Fall through: enumerate registered attributes whose paths start with
    // `path + "/"` and extract the immediate next component.
    // This handles any dynamically registered paths.
    count = sysfs_readdir_dynamic(path, out, count);
    count
}

/// Enumerate registered attributes under `dir_path` and add immediate children
/// to `out` (without duplicating entries already present).
fn sysfs_readdir_dynamic(dir_path: &[u8], out: &mut [[u8; 64]; 64], mut count: u32) -> u32 {
    // Build the prefix to match: dir_path + '/'
    // We need a small scratch buffer since we cannot allocate.
    let mut prefix = [0u8; 129];
    let plen = dir_path.len().min(127);
    prefix[..plen].copy_from_slice(&dir_path[..plen]);
    prefix[plen] = b'/';
    let prefix_len = plen + 1;

    let table = SYSFS_ATTRS.lock();
    // We'll track names already added to avoid duplicates using a small stack array.
    let mut seen = [[0u8; 64]; 64];
    let mut seen_count = 0usize;

    // Copy already-filled entries into seen.
    for i in 0..(count as usize).min(64) {
        seen[i] = out[i];
        seen_count = seen_count.saturating_add(1);
    }

    for slot in table.iter() {
        if !slot.active {
            continue;
        }
        let slen = slot.path_len as usize;
        if slen <= prefix_len {
            continue;
        }
        if &slot.path[..prefix_len] != &prefix[..prefix_len] {
            continue;
        }
        // Extract the immediate next path component after the prefix.
        let rest = &slot.path[prefix_len..slen];
        let slash_pos = rest.iter().position(|&b| b == b'/').unwrap_or(rest.len());
        let component = &rest[..slash_pos];
        if component.is_empty() || component.len() > 63 {
            continue;
        }
        // Check for duplicates.
        let mut already = false;
        for s in seen[..seen_count].iter() {
            let sname_len = s.iter().position(|&b| b == 0).unwrap_or(64);
            if &s[..sname_len] == component {
                already = true;
                break;
            }
        }
        if already {
            continue;
        }
        // Add to output.
        if (count as usize) < 64 {
            let idx = count as usize;
            let clen = component.len().min(63);
            out[idx][..clen].copy_from_slice(&component[..clen]);
            out[idx][clen] = 0;
            count = count.saturating_add(1);
            // Also add to seen to prevent further duplicates.
            if seen_count < 64 {
                seen[seen_count][..clen].copy_from_slice(&component[..clen]);
                seen[seen_count][clen] = 0;
                seen_count = seen_count.saturating_add(1);
            }
        }
    }
    count
}

// ─── Utility: byte-slice helpers ─────────────────────────────────────────────

/// Trim a single trailing newline from a byte slice.
fn trim_newline(data: &[u8]) -> &[u8] {
    if data.last() == Some(&b'\n') {
        &data[..data.len().saturating_sub(1)]
    } else {
        data
    }
}

/// Parse an ASCII decimal byte string to u32.  Returns None on non-digit or overflow.
fn parse_u32(data: &[u8]) -> Option<u32> {
    if data.is_empty() || data.len() > 10 {
        return None;
    }
    let mut val: u32 = 0;
    for &b in data {
        if b < b'0' || b > b'9' {
            return None;
        }
        let digit = (b - b'0') as u32;
        val = match val.checked_mul(10).and_then(|v| v.checked_add(digit)) {
            Some(v) => v,
            None => return None,
        };
    }
    Some(val)
}

// ─── Heap-compatible shims (for vfs.rs which still uses alloc) ───────────────
//
// vfs.rs calls `super::sysfs::read(path)` expecting `Option<String>` and
// `super::sysfs::list_dir(path)` expecting `Vec<String>`.  We provide thin
// wrappers that call the no-heap core and convert.

use alloc::string::String;
use alloc::vec::Vec;

/// Read a sysfs attribute, returning `Option<String>` for vfs.rs compatibility.
pub fn read(path: &str) -> Option<String> {
    let mut buf = [0u8; 4096];
    let n = sysfs_read(path.as_bytes(), &mut buf);
    if n < 0 {
        return None;
    }
    let slice = &buf[..n as usize];
    core::str::from_utf8(slice).ok().map(String::from)
}

/// List a sysfs directory, returning `Vec<String>` for vfs.rs compatibility.
pub fn list_dir(path: &str) -> Vec<String> {
    let mut out = [[0u8; 64]; 64];
    let count = sysfs_readdir(path.as_bytes(), &mut out);
    let mut entries = Vec::new();
    for i in 0..(count as usize) {
        let name_len = out[i].iter().position(|&b| b == 0).unwrap_or(64);
        if let Ok(s) = core::str::from_utf8(&out[i][..name_len]) {
            entries.push(String::from(s));
        }
    }
    entries
}

// ─── PCI device sysfs integration (retained from original) ───────────────────

/// Register a PCI device directory under `/sys/bus/pci/devices/{bdf}/`.
///
/// Creates entries for standard PCI pseudo-files so that `sysfs_readdir()`
/// exposes them.  Actual reads are served by the PCI driver's
/// `pci_sysfs_read()` (see `drivers/pci.rs`).
pub fn add_pci_device_dir(bdf_path: &str) {
    // Register the directory itself as a Ro attr with empty value so that
    // path-prefix enumeration can discover it during readdir.
    sysfs_register(bdf_path.as_bytes(), SysfsAttrType::Ro, b"");

    // Register each standard pseudo-file attribute.
    let pseudo_files: &[&[u8]] = &[
        b"vendor",
        b"device",
        b"class",
        b"irq",
        b"subsystem_vendor",
        b"subsystem_device",
        b"revision",
    ];
    let mut path_buf = [0u8; 128];
    let base_len = bdf_path.len().min(120);
    path_buf[..base_len].copy_from_slice(&bdf_path.as_bytes()[..base_len]);
    path_buf[base_len] = b'/';

    for f in pseudo_files {
        let flen = f.len().min(127 - base_len - 1);
        path_buf[(base_len + 1)..(base_len + 1 + flen)].copy_from_slice(&f[..flen]);
        let total = base_len + 1 + flen;
        sysfs_register(&path_buf[..total], SysfsAttrType::Ro, b"");
    }
}

/// Read a sysfs path, delegating PCI device attribute reads to the PCI driver.
pub fn read_pci_aware(path: &str) -> Option<String> {
    const PCI_DEV_PREFIX: &str = "/sys/bus/pci/devices/";

    if path.starts_with(PCI_DEV_PREFIX) {
        let rest = &path[PCI_DEV_PREFIX.len()..];
        if rest.contains('/') {
            let mut buf = [0u8; 64];
            let n = crate::drivers::pci::pci_sysfs_read(path, &mut buf);
            if n > 0 {
                let s = core::str::from_utf8(&buf[..n]).ok()?;
                return Some(String::from(s));
            }
            return None;
        }
    }

    read(path)
}

/// List directory, including dynamically discovered PCI devices.
pub fn list_dir_pci_aware(path: &str) -> Vec<String> {
    const PCI_DEVS: &str = "/sys/bus/pci/devices";
    if path == PCI_DEVS {
        let mut entries = list_dir(path);
        for bdf in crate::drivers::pci::pci_sysfs_list_devices() {
            if !entries.contains(&bdf) {
                entries.push(bdf);
            }
        }
        return entries;
    }
    list_dir(path)
}

// ─── Register a device / class device (used by driver subsystems) ─────────────

/// Register a device in sysfs under `/sys/bus/{bus}/{name}`.
pub fn register_device(bus: &str, name: &str) {
    // Build path in a stack buffer: "/sys/bus/<bus>/<name>"
    let mut path = [0u8; 128];
    let mut pos = 0usize;
    let prefix = b"/sys/bus/";
    let plen = prefix.len().min(128);
    path[..plen].copy_from_slice(&prefix[..plen]);
    pos = plen;
    let blen = bus.len().min(128 - pos - 1);
    path[pos..pos + blen].copy_from_slice(&bus.as_bytes()[..blen]);
    pos = pos.saturating_add(blen);
    if pos < 127 {
        path[pos] = b'/';
        pos = pos.saturating_add(1);
    }
    let nlen = name.len().min(128 - pos);
    path[pos..pos + nlen].copy_from_slice(&name.as_bytes()[..nlen]);
    pos = pos.saturating_add(nlen);
    sysfs_register(&path[..pos], SysfsAttrType::Ro, b"");
}

/// Register a class device in sysfs under `/sys/class/{class}/{name}`.
pub fn register_class_device(class: &str, name: &str) {
    let mut path = [0u8; 128];
    let mut pos = 0usize;
    let prefix = b"/sys/class/";
    let plen = prefix.len().min(128);
    path[..plen].copy_from_slice(&prefix[..plen]);
    pos = plen;
    let clen = class.len().min(128 - pos - 1);
    path[pos..pos + clen].copy_from_slice(&class.as_bytes()[..clen]);
    pos = pos.saturating_add(clen);
    if pos < 127 {
        path[pos] = b'/';
        pos = pos.saturating_add(1);
    }
    let nlen = name.len().min(128 - pos);
    path[pos..pos + nlen].copy_from_slice(&name.as_bytes()[..nlen]);
    pos = pos.saturating_add(nlen);
    sysfs_register(&path[..pos], SysfsAttrType::Ro, b"");
}

// ─── init ─────────────────────────────────────────────────────────────────────

/// Initialize sysfs with the standard Linux-compatible attribute hierarchy.
pub fn init() {
    // ── /sys/kernel ───────────────────────────────────────────────────────────
    sysfs_register(b"/sys/kernel/hostname", SysfsAttrType::Rw, b"genesis\n");
    sysfs_register(b"/sys/kernel/ostype", SysfsAttrType::Ro, b"Linux\n");
    sysfs_register(
        b"/sys/kernel/osrelease",
        SysfsAttrType::Ro,
        b"6.1.0-genesis\n",
    );
    sysfs_register(
        b"/sys/kernel/version",
        SysfsAttrType::Ro,
        b"#1 SMP Genesis AI OS\n",
    );
    sysfs_register(b"/sys/kernel/tainted", SysfsAttrType::Ro, b"0\n");
    sysfs_register(b"/sys/kernel/printk", SysfsAttrType::Rw, b"7 4 1 7\n");
    sysfs_register(b"/sys/kernel/panic", SysfsAttrType::Rw, b"0\n");
    sysfs_register(b"/sys/kernel/ngroups_max", SysfsAttrType::Ro, b"65536\n");
    sysfs_register(b"/sys/kernel/pid_max", SysfsAttrType::Rw, b"4194304\n");
    sysfs_register(b"/sys/kernel/threads-max", SysfsAttrType::Rw, b"1024\n");

    // ── /sys/devices/system/cpu/cpu0 ─────────────────────────────────────────
    sysfs_register(
        b"/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor",
        SysfsAttrType::Rw,
        b"ondemand\n",
    );
    sysfs_register(
        b"/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq",
        SysfsAttrType::Ro,
        b"3000000\n",
    );
    sysfs_register(
        b"/sys/devices/system/cpu/cpu0/cpufreq/scaling_min_freq",
        SysfsAttrType::Ro,
        b"800000\n",
    );
    sysfs_register(
        b"/sys/devices/system/cpu/cpu0/cpufreq/scaling_max_freq",
        SysfsAttrType::Ro,
        b"3000000\n",
    );
    sysfs_register(
        b"/sys/devices/system/cpu/cpu0/topology/core_id",
        SysfsAttrType::Ro,
        b"0\n",
    );
    sysfs_register(
        b"/sys/devices/system/cpu/cpu0/topology/physical_package_id",
        SysfsAttrType::Ro,
        b"0\n",
    );
    sysfs_register(
        b"/sys/devices/system/cpu/cpu0/online",
        SysfsAttrType::Rw,
        b"1\n",
    );
    sysfs_register(
        b"/sys/devices/system/cpu/possible",
        SysfsAttrType::Ro,
        b"0\n",
    );
    sysfs_register(
        b"/sys/devices/system/cpu/present",
        SysfsAttrType::Ro,
        b"0\n",
    );
    sysfs_register(b"/sys/devices/system/cpu/online", SysfsAttrType::Ro, b"0\n");

    // ── /sys/devices/system/memory ────────────────────────────────────────────
    sysfs_register(
        b"/sys/devices/system/memory/block_size_bytes",
        SysfsAttrType::Ro,
        b"20000000\n",
    ); // 512 MB in hex

    // ── /sys/class/net/eth0 ───────────────────────────────────────────────────
    sysfs_register(
        b"/sys/class/net/eth0/address",
        SysfsAttrType::Rw,
        b"52:54:00:12:34:56\n",
    );
    sysfs_register(b"/sys/class/net/eth0/mtu", SysfsAttrType::Rw, b"1500\n");
    sysfs_register(b"/sys/class/net/eth0/operstate", SysfsAttrType::Ro, b"up\n");
    sysfs_register(b"/sys/class/net/eth0/carrier", SysfsAttrType::Ro, b"1\n");
    sysfs_register(b"/sys/class/net/eth0/speed", SysfsAttrType::Ro, b"1000\n");
    sysfs_register(b"/sys/class/net/eth0/duplex", SysfsAttrType::Ro, b"full\n");
    sysfs_register(
        b"/sys/class/net/eth0/tx_queue_len",
        SysfsAttrType::Rw,
        b"1000\n",
    );
    sysfs_register(b"/sys/class/net/eth0/type", SysfsAttrType::Ro, b"1\n");

    // ── /sys/class/net/lo ─────────────────────────────────────────────────────
    sysfs_register(
        b"/sys/class/net/lo/address",
        SysfsAttrType::Ro,
        b"00:00:00:00:00:00\n",
    );
    sysfs_register(b"/sys/class/net/lo/mtu", SysfsAttrType::Ro, b"65536\n");
    sysfs_register(
        b"/sys/class/net/lo/operstate",
        SysfsAttrType::Ro,
        b"unknown\n",
    );

    // ── /sys/power ────────────────────────────────────────────────────────────
    sysfs_register(b"/sys/power/state", SysfsAttrType::Rw, b"freeze mem disk\n");
    sysfs_register(b"/sys/power/pm_async", SysfsAttrType::Rw, b"1\n");
    sysfs_register(b"/sys/power/pm_wakeup_irq", SysfsAttrType::Ro, b"0\n");
    sysfs_register(b"/sys/power/wakeup_count", SysfsAttrType::Rw, b"0\n");

    // ── /sys/block/sda ────────────────────────────────────────────────────────
    sysfs_register(b"/sys/block/sda/size", SysfsAttrType::Ro, b"20971520\n"); // 10 GB in 512-byte sectors
    sysfs_register(b"/sys/block/sda/removable", SysfsAttrType::Ro, b"0\n");
    sysfs_register(b"/sys/block/sda/ro", SysfsAttrType::Ro, b"0\n");
    sysfs_register(
        b"/sys/block/sda/queue/scheduler",
        SysfsAttrType::Rw,
        b"noop mq-deadline [kyber]\n",
    );
    sysfs_register(
        b"/sys/block/sda/queue/hw_sector_size",
        SysfsAttrType::Ro,
        b"512\n",
    );
    sysfs_register(
        b"/sys/block/sda/queue/logical_block_size",
        SysfsAttrType::Ro,
        b"512\n",
    );

    crate::serial_println!("  [sysfs] /sys filesystem initialized");
}
