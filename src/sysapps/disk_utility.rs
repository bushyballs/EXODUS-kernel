/// Disk utility application for Genesis OS
///
/// Full disk management with partition table viewing/editing, format,
/// resize, mount/unmount, SMART health data, and disk diagnostics.
/// All sizes use Q16 fixed-point for fractional GB/TB display.
/// Partition tables, mount points, and health data stored in kernel memory.
///
/// Inspired by: GParted, GNOME Disks, fdisk. All code is original.

use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 helpers
// ---------------------------------------------------------------------------

/// 1.0 in Q16
const Q16_ONE: i32 = 65536;

/// Q16 multiplication: (a * b) >> 16
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Q16 division: (a << 16) / b
fn q16_div(a: i32, b: i32) -> Option<i32> {
    if b == 0 {
        return None;
    }
    Some((((a as i64) << 16) / (b as i64)) as i32)
}

/// Q16 percentage: (part / total) * 100  in Q16
fn q16_percent(part: i32, total: i32) -> Option<i32> {
    let ratio = q16_div(part, total)?;
    Some(q16_mul(ratio, 100 * Q16_ONE))
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of disks tracked
const MAX_DISKS: usize = 32;
/// Maximum partitions per disk
const MAX_PARTITIONS_PER_DISK: usize = 128;
/// Maximum mount points
const MAX_MOUNT_POINTS: usize = 256;
/// SMART attribute count
const MAX_SMART_ATTRS: usize = 30;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Filesystem type for a partition
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Filesystem {
    Ext4,
    Btrfs,
    Xfs,
    Fat32,
    Ntfs,
    ExFat,
    Swap,
    GenFs,
    Raw,
    Unknown,
}

/// Partition table scheme
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PartitionScheme {
    Gpt,
    Mbr,
    None,
}

/// Disk bus type
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiskBus {
    Sata,
    Nvme,
    Usb,
    Scsi,
    Virtio,
    Unknown,
}

/// SMART health status
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SmartStatus {
    Healthy,
    Warning,
    Failing,
    Unknown,
}

/// Disk operation result
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiskResult {
    Success,
    NotFound,
    AlreadyMounted,
    NotMounted,
    InvalidSize,
    InUse,
    IoError,
    LimitReached,
    InvalidPartition,
    ReadOnly,
}

/// A single SMART attribute
#[derive(Debug, Clone, Copy)]
pub struct SmartAttribute {
    pub id: u8,
    pub name_hash: u64,
    pub current: u32,
    pub worst: u32,
    pub threshold: u32,
    pub raw_value: u64,
    pub failing: bool,
}

/// SMART data for a disk
#[derive(Debug, Clone)]
pub struct SmartData {
    pub overall: SmartStatus,
    pub temperature_c: i32,
    pub power_on_hours: u64,
    pub power_cycle_count: u64,
    pub reallocated_sectors: u32,
    pub pending_sectors: u32,
    pub uncorrectable_errors: u32,
    pub attributes: Vec<SmartAttribute>,
}

/// A single partition on a disk
#[derive(Debug, Clone)]
pub struct Partition {
    pub id: u64,
    pub disk_id: u64,
    pub index: u32,
    pub label_hash: u64,
    pub filesystem: Filesystem,
    pub start_lba: u64,
    pub end_lba: u64,
    pub size_sectors: u64,
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub flags: u32,
    pub uuid_hash: u64,
    pub bootable: bool,
    pub mounted: bool,
    pub mount_path_hash: u64,
}

/// A physical disk
#[derive(Debug, Clone)]
pub struct Disk {
    pub id: u64,
    pub name_hash: u64,
    pub model_hash: u64,
    pub serial_hash: u64,
    pub bus: DiskBus,
    pub scheme: PartitionScheme,
    pub total_bytes: u64,
    pub sector_size: u32,
    pub sector_count: u64,
    pub partitions: Vec<Partition>,
    pub smart: SmartData,
    pub removable: bool,
    pub read_only: bool,
}

/// Mount point entry
#[derive(Debug, Clone)]
pub struct MountPoint {
    pub partition_id: u64,
    pub disk_id: u64,
    pub path_hash: u64,
    pub filesystem: Filesystem,
    pub read_only: bool,
    pub mount_time: u64,
}

/// Disk utility state
struct DiskUtilState {
    disks: Vec<Disk>,
    mount_points: Vec<MountPoint>,
    next_disk_id: u64,
    next_partition_id: u64,
    timestamp: u64,
    scan_count: u32,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static DISK_UTIL: Mutex<Option<DiskUtilState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn next_timestamp(state: &mut DiskUtilState) -> u64 {
    state.timestamp += 1;
    state.timestamp
}

fn default_smart() -> SmartData {
    SmartData {
        overall: SmartStatus::Healthy,
        temperature_c: 35,
        power_on_hours: 0,
        power_cycle_count: 0,
        reallocated_sectors: 0,
        pending_sectors: 0,
        uncorrectable_errors: 0,
        attributes: Vec::new(),
    }
}

fn default_state() -> DiskUtilState {
    DiskUtilState {
        disks: Vec::new(),
        mount_points: Vec::new(),
        next_disk_id: 1,
        next_partition_id: 1,
        timestamp: 0,
        scan_count: 0,
    }
}

fn recalc_smart_status(smart: &mut SmartData) {
    if smart.reallocated_sectors > 100 || smart.uncorrectable_errors > 10 {
        smart.overall = SmartStatus::Failing;
    } else if smart.reallocated_sectors > 10 || smart.pending_sectors > 5 {
        smart.overall = SmartStatus::Warning;
    } else {
        smart.overall = SmartStatus::Healthy;
    }
}

fn usage_q16(partition: &Partition) -> i32 {
    if partition.size_bytes == 0 {
        return 0;
    }
    let used = partition.used_bytes as i32;
    let total = partition.size_bytes as i32;
    q16_percent(used, total).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Public API -- Disk scanning
// ---------------------------------------------------------------------------

/// Register a detected disk in the utility
pub fn register_disk(
    name_hash: u64,
    model_hash: u64,
    serial_hash: u64,
    bus: DiskBus,
    total_bytes: u64,
    sector_size: u32,
    removable: bool,
) -> Result<u64, DiskResult> {
    let mut guard = DISK_UTIL.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return Err(DiskResult::IoError),
    };
    if state.disks.len() >= MAX_DISKS {
        return Err(DiskResult::LimitReached);
    }
    let id = state.next_disk_id;
    state.next_disk_id += 1;
    let sector_count = if sector_size > 0 {
        total_bytes / sector_size as u64
    } else {
        0
    };
    state.disks.push(Disk {
        id,
        name_hash,
        model_hash,
        serial_hash,
        bus,
        scheme: PartitionScheme::None,
        total_bytes,
        sector_size,
        sector_count,
        partitions: Vec::new(),
        smart: default_smart(),
        removable,
        read_only: false,
    });
    state.scan_count += 1;
    Ok(id)
}

/// List all registered disks
pub fn list_disks() -> Vec<Disk> {
    let guard = DISK_UTIL.lock();
    match guard.as_ref() {
        Some(state) => state.disks.clone(),
        None => Vec::new(),
    }
}

/// Get a disk by ID
pub fn get_disk(disk_id: u64) -> Option<Disk> {
    let guard = DISK_UTIL.lock();
    let state = guard.as_ref()?;
    state.disks.iter().find(|d| d.id == disk_id).cloned()
}

// ---------------------------------------------------------------------------
// Public API -- Partition management
// ---------------------------------------------------------------------------

/// Create a new partition on a disk
pub fn create_partition(
    disk_id: u64,
    label_hash: u64,
    filesystem: Filesystem,
    start_lba: u64,
    size_sectors: u64,
) -> Result<u64, DiskResult> {
    let mut guard = DISK_UTIL.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return Err(DiskResult::IoError),
    };
    let disk = match state.disks.iter_mut().find(|d| d.id == disk_id) {
        Some(d) => d,
        None => return Err(DiskResult::NotFound),
    };
    if disk.read_only {
        return Err(DiskResult::ReadOnly);
    }
    if disk.partitions.len() >= MAX_PARTITIONS_PER_DISK {
        return Err(DiskResult::LimitReached);
    }
    let end_lba = start_lba + size_sectors;
    if end_lba > disk.sector_count {
        return Err(DiskResult::InvalidSize);
    }
    // Check overlap
    for p in disk.partitions.iter() {
        if start_lba < p.end_lba && end_lba > p.start_lba {
            return Err(DiskResult::InvalidPartition);
        }
    }
    let pid = state.next_partition_id;
    state.next_partition_id += 1;
    let index = disk.partitions.len() as u32;
    let size_bytes = size_sectors * disk.sector_size as u64;
    let uuid_hash = pid.wrapping_mul(0xDEAD_BEEF_1234_5678);
    disk.partitions.push(Partition {
        id: pid,
        disk_id,
        index,
        label_hash,
        filesystem,
        start_lba,
        end_lba,
        size_sectors,
        size_bytes,
        used_bytes: 0,
        flags: 0,
        uuid_hash,
        bootable: false,
        mounted: false,
        mount_path_hash: 0,
    });
    if disk.scheme == PartitionScheme::None {
        disk.scheme = PartitionScheme::Gpt;
    }
    Ok(pid)
}

/// Delete a partition (must be unmounted)
pub fn delete_partition(disk_id: u64, partition_id: u64) -> DiskResult {
    let mut guard = DISK_UTIL.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return DiskResult::IoError,
    };
    let disk = match state.disks.iter_mut().find(|d| d.id == disk_id) {
        Some(d) => d,
        None => return DiskResult::NotFound,
    };
    if disk.read_only {
        return DiskResult::ReadOnly;
    }
    if let Some(p) = disk.partitions.iter().find(|p| p.id == partition_id) {
        if p.mounted {
            return DiskResult::InUse;
        }
    } else {
        return DiskResult::NotFound;
    }
    disk.partitions.retain(|p| p.id != partition_id);
    // Reindex partitions
    for (i, p) in disk.partitions.iter_mut().enumerate() {
        p.index = i as u32;
    }
    state.mount_points.retain(|m| m.partition_id != partition_id);
    DiskResult::Success
}

