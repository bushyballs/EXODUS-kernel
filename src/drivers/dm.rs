/// Device mapper for Genesis -- logical volume management
///
/// Creates virtual block devices from physical block devices.
/// Supports: linear mapping, striped (RAID-0), mirror (RAID-1),
/// snapshot (copy-on-write), crypt (encrypted), error, zero targets.
///
/// Features:
///   - Device mapper table: list of (sector_start, sector_count, target_type, target_args)
///   - Linear target: map sectors to offset on underlying device
///   - Striped target: distribute sectors across N devices (RAID-0 style)
///   - Snapshot target: copy-on-write overlay for block device
///   - BIO remapping: translate virtual sector -> physical device + sector
///   - Atomic table loading/swapping
///   - Device creation/removal
///   - Suspend/resume (pause I/O during table swap)
///   - Per-target statistics: reads, writes, errors
///
/// Inspired by: Linux device mapper (drivers/md/dm.c). All code is original.
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Target types
// ---------------------------------------------------------------------------

/// Mapping target type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetType {
    /// 1:1 mapping to a contiguous range on underlying device
    Linear,
    /// Stripe across multiple devices (RAID-0)
    Striped,
    /// Mirror to N devices (RAID-1)
    Mirror,
    /// Snapshot (copy-on-write clone)
    Snapshot,
    /// Encrypted volume
    Crypt,
    /// Error target (returns I/O errors)
    Error,
    /// Zero target (reads return zeros, writes are discarded)
    Zero,
}

// ---------------------------------------------------------------------------
// I/O direction
// ---------------------------------------------------------------------------

/// I/O direction for bio requests
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoDirection {
    Read,
    Write,
}

// ---------------------------------------------------------------------------
// Per-target statistics
// ---------------------------------------------------------------------------

/// Statistics for a single target mapping
#[derive(Debug, Clone, Copy, Default)]
pub struct TargetStats {
    pub reads: u64,
    pub writes: u64,
    pub read_sectors: u64,
    pub write_sectors: u64,
    pub errors: u64,
}

impl TargetStats {
    pub const fn new() -> Self {
        TargetStats {
            reads: 0,
            writes: 0,
            read_sectors: 0,
            write_sectors: 0,
            errors: 0,
        }
    }

    /// Record a read operation
    pub fn record_read(&mut self, sectors: u64) {
        self.reads = self.reads.saturating_add(1);
        self.read_sectors = self.read_sectors.saturating_add(sectors);
    }

    /// Record a write operation
    pub fn record_write(&mut self, sectors: u64) {
        self.writes = self.writes.saturating_add(1);
        self.write_sectors = self.write_sectors.saturating_add(sectors);
    }

    /// Record an error
    pub fn record_error(&mut self) {
        self.errors = self.errors.saturating_add(1);
    }

    /// Merge two stat sets
    pub fn merge(&mut self, other: &TargetStats) {
        self.reads = self.reads.saturating_add(other.reads);
        self.writes = self.writes.saturating_add(other.writes);
        self.read_sectors = self.read_sectors.saturating_add(other.read_sectors);
        self.write_sectors = self.write_sectors.saturating_add(other.write_sectors);
        self.errors = self.errors.saturating_add(other.errors);
    }
}

// ---------------------------------------------------------------------------
// Snapshot tracking
// ---------------------------------------------------------------------------

/// Snapshot copy-on-write entry: maps a virtual chunk to its COW location
#[derive(Clone)]
struct CowEntry {
    /// Virtual chunk number
    virtual_chunk: u64,
    /// Physical location: device index + sector offset in COW area
    cow_device: u32,
    cow_sector: u64,
}

/// Snapshot state for a snapshot target
#[derive(Clone)]
struct SnapshotState {
    /// Origin device (the device being snapshotted)
    origin_device: u32,
    /// COW device (where modified chunks are stored)
    cow_device: u32,
    /// Next free sector on the COW device
    cow_next_free: u64,
    /// COW chunk size in sectors
    chunk_size: u32,
    /// Map of virtual chunks that have been COW'd
    cow_map: Vec<CowEntry>,
    /// Whether the snapshot is valid (not overflowed)
    valid: bool,
}

impl SnapshotState {
    fn new(origin_device: u32, cow_device: u32, chunk_size: u32) -> Self {
        SnapshotState {
            origin_device,
            cow_device,
            cow_next_free: 0,
            chunk_size,
            cow_map: Vec::new(),
            valid: true,
        }
    }

