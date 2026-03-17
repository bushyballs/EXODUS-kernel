/// ZFS-inspired storage pool management
///
/// Part of the AIOS storage layer.
///
/// Implements a simplified ZFS-like storage pool (zpool) with virtual devices,
/// metaslab-based allocation, an intent log (ZIL) for synchronous writes,
/// and an adaptive replacement cache (ARC) for read caching.
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// State of a virtual device in the pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VdevState {
    Online,
    Degraded,
    Faulted,
    Offline,
    Removed,
}

/// A virtual device (vdev) in the pool.
struct Vdev {
    path: String,
    state: VdevState,
    capacity_bytes: u64,
    used_bytes: u64,
    read_errors: u64,
    write_errors: u64,
    checksum_errors: u64,
}

/// A metaslab: a contiguous region of free/used space within a vdev.
struct Metaslab {
    vdev_index: usize,
    offset: u64,
    size: u64,
    free: u64,
}

/// ZFS Intent Log entry (simulates write-ahead log for sync writes).
struct ZilEntry {
    txg: u64, // transaction group
    offset: u64,
    length: u32,
    checksum: u64,
}

/// ARC (Adaptive Replacement Cache) statistics.
struct ArcStats {
    hits: u64,
    misses: u64,
    size_bytes: u64,
    target_size: u64,
}

pub struct Zpool {
    name: String,
    vdevs: Vec<Vdev>,
    metaslabs: Vec<Metaslab>,
    zil: Vec<ZilEntry>,
    arc: ArcStats,
    /// Current transaction group number.
    txg: u64,
    /// Total pool capacity in bytes.
    total_capacity: u64,
    /// Total used bytes.
    used_bytes: u64,
    /// Pool creation timestamp.
    created_at: u64,
    /// Last scrub timestamp.
    last_scrub: u64,
    /// Scrub error count from most recent scrub.
    scrub_errors: u64,
}

impl Zpool {
    /// Create a new storage pool from a set of device paths.
    pub fn create(name: &str, devices: &[&str]) -> Result<Self, ()> {
        if devices.is_empty() {
            serial_println!("  [zfs] Cannot create pool '{}': no devices", name);
            return Err(());
        }

        let mut vdevs = Vec::new();
        let mut metaslabs = Vec::new();
        let mut total_capacity = 0u64;
        let default_vdev_size = 1024 * 1024 * 1024; // 1 GiB default per vdev
        let metaslab_size = 64 * 1024 * 1024; // 64 MiB metaslabs

        for (i, dev) in devices.iter().enumerate() {
            let cap = default_vdev_size;
            vdevs.push(Vdev {
                path: String::from(*dev),
                state: VdevState::Online,
                capacity_bytes: cap,
                used_bytes: 0,
                read_errors: 0,
                write_errors: 0,
                checksum_errors: 0,
            });
            total_capacity += cap;

            // Create metaslabs for this vdev
            let num_metaslabs = cap / metaslab_size;
            for m in 0..num_metaslabs {
                metaslabs.push(Metaslab {
                    vdev_index: i,
                    offset: m * metaslab_size,
                    size: metaslab_size,
                    free: metaslab_size,
                });
            }
        }

        serial_println!(
            "  [zfs] Pool '{}' created with {} vdevs ({} bytes total)",
            name,
            devices.len(),
            total_capacity
        );

        Ok(Zpool {
            name: String::from(name),
            vdevs,
            metaslabs,
            zil: Vec::new(),
            arc: ArcStats {
                hits: 0,
                misses: 0,
                size_bytes: 0,
                target_size: 256 * 1024 * 1024, // 256 MiB default ARC target
            },
            txg: 1,
            total_capacity,
            used_bytes: 0,
            created_at: crate::time::clock::unix_time(),
            last_scrub: 0,
            scrub_errors: 0,
        })
    }