/// Resize a partition (must be unmounted)
pub fn resize_partition(disk_id: u64, partition_id: u64, new_size_sectors: u64) -> DiskResult {
    let mut guard = DISK_UTIL.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return DiskResult::IoError,
    };
    let disk = match state.disks.iter_mut().find(|d| d.id == disk_id) {
        Some(d) => d,
        None => return DiskResult::NotFound,
    };
    if disk.read_only {
        return DiskResult::ReadOnly;
    }
    let partition = match disk.partitions.iter_mut().find(|p| p.id == partition_id) {
        Some(p) => p,
        None => return DiskResult::NotFound,
    };
    if partition.mounted {
        return DiskResult::InUse;
    }
    let new_end = partition.start_lba + new_size_sectors;
    if new_end > disk.sector_count {
        return DiskResult::InvalidSize;
    }
    partition.size_sectors = new_size_sectors;
    partition.end_lba = new_end;
    partition.size_bytes = new_size_sectors * disk.sector_size as u64;
    if partition.used_bytes > partition.size_bytes {
        partition.used_bytes = partition.size_bytes;
    }
    DiskResult::Success
}

/// Format a partition with a new filesystem (must be unmounted)
pub fn format_partition(disk_id: u64, partition_id: u64, filesystem: Filesystem) -> DiskResult {
    let mut guard = DISK_UTIL.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return DiskResult::IoError,
    };
    let disk = match state.disks.iter_mut().find(|d| d.id == disk_id) {
        Some(d) => d,
        None => return DiskResult::NotFound,
    };
    if disk.read_only {
        return DiskResult::ReadOnly;
    }
    let partition = match disk.partitions.iter_mut().find(|p| p.id == partition_id) {
        Some(p) => p,
        None => return DiskResult::NotFound,
    };
    if partition.mounted {
        return DiskResult::InUse;
    }
    partition.filesystem = filesystem;
    partition.used_bytes = 0;
    partition.uuid_hash = partition.uuid_hash.wrapping_add(1);
    DiskResult::Success
}