    /// Check if a chunk has already been COW'd
    fn find_cow(&self, virtual_chunk: u64) -> Option<(u32, u64)> {
        for entry in &self.cow_map {
            if entry.virtual_chunk == virtual_chunk {
                return Some((entry.cow_device, entry.cow_sector));
            }
        }
        None
    }

    /// Allocate a new COW chunk
    fn alloc_cow(&mut self, virtual_chunk: u64) -> Option<(u32, u64)> {
        if !self.valid {
            return None;
        }
        let sector = self.cow_next_free;
        self.cow_next_free = self.cow_next_free.saturating_add(self.chunk_size as u64);

        self.cow_map.push(CowEntry {
            virtual_chunk,
            cow_device: self.cow_device,
            cow_sector: sector,
        });
        Some((self.cow_device, sector))
    }

    /// Number of COW'd chunks
    fn cow_count(&self) -> usize {
        self.cow_map.len()
    }
}

// ---------------------------------------------------------------------------
// Mapping table entry
// ---------------------------------------------------------------------------

/// A mapping table entry
#[derive(Clone)]
pub struct DmTarget {
    /// Target type
    pub target_type: TargetType,
    /// Start sector in the virtual device
    pub start: u64,
    /// Length in sectors
    pub length: u64,
    /// Underlying device index (for linear, crypt, mirror)
    pub device: u32,
    /// Start offset on the underlying device
    pub offset: u64,
    /// For striped: stripe size in sectors
    pub stripe_size: u32,
    /// For striped: number of stripes/devices
    pub stripe_count: u32,
    /// For striped: list of (device_idx, offset) per stripe device
    pub stripe_devices: Vec<(u32, u64)>,
    /// For crypt: cipher name
    pub cipher: String,
    /// For mirror: list of mirror device indices
    pub mirror_devices: Vec<u32>,
    /// Statistics
    pub stats: TargetStats,
    /// Snapshot state (only for Snapshot target)
    snapshot: Option<SnapshotState>,
}

impl DmTarget {
    /// Create a new linear target
    pub fn new_linear(start: u64, length: u64, device: u32, offset: u64) -> Self {
        DmTarget {
            target_type: TargetType::Linear,
            start,
            length,
            device,
            offset,
            stripe_size: 0,
            stripe_count: 0,
            stripe_devices: Vec::new(),
            cipher: String::new(),
            mirror_devices: Vec::new(),
            stats: TargetStats::new(),
            snapshot: None,
        }
    }

    /// Create a new striped target
    pub fn new_striped(start: u64, length: u64, stripe_size: u32, devices: &[(u32, u64)]) -> Self {
        DmTarget {
            target_type: TargetType::Striped,
            start,
            length,
            device: devices.first().map_or(0, |d| d.0),
            offset: devices.first().map_or(0, |d| d.1),
            stripe_size,
            stripe_count: devices.len() as u32,
            stripe_devices: devices.to_vec(),
            cipher: String::new(),
            mirror_devices: Vec::new(),
            stats: TargetStats::new(),
            snapshot: None,
        }
    }

    /// Create a new snapshot target
    pub fn new_snapshot(
        start: u64,
        length: u64,
        origin_device: u32,
        cow_device: u32,
        chunk_size: u32,
    ) -> Self {
        DmTarget {
            target_type: TargetType::Snapshot,
            start,
            length,
            device: origin_device,
            offset: 0,
            stripe_size: chunk_size,
            stripe_count: 0,
            stripe_devices: Vec::new(),
            cipher: String::new(),
            mirror_devices: Vec::new(),
            stats: TargetStats::new(),
            snapshot: Some(SnapshotState::new(origin_device, cow_device, chunk_size)),
        }
    }

    /// Create an error target (all I/O returns errors)
    pub fn new_error(start: u64, length: u64) -> Self {
        DmTarget {
            target_type: TargetType::Error,
            start,
            length,
            device: 0,
            offset: 0,
            stripe_size: 0,
            stripe_count: 0,
            stripe_devices: Vec::new(),
            cipher: String::new(),
            mirror_devices: Vec::new(),
            stats: TargetStats::new(),
            snapshot: None,
        }
    }

    /// Create a zero target (reads return zeros, writes discarded)
    pub fn new_zero(start: u64, length: u64) -> Self {
        DmTarget {
            target_type: TargetType::Zero,
            start,
            length,
            device: 0,
            offset: 0,
            stripe_size: 0,
            stripe_count: 0,
            stripe_devices: Vec::new(),
            cipher: String::new(),
            mirror_devices: Vec::new(),
            stats: TargetStats::new(),
            snapshot: None,
        }
    }

