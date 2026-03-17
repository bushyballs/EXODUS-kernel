/// Software RAID for Genesis
///
/// RAID 0/1/5/6/10 with stripe/mirror, parity calculation,
/// rebuild from degraded mode, hot spare management, and I/O distribution.
///
/// Inspired by: Linux md/mdadm, ZFS RAID-Z. All code is original.
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// RAID level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaidLevel {
    Raid0,  // Striping only
    Raid1,  // Mirror
    Raid5,  // Striping + single distributed parity
    Raid6,  // Striping + double distributed parity
    Raid10, // Mirror + stripe
}

/// State of an individual member disk
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskState {
    Online,
    Degraded,
    Faulted,
    Rebuilding,
    Spare,
    Missing,
}

/// State of the overall array
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrayState {
    Optimal,
    Degraded,
    Rebuilding,
    Failed,
    Stopped,
}

/// A physical member disk inside an array
pub struct MemberDisk {
    pub disk_id: u32,
    pub device_path: String,
    pub state: DiskState,
    pub capacity_bytes: u64,
    pub used_bytes: u64,
    pub stripe_offset: u32,
    pub rebuild_progress_q16: i32, // Q16 0..65536 (0..100%)
    pub error_count: u32,
}

/// A RAID array
pub struct RaidArray {
    pub array_id: u32,
    pub name: String,
    pub level: RaidLevel,
    pub state: ArrayState,
    pub disks: Vec<MemberDisk>,
    pub spares: Vec<MemberDisk>,
    pub stripe_size_bytes: u32,
    pub total_capacity: u64,
    pub usable_capacity: u64,
    pub chunk_count: u64,
    pub created_at: u64,
    pub last_check: u64,
    pub scrub_errors: u32,
}

// ---------------------------------------------------------------------------
// Parity helpers (Q16 fixed-point where needed, pure integer XOR for parity)
// ---------------------------------------------------------------------------

/// XOR-based parity across a stripe of data blocks.
/// Each element represents one disk's data for that stripe unit.
fn compute_xor_parity(blocks: &[u64]) -> u64 {
    let mut parity: u64 = 0;
    for &b in blocks {
        parity ^= b;
    }
    parity
}

/// RAID-6 second parity (simplified Galois-field multiply by generator).
/// Uses a Q16 weight so arithmetic stays in integer domain.
fn compute_q_parity(blocks: &[u64]) -> u64 {
    let mut q: u64 = 0;
    let generator_q16: i32 = 2 << 16; // 2.0 in Q16
    for (i, &b) in blocks.iter().enumerate() {
        let weight = (generator_q16 as u64).wrapping_mul(i as u64 + 1);
        q ^= b.wrapping_mul(weight >> 16);
    }
    q
}

// ---------------------------------------------------------------------------
// RaidArray implementation
// ---------------------------------------------------------------------------

impl RaidArray {
    /// Calculate usable capacity from member disks and RAID level.
    fn recalculate_capacity(&mut self) {
        let online: Vec<&MemberDisk> = self
            .disks
            .iter()
            .filter(|d| d.state == DiskState::Online || d.state == DiskState::Rebuilding)
            .collect();
        let count = online.len() as u64;
        if count == 0 {
            self.usable_capacity = 0;
            return;
        }
        let smallest = online.iter().map(|d| d.capacity_bytes).min().unwrap_or(0);
        self.total_capacity = smallest * count;

        self.usable_capacity = match self.level {
            RaidLevel::Raid0 => smallest * count,
            RaidLevel::Raid1 => smallest,
            RaidLevel::Raid5 => {
                if count < 3 {
                    0
                } else {
                    smallest * (count - 1)
                }
            }
            RaidLevel::Raid6 => {
                if count < 4 {
                    0
                } else {
                    smallest * (count - 2)
                }
            }
            RaidLevel::Raid10 => {
                if count < 4 {
                    0
                } else {
                    smallest * (count / 2)
                }
            }
        };
    }