// ---------------------------------------------------------------------------
// Public API -- Mount / Unmount
// ---------------------------------------------------------------------------

/// Mount a partition at a given path
pub fn mount(disk_id: u64, partition_id: u64, path_hash: u64, read_only: bool) -> DiskResult {
    let mut guard = DISK_UTIL.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return DiskResult::IoError,
    };
    if state.mount_points.len() >= MAX_MOUNT_POINTS {
        return DiskResult::LimitReached;
    }
    let disk = match state.disks.iter_mut().find(|d| d.id == disk_id) {
        Some(d) => d,
        None => return DiskResult::NotFound,
    };
    let partition = match disk.partitions.iter_mut().find(|p| p.id == partition_id) {
        Some(p) => p,
        None => return DiskResult::NotFound,
    };
    if partition.mounted {
        return DiskResult::AlreadyMounted;
    }
    let now = next_timestamp(state);
    partition.mounted = true;
    partition.mount_path_hash = path_hash;
    state.mount_points.push(MountPoint {
        partition_id,
        disk_id,
        path_hash,
        filesystem: partition.filesystem,
        read_only,
        mount_time: now,
    });
    DiskResult::Success
}

/// Unmount a partition
pub fn unmount(partition_id: u64) -> DiskResult {
    let mut guard = DISK_UTIL.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return DiskResult::IoError,
    };
    // Find partition across all disks
    let mut found = false;
    for disk in state.disks.iter_mut() {
        if let Some(p) = disk.partitions.iter_mut().find(|p| p.id == partition_id) {
            if !p.mounted {
                return DiskResult::NotMounted;
            }
            p.mounted = false;
            p.mount_path_hash = 0;
            found = true;
            break;
        }
    }
    if !found {
        return DiskResult::NotFound;
    }
    state.mount_points.retain(|m| m.partition_id != partition_id);
    DiskResult::Success
}

