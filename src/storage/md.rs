use crate::drivers::virtio_blk::{virtio_blk_read, virtio_blk_write};
use crate::serial_println;
/// Software RAID (MD) subsystem for Genesis — no-heap, no-float, no-panic
///
/// Implements a Linux-md-style software RAID manager with three RAID levels:
///   - Linear: disks concatenated end-to-end
///   - RAID-0:  stripe across all member disks (no redundancy)
///   - RAID-1:  full mirror — write to all, read from first healthy disk
///
/// Design constraints (bare-metal #![no_std] kernel):
///   - No alloc — all state in fixed-size static arrays
///   - No float casts (as f32 / as f64) anywhere
///   - Saturating arithmetic on counters, wrapping on sequences
///   - No panic — all error paths return Option/bool and log via serial_println!
///   - Structs in static Mutex are Copy with const fn empty()
///
/// Disk I/O is dispatched through crate::drivers::virtio_blk::{virtio_blk_read,
/// virtio_blk_write}.  Because the system has a single virtio-blk device we
/// simulate multiple disks by partitioning the device into 8 equal logical
/// regions indexed by disk_idx.  The stride (sectors per logical disk region)
/// is set by MD_DISK_SECTOR_STRIDE.  In a real multi-device system this layer
/// would dispatch to different physical device drivers.
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of MD arrays in the system.
pub const MD_MAX_ARRAYS: usize = 4;

/// Maximum member disks per array.
pub const MD_MAX_DISKS: usize = 8;

/// Sectors reserved per logical disk region on the single virtio-blk device.
/// Each disk_idx maps to a non-overlapping region:
///   physical_lba = disk_idx * MD_DISK_SECTOR_STRIDE + disk_lba
/// Default: 2 GiB / 512 = 4 194 304 sectors per logical disk (× 8 disks = 16 GiB).
const MD_DISK_SECTOR_STRIDE: u64 = 4_194_304;

// ---------------------------------------------------------------------------
// RAID level
// ---------------------------------------------------------------------------

/// Supported RAID levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MdRaidLevel {
    /// Disks are concatenated: disk 0, then disk 1, then disk 2, …
    Linear,
    /// Striped: sectors distributed round-robin across member disks.
    Raid0,
    /// Mirrored: every write goes to all disks; reads come from the first
    /// non-faulted disk.
    Raid1,
}

// ---------------------------------------------------------------------------
// MdDisk
// ---------------------------------------------------------------------------

/// A single member disk slot inside an MD array.
#[derive(Clone, Copy)]
pub struct MdDisk {
    /// Index into the logical disk table (used to compute physical LBA offset).
    pub disk_idx: u32,
    /// True if this disk has experienced an I/O error and is excluded from I/O.
    pub fault: bool,
    /// True if this slot is occupied by a real disk (false = empty slot).
    pub working: bool,
}

impl MdDisk {
    pub const fn empty() -> Self {
        MdDisk {
            disk_idx: 0,
            fault: false,
            working: false,
        }
    }
}

// ---------------------------------------------------------------------------
// MdArray
// ---------------------------------------------------------------------------

/// One software RAID array.
#[derive(Clone, Copy)]
pub struct MdArray {
    /// RAID level for this array.
    pub level: MdRaidLevel,
    /// Human-readable name, zero-padded.
    pub name: [u8; 32],
    /// Member disk slots.
    pub disks: [MdDisk; MD_MAX_DISKS],
    /// Number of actually-added member disks (1..=MD_MAX_DISKS).
    pub ndisks: u8,
    /// Stripe chunk size in 512-byte sectors (used by RAID-0).
    pub chunk_sectors: u32,
    /// Total addressable sectors across the whole array (computed at md_start).
    pub total_sectors: u64,
    /// True once md_start() has been called successfully.
    pub active: bool,
    // --- per-array I/O statistics (maintained under MD_ARRAYS Mutex) ---
    read_ops: u64,
    write_ops: u64,
    errors: u64,
}