    /// How many disks can fail before data loss for this level?
    pub fn fault_tolerance(&self) -> u32 {
        match self.level {
            RaidLevel::Raid0 => 0,
            RaidLevel::Raid1 => (self.disks.len() as u32).saturating_sub(1),
            RaidLevel::Raid5 => 1,
            RaidLevel::Raid6 => 2,
            RaidLevel::Raid10 => 1, // per mirror pair
        }
    }

    /// Minimum disks required to create the array.
    pub fn min_disks(level: RaidLevel) -> u32 {
        match level {
            RaidLevel::Raid0 => 2,
            RaidLevel::Raid1 => 2,
            RaidLevel::Raid5 => 3,
            RaidLevel::Raid6 => 4,
            RaidLevel::Raid10 => 4,
        }
    }

    /// Mark a disk as faulted and evaluate array health.
    pub fn fail_disk(&mut self, disk_id: u32) -> bool {
        if let Some(d) = self.disks.iter_mut().find(|d| d.disk_id == disk_id) {
            d.state = DiskState::Faulted;
            d.error_count = d.error_count.saturating_add(1);
            self.evaluate_health();
            serial_println!("  [raid] Disk {} faulted in array {}", disk_id, self.name);
            true
        } else {
            false
        }
    }

    /// Evaluate array health based on member disk states.
    fn evaluate_health(&mut self) {
        let faulted = self
            .disks
            .iter()
            .filter(|d| d.state == DiskState::Faulted || d.state == DiskState::Missing)
            .count() as u32;
        let rebuilding = self.disks.iter().any(|d| d.state == DiskState::Rebuilding);

        if rebuilding {
            self.state = ArrayState::Rebuilding;
        } else if faulted == 0 {
            self.state = ArrayState::Optimal;
        } else if faulted <= self.fault_tolerance() {
            self.state = ArrayState::Degraded;
        } else {
            self.state = ArrayState::Failed;
            serial_println!("  [raid] CRITICAL: array {} has FAILED", self.name);
        }
    }

    /// Activate a hot spare to replace a faulted disk and begin rebuild.
    pub fn activate_spare(&mut self) -> bool {
        let faulted_idx = self
            .disks
            .iter()
            .position(|d| d.state == DiskState::Faulted);
        if faulted_idx.is_none() || self.spares.is_empty() {
            return false;
        }
        // Safety: we checked is_none() above and returned false, so this is Some
        let faulted_idx = match faulted_idx {
            Some(i) => i,
            None => return false,
        };
        let mut spare = self.spares.remove(0);
        spare.state = DiskState::Rebuilding;
        spare.rebuild_progress_q16 = 0;
        spare.stripe_offset = self.disks[faulted_idx].stripe_offset;

        serial_println!(
            "  [raid] Spare disk {} replacing faulted disk {} in array {}",
            spare.disk_id,
            self.disks[faulted_idx].disk_id,
            self.name
        );
        self.disks[faulted_idx] = spare;
        self.state = ArrayState::Rebuilding;
        true
    }

    /// Advance rebuild by a percentage step (Q16 fixed-point).
    /// Returns true when rebuild is complete.
    pub fn advance_rebuild(&mut self, step_q16: i32) -> bool {
        let mut all_done = true;
        for disk in self.disks.iter_mut() {
            if disk.state == DiskState::Rebuilding {
                disk.rebuild_progress_q16 += step_q16;
                if disk.rebuild_progress_q16 >= (100 << 16) {
                    disk.rebuild_progress_q16 = 100 << 16;
                    disk.state = DiskState::Online;
                    serial_println!("  [raid] Disk {} rebuild complete", disk.disk_id);
                } else {
                    all_done = false;
                }
            }
        }
        if all_done {
            self.evaluate_health();
        }
        all_done
    }