    /// Run a scrub: verify checksums of all allocated data.
    pub fn scrub(&mut self) -> Result<(), ()> {
        serial_println!("  [zfs] Starting scrub on pool '{}'", self.name);

        let mut errors = 0u64;

        // Verify each vdev is accessible
        for vdev in &mut self.vdevs {
            if vdev.state == VdevState::Faulted || vdev.state == VdevState::Removed {
                errors += 1;
                serial_println!(
                    "  [zfs] Scrub: vdev '{}' is {:?}, skipping",
                    vdev.path,
                    vdev.state
                );
                continue;
            }

            // In a real implementation:
            // 1. Walk the block pointer tree (uber -> dnode -> indirect -> leaf)
            // 2. Read each data block
            // 3. Compute fletcher4/SHA256 checksum
            // 4. Compare against stored checksum
            // 5. If mismatch and redundancy exists, repair from mirror/parity
            // 6. Count errors

            // Simulate: check if any error counters are non-zero
            errors += vdev.checksum_errors;
        }

        self.last_scrub = crate::time::clock::unix_time();
        self.scrub_errors = errors;

        serial_println!(
            "  [zfs] Scrub complete on '{}': {} errors found",
            self.name,
            errors
        );
        Ok(())
    }

    /// Return a human-readable status string for the pool.
    pub fn status(&self) -> String {
        let state_str = if self.scrub_errors > 0 {
            "DEGRADED"
        } else {
            let all_online = self.vdevs.iter().all(|v| v.state == VdevState::Online);
            if all_online {
                "ONLINE"
            } else {
                "DEGRADED"
            }
        };

        let usage_pct = if self.total_capacity > 0 {
            (self.used_bytes * 100) / self.total_capacity
        } else {
            0
        };

        format!(
            "pool '{}': {} ({} vdevs, {}% used, {} scrub errors)",
            self.name,
            state_str,
            self.vdevs.len(),
            usage_pct,
            self.scrub_errors
        )
    }

    /// Allocate space from the pool using metaslab allocation.
    pub fn allocate(&mut self, size: u64) -> Option<(usize, u64)> {
        // Find a metaslab with enough free space (first-fit)
        for ms in &mut self.metaslabs {
            if ms.free >= size {
                let offset = ms.offset + (ms.size - ms.free);
                ms.free -= size;
                self.used_bytes += size;
                if let Some(vdev) = self.vdevs.get_mut(ms.vdev_index) {
                    vdev.used_bytes += size;
                }
                return Some((ms.vdev_index, offset));
            }
        }
        None
    }

    /// Write to the ZIL (synchronous write intent log).
    pub fn zil_commit(&mut self, offset: u64, length: u32) {
        // FNV-1a checksum over offset+length for integrity
        let mut hash: u64 = 0xcbf29ce484222325;
        let prime: u64 = 0x100000001b3;
        for &byte in offset
            .to_le_bytes()
            .iter()
            .chain(length.to_le_bytes().iter())
        {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(prime);
        }

        self.zil.push(ZilEntry {
            txg: self.txg,
            offset,
            length,
            checksum: hash,
        });
    }

    /// Advance to the next transaction group, flushing the ZIL.
    pub fn sync_txg(&mut self) {
        self.txg += 1;
        self.zil.clear();
    }

    /// Return pool name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return total capacity.
    pub fn total_capacity(&self) -> u64 {
        self.total_capacity
    }

    /// Return used bytes.
    pub fn used_bytes(&self) -> u64 {
        self.used_bytes
    }

    /// Return number of vdevs.
    pub fn vdev_count(&self) -> usize {
        self.vdevs.len()
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

pub struct ZfsSubsystem {
    pools: Vec<Zpool>,
}

impl ZfsSubsystem {
    const fn new() -> Self {
        ZfsSubsystem { pools: Vec::new() }
    }
}

static ZFS_SUBSYSTEM: Mutex<Option<ZfsSubsystem>> = Mutex::new(None);

pub fn init() {
    let mut guard = ZFS_SUBSYSTEM.lock();
    *guard = Some(ZfsSubsystem::new());
    serial_println!("  [storage] ZFS pool manager initialized");
}

/// Access the ZFS subsystem under lock.
pub fn with_zfs<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut ZfsSubsystem) -> R,
{
    let mut guard = ZFS_SUBSYSTEM.lock();
    guard.as_mut().map(f)
}