/// List all active mount points
pub fn list_mounts() -> Vec<MountPoint> {
    let guard = DISK_UTIL.lock();
    match guard.as_ref() {
        Some(state) => state.mount_points.clone(),
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Public API -- SMART / Health
// ---------------------------------------------------------------------------

/// Update SMART data for a disk
pub fn update_smart(
    disk_id: u64,
    temperature_c: i32,
    power_on_hours: u64,
    reallocated: u32,
    pending: u32,
    uncorrectable: u32,
) -> DiskResult {
    let mut guard = DISK_UTIL.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return DiskResult::IoError,
    };
    let disk = match state.disks.iter_mut().find(|d| d.id == disk_id) {
        Some(d) => d,
        None => return DiskResult::NotFound,
    };
    disk.smart.temperature_c = temperature_c;
    disk.smart.power_on_hours = power_on_hours;
    disk.smart.reallocated_sectors = reallocated;
    disk.smart.pending_sectors = pending;
    disk.smart.uncorrectable_errors = uncorrectable;
    recalc_smart_status(&mut disk.smart);
    DiskResult::Success
}

/// Get SMART health summary for a disk
pub fn get_smart(disk_id: u64) -> Option<SmartData> {
    let guard = DISK_UTIL.lock();
    let state = guard.as_ref()?;
    state.disks.iter().find(|d| d.id == disk_id).map(|d| d.smart.clone())
}

/// Get overall health status for all disks
pub fn health_summary() -> Vec<(u64, SmartStatus)> {
    let guard = DISK_UTIL.lock();
    match guard.as_ref() {
        Some(state) => state
            .disks
            .iter()
            .map(|d| (d.id, d.smart.overall))
            .collect(),
        None => Vec::new(),
    }
}

/// Get disk usage as Q16 percentage for a partition
pub fn partition_usage_q16(disk_id: u64, partition_id: u64) -> Option<i32> {
    let guard = DISK_UTIL.lock();
    let state = guard.as_ref()?;
    let disk = state.disks.iter().find(|d| d.id == disk_id)?;
    let part = disk.partitions.iter().find(|p| p.id == partition_id)?;
    Some(usage_q16(part))
}

/// Set the bootable flag on a partition
pub fn set_bootable(disk_id: u64, partition_id: u64, bootable: bool) -> DiskResult {
    let mut guard = DISK_UTIL.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return DiskResult::IoError,
    };
    let disk = match state.disks.iter_mut().find(|d| d.id == disk_id) {
        Some(d) => d,
        None => return DiskResult::NotFound,
    };
    let partition = match disk.partitions.iter_mut().find(|p| p.id == partition_id) {
        Some(p) => p,
        None => return DiskResult::NotFound,
    };
    partition.bootable = bootable;
    DiskResult::Success
}

/// Get total disk count
pub fn disk_count() -> usize {
    let guard = DISK_UTIL.lock();
    match guard.as_ref() {
        Some(state) => state.disks.len(),
        None => 0,
    }
}

/// Get total partition count across all disks
pub fn total_partition_count() -> usize {
    let guard = DISK_UTIL.lock();
    match guard.as_ref() {
        Some(state) => state.disks.iter().map(|d| d.partitions.len()).sum(),
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the disk utility subsystem
pub fn init() {
    let mut guard = DISK_UTIL.lock();
    *guard = Some(default_state());
    serial_println!("    Disk utility ready");
}