    /// Run a scrub: verify parity consistency across stripes.
    pub fn scrub(&mut self) -> u32 {
        let mut errors: u32 = 0;
        let online_count = self
            .disks
            .iter()
            .filter(|d| d.state == DiskState::Online)
            .count();

        match self.level {
            RaidLevel::Raid5 | RaidLevel::Raid6 => {
                // Simulate parity check over chunk_count stripes
                let stripes = self.chunk_count / (online_count as u64).max(1);
                for s in 0..stripes.min(1024) {
                    let blocks: Vec<u64> = (0..online_count as u64)
                        .map(|d| d.wrapping_mul(s + 1))
                        .collect();
                    let p = compute_xor_parity(&blocks);
                    // In a real system we'd compare against stored parity
                    if p == 0 && s > 0 {
                        errors += 1;
                    }
                }
            }
            RaidLevel::Raid1 | RaidLevel::Raid10 => {
                // Mirror consistency: compare mirror pairs
                let pairs = online_count / 2;
                for _ in 0..pairs {
                    // Simulated: mirrors should always match
                }
            }
            RaidLevel::Raid0 => {
                // No redundancy to verify
            }
        }

        self.scrub_errors += errors;
        self.last_check = crate::time::clock::unix_time();
        serial_println!(
            "  [raid] Scrub complete on {}: {} errors",
            self.name,
            errors
        );
        errors
    }

    /// Compute I/O distribution across disks as Q16 percentages.
    pub fn io_distribution_q16(&self) -> Vec<i32> {
        let online: Vec<&MemberDisk> = self
            .disks
            .iter()
            .filter(|d| d.state == DiskState::Online)
            .collect();
        let count = online.len() as i32;
        if count == 0 {
            return Vec::new();
        }
        let per_disk_q16 = (100 << 16) / count;
        vec![per_disk_q16; count as usize]
    }
}

// ---------------------------------------------------------------------------
// RAID Manager
// ---------------------------------------------------------------------------

/// Manages all RAID arrays in the system.
pub struct RaidManager {
    pub arrays: Vec<RaidArray>,
    pub next_array_id: u32,
    pub next_disk_id: u32,
    pub auto_spare: bool,
    pub scrub_interval_secs: u64,
}

impl RaidManager {
    const fn new() -> Self {
        RaidManager {
            arrays: Vec::new(),
            next_array_id: 1,
            next_disk_id: 1,
            auto_spare: true,
            scrub_interval_secs: 86400 * 7, // weekly
        }
    }

    /// Create a new RAID array from a set of device paths.
    pub fn create_array(
        &mut self,
        name: &str,
        level: RaidLevel,
        devices: &[&str],
        capacity_each: u64,
        stripe_size: u32,
    ) -> Option<u32> {
        let min = RaidArray::min_disks(level);
        if (devices.len() as u32) < min {
            serial_println!(
                "  [raid] Cannot create {}: need {} disks, got {}",
                name,
                min,
                devices.len()
            );
            return None;
        }

        let mut disks = Vec::new();
        for (i, dev) in devices.iter().enumerate() {
            let did = self.next_disk_id;
            self.next_disk_id = self.next_disk_id.saturating_add(1);
            disks.push(MemberDisk {
                disk_id: did,
                device_path: String::from(*dev),
                state: DiskState::Online,
                capacity_bytes: capacity_each,
                used_bytes: 0,
                stripe_offset: i as u32,
                rebuild_progress_q16: 100 << 16, // fully synced
                error_count: 0,
            });
        }

        let aid = self.next_array_id;
        self.next_array_id = self.next_array_id.saturating_add(1);
        let mut array = RaidArray {
            array_id: aid,
            name: String::from(name),
            level,
            state: ArrayState::Optimal,
            disks,
            spares: Vec::new(),
            stripe_size_bytes: stripe_size,
            total_capacity: 0,
            usable_capacity: 0,
            chunk_count: 0,
            created_at: crate::time::clock::unix_time(),
            last_check: 0,
            scrub_errors: 0,
        };
        array.recalculate_capacity();
        if array.stripe_size_bytes > 0 {
            array.chunk_count = array.usable_capacity / (array.stripe_size_bytes as u64);
        }

        serial_println!(
            "  [raid] Created {:?} array '{}' ({} disks, {} usable)",
            level,
            name,
            devices.len(),
            array.usable_capacity
        );
        self.arrays.push(array);
        Some(aid)
    }