    /// Check if a sector falls within this target
    pub fn contains(&self, sector: u64) -> bool {
        sector >= self.start && sector < self.start.saturating_add(self.length)
    }

    /// Format as dmsetup-style table line
    pub fn table_line(&self) -> String {
        match self.target_type {
            TargetType::Linear => {
                format!(
                    "{} {} linear {}:{}",
                    self.start, self.length, self.device, self.offset
                )
            }
            TargetType::Striped => {
                let devs: Vec<String> = self
                    .stripe_devices
                    .iter()
                    .map(|(d, o)| format!("{}:{}", d, o))
                    .collect();
                format!(
                    "{} {} striped {} {} {}",
                    self.start,
                    self.length,
                    self.stripe_size,
                    self.stripe_count,
                    devs.join(" ")
                )
            }
            TargetType::Mirror => {
                let devs: Vec<String> = self
                    .mirror_devices
                    .iter()
                    .map(|d| format!("{}", d))
                    .collect();
                format!("{} {} mirror {}", self.start, self.length, devs.join(" "))
            }
            TargetType::Snapshot => {
                let ss = self.snapshot.as_ref().map_or(String::from("?"), |s| {
                    format!(
                        "origin={} cow={} chunk={}",
                        s.origin_device, s.cow_device, s.chunk_size
                    )
                });
                format!("{} {} snapshot {}", self.start, self.length, ss)
            }
            TargetType::Crypt => {
                format!(
                    "{} {} crypt {} {}:{}",
                    self.start, self.length, self.cipher, self.device, self.offset
                )
            }
            TargetType::Error => {
                format!("{} {} error", self.start, self.length)
            }
            TargetType::Zero => {
                format!("{} {} zero", self.start, self.length)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// BIO remap result
// ---------------------------------------------------------------------------

/// Result of remapping a sector through the device mapper
#[derive(Debug, Clone, Copy)]
pub enum RemapResult {
    /// I/O should go to this physical device and sector
    Remap { device: u32, sector: u64 },
    /// I/O should return zeros (for zero target reads)
    Zero,
    /// I/O should be discarded (for zero target writes)
    Discard,
    /// I/O should return an error
    Error,
}

// ---------------------------------------------------------------------------
// Mapped device
// ---------------------------------------------------------------------------

/// A mapped device
pub struct MappedDevice {
    pub name: String,
    pub uuid: String,
    pub size_sectors: u64,
    /// Active table (currently serving I/O)
    pub targets: Vec<DmTarget>,
    /// Inactive table (being loaded, not yet swapped in)
    inactive_targets: Vec<DmTarget>,
    pub active: bool,
    pub suspended: bool,
    pub read_only: bool,
    pub open_count: u32,
    /// Aggregate stats across all targets
    pub total_stats: TargetStats,
    /// I/O queue depth (requests in flight)
    pub io_in_flight: u32,
    /// Creation timestamp (tick count)
    pub created_tick: u64,
}

impl MappedDevice {
    fn new(name: &str, uuid: &str) -> Self {
        MappedDevice {
            name: String::from(name),
            uuid: String::from(uuid),
            size_sectors: 0,
            targets: Vec::new(),
            inactive_targets: Vec::new(),
            active: false,
            suspended: false,
            read_only: false,
            open_count: 0,
            total_stats: TargetStats::new(),
            io_in_flight: 0,
            created_tick: 0,
        }
    }

    /// Recalculate device size from targets
    fn recalculate_size(&mut self) {
        self.size_sectors = self
            .targets
            .iter()
            .map(|t| t.start.saturating_add(t.length))
            .max()
            .unwrap_or(0);
    }

    /// Find the target that handles a given sector
    fn find_target(&self, sector: u64) -> Option<usize> {
        self.targets.iter().position(|t| t.contains(sector))
    }

    /// Verify table integrity: no gaps or overlaps
    fn verify_table(targets: &[DmTarget]) -> Result<(), &'static str> {
        if targets.is_empty() {
            return Err("empty table");
        }
        // Sort check: targets must be ordered by start sector
        for i in 1..targets.len() {
            let prev_end = targets[i - 1].start.saturating_add(targets[i - 1].length);
            if targets[i].start < prev_end {
                return Err("overlapping targets");
            }
        }
        // Contiguity check: no gaps
        for i in 1..targets.len() {
            let prev_end = targets[i - 1].start.saturating_add(targets[i - 1].length);
            if targets[i].start != prev_end {
                return Err("gap in table");
            }
        }
        // First target must start at 0
        if targets[0].start != 0 {
            return Err("table does not start at sector 0");
        }
        Ok(())
    }

    /// Get aggregate statistics
    fn aggregate_stats(&self) -> TargetStats {
        let mut total = TargetStats::new();
        for t in &self.targets {
            total.merge(&t.stats);
        }
        total
    }
}

// ---------------------------------------------------------------------------
// Device mapper
// ---------------------------------------------------------------------------

/// Device mapper
pub struct DeviceMapper {
    devices: Vec<MappedDevice>,
    /// Global tick counter (for timestamps)
    tick: u64,
}

impl DeviceMapper {
    const fn new() -> Self {
        DeviceMapper {
            devices: Vec::new(),
            tick: 0,
        }
    }

    /// Create a new mapped device
    pub fn create(&mut self, name: &str, uuid: &str) -> Option<usize> {
        if self.devices.iter().any(|d| d.name == name) {
            return None; // already exists
        }

        let idx = self.devices.len();
        let mut dev = MappedDevice::new(name, uuid);
        dev.created_tick = self.tick;
        self.devices.push(dev);
        crate::serial_println!("    [dm] Created device '{}'", name);
        Some(idx)
    }

    /// Load a target into the inactive table of a device
    pub fn load_target(&mut self, name: &str, target: DmTarget) -> bool {
        if let Some(dm) = self.devices.iter_mut().find(|d| d.name == name) {
            dm.inactive_targets.push(target);
            true
        } else {
            false
        }
    }

    /// Clear the inactive table
    pub fn clear_inactive(&mut self, name: &str) -> bool {
        if let Some(dm) = self.devices.iter_mut().find(|d| d.name == name) {
            dm.inactive_targets.clear();
            true
        } else {
            false
        }
    }

    /// Atomically swap the inactive table into the active table.
    /// The device must be suspended for the swap to succeed.
    pub fn swap_tables(&mut self, name: &str) -> Result<(), &'static str> {
        let dm = self
            .devices
            .iter_mut()
            .find(|d| d.name == name)
            .ok_or("device not found")?;

        if !dm.suspended {
            return Err("device must be suspended for table swap");
        }

        // Verify the new table
        MappedDevice::verify_table(&dm.inactive_targets)?;

        // Swap
        core::mem::swap(&mut dm.targets, &mut dm.inactive_targets);
        dm.inactive_targets.clear();
        dm.recalculate_size();

        crate::serial_println!(
            "    [dm] '{}': table swapped ({} targets, {} sectors)",
            dm.name,
            dm.targets.len(),
            dm.size_sectors
        );
        Ok(())
    }

    /// Add a linear target to a device (convenience, loads directly to active table)
    pub fn add_linear_target(
        &mut self,
        name: &str,
        start: u64,
        length: u64,
        device: u32,
        offset: u64,
    ) -> bool {
        if let Some(dm) = self.devices.iter_mut().find(|d| d.name == name) {
            dm.targets
                .push(DmTarget::new_linear(start, length, device, offset));
            dm.recalculate_size();
            true
        } else {
            false
        }
    }

    /// Add a striped (RAID-0) target
    pub fn add_striped_target(
        &mut self,
        name: &str,
        start: u64,
        length: u64,
        stripe_size: u32,
        devices: &[(u32, u64)],
    ) -> bool {
        if let Some(dm) = self.devices.iter_mut().find(|d| d.name == name) {
            dm.targets
                .push(DmTarget::new_striped(start, length, stripe_size, devices));
            dm.recalculate_size();
            true
        } else {
            false
        }
    }

    /// Add a snapshot target
    pub fn add_snapshot_target(
        &mut self,
        name: &str,
        start: u64,
        length: u64,
        origin_device: u32,
        cow_device: u32,
        chunk_size: u32,
    ) -> bool {
        if let Some(dm) = self.devices.iter_mut().find(|d| d.name == name) {
            dm.targets.push(DmTarget::new_snapshot(
                start,
                length,
                origin_device,
                cow_device,
                chunk_size,
            ));
            dm.recalculate_size();
            true
        } else {
            false
        }
    }

    /// Add a crypt target (encrypted volume)
    pub fn add_crypt_target(
        &mut self,
        name: &str,
        start: u64,
        length: u64,
        device: u32,
        offset: u64,
        cipher: &str,
    ) -> bool {
        if let Some(dm) = self.devices.iter_mut().find(|d| d.name == name) {
            dm.targets.push(DmTarget {
                target_type: TargetType::Crypt,
                start,
                length,
                device,
                offset,
                stripe_size: 0,
                stripe_count: 0,
                stripe_devices: Vec::new(),
                cipher: String::from(cipher),
                mirror_devices: Vec::new(),
                stats: TargetStats::new(),
                snapshot: None,
            });
            dm.recalculate_size();
            true
        } else {
            false
        }
    }

    /// Add a mirror target (RAID-1): writes to all devices, reads from primary
    pub fn add_mirror_target(
        &mut self,
        name: &str,
        start: u64,
        length: u64,
        primary_device: u32,
        primary_offset: u64,
        mirror_devs: &[u32],
    ) -> bool {
        if let Some(dm) = self.devices.iter_mut().find(|d| d.name == name) {
            let mut mirror_list = Vec::with_capacity(mirror_devs.len());
            for &dev in mirror_devs {
                mirror_list.push(dev);
            }
            dm.targets.push(DmTarget {
                target_type: TargetType::Mirror,
                start,
                length,
                device: primary_device,
                offset: primary_offset,
                stripe_size: 0,
                stripe_count: 0,
                stripe_devices: Vec::new(),
                cipher: String::new(),
                mirror_devices: mirror_list,
                stats: TargetStats::new(),
                snapshot: None,
            });
            dm.recalculate_size();
            true
        } else {
            false
        }
    }

    /// Add an error target
    pub fn add_error_target(&mut self, name: &str, start: u64, length: u64) -> bool {
        if let Some(dm) = self.devices.iter_mut().find(|d| d.name == name) {
            dm.targets.push(DmTarget::new_error(start, length));
            dm.recalculate_size();
            true
        } else {
            false
        }
    }

    /// Add a zero target
    pub fn add_zero_target(&mut self, name: &str, start: u64, length: u64) -> bool {
        if let Some(dm) = self.devices.iter_mut().find(|d| d.name == name) {
            dm.targets.push(DmTarget::new_zero(start, length));
            dm.recalculate_size();
            true
        } else {
            false
        }
    }

    /// Activate a mapped device (make it usable)
    pub fn activate(&mut self, name: &str) -> bool {
        if let Some(dm) = self.devices.iter_mut().find(|d| d.name == name) {
            if dm.targets.is_empty() {
                return false;
            }
            dm.active = true;
            dm.suspended = false;
            crate::serial_println!(
                "    [dm] '{}': activated ({} sectors)",
                dm.name,
                dm.size_sectors
            );
            true
        } else {
            false
        }
    }

    /// Suspend a mapped device (pause I/O, allow table swap)
    pub fn suspend(&mut self, name: &str) -> bool {
        if let Some(dm) = self.devices.iter_mut().find(|d| d.name == name) {
            if !dm.active {
                return false;
            }
            dm.suspended = true;
            crate::serial_println!(
                "    [dm] '{}': suspended (io_in_flight={})",
                dm.name,
                dm.io_in_flight
            );
            true
        } else {
            false
        }
    }

    /// Resume a suspended device
    pub fn resume(&mut self, name: &str) -> bool {
        if let Some(dm) = self.devices.iter_mut().find(|d| d.name == name) {
            if !dm.suspended {
                return false;
            }
            dm.suspended = false;
            crate::serial_println!("    [dm] '{}': resumed", dm.name);
            true
        } else {
            false
        }
    }

    /// Remove a mapped device
    pub fn remove(&mut self, name: &str) -> bool {
        if let Some(pos) = self.devices.iter().position(|d| d.name == name) {
            if self.devices[pos].open_count > 0 {
                crate::serial_println!(
                    "    [dm] Cannot remove '{}': {} opens",
                    name,
                    self.devices[pos].open_count
                );
                return false;
            }
            self.devices.remove(pos);
            crate::serial_println!("    [dm] '{}': removed", name);
            true
        } else {
            false
        }
    }

    /// Remap a BIO (sector I/O request) through the device mapper.
    /// Returns the physical device and sector, or an error.
    pub fn remap(&mut self, name: &str, sector: u64, direction: IoDirection) -> RemapResult {
        let dm = match self.devices.iter_mut().find(|d| d.name == name && d.active) {
            Some(d) => d,
            None => return RemapResult::Error,
        };

        if dm.suspended {
            return RemapResult::Error; // I/O queued while suspended
        }

        if dm.read_only && direction == IoDirection::Write {
            return RemapResult::Error;
        }

        // Find the target for this sector
        let target_idx = match dm.find_target(sector) {
            Some(i) => i,
            None => return RemapResult::Error,
        };

        let target = &mut dm.targets[target_idx];
        let offset_in_target = sector.saturating_sub(target.start);

        // Record statistics
        match direction {
            IoDirection::Read => target.stats.record_read(1),
            IoDirection::Write => target.stats.record_write(1),
        }

        match target.target_type {
            TargetType::Linear => RemapResult::Remap {
                device: target.device,
                sector: target.offset.saturating_add(offset_in_target),
            },

            TargetType::Striped => {
                if target.stripe_count == 0 || target.stripe_size == 0 {
                    target.stats.record_error();
                    return RemapResult::Error;
                }
                let stripe_size = target.stripe_size as u64;
                let stripe_count = target.stripe_count as u64;

                // Which stripe does this sector land on?
                let stripe_number = offset_in_target / stripe_size;
                let position_in_stripe = offset_in_target % stripe_size;
                let device_index = (stripe_number % stripe_count) as usize;
                let chunk_number = stripe_number / stripe_count;
                let physical_sector = chunk_number
                    .saturating_mul(stripe_size)
                    .saturating_add(position_in_stripe);

                if device_index < target.stripe_devices.len() {
                    let (dev, dev_offset) = target.stripe_devices[device_index];
                    RemapResult::Remap {
                        device: dev,
                        sector: dev_offset.saturating_add(physical_sector),
                    }
                } else {
                    target.stats.record_error();
                    RemapResult::Error
                }
            }

            TargetType::Mirror => {
                // For reads: use the primary device
                // For writes: would need to write to all mirrors (handled at a higher level)
                RemapResult::Remap {
                    device: target.device,
                    sector: target.offset.saturating_add(offset_in_target),
                }
            }

            TargetType::Snapshot => {
                let chunk_size = target.stripe_size as u64; // reusing stripe_size field
                if chunk_size == 0 {
                    target.stats.record_error();
                    return RemapResult::Error;
                }
                let chunk = offset_in_target / chunk_size;
                let within_chunk = offset_in_target % chunk_size;

                if let Some(ref mut ss) = target.snapshot {
                    // Check if this chunk has been COW'd
                    if let Some((dev, cow_sector)) = ss.find_cow(chunk) {
                        // Read/write from COW area
                        return RemapResult::Remap {
                            device: dev,
                            sector: cow_sector.saturating_add(within_chunk),
                        };
                    }

                    match direction {
                        IoDirection::Read => {
                            // Not COW'd yet: read from origin
                            RemapResult::Remap {
                                device: ss.origin_device,
                                sector: target.offset.saturating_add(offset_in_target),
                            }
                        }
                        IoDirection::Write => {
                            // COW: allocate new chunk, copy origin data there, then write
                            if let Some((dev, cow_sector)) = ss.alloc_cow(chunk) {
                                // In a real implementation, we'd copy the origin chunk first.
                                // Here we just redirect the write.
                                RemapResult::Remap {
                                    device: dev,
                                    sector: cow_sector.saturating_add(within_chunk),
                                }
                            } else {
                                target.stats.record_error();
                                RemapResult::Error // COW space exhausted
                            }
                        }
                    }
                } else {
                    target.stats.record_error();
                    RemapResult::Error
                }
            }

            TargetType::Crypt => {
                // Encryption/decryption would happen at a higher level
                // We just remap to the underlying device
                RemapResult::Remap {
                    device: target.device,
                    sector: target.offset.saturating_add(offset_in_target),
                }
            }

            TargetType::Error => {
                target.stats.record_error();
                RemapResult::Error
            }

            TargetType::Zero => match direction {
                IoDirection::Read => RemapResult::Zero,
                IoDirection::Write => RemapResult::Discard,
            },
        }
    }

    /// Translate a sector on a virtual device to the underlying device
    /// (simplified version for backward compatibility)
    pub fn translate(&self, name: &str, sector: u64) -> Option<(u32, u64)> {
        let dm = self.devices.iter().find(|d| d.name == name && d.active)?;

        for target in &dm.targets {
            if sector >= target.start && sector < target.start.saturating_add(target.length) {
                let offset_in_target = sector.saturating_sub(target.start);
                match target.target_type {
                    TargetType::Linear => {
                        return Some((
                            target.device,
                            target.offset.saturating_add(offset_in_target),
                        ));
                    }
                    TargetType::Striped => {
                        if target.stripe_count == 0 || target.stripe_size == 0 {
                            return None;
                        }
                        let stripe_size = target.stripe_size as u64;
                        let stripe_count = target.stripe_count as u64;
                        let stripe_number = offset_in_target / stripe_size;
                        let pos_in_stripe = offset_in_target % stripe_size;
                        let dev_idx = (stripe_number % stripe_count) as usize;
                        let chunk = stripe_number / stripe_count;

                        if dev_idx < target.stripe_devices.len() {
                            let (dev, off) = target.stripe_devices[dev_idx];
                            let phys = chunk
                                .saturating_mul(stripe_size)
                                .saturating_add(pos_in_stripe);
                            return Some((dev, off.saturating_add(phys)));
                        }
                        return None;
                    }
                    TargetType::Crypt => {
                        return Some((
                            target.device,
                            target.offset.saturating_add(offset_in_target),
                        ));
                    }
                    TargetType::Error => return None,
                    TargetType::Zero => return Some((0, 0)),
                    _ => {
                        return Some((
                            target.device,
                            target.offset.saturating_add(offset_in_target),
                        ))
                    }
                }
            }
        }
        None
    }

    /// List all mapped devices: (name, size_sectors, active, target_count)
    pub fn list(&self) -> Vec<(String, u64, bool, usize)> {
        self.devices
            .iter()
            .map(|d| (d.name.clone(), d.size_sectors, d.active, d.targets.len()))
            .collect()
    }

    /// Format dmsetup table output
    pub fn table(&self, name: &str) -> Option<String> {
        let dm = self.devices.iter().find(|d| d.name == name)?;
        let mut s = String::new();
        for t in &dm.targets {
            s.push_str(&t.table_line());
            s.push('\n');
        }
        Some(s)
    }

    /// Get device info
    pub fn info(&self, name: &str) -> Option<(u64, bool, bool, bool, u32)> {
        let dm = self.devices.iter().find(|d| d.name == name)?;
        Some((
            dm.size_sectors,
            dm.active,
            dm.suspended,
            dm.read_only,
            dm.open_count,
        ))
    }

    /// Get per-target statistics for a device
    pub fn stats(&self, name: &str) -> Option<Vec<(TargetType, TargetStats)>> {
        let dm = self.devices.iter().find(|d| d.name == name)?;
        Some(
            dm.targets
                .iter()
                .map(|t| (t.target_type, t.stats))
                .collect(),
        )
    }

    /// Get aggregate stats for a device
    pub fn aggregate_stats(&self, name: &str) -> Option<TargetStats> {
        let dm = self.devices.iter().find(|d| d.name == name)?;
        Some(dm.aggregate_stats())
    }

    /// Open a device (increment reference count)
    pub fn open(&mut self, name: &str) -> bool {
        if let Some(dm) = self.devices.iter_mut().find(|d| d.name == name && d.active) {
            dm.open_count = dm.open_count.saturating_add(1);
            true
        } else {
            false
        }
    }

    /// Close a device (decrement reference count)
    pub fn close(&mut self, name: &str) -> bool {
        if let Some(dm) = self.devices.iter_mut().find(|d| d.name == name) {
            if dm.open_count > 0 {
                dm.open_count = dm.open_count.saturating_sub(1);
            }
            true
        } else {
            false
        }
    }

    /// Set a device as read-only
    pub fn set_read_only(&mut self, name: &str, ro: bool) -> bool {
        if let Some(dm) = self.devices.iter_mut().find(|d| d.name == name) {
            dm.read_only = ro;
            true
        } else {
            false
        }
    }

    /// Get snapshot COW usage for a device
    pub fn snapshot_usage(&self, name: &str) -> Option<(usize, u64)> {
        let dm = self.devices.iter().find(|d| d.name == name)?;
        for target in &dm.targets {
            if target.target_type == TargetType::Snapshot {
                if let Some(ref ss) = target.snapshot {
                    return Some((ss.cow_count(), ss.cow_next_free));
                }
            }
        }
        None
    }

    /// Advance internal tick
    pub fn tick(&mut self) {
        self.tick = self.tick.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static DM: Mutex<DeviceMapper> = Mutex::new(DeviceMapper::new());

// ---------------------------------------------------------------------------
// Module-level API (convenience wrappers)
// ---------------------------------------------------------------------------

pub fn init() {
    crate::serial_println!("  [dm] Device mapper initialized");
}

pub fn create(name: &str, uuid: &str) -> Option<usize> {
    DM.lock().create(name, uuid)
}

pub fn add_linear(name: &str, start: u64, len: u64, dev: u32, off: u64) -> bool {
    DM.lock().add_linear_target(name, start, len, dev, off)
}

pub fn add_striped(
    name: &str,
    start: u64,
    len: u64,
    stripe_size: u32,
    devices: &[(u32, u64)],
) -> bool {
    DM.lock()
        .add_striped_target(name, start, len, stripe_size, devices)
}

pub fn add_snapshot(
    name: &str,
    start: u64,
    len: u64,
    origin: u32,
    cow_dev: u32,
    chunk_size: u32,
) -> bool {
    DM.lock()
        .add_snapshot_target(name, start, len, origin, cow_dev, chunk_size)
}

pub fn activate(name: &str) -> bool {
    DM.lock().activate(name)
}

pub fn suspend(name: &str) -> bool {
    DM.lock().suspend(name)
}

pub fn resume(name: &str) -> bool {
    DM.lock().resume(name)
}

pub fn remove(name: &str) -> bool {
    DM.lock().remove(name)
}

pub fn remap(name: &str, sector: u64, dir: IoDirection) -> RemapResult {
    DM.lock().remap(name, sector, dir)
}

pub fn list() -> Vec<(String, u64, bool, usize)> {
    DM.lock().list()
}

pub fn table(name: &str) -> Option<String> {
    DM.lock().table(name)
}

pub fn stats(name: &str) -> Option<Vec<(TargetType, TargetStats)>> {
    DM.lock().stats(name)
}

pub fn aggregate_stats(name: &str) -> Option<TargetStats> {
    DM.lock().aggregate_stats(name)
}

pub fn info(name: &str) -> Option<(u64, bool, bool, bool, u32)> {
    DM.lock().info(name)
}

pub fn translate(name: &str, sector: u64) -> Option<(u32, u64)> {
    DM.lock().translate(name, sector)
}

pub fn open(name: &str) -> bool {
    DM.lock().open(name)
}

pub fn close(name: &str) -> bool {
    DM.lock().close(name)
}

pub fn set_read_only(name: &str, ro: bool) -> bool {
    DM.lock().set_read_only(name, ro)
}

pub fn snapshot_usage(name: &str) -> Option<(usize, u64)> {
    DM.lock().snapshot_usage(name)
}

/// Add a mirror target (RAID-1): writes go to all mirrors, reads from primary
pub fn add_mirror(
    name: &str,
    start: u64,
    len: u64,
    primary_dev: u32,
    primary_off: u64,
    mirror_devs: &[u32],
) -> bool {
    DM.lock()
        .add_mirror_target(name, start, len, primary_dev, primary_off, mirror_devs)
}

/// Add an error target
pub fn add_error(name: &str, start: u64, len: u64) -> bool {
    DM.lock().add_error_target(name, start, len)
}

/// Add a zero target
pub fn add_zero(name: &str, start: u64, len: u64) -> bool {
    DM.lock().add_zero_target(name, start, len)
}

/// Add a crypt target
pub fn add_crypt(name: &str, start: u64, len: u64, dev: u32, off: u64, cipher: &str) -> bool {
    DM.lock()
        .add_crypt_target(name, start, len, dev, off, cipher)
}

/// Load a target into the inactive table (for atomic table swap)
pub fn load_target(name: &str, target: DmTarget) -> bool {
    DM.lock().load_target(name, target)
}

/// Clear the inactive table
pub fn clear_inactive(name: &str) -> bool {
    DM.lock().clear_inactive(name)
}

/// Swap active and inactive tables (device must be suspended)
pub fn swap_tables(name: &str) -> Result<(), &'static str> {
    DM.lock().swap_tables(name)
}

/// Convenience: create a simple linear device in one call
pub fn create_linear(name: &str, dev: u32, offset: u64, length: u64) -> bool {
    let mut dm = DM.lock();
    if dm.create(name, "").is_none() {
        return false;
    }
    dm.add_linear_target(name, 0, length, dev, offset);
    dm.activate(name)
}

/// Convenience: create a simple striped (RAID-0) device in one call
pub fn create_striped(
    name: &str,
    stripe_size: u32,
    devices: &[(u32, u64)],
    total_sectors: u64,
) -> bool {
    let mut dm = DM.lock();
    if dm.create(name, "").is_none() {
        return false;
    }
    dm.add_striped_target(name, 0, total_sectors, stripe_size, devices);
    dm.activate(name)
}

/// Get device count
pub fn device_count() -> usize {
    DM.lock().devices.len()
}

/// Advance the DM tick counter
pub fn tick() {
    DM.lock().tick();
}