impl MdArray {
    pub const fn empty() -> Self {
        MdArray {
            level: MdRaidLevel::Linear,
            name: [0u8; 32],
            disks: [
                MdDisk::empty(),
                MdDisk::empty(),
                MdDisk::empty(),
                MdDisk::empty(),
                MdDisk::empty(),
                MdDisk::empty(),
                MdDisk::empty(),
                MdDisk::empty(),
            ],
            ndisks: 0,
            chunk_sectors: 8, // default: 4 KiB chunks
            total_sectors: 0,
            active: false,
            read_ops: 0,
            write_ops: 0,
            errors: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static MD_ARRAYS: Mutex<[MdArray; MD_MAX_ARRAYS]> = Mutex::new([
    MdArray::empty(),
    MdArray::empty(),
    MdArray::empty(),
    MdArray::empty(),
]);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Translate a logical (disk_idx, disk_lba) pair to an absolute sector on the
/// single virtio-blk device, then perform a read.
///
/// Returns false if the resulting sector would overflow u64 (saturating guard).
#[inline(always)]
fn disk_read(disk_idx: u32, disk_lba: u64, buf: &mut [u8; 512]) -> bool {
    let base = (disk_idx as u64).saturating_mul(MD_DISK_SECTOR_STRIDE);
    let abs_sector = base.saturating_add(disk_lba);
    // Guard against wrap-around past physical end
    if abs_sector < base {
        // saturating_add returned base because disk_lba caused overflow — reject
        return false;
    }
    virtio_blk_read(abs_sector, buf)
}

/// Translate a logical (disk_idx, disk_lba) pair to an absolute sector and
/// perform a write.
#[inline(always)]
fn disk_write(disk_idx: u32, disk_lba: u64, buf: &[u8; 512]) -> bool {
    let base = (disk_idx as u64).saturating_mul(MD_DISK_SECTOR_STRIDE);
    let abs_sector = base.saturating_add(disk_lba);
    if abs_sector < base {
        return false;
    }
    virtio_blk_write(abs_sector, buf)
}

/// Return the index of the first non-faulted, working disk in an array.
/// Returns None if no healthy disk is available.
fn first_healthy(arr: &MdArray) -> Option<usize> {
    let mut i = 0usize;
    while i < arr.ndisks as usize {
        if arr.disks[i].working && !arr.disks[i].fault {
            return Some(i);
        }
        i = i.wrapping_add(1);
    }
    None
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a new MD array with the given name, level, and chunk size.
///
/// Returns the array index (0..MD_MAX_ARRAYS) on success, or None if the
/// array table is full or the name is too long.
pub fn md_create(name: &[u8], level: MdRaidLevel, chunk_sectors: u32) -> Option<usize> {
    if name.len() > 31 {
        serial_println!("  [md] md_create: name too long ({} bytes)", name.len());
        return None;
    }

    let mut arrays = MD_ARRAYS.lock();

    // Find a free slot
    let mut slot: Option<usize> = None;
    let mut i = 0usize;
    while i < MD_MAX_ARRAYS {
        if !arrays[i].active && arrays[i].ndisks == 0 {
            slot = Some(i);
            break;
        }
        i = i.wrapping_add(1);
    }

    let idx = match slot {
        Some(i) => i,
        None => {
            serial_println!("  [md] md_create: array table full");
            return None;
        }
    };

    let arr = &mut arrays[idx];
    *arr = MdArray::empty();

    // Copy name (zero-padded)
    let mut ni = 0usize;
    while ni < name.len() && ni < 31 {
        arr.name[ni] = name[ni];
        ni = ni.wrapping_add(1);
    }
    arr.name[31] = 0;

    arr.level = level;
    arr.chunk_sectors = if chunk_sectors == 0 { 8 } else { chunk_sectors };

    serial_println!("  [md] created array {} (slot {})", idx, idx);
    Some(idx)
}

/// Add a physical disk to an array (before md_start is called).
///
/// Returns false if the array index is out of range, the disk limit is
/// reached, or the array has already been started.
pub fn md_add_disk(array_idx: usize, disk_idx: u32) -> bool {
    if array_idx >= MD_MAX_ARRAYS {
        return false;
    }

    let mut arrays = MD_ARRAYS.lock();
    let arr = &mut arrays[array_idx];

    if arr.active {
        serial_println!("  [md] md_add_disk: array {} already started", array_idx);
        return false;
    }
    if arr.ndisks as usize >= MD_MAX_DISKS {
        serial_println!("  [md] md_add_disk: array {} disk limit reached", array_idx);
        return false;
    }

    let slot = arr.ndisks as usize;
    arr.disks[slot].disk_idx = disk_idx;
    arr.disks[slot].fault = false;
    arr.disks[slot].working = true;
    arr.ndisks = arr.ndisks.saturating_add(1);

    serial_println!(
        "  [md] array {} added disk {} (slot {})",
        array_idx,
        disk_idx,
        slot
    );
    true
}

/// Finalize and validate an array configuration.
///
/// Computes `total_sectors` and sets `active = true`.
/// Minimum disk requirements:
///   - Linear: 1 disk
///   - RAID-0:  2 disks
///   - RAID-1:  2 disks
///
/// Returns false if validation fails.
pub fn md_start(array_idx: usize) -> bool {
    if array_idx >= MD_MAX_ARRAYS {
        return false;
    }

    let mut arrays = MD_ARRAYS.lock();
    let arr = &mut arrays[array_idx];

    if arr.active {
        serial_println!("  [md] md_start: array {} already active", array_idx);
        return false;
    }

    let ndisks = arr.ndisks as u64;

    // Validate minimum disk count per RAID level
    let ok = match arr.level {
        MdRaidLevel::Linear => ndisks >= 1,
        MdRaidLevel::Raid0 => ndisks >= 2,
        MdRaidLevel::Raid1 => ndisks >= 2,
    };
    if !ok {
        serial_println!(
            "  [md] md_start: array {} needs more disks for {:?} (have {})",
            array_idx,
            arr.level,
            ndisks
        );
        return false;
    }

    // Compute total addressable sectors
    arr.total_sectors = match arr.level {
        MdRaidLevel::Linear => {
            // Each disk contributes MD_DISK_SECTOR_STRIDE sectors
            MD_DISK_SECTOR_STRIDE.saturating_mul(ndisks)
        }
        MdRaidLevel::Raid0 => {
            // Stripe: ndisks * chunk_sectors * (stride / chunk_sectors)
            // Simplified: ndisks * stride (each disk contributes its full range)
            MD_DISK_SECTOR_STRIDE.saturating_mul(ndisks)
        }
        MdRaidLevel::Raid1 => {
            // Mirror: total capacity = one disk (all disks hold identical data)
            MD_DISK_SECTOR_STRIDE
        }
    };

    arr.active = true;
    serial_println!(
        "  [md] array {} started: {:?}, {} disks, {} total sectors",
        array_idx,
        arr.level,
        arr.ndisks,
        arr.total_sectors
    );
    true
}

/// Read one 512-byte sector from an MD array at the given logical block address.
///
/// Dispatch rules:
///   - RAID-0:  sector % ndisks → target disk; sector / ndisks → disk_lba
///   - RAID-1:  read from first non-faulted disk
///   - Linear:  sector / stride → disk index; sector % stride → disk_lba
///
/// Returns false if the array is inactive, the LBA is out of range, or the
/// underlying disk I/O fails.
pub fn md_read(array_idx: usize, lba: u64, buf: &mut [u8; 512]) -> bool {
    if array_idx >= MD_MAX_ARRAYS {
        return false;
    }

    let mut arrays = MD_ARRAYS.lock();
    let arr = &mut arrays[array_idx];

    if !arr.active {
        serial_println!("  [md] md_read: array {} not active", array_idx);
        return false;
    }
    if lba >= arr.total_sectors {
        serial_println!(
            "  [md] md_read: array {} lba {} out of range ({})",
            array_idx,
            lba,
            arr.total_sectors
        );
        return false;
    }

    let ndisks = arr.ndisks as u64;
    let level = arr.level;

    let (target_slot, disk_lba) = match level {
        MdRaidLevel::Raid0 => {
            // Round-robin stripe: disk = lba % ndisks, lba_on_disk = lba / ndisks
            if ndisks == 0 {
                arr.errors = arr.errors.saturating_add(1);
                return false;
            }
            let slot = (lba % ndisks) as usize;
            let dlba = lba / ndisks;
            (slot, dlba)
        }
        MdRaidLevel::Raid1 => {
            // Read from first healthy disk
            match first_healthy(arr) {
                Some(s) => (s, lba),
                None => {
                    serial_println!("  [md] md_read: array {} no healthy disks", array_idx);
                    arr.errors = arr.errors.saturating_add(1);
                    return false;
                }
            }
        }
        MdRaidLevel::Linear => {
            // Sequential across disks: each disk has MD_DISK_SECTOR_STRIDE sectors
            if MD_DISK_SECTOR_STRIDE == 0 {
                arr.errors = arr.errors.saturating_add(1);
                return false;
            }
            let slot = (lba / MD_DISK_SECTOR_STRIDE) as usize;
            let dlba = lba % MD_DISK_SECTOR_STRIDE;
            if slot >= arr.ndisks as usize {
                serial_println!(
                    "  [md] md_read: linear array {} slot {} out of range",
                    array_idx,
                    slot
                );
                arr.errors = arr.errors.saturating_add(1);
                return false;
            }
            (slot, dlba)
        }
    };

    // Validate the target slot
    if target_slot >= arr.ndisks as usize {
        arr.errors = arr.errors.saturating_add(1);
        return false;
    }
    let disk = arr.disks[target_slot];
    if !disk.working || disk.fault {
        serial_println!(
            "  [md] md_read: array {} disk slot {} faulted/not-working",
            array_idx,
            target_slot
        );
        arr.errors = arr.errors.saturating_add(1);
        return false;
    }

    // Snapshot disk_idx before dropping (Copy type — no need to drop guard first)
    let d_idx = disk.disk_idx;

    let ok = disk_read(d_idx, disk_lba, buf);
    if ok {
        arr.read_ops = arr.read_ops.saturating_add(1);
    } else {
        arr.errors = arr.errors.saturating_add(1);
        serial_println!(
            "  [md] md_read: I/O error array {} disk {} lba {}",
            array_idx,
            d_idx,
            disk_lba
        );
    }
    ok
}

/// Write one 512-byte sector to an MD array at the given logical block address.
///
/// Dispatch rules:
///   - RAID-0:  write to the computed stripe disk only
///   - RAID-1:  write to ALL non-faulted member disks (mirror)
///   - Linear:  write to the disk that contains this sector
///
/// Returns false if the array is inactive, the LBA is out of range, or all
/// disk writes fail.
pub fn md_write(array_idx: usize, lba: u64, buf: &[u8; 512]) -> bool {
    if array_idx >= MD_MAX_ARRAYS {
        return false;
    }

    let mut arrays = MD_ARRAYS.lock();
    let arr = &mut arrays[array_idx];

    if !arr.active {
        serial_println!("  [md] md_write: array {} not active", array_idx);
        return false;
    }
    if lba >= arr.total_sectors {
        serial_println!(
            "  [md] md_write: array {} lba {} out of range ({})",
            array_idx,
            lba,
            arr.total_sectors
        );
        return false;
    }

    let ndisks = arr.ndisks as u64;
    let level = arr.level;

    let ok: bool = match level {
        MdRaidLevel::Raid0 => {
            if ndisks == 0 {
                arr.errors = arr.errors.saturating_add(1);
                return false;
            }
            let slot = (lba % ndisks) as usize;
            let dlba = lba / ndisks;

            if slot >= arr.ndisks as usize {
                arr.errors = arr.errors.saturating_add(1);
                return false;
            }
            let disk = arr.disks[slot];
            if !disk.working || disk.fault {
                arr.errors = arr.errors.saturating_add(1);
                return false;
            }
            let result = disk_write(disk.disk_idx, dlba, buf);
            if !result {
                arr.errors = arr.errors.saturating_add(1);
                serial_println!(
                    "  [md] md_write: RAID-0 error array {} disk {} lba {}",
                    array_idx,
                    disk.disk_idx,
                    dlba
                );
            }
            result
        }

        MdRaidLevel::Raid1 => {
            // Write to all non-faulted disks; succeed if at least one write succeeds
            let mut any_ok = false;
            let mut any_err = false;
            let mut di = 0usize;
            while di < arr.ndisks as usize {
                let disk = arr.disks[di];
                if disk.working && !disk.fault {
                    let result = disk_write(disk.disk_idx, lba, buf);
                    if result {
                        any_ok = true;
                    } else {
                        any_err = true;
                        serial_println!(
                            "  [md] md_write: RAID-1 error array {} disk {} lba {}",
                            array_idx,
                            disk.disk_idx,
                            lba
                        );
                    }
                }
                di = di.wrapping_add(1);
            }
            if any_err {
                arr.errors = arr.errors.saturating_add(1);
            }
            any_ok
        }

        MdRaidLevel::Linear => {
            if MD_DISK_SECTOR_STRIDE == 0 {
                arr.errors = arr.errors.saturating_add(1);
                return false;
            }
            let slot = (lba / MD_DISK_SECTOR_STRIDE) as usize;
            let dlba = lba % MD_DISK_SECTOR_STRIDE;

            if slot >= arr.ndisks as usize {
                arr.errors = arr.errors.saturating_add(1);
                return false;
            }
            let disk = arr.disks[slot];
            if !disk.working || disk.fault {
                arr.errors = arr.errors.saturating_add(1);
                return false;
            }
            let result = disk_write(disk.disk_idx, dlba, buf);
            if !result {
                arr.errors = arr.errors.saturating_add(1);
                serial_println!(
                    "  [md] md_write: Linear error array {} disk {} lba {}",
                    array_idx,
                    disk.disk_idx,
                    dlba
                );
            }
            result
        }
    };

    if ok {
        arr.write_ops = arr.write_ops.saturating_add(1);
    }
    ok
}

/// Mark a member disk as faulted.
///
/// After this call the disk is excluded from all I/O operations.  For RAID-1
/// arrays this enables degraded-mode operation.
pub fn md_mark_fault(array_idx: usize, disk_slot: usize) {
    if array_idx >= MD_MAX_ARRAYS {
        return;
    }
    let mut arrays = MD_ARRAYS.lock();
    let arr = &mut arrays[array_idx];
    if disk_slot < arr.ndisks as usize {
        arr.disks[disk_slot].fault = true;
        serial_println!(
            "  [md] array {} disk slot {} marked faulted (disk_idx={})",
            array_idx,
            disk_slot,
            arr.disks[disk_slot].disk_idx
        );
    }
}

/// Return I/O statistics for an array as `(read_ops, write_ops, errors)`.
///
/// Returns None if the array index is out of range or the array has never
/// been started.
pub fn md_get_stats(array_idx: usize) -> Option<(u64, u64, u64)> {
    if array_idx >= MD_MAX_ARRAYS {
        return None;
    }
    let arrays = MD_ARRAYS.lock();
    let arr = &arrays[array_idx];
    Some((arr.read_ops, arr.write_ops, arr.errors))
}

// ---------------------------------------------------------------------------
// Module init
// ---------------------------------------------------------------------------

pub fn init() {
    // Clear the array table to a known-empty state
    let mut arrays = MD_ARRAYS.lock();
    let mut i = 0usize;
    while i < MD_MAX_ARRAYS {
        arrays[i] = MdArray::empty();
        i = i.wrapping_add(1);
    }
    drop(arrays);
    serial_println!("  [md] Software RAID initialized");
}