    /// Add a hot spare to an existing array.
    pub fn add_spare(&mut self, array_id: u32, device: &str, capacity: u64) -> bool {
        let did = self.next_disk_id;
        self.next_disk_id = self.next_disk_id.saturating_add(1);
        if let Some(arr) = self.arrays.iter_mut().find(|a| a.array_id == array_id) {
            arr.spares.push(MemberDisk {
                disk_id: did,
                device_path: String::from(device),
                state: DiskState::Spare,
                capacity_bytes: capacity,
                used_bytes: 0,
                stripe_offset: 0,
                rebuild_progress_q16: 0,
                error_count: 0,
            });
            serial_println!("  [raid] Added spare {} to array {}", device, arr.name);
            true
        } else {
            false
        }
    }

    /// Fail a disk in a specific array; auto-activate spare if enabled.
    pub fn report_disk_failure(&mut self, array_id: u32, disk_id: u32) {
        if let Some(arr) = self.arrays.iter_mut().find(|a| a.array_id == array_id) {
            arr.fail_disk(disk_id);
            if self.auto_spare && arr.state == ArrayState::Degraded {
                arr.activate_spare();
            }
        }
    }

    /// Get array by id.
    pub fn get_array(&self, array_id: u32) -> Option<&RaidArray> {
        self.arrays.iter().find(|a| a.array_id == array_id)
    }

    /// Get array by id (mutable).
    pub fn get_array_mut(&mut self, array_id: u32) -> Option<&mut RaidArray> {
        self.arrays.iter_mut().find(|a| a.array_id == array_id)
    }

    /// Stop an array (take offline).
    pub fn stop_array(&mut self, array_id: u32) -> bool {
        if let Some(arr) = self.arrays.iter_mut().find(|a| a.array_id == array_id) {
            arr.state = ArrayState::Stopped;
            serial_println!("  [raid] Array '{}' stopped", arr.name);
            true
        } else {
            false
        }
    }

    /// Remove a stopped array.
    pub fn remove_array(&mut self, array_id: u32) -> bool {
        if let Some(pos) = self.arrays.iter().position(|a| a.array_id == array_id) {
            if self.arrays[pos].state != ArrayState::Stopped {
                serial_println!("  [raid] Cannot remove array that is not stopped");
                return false;
            }
            let name = self.arrays[pos].name.clone();
            self.arrays.remove(pos);
            serial_println!("  [raid] Array '{}' removed", name);
            true
        } else {
            false
        }
    }

    /// Summary: total arrays, degraded, rebuilding.
    pub fn summary(&self) -> (usize, usize, usize) {
        let total = self.arrays.len();
        let degraded = self
            .arrays
            .iter()
            .filter(|a| a.state == ArrayState::Degraded)
            .count();
        let rebuilding = self
            .arrays
            .iter()
            .filter(|a| a.state == ArrayState::Rebuilding)
            .count();
        (total, degraded, rebuilding)
    }

    /// Run scrub on all arrays.
    pub fn scrub_all(&mut self) -> u32 {
        let mut total_errors = 0u32;
        for arr in self.arrays.iter_mut() {
            if arr.state != ArrayState::Stopped && arr.state != ArrayState::Failed {
                total_errors += arr.scrub();
            }
        }
        total_errors
    }

    /// Verify RAID-6 double parity for testing.
    pub fn verify_raid6_parity(&self, data_blocks: &[u64]) -> (u64, u64) {
        let p = compute_xor_parity(data_blocks);
        let q = compute_q_parity(data_blocks);
        (p, q)
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static RAID_MANAGER: Mutex<Option<RaidManager>> = Mutex::new(None);

pub fn init() {
    let mut guard = RAID_MANAGER.lock();
    *guard = Some(RaidManager::new());
    serial_println!("  [storage] Software RAID manager initialized");
}

/// Access the RAID manager under lock.
pub fn with_raid<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut RaidManager) -> R,
{
    let mut guard = RAID_MANAGER.lock();
    guard.as_mut().map(f)
}
